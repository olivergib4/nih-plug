//! Hit analysis: the measurements the plugin shows live and computes for a reference file.
//! Streaming, allocation-free after construction, one instance per analysed signal. Fed with
//! stereo samples and with onset times (from the onset detector live, from a list offline), it
//! keeps:
//!
//! - **hit profile**: per clap, the mid-channel RMS in windows after the (HF-aligned) onset
//!   (0-3 / 3-10 / 10-25 / 25-60 / 60-120 / 120-200 ms) and the share of samples at the rail
//!   (either channel >= 0.999) in windows 0-10 … 120-150 ms; 5-10 kHz level at 25-60 ms;
//! - **drums vs rest**: mean power per band (full, <250, 250-1k, 1-4k, 4-10k; mid channel)
//!   inside the 150 ms after the claps and the opposite-beat hits versus everywhere else,
//!   silence excluded;
//! - **500-6 kHz spread**: p90 - p10 of 50 ms window levels (stereo), the "bricked" readout;
//! - **250-1k peakiness**: p99 - median of 100 ms window levels of the low mids;
//! - **pinned share**: samples at the rail overall, inside hits, between hits;
//! - **loudness**: mean sone/phon (stereo), at claps (30 ms after) and between claps.
//!
//! Which hits are *claps*: with `HitKind::Auto` the analyser finds the half-bar grid (period
//! 1.2-2.4 s, the phase that collects the most hits in the last 20 s) and flags the hits on it,
//! like the offline detector; until a grid locks (4 hits) the loudest-2-8 kHz rule is used.
//! The *opposite-beat hit* is the loudest other hit between two consecutive claps. The
//! drums/rest accumulation runs ~2.2 s behind the input so those decisions are known in time.
//!
//! Definitions follow the offline Python scripts the targets were measured with (bite.py,
//! dom.py, measure_bounce.py); band filters are the same 4th-order Butterworth designs, applied
//! twice (|H|^2) to match the zero-phase offline filtering. Onset alignment: the first sample in
//! [onset - 40 ms, onset + 60 ms] where the 1 ms RMS envelope of the 2-8 kHz band reaches -6 dB
//! of its peak in that range (500-8 kHz for hits that are not claps).

