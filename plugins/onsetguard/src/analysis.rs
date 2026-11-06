//! Hit analysis: the measurements the plugin shows live and computes for a reference file.
//! Streaming, allocation-free after construction, one instance per analysed signal. Fed with
//! stereo samples and with onset times (from the onset detector live, from a list offline), it
//! keeps:
//!
//! - **hit profile**: per clap, the mid-channel RMS in windows after the (HF-aligned) onset
//!   (0-3 / 3-10 / 10-25 / 25-60 / 60-120 / 120-200 ms) and the share of samples at the rail
//!   (either channel >= 0.999) in windows 0-10 … 120-150 ms; 5-10 kHz level at 25-60 ms;
//! - **drums vs rest**: mean power per band (full, <250, 250-1k, 1-4k, 4-10k; mid channel)
//!   inside the 150 ms after every hit versus everywhere else, silence excluded;
//! - **500-6 kHz spread**: p90 - p10 of 50 ms window levels (stereo), the "bricked" readout;
//! - **pinned share**: samples at the rail overall, inside hits, between hits;
//! - **loudness**: mean sone/phon (stereo), at hits (30 ms after the clap) and between hits.
//!
//! Definitions follow the offline Python scripts the targets were measured with (bite.py,
//! dom.py, measure_bounce.py); band filters are the same 4th-order Butterworth designs.
//! Band filters are applied twice (|H|^2) to match the zero-phase offline filtering.
//! Onset alignment: the first sample in [onset - 40 ms, onset + 60 ms] where the 1 ms RMS
//! envelope of the 2-8 kHz band reaches -6 dB of its peak in that range (500-8 kHz for hits
//! that are not claps).

use crate::dsp::butter::{butter_bandpass, SosChain};
use crate::dsp::sone::{SoneMeter, HOP as SONE_HOP};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const LEVEL_WINS_MS: [(f32, f32); 6] =
    [(0.0, 3.0), (3.0, 10.0), (10.0, 25.0), (25.0, 60.0), (60.0, 120.0), (120.0, 200.0)];
pub const RAIL_WINS_MS: [(f32, f32); 7] = [
    (0.0, 10.0),
    (10.0, 20.0),
    (20.0, 40.0),
    (40.0, 60.0),
    (60.0, 90.0),
    (90.0, 120.0),
    (120.0, 150.0),
];
/// Band index 0 is the full signal.
pub const BANDS: [(&str, f32, f32); 5] = [
    ("full", 0.0, 0.0),
    ("<250", 20.0, 250.0),
    ("250-1k", 250.0, 1000.0),
    ("1-4k", 1000.0, 4000.0),
    ("4-10k", 4000.0, 10000.0),
];
pub const NBANDS: usize = BANDS.len();
pub const HIT_MS: f32 = 150.0;
pub const TAIL_WIN_MS: (f32, f32) = (25.0, 60.0);
pub const SPREAD_WIN_MS: f32 = 50.0;
pub const RAIL: f32 = 0.999;
/// 50 ms mean power below this (-50 dBFS) is silence and excluded from the masks.
pub const SILENCE_POW: f32 = 1e-5;
const ALIGN_PRE_MS: f32 = 40.0;
const ALIGN_POST_MS: f32 = 60.0;
const PROFILE_MS: f32 = 200.0;
const MAX_PENDING: usize = 32;
const MAX_PROFILES: usize = 64;
const MAX_SPREAD: usize = 4096;
const HF_RECENT: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    /// Profile this hit (clap / snare).
    Clap,
    /// Counts for the drums mask only.
    Other,
    /// Decide from the 2-8 kHz level: within 6 dB of the loudest recent hit → clap.
    Auto,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HitProfile {
    /// Aligned onset, absolute sample index.
    pub at: usize,
    pub is_clap: bool,
    pub level_db: [f32; 6],
    pub rail_pct: [f32; 7],
    pub tail_db: f32,
    /// 2-8 kHz RMS over 0-25 ms, dBFS.
    pub hf_db: f32,
    pub sone_at: f32,
    /// Sone at the midpoint between this clap and the previous one (0 if none).
    pub sone_between: f32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub hits: usize,
    pub claps: usize,
    pub level_db: [f32; 6],
    pub rail_pct: [f32; 7],
    pub tail_db: f32,
    pub band_hits_db: [f32; NBANDS],
    pub band_rest_db: [f32; NBANDS],
    pub spread_db: f32,
    /// 250-1k band, mid: p99 - median of 100 ms window levels (low-mid peakiness; growl bumps).
    pub lowmid_peak_db: f32,
    /// Mean power per band over hits and rest together.
    pub band_all_db: [f32; NBANDS],
    pub pinned_pct: f32,
    pub pinned_hits_pct: f32,
    pub pinned_rest_pct: f32,
    pub phon: f32,
    pub phon_hits: f32,
    pub phon_between: f32,
}