use crate::dsp::butter::{butter_bandpass, SosChain};
use crate::dsp::sone::{SoneMeter, HOP as SONE_HOP};
use serde::{Deserialize, Serialize};

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
pub const LOWMID_WIN_MS: f32 = 100.0;
pub const RAIL: f32 = 0.999;
/// 50 ms mean power below this (-50 dBFS) is silence and excluded from the masks.
pub const SILENCE_POW: f32 = 1e-5;
pub const MAX_HITS: usize = 256;
const ALIGN_PRE_MS: f32 = 40.0;
const ALIGN_POST_MS: f32 = 60.0;
/// Alignment and the 0-25 ms levels need this much audio after the onset.
const ALIGN_WAIT_MS: f32 = 90.0;
const PROFILE_MS: f32 = 200.0;
const ACC_DELAY_S: f32 = 2.2;
const GRID_WINDOW_S: f32 = 20.0;
const GRID_MIN_HITS: usize = 4;
const GRID_P_LO: f32 = 1.2;
const GRID_P_HI: f32 = 2.4;
const GRID_BIN_S: f32 = 0.02;
const GRID_TOL_S: f32 = 0.03;
const CLASSIFY_EVERY_S: f32 = 0.5;
/// Hits dimmer than this below the brightest recent hit are not clap candidates.
const GRID_HF_RANGE_DB: f32 = 10.0;
const MAX_PENDING: usize = 32;
const MAX_SPREAD: usize = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HitKind {
    /// Profile this hit (clap / snare).
    Clap,
    /// Only a candidate for the opposite-beat mask.
    Other,
    /// Decide from the half-bar grid (fallback: within 6 dB of the loudest recent 2-8 kHz hit).
    Auto,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct HitProfile {
    /// Aligned onset, absolute sample index.
    pub at: usize,
    pub kind_auto: bool,
    pub is_clap: bool,
    pub is_opp: bool,
    pub evaluated: bool,
    pub level_db: [f32; 6],
    pub rail_pct: [f32; 7],
    pub tail_db: f32,
    /// 2-8 kHz RMS over 0-25 ms, dBFS.
    pub hf_db: f32,
    /// Full-band mid RMS over 0-25 ms, dBFS.
    pub lvl_db: f32,
    pub sone_at: f32,
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Report {
    pub hits: usize,
    pub claps: usize,
    /// Half-bar period the clap grid locked to, seconds (0 = not locked).
    pub period_s: f32,
    pub level_db: [f32; 6],
    pub rail_pct: [f32; 7],
    pub tail_db: f32,
    pub band_hits_db: [f32; NBANDS],
    pub band_rest_db: [f32; NBANDS],
    /// Mean power per band over hits and rest together.
    pub band_all_db: [f32; NBANDS],
    pub spread_db: f32,
    /// 250-1k band, mid: p99 - median of 100 ms window levels (low-mid peakiness; growl bumps).
    pub lowmid_peak_db: f32,
    pub pinned_pct: f32,
    pub pinned_hits_pct: f32,
    pub pinned_rest_pct: f32,
    pub phon: f32,
    pub phon_hits: f32,
    pub phon_between: f32,
    /// Seconds of non-silent audio the statistics cover.
    #[serde(default)]
    pub measured_s: f32,
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
}

/// Moving RMS envelope over `len` samples.
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
    pos: usize,
    delay: usize,
    span: (usize, usize),
    // short rings (0.5 s): profile evaluation
    smask: usize,
    mid: Vec<f32>,
    hf_env: Vec<f32>,
    mf_env: Vec<f32>,
    tail_sq: Vec<f32>,
    // long rings (> delay): accumulation
    lmask: usize,
    rail: Vec<u8>,
    silent: Vec<u8>,
    band_sq: Vec<[f32; NBANDS]>,
    spread_pow: Vec<f32>,
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
    // hits
    pending: Vec<Pending>,
    hits: Vec<HitProfile>,
    hit_count: u64,
    eval_k: u64,
    acc_k: u64,
    mask_until: usize,
    onsets_seen: usize,
    period_s: f32,
    last_classify: usize,
    /// Next sample index the accumulation has not seen yet.
    acc_upto: usize,
    // accumulators
    hits_pow: [f64; NBANDS],
    rest_pow: [f64; NBANDS],
    hits_n: u64,
    rest_n: u64,
    hits_rail: u64,
    rest_rail: u64,
    spread_sum: f64,
    /// Samples accumulated since the statistics were last cleared (windows fill from here).
    win_cnt: usize,
    spread_win: usize,
    spread_levels: Vec<f32>,
    spread_n: usize,
    spread_head: usize,
    lm_sum: f64,
    lm_win: usize,
    lm_levels: Vec<f32>,
    lm_n: usize,
    lm_head: usize,
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

fn phon_of(sone: f32) -> f32 {
    if sone > 1e-4 {
        40.0 + 10.0 * sone.log2()
    } else {
        0.0
    }
}

/// Half-bar grid over hit times (seconds): period in [1.2, 2.4) s (1 ms steps) and phase (20 ms
/// bins) that collect the most hits. Returns (count, period, phase). Allocation-free.
pub fn grid_fit(times: &[f32]) -> (usize, f32, f32) {
    let mut best = (0usize, 0.0f32, 0.0f32);
    let mut hist = [0u16; 128];
    let mut p = GRID_P_LO;
    while p < GRID_P_HI {
        let nbins = ((p / GRID_BIN_S) as usize + 1).min(hist.len());
        hist[..nbins].fill(0);
        for &t in times {
            let ph = t.rem_euclid(p);
            let k = ((ph / p) * nbins as f32) as usize;
            hist[k.min(nbins - 1)] += 1;
        }
        let (k, &c) = hist[..nbins].iter().enumerate().max_by_key(|(_, &c)| c).unwrap();
        if c as usize > best.0 {
            best = (c as usize, p, (k as f32 + 0.5) * p / nbins as f32);
        }
        p += 0.001;
    }
    best
}

#[inline]
pub fn on_grid(t: f32, period: f32, phase: f32) -> bool {
    let d = (t - phase + period / 2.0).rem_euclid(period) - period / 2.0;
    d.abs() < GRID_TOL_S
}

/// For a locked period, the phase bin (20 ms) whose hits are brightest among the bins that
/// collect at least 60 % of the best count. Returns the chosen phase.
///
/// `hfs` is the 2-8 kHz level of the first 25 ms of each hit; `tails` is its 5-10 kHz level at
/// 25-60 ms (NaN while a hit is not measured yet). A kick layered with a bright zap can match the
/// clap in the first 25 ms, but only the clap stays bright afterwards, so the tail decides when every
/// candidate phase has at least two measured hits; otherwise the first 25 ms decide.
pub fn brightest_phase(times: &[f32], hfs: &[f32], tails: &[f32], period: f32, best_count: usize, best_phase: f32) -> f32 {
    let nbins = ((period / GRID_BIN_S) as usize + 1).min(128);
    let mut count = [0u16; 128];
    let mut hf_sum = [0.0f32; 128];
    let mut tail_n = [0u16; 128];
    let mut tail_sum = [0.0f32; 128];
    for ((&t, &h), &tl) in times.iter().zip(hfs).zip(tails) {
        let k = (((t.rem_euclid(period)) / period) * nbins as f32) as usize;
        let k = k.min(nbins - 1);
        count[k] += 1;
        hf_sum[k] += h;
        if tl.is_finite() {
            tail_n[k] += 1;
            tail_sum[k] += tl;
        }
    }
    // A phase can straddle two bins (the fitted period is never exact, so a phase drifts by a few ms over the
    // window), so each candidate is a bin together with its two neighbours.
    let at = |v: &[u16; 128], k: usize| v[(k + nbins - 1) % nbins] + v[k] + v[(k + 1) % nbins];
    let atf = |v: &[f32; 128], k: usize| v[(k + nbins - 1) % nbins] + v[k] + v[(k + 1) % nbins];
    let min_count = ((best_count as f32) * 0.6).ceil().max(GRID_MIN_HITS as f32) as u16;
    let use_tail = (0..nbins).filter(|&k| at(&count, k) >= min_count).all(|k| at(&tail_n, k) >= 2);
    let mut best = (f32::NEG_INFINITY, 0u16, usize::MAX);
    for k in 0..nbins {
        let c = at(&count, k);
        if c >= min_count {
            let mean = if use_tail { atf(&tail_sum, k) / at(&tail_n, k) as f32 } else { atf(&hf_sum, k) / c as f32 };
            // brighter wins; among equally bright neighbours (the same hits seen from the next bin), the fuller
            if mean > best.0 + 0.5 || (mean > best.0 - 0.5 && c > best.1) {
                best = (mean.max(best.0), c, k);
            }
        }
    }
    if best.2 == usize::MAX {
        return best_phase;
    }
    // phase = mean position of the hits in the chosen bins, measured from the middle bin's centre
    let centre = (best.2 as f32 + 0.5) * period / nbins as f32;
    let (mut acc, mut m) = (0.0f32, 0u32);
    for &t in times {
        let d = (t - centre + period / 2.0).rem_euclid(period) - period / 2.0;
        if d.abs() <= 1.5 * period / nbins as f32 {
            acc += d;
            m += 1;
        }
    }
    (centre + if m > 0 { acc / m as f32 } else { 0.0 }).rem_euclid(period)
}

impl Analyzer {
    pub fn new(sample_rate: f32) -> Self {
        let sr = sample_rate as f64;
        let ms = |m: f32| (m * sample_rate / 1000.0) as usize;
        let sring = ((0.5 * sample_rate) as usize).next_power_of_two();
        let delay = (ACC_DELAY_S * sample_rate) as usize;
        let lring = (delay + ms(300.0)).next_power_of_two();
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
        let sone_len = (GRID_WINDOW_S * sample_rate) as usize / SONE_HOP + 2;
        Analyzer {
            sr: sample_rate,
            pos: 0,
            delay,
            span: (0, usize::MAX),
            smask: sring - 1,
            mid: vec![0.0; sring],
            hf_env: vec![0.0; sring],
            mf_env: vec![0.0; sring],
            tail_sq: vec![0.0; sring],
            lmask: lring - 1,
            rail: vec![0; lring],
            silent: vec![1; lring],
            band_sq: vec![[0.0; NBANDS]; lring],
            spread_pow: vec![0.0; lring],
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
            hits: vec![HitProfile::default(); MAX_HITS],
            hit_count: 0,
            eval_k: 0,
            acc_k: 0,
            mask_until: 0,
            onsets_seen: 0,
            period_s: 0.0,
            last_classify: 0,
            acc_upto: 0,
            hits_pow: [0.0; NBANDS],
            rest_pow: [0.0; NBANDS],
            hits_n: 0,
            rest_n: 0,
            hits_rail: 0,
            rest_rail: 0,
            spread_sum: 0.0,
            win_cnt: 0,
            spread_win: sil_len,
            spread_levels: vec![0.0; MAX_SPREAD],
            spread_n: 0,
            spread_head: 0,
            lm_sum: 0.0,
            lm_win: ms(LOWMID_WIN_MS),
            lm_levels: vec![0.0; MAX_SPREAD],
            lm_n: 0,
            lm_head: 0,
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
        self.hf_env.fill(0.0);
        self.mf_env.fill(0.0);
        self.tail_sq.fill(0.0);
        self.rail.fill(0);
        self.silent.fill(1);
        for b in &mut self.band_sq {
            *b = [0.0; NBANDS];
        }
        self.spread_pow.fill(0.0);
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
        self.clear_accumulators();
        self.sone.reset();
        self.sone_trace.fill(0.0);
        self.last_classify = 0;
    }

    /// Restart the statistics (hits, drums/rest, pinned, spread, loudness), keeping the signal
    /// state.
    pub fn clear_accumulators(&mut self) {
        self.hit_count = 0;
        self.eval_k = 0;
        self.acc_k = 0;
        self.mask_until = 0;
        self.onsets_seen = 0;
        self.period_s = 0.0;
        self.hits_pow = [0.0; NBANDS];
        self.rest_pow = [0.0; NBANDS];
        self.hits_n = 0;
        self.rest_n = 0;
        self.hits_rail = 0;
        self.rest_rail = 0;
        self.spread_sum = 0.0;
        self.win_cnt = 0;
        self.spread_n = 0;
        self.spread_head = 0;
        self.lm_sum = 0.0;
        self.lm_n = 0;
        self.lm_head = 0;
        self.sone_sum = 0.0;
        self.sone_n = 0;
    }

    /// Register a hit at absolute sample `at` (may be in the past, up to ~250 ms).
    pub fn onset(&mut self, at: usize, kind: HitKind) {
        if self.pending.len() >= MAX_PENDING {
            self.pending.remove(0);
        }
        if self.pos.saturating_sub(at) + self.ms(ALIGN_PRE_MS) + 8 >= self.smask {
            return; // too old for the ring
        }
        self.onsets_seen += 1;
        self.pending.push(Pending { onset: at, kind });
    }

    #[inline]
    fn ms(&self, m: f32) -> usize {
        (m * self.sr / 1000.0) as usize
    }

    #[inline]
    fn hit(&self, k: u64) -> &HitProfile {
        &self.hits[(k % MAX_HITS as u64) as usize]
    }

    #[inline]
    fn hit_mut(&mut self, k: u64) -> &mut HitProfile {
        &mut self.hits[(k % MAX_HITS as u64) as usize]
    }

    fn oldest_k(&self) -> u64 {
        self.hit_count.saturating_sub(MAX_HITS as u64)
    }

    /// Push one stereo sample.
    #[inline]
    pub fn push(&mut self, l: f32, r: f32) {
        let l = l.clamp(-1.0, 1.0);
        let r = r.clamp(-1.0, 1.0);
        let m = 0.5 * (l + r);
        let si = self.pos & self.smask;
        let li = self.pos & self.lmask;
        self.mid[si] = m;
        self.rail[li] = (l.abs() >= RAIL || r.abs() >= RAIL) as u8;
        let md = m as f64;
        let hf = self.hf.process(md) as f32;
        let mf = self.mf.process(md) as f32;
        let he = self.hf_e.push(hf * hf);
        let me = self.mf_e.push(mf * mf);
        let ci = self.pos.wrapping_sub(self.env_half) & self.smask;
        self.hf_env[ci] = he;
        self.mf_env[ci] = me;
        let mut sq = [0.0f32; NBANDS];
        sq[0] = m * m;
        for (k, b) in self.bands.iter_mut().enumerate() {
            let y = b.process(md) as f32;
            sq[k + 1] = y * y;
        }
        self.band_sq[li] = sq;
        let t = self.tail.process(md) as f32;
        self.tail_sq[si] = t * t;
        let sl = self.spread_l.process(l as f64) as f32;
        let sr_ = self.spread_r.process(r as f64) as f32;
        self.spread_pow[li] = 0.5 * (sl * sl + sr_ * sr_);
        let se = self.sil_e.push(m * m);
        let sili = self.pos.wrapping_sub(self.sil_half) & self.lmask;
        self.silent[sili] = (se * se < SILENCE_POW) as u8;
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
        if self.pos - self.last_classify >= (CLASSIFY_EVERY_S * self.sr) as usize {
            self.last_classify = self.pos;
            self.classify();
        }
        if self.pos > self.delay {
            let a = self.pos - 1 - self.delay;
            if a >= self.acc_upto {
                self.accumulate(a);
                self.acc_upto = a + 1;
            }
        }
    }

    fn service_hits(&mut self) {
        let wait = self.ms(ALIGN_WAIT_MS);
        let post = self.ms(ALIGN_POST_MS);
        let pre = self.ms(ALIGN_PRE_MS);
        let prof = self.ms(PROFILE_MS);
        // align pending onsets
        let mut k = 0;
        while k < self.pending.len() {
            let p = self.pending[k];
            if self.pos > p.onset + wait {
                let a0 = p.onset.saturating_sub(pre);
                let a1 = p.onset + post;
                let use_hf = match p.kind {
                    HitKind::Clap => true,
                    HitKind::Other => false,
                    HitKind::Auto => {
                        let mut pk = 0.0f32;
                        for t in a0..a1 {
                            pk = pk.max(self.hf_env[t & self.smask]);
                        }
                        pk > 0.01
                    }
                };
                let mut pk = 0.0f32;
                for t in a0..a1 {
                    let v = if use_hf { self.hf_env[t & self.smask] } else { self.mf_env[t & self.smask] };
                    pk = pk.max(v);
                }
                let thr = pk * 0.501_187_2; // -6 dB
                let mut al = a0;
                for t in a0..a1 {
                    let v = if use_hf { self.hf_env[t & self.smask] } else { self.mf_env[t & self.smask] };
                    if v >= thr {
                        al = t;
                        break;
                    }
                }
                // 0-25 ms levels
                let w25 = self.ms(25.0);
                let (mut hs, mut ls) = (0.0f64, 0.0f64);
                for t in al..al + w25 {
                    let e = self.hf_env[t & self.smask];
                    hs += (e * e) as f64;
                    let m = self.mid[t & self.smask];
                    ls += (m * m) as f64;
                }
                let hf_db = db_amp((hs / w25.max(1) as f64).sqrt());
                let lvl_db = db_amp((ls / w25.max(1) as f64).sqrt());
                let n = self.sone_trace.len();
                let sone_at = self.sone_trace[((al + self.ms(30.0)) / SONE_HOP) % n];
                let rec = HitProfile {
                    at: al,
                    kind_auto: p.kind == HitKind::Auto,
                    is_clap: p.kind == HitKind::Clap,
                    is_opp: false,
                    evaluated: false,
                    hf_db,
                    lvl_db,
                    sone_at,
                    ..Default::default()
                };
                let kk = self.hit_count;
                *self.hit_mut(kk) = rec;
                self.hit_count += 1;
                self.pending.remove(k);
            } else {
                k += 1;
            }
        }
        // evaluate profiles once 200 ms of audio is in
        while self.eval_k < self.hit_count {
            let al = self.hit(self.eval_k).at;
            if self.pos > al + prof {
                self.evaluate(self.eval_k);
                self.eval_k += 1;
            } else {
                break;
            }
        }
    }

    fn evaluate(&mut self, k: u64) {
        let al = self.hit(k).at;
        let w = |s: &Self, a: f32, b: f32, f: &dyn Fn(usize) -> f32| -> f64 {
            let (i0, i1) = (al + s.ms(a), al + s.ms(b));
            let mut acc = 0.0f64;
            for t in i0..i1 {
                acc += f(t) as f64;
            }
            acc / (i1 - i0).max(1) as f64
        };
        let mut level = [0.0f32; 6];
        for (q, (a, b)) in LEVEL_WINS_MS.iter().enumerate() {
            let p = w(self, *a, *b, &|t| {
                let v = self.mid[t & self.smask];
                v * v
            });
            level[q] = db_amp(p.sqrt());
        }
        let mut rail = [0.0f32; 7];
        for (q, (a, b)) in RAIL_WINS_MS.iter().enumerate() {
            rail[q] = 100.0 * w(self, *a, *b, &|t| self.rail[t & self.lmask] as f32) as f32;
        }
        let tail_db = db_amp(w(self, TAIL_WIN_MS.0, TAIL_WIN_MS.1, &|t| self.tail_sq[t & self.smask]).sqrt());
        let h = self.hit_mut(k);
        h.level_db = level;
        h.rail_pct = rail;
        h.tail_db = tail_db;
        h.evaluated = true;
    }

    /// Clap / opposite-beat flags over the stored hits: half-bar grid over the last 20 s of
    /// Auto hits (fallback: loudest-HF rule), then the loudest other hit between claps.
    fn classify(&mut self) {
        let lo = self.oldest_k();
        let hi = self.hit_count;
        if hi == lo {
            return;
        }
        let win = (GRID_WINDOW_S * self.sr) as usize;
        let t0 = self.pos.saturating_sub(win);
        // grid over the bright Auto hits in the window (within 10 dB of the brightest 2-8 kHz
        // hit: claps/snares, not hats or kicks)
        let mut hf_max = f32::NEG_INFINITY;
        for k in lo..hi {
            let h = self.hit(k);
            if h.kind_auto && h.at >= t0 {
                hf_max = hf_max.max(h.hf_db);
            }
        }
        let mut times = [0.0f32; MAX_HITS];
        let mut hfs = [0.0f32; MAX_HITS];
        let mut tails = [f32::NAN; MAX_HITS];
        let mut n = 0;
        for k in lo..hi {
            let h = self.hit(k);
            if h.kind_auto && h.at >= t0 && h.hf_db >= hf_max - GRID_HF_RANGE_DB {
                times[n] = h.at as f32 / self.sr;
                hfs[n] = h.hf_db;
                tails[n] = if h.evaluated { h.tail_db } else { f32::NAN };
                n += 1;
            }
        }
        if hf_max.is_finite() {
            let (count, period, phase) = if n >= GRID_MIN_HITS { grid_fit(&times[..n]) } else { (0, 0.0, 0.0) };
            let locked = count >= GRID_MIN_HITS;
            // among the phases of that period that collect nearly as many hits, take the
            // brightest (the clap/snare, not the kick/zap on the other beat)
            let phase = if locked { brightest_phase(&times[..n], &hfs[..n], &tails[..n], period, count, phase) } else { phase };
            self.period_s = if locked { period } else { 0.0 };
            for k in lo..hi {
                let sr = self.sr;
                let h = self.hit_mut(k);
                if h.kind_auto {
                    h.is_clap = if locked {
                        h.hf_db >= hf_max - GRID_HF_RANGE_DB && on_grid(h.at as f32 / sr, period, phase)
                    } else {
                        h.hf_db >= hf_max - 6.0
                    };
                }
            }
        }
        // opposite: loudest non-clap hit strictly between consecutive claps
        for k in lo..hi {
            self.hit_mut(k).is_opp = false;
        }
        let mut prev_clap: Option<u64> = None;
        for k in lo..hi {
            if self.hit(k).is_clap {
                if let Some(pc) = prev_clap {
                    let mut best: Option<u64> = None;
                    for j in pc + 1..k {
                        let h = self.hit(j);
                        if best.map_or(true, |b| h.lvl_db > self.hit(b).lvl_db) {
                            best = Some(j);
                        }
                    }
                    if let Some(b) = best {
                        self.hit_mut(b).is_opp = true;
                    }
                }
                prev_clap = Some(k);
            }
        }
    }

    fn accumulate(&mut self, a: usize) {
        let hit_len = self.ms(HIT_MS);
        while self.acc_k < self.hit_count {
            let h = *self.hit(self.acc_k);
            if h.at <= a {
                if h.is_clap || h.is_opp {
                    self.mask_until = self.mask_until.max(h.at + hit_len);
                }
                self.acc_k += 1;
            } else {
                break;
            }
        }
        if a < self.span.0 || a >= self.span.1 {
            return;
        }
        let i = a & self.lmask;
        // counted from the last clear (or the span start), so a window never mixes in audio from
        // before the statistics were restarted
        self.win_cnt += 1;
        let rel = self.win_cnt;
        let quiet = self.silent[i] != 0;
        // spread: 50 ms windows every 25 ms from the span start, stereo 500-6k power
        self.spread_sum += self.spread_pow[i] as f64;
        if rel >= self.spread_win {
            let old = self.spread_pow[(a + 1).wrapping_sub(self.spread_win) & self.lmask] as f64;
            if (rel - self.spread_win) % (self.spread_win / 2) == 0 {
                let v = db_pow(self.spread_sum.max(0.0) / self.spread_win as f64);
                if !quiet {
                    self.spread_levels[self.spread_head] = v;
                    self.spread_head = (self.spread_head + 1) % MAX_SPREAD;
                    self.spread_n = (self.spread_n + 1).min(MAX_SPREAD);
                }
            }
            self.spread_sum -= old;
        }
        // low-mid peakiness: 100 ms windows every 50 ms of the 250-1k band
        self.lm_sum += self.band_sq[i][2] as f64;
        if rel >= self.lm_win {
            let old = self.band_sq[(a + 1).wrapping_sub(self.lm_win) & self.lmask][2] as f64;
            if (rel - self.lm_win) % (self.lm_win / 2) == 0 {
                let v = db_pow(self.lm_sum.max(0.0) / self.lm_win as f64);
                if !quiet {
                    self.lm_levels[self.lm_head] = v;
                    self.lm_head = (self.lm_head + 1) % MAX_SPREAD;
                    self.lm_n = (self.lm_n + 1).min(MAX_SPREAD);
                }
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

    /// Bring the statistics up to date with everything pushed so far (classify the stored hits
    /// and accumulate the delayed tail). Used at the end of a file and when playback stops; the
    /// analyser keeps working normally afterwards.
    pub fn flush(&mut self) {
        self.pending.clear();
        self.classify();
        while self.acc_upto < self.pos {
            let a = self.acc_upto;
            self.accumulate(a);
            self.acc_upto += 1;
        }
    }

    /// End of a stream (offline use).
    pub fn finish(&mut self) {
        self.flush();
    }

    /// Seconds of (non-silent) audio in the current statistics.
    pub fn measured_s(&self) -> f32 {
        (self.hits_n + self.rest_n) as f32 / self.sr
    }

    /// Stored hits, oldest first.
    pub fn hits(&self) -> impl Iterator<Item = &HitProfile> {
        (self.oldest_k()..self.hit_count).map(move |k| self.hit(k))
    }

    pub fn last_profile(&self) -> Option<&HitProfile> {
        if self.hit_count == 0 {
            return None;
        }
        let mut k = self.hit_count;
        while k > self.oldest_k() {
            k -= 1;
            let h = self.hit(k);
            if h.evaluated && h.is_clap {
                return Some(h);
            }
        }
        None
    }

    /// Medians over the stored clap profiles plus the accumulators. Allocation-free (stack
    /// buffers), fine to call from the audio thread every few blocks.
    pub fn report(&self) -> Report {
        let lo = self.oldest_k();
        let hi = self.hit_count;
        let mut r = Report { hits: self.onsets_seen, period_s: self.period_s, ..Default::default() };
        let mut buf = [0.0f32; MAX_HITS];
        let fill = |f: &dyn Fn(&HitProfile) -> Option<f32>, buf: &mut [f32; MAX_HITS]| -> usize {
            let mut n = 0;
            for k in lo..hi {
                let h = self.hit(k);
                if h.evaluated && h.is_clap {
                    if let Some(v) = f(h) {
                        buf[n] = v;
                        n += 1;
                    }
                }
            }
            n
        };
        r.claps = fill(&|_| Some(0.0), &mut buf);
        for w in 0..6 {
            let n = fill(&|p| Some(p.level_db[w]), &mut buf);
            r.level_db[w] = median(&mut buf[..n]);
        }
        for w in 0..7 {
            let n = fill(&|p| Some(p.rail_pct[w]), &mut buf);
            r.rail_pct[w] = median(&mut buf[..n]);
        }
        let n = fill(&|p| Some(p.tail_db), &mut buf);
        r.tail_db = median(&mut buf[..n]);
        let n = fill(&|p| Some(p.sone_at), &mut buf);
        r.phon_hits = phon_of(median(&mut buf[..n]));
        // between: sone at the midpoint of consecutive claps (within the trace ring)
        let tn = self.sone_trace.len();
        let mut n = 0;
        let mut prev: Option<usize> = None;
        for k in lo..hi {
            let h = self.hit(k);
            if h.evaluated && h.is_clap {
                if let Some(p) = prev {
                    let mid = (p + h.at) / 2;
                    if self.pos.saturating_sub(mid) < tn * SONE_HOP && h.at - p < (3.0 * self.sr) as usize {
                        buf[n] = self.sone_trace[(mid / SONE_HOP) % tn];
                        n += 1;
                    }
                }
                prev = Some(h.at);
            }
        }
        r.phon_between = phon_of(median(&mut buf[..n]));
        let tot = self.hits_n + self.rest_n;
        for b in 0..NBANDS {
            r.band_hits_db[b] = db_pow(self.hits_pow[b] / self.hits_n.max(1) as f64);
            r.band_rest_db[b] = db_pow(self.rest_pow[b] / self.rest_n.max(1) as f64);
            r.band_all_db[b] = db_pow((self.hits_pow[b] + self.rest_pow[b]) / tot.max(1) as f64);
        }
        let mut sp = [0.0f32; MAX_SPREAD];
        sp[..self.spread_n].copy_from_slice(&self.spread_levels[..self.spread_n]);
        r.spread_db = if self.spread_n >= 4 { percentile(&mut sp[..self.spread_n], 90.0) - percentile(&mut sp[..self.spread_n], 10.0) } else { f32::NAN };
        sp[..self.lm_n].copy_from_slice(&self.lm_levels[..self.lm_n]);
        r.lowmid_peak_db = if self.lm_n >= 8 { percentile(&mut sp[..self.lm_n], 99.0) - percentile(&mut sp[..self.lm_n], 50.0) } else { f32::NAN };
        r.measured_s = self.measured_s();
        r.pinned_pct = 100.0 * (self.hits_rail + self.rest_rail) as f32 / tot.max(1) as f32;
        r.pinned_hits_pct = 100.0 * self.hits_rail as f32 / self.hits_n.max(1) as f32;
        r.pinned_rest_pct = 100.0 * self.rest_rail as f32 / self.rest_n.max(1) as f32;
        let ms = self.sone_sum / self.sone_n.max(1) as f64;
        r.phon = if ms > 1e-4 { (40.0 + 10.0 * ms.log2()) as f32 } else { 0.0 };
        r
    }
}

/// Offline analysis of a whole buffer with known hits `(sample, kind)`, sorted by sample.
/// `span` limits the accumulators (default: first clap to last clap, like the offline scripts).
pub fn analyze_buffer(l: &[f32], r: &[f32], sr: f32, hits: &[(usize, HitKind)], span: Option<(usize, usize)>) -> Report {
    let mut an = Analyzer::new(sr);
    let (lo, hi) = span.unwrap_or_else(|| {
        let claps: Vec<usize> = hits.iter().filter(|h| h.1 == HitKind::Clap).map(|h| h.0).collect();
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
        // -40 dB noise bed with a 150 ms burst at 0 dB every 500 ms
        let sr = 44100.0;
        let n = 4 * 44100;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        let mut seed = 1u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        let hits: Vec<(usize, HitKind)> = (1..6).map(|k| (k * 22050, HitKind::Clap)).collect();
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
        let rep = analyze_buffer(&l, &r, sr, &hits, Some((22050, 4 * 44100)));
        assert_eq!(rep.claps, 5, "{rep:?}");
        // uniform noise burst, mid channel RMS = sqrt(1/6) → -7.8 dBFS
        assert!((rep.level_db[1] + 7.8).abs() < 1.0, "{:?}", rep.level_db);
        // 120-200 ms window: 30 ms of burst + 50 ms of bed → -7.8 - 10log10(80/30) ≈ -12.0
        assert!((rep.level_db[5] + 12.0).abs() < 1.0, "{:?}", rep.level_db);
        assert!(rep.band_delta(0) > 25.0, "{rep:?}");
        assert!(rep.pinned_pct < 1.0);
    }

    #[test]
    fn spread_is_sane_after_restart() {
        // loud noise, then clear mid-stream: windows must not mix pre-clear audio (was ~190 dB)
        let sr = 44100.0;
        let mut an = Analyzer::new(sr);
        let mut seed = 9u32;
        let mut rnd = || {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        };
        for _ in 0..(3 * 44100) {
            let v = rnd() * 0.5;
            an.push(v, v);
        }
        an.clear_accumulators();
        for _ in 0..(3 * 44100) {
            let v = rnd() * 0.5;
            an.push(v, v);
        }
        an.flush();
        let r = an.report();
        assert!(r.spread_db.is_finite() && r.spread_db < 3.0, "{}", r.spread_db);
        assert!(r.lowmid_peak_db.is_finite() && r.lowmid_peak_db < 3.0, "{}", r.lowmid_peak_db);
    }

    #[test]
    fn brightest_phase_handles_drift_and_bright_kicks() {
        // period 1.654 s; kicks at phase ~0.667 (steady), claps at phase 1.487..1.500 (drifting over a bin edge)
        let period = 1.654f32;
        let (mut times, mut hfs, mut tails) = (vec![], vec![], vec![]);
        for i in 0..12 {
            let base = 5.0 + i as f32 * 1.6552;
            times.push(base);
            hfs.push(-6.0); // kick with a zap: as bright as the clap at first
            tails.push(-25.0);
            times.push(base + 0.8276);
            hfs.push(-6.0);
            tails.push(-11.0);
        }
        let kick_phase = times[0].rem_euclid(period);
        let ph = brightest_phase(&times, &hfs, &tails, period, 12, kick_phase);
        let clap_phase = times[1].rem_euclid(period);
        let d = (ph - clap_phase + period / 2.0).rem_euclid(period) - period / 2.0;
        assert!(d.abs() < 0.02, "picked {ph}, clap at {clap_phase}, kick at {kick_phase}");
        for (i, &t) in times.iter().enumerate() {
            assert_eq!(on_grid(t, period, ph), i % 2 == 1, "hit {i}");
        }
    }

    #[test]
    fn grid_fit_finds_period() {
        let times: Vec<f32> = (0..12).map(|k| 1.0 + k as f32 * 1.714).collect();
        let (c, p, ph) = grid_fit(&times);
        assert_eq!(c, 12);
        assert!((p - 1.714).abs() < 0.002, "{p}");
        for &t in &times {
            assert!(on_grid(t, p, ph));
        }
        assert!(!on_grid(1.0 + 0.857, p, ph));
    }
}