impl Report {
    pub fn band_delta(&self, b: usize) -> f32 {
        self.band_hits_db[b] - self.band_rest_db[b]
    }
}

#[derive(Clone, Copy)]
struct Pending {
    onset: usize,
    kind: HitKind,
    aligned: Option<usize>,
}

/// 1 ms RMS envelope, centred (stored half a window late to match a centred moving average).
struct Env {
    ring: Vec<f32>,
    sum: f64,
    len: usize,
    pos: usize,
}

impl Env {
    fn new(len: usize) -> Self {
        Env { ring: vec![0.0; len], sum: 0.0, len, pos: 0 }
    }
    #[inline]
    fn push(&mut self, sq: f32) -> f32 {
        let i = self.pos % self.len;
        self.sum += sq as f64 - self.ring[i] as f64;
        self.ring[i] = sq;
        self.pos += 1;
        (self.sum.max(0.0) / self.len as f64).sqrt() as f32
    }
    fn reset(&mut self) {
        self.ring.fill(0.0);
        self.sum = 0.0;
        self.pos = 0;
    }
}

pub struct Analyzer {
    sr: f32,
    ring: usize,
    mask: usize,
    pos: usize,
    /// Accumulation runs this many samples behind `pos` so hit masks are known in time.
    delay: usize,
    span: (usize, usize),
    // per-sample rings
    mid: Vec<f32>,
    rail: Vec<u8>,
    hf_env: Vec<f32>,
    mf_env: Vec<f32>,
    band_sq: Vec<[f32; NBANDS]>,
    tail_sq: Vec<f32>,
    spread_pow: Vec<f32>,
    silent: Vec<u8>,
    // filters (mid channel unless noted)
    hf: SosChain,
    mf: SosChain,
    bands: [SosChain; NBANDS - 1],
    tail: SosChain,
    spread_l: SosChain,
    spread_r: SosChain,
    hf_e: Env,
    mf_e: Env,
    sil_e: Env,
    env_half: usize,
    sil_half: usize,
    // hit bookkeeping
    pending: Vec<Pending>,
    masks: VecDeque<(usize, usize)>,
    mask_until: usize,
    profiles: VecDeque<HitProfile>,
    recent_hf: VecDeque<f32>,
    last_clap_at: Option<usize>,
    onsets_seen: usize,
    // accumulators
    hits_pow: [f64; NBANDS],
    rest_pow: [f64; NBANDS],
    hits_n: u64,
    rest_n: u64,
    hits_rail: u64,
    rest_rail: u64,
    spread_sum: f64,
    spread_win: usize,
    spread_levels: VecDeque<f32>,
    lm_sum: f64,
    lm_win: usize,
    lm_levels: VecDeque<f32>,
    // loudness
    sone: SoneMeter,
    sone_trace: Vec<f32>,
    sone_sum: f64,
    sone_n: u64,
}

#[inline]
fn db_pow(p: f64) -> f32 {
    (10.0 * p.max(1e-20).log10()) as f32
}

#[inline]
fn db_amp(a: f64) -> f32 {
    (20.0 * a.max(1e-10).log10()) as f32
}

fn median(v: &mut [f32]) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

fn percentile(v: &mut [f32], p: f32) -> f32 {
    if v.is_empty() {
        return f32::NAN;
    }
    v.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // numpy default: linear interpolation
    let x = p / 100.0 * (v.len() - 1) as f32;
    let i = x.floor() as usize;
    let f = x - i as f32;
    if i + 1 < v.len() {
        v[i] * (1.0 - f) + v[i + 1] * f
    } else {
        v[i]
    }
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate as f64;
        let ring = ((0.5 * sample_rate) as usize).next_power_of_two();
        let ms = |m: f32| (m * sample_rate / 1000.0) as usize;
        let env_len = (0.001 * sample_rate).round().max(4.0) as usize;
        let sil_len = ms(SPREAD_WIN_MS);
        // the offline scripts filter forward and backward (zero phase), i.e. the band shape is
        // |H|^2 of the 4th-order design; applying the causal filter twice gives the same shape
        let bp = |lo: f32, hi: f32| {
            let mut secs = butter_bandpass(4, lo as f64, hi as f64, sr);
            let twice = secs.clone();
            secs.extend(twice);
            SosChain::new(secs)
        };
        let bands = [
            bp(BANDS[1].1, BANDS[1].2),
            bp(BANDS[2].1, BANDS[2].2),
            bp(BANDS[3].1, BANDS[3].2),
            bp(BANDS[4].1, BANDS[4].2),
        ];
        let sone_len = (4.0 * sample_rate) as usize / SONE_HOP + 1;
        Analyzer {
            sr: sample_rate,
            ring,
            mask: ring - 1,
            pos: 0,
            delay: ms(ALIGN_POST_MS + ALIGN_PRE_MS) + env_len + 64,
            span: (0, usize::MAX),
            mid: vec![0.0; ring],
            rail: vec![0; ring],
            hf_env: vec![0.0; ring],
            mf_env: vec![0.0; ring],
            band_sq: vec![[0.0; NBANDS]; ring],
            tail_sq: vec![0.0; ring],
            spread_pow: vec![0.0; ring],
            silent: vec![1; ring],
            hf: bp(2000.0, 8000.0),
            mf: bp(500.0, 8000.0),
            bands,
            tail: bp(5000.0, 10000.0),
            spread_l: bp(500.0, 6000.0),
            spread_r: bp(500.0, 6000.0),
            hf_e: Env::new(env_len),
            mf_e: Env::new(env_len),
            sil_e: Env::new(sil_len),
            env_half: env_len / 2,
            sil_half: sil_len / 2,
            pending: Vec::with_capacity(MAX_PENDING),
            masks: VecDeque::with_capacity(MAX_PENDING),
            mask_until: 0,
            profiles: VecDeque::with_capacity(MAX_PROFILES),
            recent_hf: VecDeque::with_capacity(HF_RECENT),
            last_clap_at: None,
            onsets_seen: 0,
            hits_pow: [0.0; NBANDS],
            rest_pow: [0.0; NBANDS],
            hits_n: 0,
            rest_n: 0,
            hits_rail: 0,
            rest_rail: 0,
            spread_sum: 0.0,
            spread_win: sil_len,
            spread_levels: VecDeque::with_capacity(MAX_SPREAD),
            lm_sum: 0.0,
            lm_win: ms(100.0),
            lm_levels: VecDeque::with_capacity(MAX_SPREAD),
            sone: SoneMeter::new(sample_rate, 2, 100.0),
            sone_trace: vec![0.0; sone_len],
            sone_sum: 0.0,
            sone_n: 0,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sr
    }

    pub fn position(&self) -> usize {
        self.pos
    }

    /// Only samples in `[lo, hi)` count for drums/rest, pinned, spread and loudness. Hits are
    /// profiled regardless.
    pub fn set_span(&mut self, lo: usize, hi: usize) {
        self.span = (lo, hi);
    }

    pub fn reset(&mut self) {
        self.pos = 0;
        self.mid.fill(0.0);
        self.rail.fill(0);
        self.hf_env.fill(0.0);
        self.mf_env.fill(0.0);
        for b in &mut self.band_sq {
            *b = [0.0; NBANDS];
        }
        self.tail_sq.fill(0.0);
        self.spread_pow.fill(0.0);
        self.silent.fill(1);
        self.hf.reset();
        self.mf.reset();
        for b in &mut self.bands {
            b.reset();
        }
        self.tail.reset();
        self.spread_l.reset();
        self.spread_r.reset();
        self.hf_e.reset();
        self.mf_e.reset();
        self.sil_e.reset();
        self.pending.clear();
        self.masks.clear();
        self.mask_until = 0;
        self.profiles.clear();
        self.recent_hf.clear();
        self.last_clap_at = None;
        self.onsets_seen = 0;
        self.clear_accumulators();
        self.sone.reset();
        self.sone_trace.fill(0.0);
    }

    /// Restart the long-term accumulators (drums/rest, pinned, spread, loudness) keeping the
    /// signal state; the per-hit profiles are kept too.
    pub fn clear_accumulators(&mut self) {
        self.hits_pow = [0.0; NBANDS];
        self.rest_pow = [0.0; NBANDS];
        self.hits_n = 0;
        self.rest_n = 0;
        self.hits_rail = 0;
        self.rest_rail = 0;
        self.spread_sum = 0.0;
        self.spread_levels.clear();
        self.lm_sum = 0.0;
        self.lm_levels.clear();
        self.sone_sum = 0.0;
        self.sone_n = 0;
    }

    /// Register a hit at absolute sample `at` (may be in the past, up to ~250 ms).
    pub fn onset(&mut self, at: usize, kind: HitKind) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        if self.pos.saturating_sub(at) + (ALIGN_PRE_MS * self.sr / 1000.0) as usize + 8 >= self.ring {
            return; // too old for the ring
        }
        self.onsets_seen += 1;
        self.pending.push(Pending { onset: at, kind, aligned: None });
    }

    #[inline]
    fn ms(&self, m: f32) -> usize {
        (m * self.sr / 1000.0) as usize
    }

    /// Push one stereo sample.
    #[inline]
    pub fn push(&mut self, l: f32, r: f32) {
        let l = l.clamp(-1.0, 1.0);
        let r = r.clamp(-1.0, 1.0);
        let m = 0.5 * (l + r);
        let i = self.pos & self.mask;
        self.mid[i] = m;
        self.rail[i] = (l.abs() >= RAIL || r.abs() >= RAIL) as u8;
        let md = m as f64;
        let hf = self.hf.process(md) as f32;
        let mf = self.mf.process(md) as f32;
        let he = self.hf_e.push(hf * hf);
        let me = self.mf_e.push(mf * mf);
        let ci = self.pos.wrapping_sub(self.env_half) & self.mask;
        self.hf_env[ci] = he;
        self.mf_env[ci] = me;
        let mut sq = [0.0f32; NBANDS];
        sq[0] = m * m;
        for (k, b) in self.bands.iter_mut().enumerate() {
            let y = b.process(md) as f32;
            sq[k + 1] = y * y;
        }
        self.band_sq[i] = sq;
        let t = self.tail.process(md) as f32;
        self.tail_sq[i] = t * t;
        let sl = self.spread_l.process(l as f64) as f32;
        let sr_ = self.spread_r.process(r as f64) as f32;
        self.spread_pow[i] = 0.5 * (sl * sl + sr_ * sr_);
        let se = self.sil_e.push(m * m);
        let si = self.pos.wrapping_sub(self.sil_half) & self.mask;
        self.silent[si] = (se * se < SILENCE_POW) as u8;
        // loudness
        self.sone.push(&[l, r]);
        if self.pos % SONE_HOP == 0 {
            let n = self.sone_trace.len();
            self.sone_trace[(self.pos / SONE_HOP) % n] = self.sone.sone;
            if self.pos >= self.span.0 && self.pos < self.span.1 && self.pos >= self.sr as usize / 2 {
                self.sone_sum += self.sone.sone as f64;
                self.sone_n += 1;
            }
        }
        self.pos += 1;
        self.service_hits();
        if self.pos > self.delay {
            self.accumulate(self.pos - 1 - self.delay);
        }
    }

    fn service_hits(&mut self) {
        let post = self.ms(ALIGN_POST_MS);
        let pre = self.ms(ALIGN_PRE_MS);
        let prof = self.ms(PROFILE_MS);
        let mut k = 0;
        while k < self.pending.len() {
            let p = self.pending[k];
            match p.aligned {
                None => {
                    if self.pos > p.onset + post {
                        let a0 = p.onset.saturating_sub(pre);
                        let a1 = p.onset + post;
                        let use_hf = match p.kind {
                            HitKind::Clap => true,
                            HitKind::Other => false,
                            HitKind::Auto => {
                                let mut pk = 0.0f32;
                                for t in a0..a1 {
                                    pk = pk.max(self.hf_env[t & self.mask]);
                                }
                                pk > 0.01
                            }
                        };
                        let mut pk = 0.0f32;
                        for t in a0..a1 {
                            let v = if use_hf { self.hf_env[t & self.mask] } else { self.mf_env[t & self.mask] };
                            pk = pk.max(v);
                        }
                        let thr = pk * 0.501_187_2; // -6 dB
                        let mut al = a0;
                        for t in a0..a1 {
                            let v = if use_hf { self.hf_env[t & self.mask] } else { self.mf_env[t & self.mask] };
                            if v >= thr {
                                al = t;
                                break;
                            }
                        }
                        self.pending[k].aligned = Some(al);
                        if self.masks.len() >= MAX_PENDING {
                            self.masks.pop_front();
                        }
                        self.masks.push_back((al, al + self.ms(HIT_MS)));
                        if p.kind == HitKind::Other {
                            self.pending.remove(k);
                            continue;
                        }
                    }
                    k += 1;
                }
                Some(al) => {
                    if self.pos > al + prof {
                        self.evaluate(al, p.kind);
                        self.pending.remove(k);
                    } else {
                        k += 1;
                    }
                }
            }
        }
    }

    fn evaluate(&mut self, al: usize, kind: HitKind) {
        let w = |s: &Self, a: f32, b: f32, f: &dyn Fn(usize) -> f32| -> f64 {
            let (i0, i1) = (al + s.ms(a), al + s.ms(b));
            let mut acc = 0.0f64;
            for t in i0..i1 {
                acc += f(t & s.mask) as f64;
            }
            acc / (i1 - i0).max(1) as f64
        };
        let mut level = [0.0f32; 6];
        for (k, (a, b)) in LEVEL_WINS_MS.iter().enumerate() {
            let p = w(self, *a, *b, &|i| self.mid[i] * self.mid[i]);
            level[k] = db_amp(p.sqrt());
        }
        let mut rail = [0.0f32; 7];
        for (k, (a, b)) in RAIL_WINS_MS.iter().enumerate() {
            rail[k] = 100.0 * w(self, *a, *b, &|i| self.rail[i] as f32) as f32;
        }
        let tail_db = db_amp(w(self, TAIL_WIN_MS.0, TAIL_WIN_MS.1, &|i| self.tail_sq[i]).sqrt());
        // 2-8 kHz 0-25 ms from the (centred) envelope: mean of env^2
        let hf_db = db_amp(w(self, 0.0, 25.0, &|i| self.hf_env[i] * self.hf_env[i]).sqrt());
        let is_clap = match kind {
            HitKind::Clap => true,
            HitKind::Other => false,
            HitKind::Auto => {
                let mx = self.recent_hf.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
                let c = !mx.is_finite() || hf_db >= mx - 6.0;
                if self.recent_hf.len() >= HF_RECENT {
                    self.recent_hf.pop_front();
                }
                self.recent_hf.push_back(hf_db);
                c
            }
        };
        let n = self.sone_trace.len();
        let tr = |s: &Self, t: usize| s.sone_trace[(t / SONE_HOP) % n];
        let sone_at = tr(self, al + self.ms(30.0));
        let sone_between = match self.last_clap_at {
            Some(prev) if is_clap && al > prev && al - prev < (3.0 * self.sr) as usize => tr(self, (prev + al) / 2),
            _ => 0.0,
        };
        if is_clap {
            self.last_clap_at = Some(al);
        }
        if self.profiles.len() >= MAX_PROFILES {
            self.profiles.pop_front();
        }
        self.profiles.push_back(HitProfile { at: al, is_clap, level_db: level, rail_pct: rail, tail_db, hf_db, sone_at, sone_between });
    }

    fn accumulate(&mut self, a: usize) {
        while let Some(&(s, e)) = self.masks.front() {
            if s <= a {
                self.mask_until = self.mask_until.max(e);
                self.masks.pop_front();
            } else {
                break;
            }
        }
        if a < self.span.0 || a >= self.span.1 {
            return;
        }
        let i = a & self.mask;
        // spread: 50 ms windows every 25 ms from the span start, stereo 500-6k power
        self.spread_sum += self.spread_pow[i] as f64;
        let rel = a - self.span.0 + 1;
        if rel >= self.spread_win {
            let old = self.spread_pow[(a + 1).wrapping_sub(self.spread_win) & self.mask] as f64;
            if (rel - self.spread_win) % (self.spread_win / 2) == 0 {
                if self.spread_levels.len() >= MAX_SPREAD {
                    self.spread_levels.pop_front();
                }
                self.spread_levels.push_back(db_pow(self.spread_sum / self.spread_win as f64));
            }
            self.spread_sum -= old;
        }
        // low-mid peakiness: 100 ms windows every 50 ms of the 250-1k band
        let lm = self.band_sq[i][2] as f64;
        self.lm_sum += lm;
        if rel >= self.lm_win {
            let old = self.band_sq[(a + 1).wrapping_sub(self.lm_win) & self.mask][2] as f64;
            if (rel - self.lm_win) % (self.lm_win / 2) == 0 {
                if self.lm_levels.len() >= MAX_SPREAD {
                    self.lm_levels.pop_front();
                }
                self.lm_levels.push_back(db_pow(self.lm_sum / self.lm_win as f64));
            }
            self.lm_sum -= old;
        }
        if self.silent[i] != 0 {
            return;
        }
        let in_hit = a < self.mask_until;
        let sq = self.band_sq[i];
        let rl = self.rail[i] as u64;
        if in_hit {
            for b in 0..NBANDS {
                self.hits_pow[b] += sq[b] as f64;
            }
            self.hits_n += 1;
            self.hits_rail += rl;
        } else {
            for b in 0..NBANDS {
                self.rest_pow[b] += sq[b] as f64;
            }
            self.rest_n += 1;
            self.rest_rail += rl;
        }
    }

    /// Evaluate what can still be evaluated at the end of a stream (offline use).
    pub fn finish(&mut self) {
        // nothing more arrives; hits whose profile window is incomplete are dropped
        self.pending.clear();
    }

    pub fn profiles(&self) -> impl Iterator<Item = &HitProfile> {
        self.profiles.iter()
    }

    pub fn last_profile(&self) -> Option<&HitProfile> {
        self.profiles.back()
    }

    /// Medians over the stored clap profiles plus the accumulators. Allocation-free (stack
    /// buffers), fine to call from the audio thread every few blocks.
    pub fn report(&self) -> Report {
        let any_clap = self.profiles.iter().any(|p| p.is_clap);
        let sel = |p: &&HitProfile| !any_clap || p.is_clap;
        let n = self.profiles.iter().filter(sel).count();
        let mut r = Report { hits: self.onsets_seen, claps: n, ..Default::default() };
        let mut buf = [0.0f32; MAX_PROFILES];
        let fill = |f: &dyn Fn(&HitProfile) -> Option<f32>, buf: &mut [f32; MAX_PROFILES]| -> usize {
            let mut k = 0;
            for p in self.profiles.iter().filter(sel) {
                if let Some(v) = f(p) {
                    buf[k] = v;
                    k += 1;
                }
            }
            k
        };
        for w in 0..6 {
            let k = fill(&|p| Some(p.level_db[w]), &mut buf);
            r.level_db[w] = median(&mut buf[..k]);
        }
        for w in 0..7 {
            let k = fill(&|p| Some(p.rail_pct[w]), &mut buf);
            r.rail_pct[w] = median(&mut buf[..k]);
        }
        let k = fill(&|p| Some(p.tail_db), &mut buf);
        r.tail_db = median(&mut buf[..k]);
        let k = fill(&|p| Some(p.sone_at), &mut buf);
        let s = median(&mut buf[..k]);
        r.phon_hits = if s > 1e-4 { 40.0 + 10.0 * s.log2() } else { 0.0 };
        let k = fill(&|p| if p.sone_between > 0.0 { Some(p.sone_between) } else { None }, &mut buf);
        let s = median(&mut buf[..k]);
        r.phon_between = if s > 1e-4 { 40.0 + 10.0 * s.log2() } else { 0.0 };
        for b in 0..NBANDS {
            r.band_hits_db[b] = db_pow(self.hits_pow[b] / self.hits_n.max(1) as f64);
            r.band_rest_db[b] = db_pow(self.rest_pow[b] / self.rest_n.max(1) as f64);
        }
        let mut sp = [0.0f32; MAX_SPREAD];
        let mut k = 0;
        for &v in self.spread_levels.iter() {
            sp[k] = v;
            k += 1;
        }
        r.spread_db = if k >= 4 { percentile(&mut sp[..k], 90.0) - percentile(&mut sp[..k], 10.0) } else { f32::NAN };
        let mut k = 0;
        for &v in self.lm_levels.iter() {
            sp[k] = v;
            k += 1;
        }
        r.lowmid_peak_db = if k >= 8 { percentile(&mut sp[..k], 99.0) - percentile(&mut sp[..k], 50.0) } else { f32::NAN };
        let tot = self.hits_n + self.rest_n;
        for b in 0..NBANDS {
            r.band_all_db[b] = db_pow((self.hits_pow[b] + self.rest_pow[b]) / tot.max(1) as f64);
        }
        r.pinned_pct = 100.0 * (self.hits_rail + self.rest_rail) as f32 / tot.max(1) as f32;
        r.pinned_hits_pct = 100.0 * self.hits_rail as f32 / self.hits_n.max(1) as f32;
        r.pinned_rest_pct = 100.0 * self.rest_rail as f32 / self.rest_n.max(1) as f32;
        let ms = self.sone_sum / self.sone_n.max(1) as f64;
        r.phon = if ms > 1e-4 { (40.0 + 10.0 * ms.log2()) as f32 } else { 0.0 };
        r
    }
}

/// Offline analysis of a whole buffer with known hits `(sample, kind)`, sorted by sample.
/// `span` limits the accumulators (default: first hit to last hit, like the offline scripts).
pub fn analyze_buffer(l: &[f32], r: &[f32], sr: f32, hits: &[(usize, HitKind)], span: Option<(usize, usize)>) -> Report {
    let mut an = Analyzer::new(sr);
    let (lo, hi) = span.unwrap_or_else(|| {
        let claps: Vec<usize> = hits.iter().filter(|h| h.1 != HitKind::Other).map(|h| h.0).collect();
        match (claps.first(), claps.last()) {
            (Some(&a), Some(&b)) if b > a => (a, b),
            _ => (0, usize::MAX),
        }
    });
    an.set_span(lo, hi);
    let mut next = 0;
    let n = l.len().min(r.len());
    for i in 0..n {
        an.push(l[i], r[i]);
        while next < hits.len() && hits[next].0 < i + 1 {
            an.onset(hits[next].0, hits[next].1);
            next += 1;
        }
    }
    an.finish();
    an.report()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn synthetic_hits_profile() {
        // 1 s of -40 dB noise bed with a 150 ms burst at 0 dB every 500 ms
        let sr = 44100.0;
        let n = 3 * 44100;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let hits: Vec<(usize, HitKind)> = (1..5).map(|k| (k * 22050, HitKind::Clap)).collect();
        for i in 0..n {
            let mut g = 0.01;
            for h in &hits {
                if i >= h.0 && i < h.0 + 6615 {
                    g = 1.0;
                }
            }
            l[i] = (rnd() * g).clamp(-1.0, 1.0);
            r[i] = (rnd() * g).clamp(-1.0, 1.0);
        }
        let rep = analyze_buffer(&l, &r, sr, &hits, Some((22050, 3 * 44100)));
        assert_eq!(rep.claps, 4, "{rep:?}");
        // uniform noise burst, mid channel RMS = sqrt(1/6) → -7.8 dBFS
        assert!((rep.level_db[1] + 7.8).abs() < 1.0, "{:?}", rep.level_db);
        // 120-200 ms window: 30 ms of burst + 50 ms of bed → -7.8 - 10log10(80/30) ≈ -12.0
        assert!((rep.level_db[5] + 12.0).abs() < 1.0, "{:?}", rep.level_db);
        assert!(rep.band_delta(0) > 25.0, "{rep:?}");
        assert!(rep.pinned_pct < 1.0);
    }
}
