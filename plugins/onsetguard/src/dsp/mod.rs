//! OnsetGuard DSP engine. Pure DSP, no plugin-framework types, so it can be unit tested.
//!
//! Signal flow (per channel, all stages zero-phase and sample-aligned):
//!   input ─┬─ long STFT (mono-below + low-band dynamics) ─ short STFT (high-band dynamics) ─┐
//!          ├─ long STFT (mono-below only, reference) ─ delay ───────────────────────────────┤
//!          ├─ onset detector ─ protection / transient rings ─────────────────────────────── blend
//!          │                                                                                  │
//!          │   drive ─ short STFT (per-bin ceiling) ─ oversampled soft clip ─ transient blend ─┤
//!          └─ dry delay ──────────────────────────────────────────────────── mix / matched bypass

#![allow(clippy::needless_range_loop)]

pub mod clip;
pub mod dynamics;
pub mod loudness;
pub mod onset;
pub mod sone;
pub mod stft;

use dynamics::{Band, Dynamics, DynamicsSettings, Weighting};
use realfft::num_complex::Complex32;

pub const MIN_LONG_ORDER: usize = 9;
pub const MAX_LONG_ORDER: usize = 15;
pub const MIN_SHORT_ORDER: usize = 6;
pub const MAX_SHORT_ORDER: usize = 12;
pub const MAX_OVERSAMPLE: usize = 8;
/// Maximum protection pre-window the latency budget accounts for.
pub const MAX_PRE_MS: f32 = 120.0;
const TIMING_FADE_S: f32 = 0.15;

/// What happens to the band below `sub_hz` in the clip stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SubMode {
    /// Everything goes through one clipper.
    Off,
    /// The sub bypasses the spectral and soft clippers and is summed back clean. Its peak
    /// envelope reserves headroom, so the rest is clipped into whatever is left: a clean sub
    /// costs loudness above it.
    Protect,
    /// The sub gets its own clipper at its own ceiling (`sub_ceil_db` below the output
    /// ceiling) and the rest is clipped into the remaining headroom; no intermodulation
    /// between them.
    Separate,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub dyn_: DynamicsSettings,
    pub long_order: usize,
    pub short_order: usize,

    pub protect_on: bool,
    pub onset_sens: f32,
    pub pre_ms: f32,
    pub hold_ms: f32,
    pub post_ms: f32,
    pub trans_boost_db: f32,
    pub trans_boost_ms: f32,

    pub clip_on: bool,
    pub drive_db: f32,
    pub bin_ceil_db: f32,
    pub ceil_db: f32,
    pub hardness: f32,
    pub trans_clip: f32,
    pub clip_link: f32,
    pub oversample: usize,
    /// Sub split mode and crossover. The sub band is separated inside the clip STFT (zero-phase).
    /// Preserve highs: the band above `hf_hz` bypasses the clipper and is summed back; its
    /// peak envelope reserves (a little) headroom so the ceiling still holds.
    pub hf_preserve: bool,
    pub hf_hz: f32,
    pub sub_mode: SubMode,
    pub sub_hz: f32,
    /// Separate mode: the sub's own ceiling, dB relative to the output ceiling (<= 0).
    pub sub_ceil_db: f32,

    pub mix: f32,
    pub output_gain: f32,
    pub matched_bypass: bool,
    /// Playback calibration for the sone model: a full-scale sine plays at this dB SPL.
    pub ref_spl: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            dyn_: DynamicsSettings {
                weighting: Weighting::Off,
                threshold_db: -24.0,
                center_hz: 1000.0,
                slope_db_oct: -4.5,
                curve_db_oct2: 0.0,
                down_ratio: 3.0,
                up_ratio: 2.0,
                up_range_db: 12.0,
                attack_ms: 10.0,
                release_ms: 120.0,
                smooth_oct: 0.17,
                stereo_link: 1.0,
                crossover_hz: 300.0,
                mono_below_hz: 120.0,
            },
            long_order: 13,
            short_order: 10,
            protect_on: true,
            onset_sens: 0.6,
            pre_ms: 25.0,
            hold_ms: 4.0,
            post_ms: 30.0,
            trans_boost_db: 0.0,
            trans_boost_ms: 15.0,
            clip_on: true,
            drive_db: 0.0,
            bin_ceil_db: 0.0,
            ceil_db: 0.0,
            hardness: 1.0,
            trans_clip: 1.0,
            clip_link: 1.0,
            oversample: 4,
            hf_preserve: true,
            hf_hz: 8000.0,
            sub_mode: SubMode::Off,
            sub_hz: 100.0,
            sub_ceil_db: -6.0,
            mix: 1.0,
            output_gain: 1.0,
            matched_bypass: false,
            ref_spl: 100.0,
        }
    }
}

/// Multi-channel delay line with a fixed maximum and a settable current delay.
struct Delay {
    buf: Vec<Vec<f32>>,
    len: usize,
    pos: usize,
    delay: usize,
}

impl Delay {
    fn new(num_channels: usize, max_delay: usize) -> Self {
        let len = (max_delay + 1).next_power_of_two();
        Delay { buf: vec![vec![0.0; len]; num_channels], len, pos: 0, delay: 0 }
    }
    fn set(&mut self, d: usize) {
        assert!(d < self.len);
        self.delay = d;
    }
    fn reset(&mut self) {
        for b in &mut self.buf {
            b.fill(0.0);
        }
        self.pos = 0;
    }
    /// Push one sample per channel, returning the delayed values into `out`.
    #[inline]
    fn tick(&mut self, input: &[f32], out: &mut [f32]) {
        let rp = (self.pos + self.len - self.delay) & (self.len - 1);
        for ch in 0..input.len() {
            self.buf[ch][self.pos] = input[ch];
            out[ch] = self.buf[ch][rp];
        }
        self.pos = (self.pos + 1) & (self.len - 1);
    }
}

/// A ring of per-sample envelope values indexed by absolute input time, with a reset value.
struct EnvRing {
    buf: Vec<f32>,
    mask: usize,
    reset_value: f32,
}

impl EnvRing {
    fn new(min_len: usize, reset_value: f32) -> Self {
        let len = min_len.next_power_of_two();
        EnvRing { buf: vec![reset_value; len], mask: len - 1, reset_value }
    }
    fn clear(&mut self) {
        self.buf.fill(self.reset_value);
    }
    #[inline]
    fn take(&mut self, t: usize) -> f32 {
        let i = t & self.mask;
        let v = self.buf[i];
        self.buf[i] = self.reset_value;
        v
    }
    #[inline]
    fn min_at(&mut self, t: usize, v: f32) {
        let i = t & self.mask;
        if v < self.buf[i] {
            self.buf[i] = v;
        }
    }
    #[inline]
    fn max_at(&mut self, t: usize, v: f32) {
        let i = t & self.mask;
        if v > self.buf[i] {
            self.buf[i] = v;
        }
    }
}

pub struct Engine {
    num_channels: usize,
    sample_rate: f32,
    max_block: usize,

    long_stft: stft::StftStage, // 4*num_channels: [proc.., ref.., sub_proc.., sub_ref..]
    short_stft: stft::StftStage,
    /// Spectral clip on the rest band.
    clip_stft: stft::StftStage,
    sub_oversampler: clip::Oversampler,
    low_dyn: Dynamics,
    high_dyn: Dynamics,
    oversampler: clip::Oversampler,
    detector: onset::OnsetDetector,

    ref_delay: Delay,   // reference after long stage: + short + extra
    sub_a_delay: Delay, // sub (processed) after long stage: + short + extra
    sub_r_delay: Delay, // sub (reference): + short + extra
    sub_clip_delay: Delay, // sub in the clip path: + clip stft window
    proc_delay: Delay,  // processed after short stage: + extra
    driven_delay: Delay, // driven signal: + clip stft + oversampler
    dry_delay: Delay,   // total latency

    p_ring: EnvRing,  // protection (1 = processed, 0 = reference)
    t_ring: EnvRing,  // transient boost (0 = none)
    pc_ring: EnvRing, // clip protection (1 = clip fully, 0 = per trans_clip)

    /// Absolute input sample counter.
    pos: usize,
    extra_delay: usize,
    latency: usize,

    timing_scale: f32,
    k_dry: loudness::KMeter,
    k_wet: loudness::KMeter,
    /// Sone meters: processed output, and dry scaled by `match_gain` (so the bypass servo
    /// matches in sones, not in a proxy).
    sone_wet: sone::SoneMeter,
    sone_dry: sone::SoneMeter,
    match_gain: f32,
    /// Gain reduction meters (dB, <= 0): spectral (per-bin ceiling) stage and post soft clip.
    gr_spec_db: f32,
    gr_post_db: f32,
    pending_pk_spec: f32,
    pending_clip_on: bool,
    /// Fraction of the last block where the protection envelope was active (for display).
    protect_vis: f32,

    // scratch, sized max_block
    wide: Vec<Vec<f32>>, // 4*num_channels for the long stage
    a: Vec<Vec<f32>>,
    refd: Vec<Vec<f32>>,
    driven: Vec<Vec<f32>>,
    cl: Vec<Vec<f32>>,
    /// 3*num_channels scratch for the clip stage: [rest.., sub.., highs..]
    clw: Vec<Vec<f32>>,
    hf_delay: Delay, // highs: + oversampler latency
    hf_env: f32,
    hf_hold: usize,
    /// Sub band (processed, reference) after the long stage, then the blended driven sub.
    sub_a: Vec<Vec<f32>>,
    sub_r: Vec<Vec<f32>>,
    clsub: Vec<Vec<f32>>,
    /// Protect mode: per-sample ceiling left for the rest after the sub's peak envelope.
    rest_ceil: Vec<f32>,
    sub_env: f32,
    sub_hold: usize,
    dry: Vec<Vec<f32>>,
    tmp_in: Vec<f32>,
    tmp_out: Vec<f32>,
    cur: Settings,
}

impl Engine {
    pub fn new(num_channels: usize, sample_rate: f32, max_block: usize) -> Self {
        let max_long = 1 << MAX_LONG_ORDER;
        let max_short = 1 << MAX_SHORT_ORDER;
        let pre_max = (MAX_PRE_MS / 1000.0 * sample_rate).ceil() as usize;
        let extra_max = pre_max + onset::MAX_LATENCY;
        let ring_len = max_long + 2 * max_short + extra_max + 2 * pre_max + sample_rate as usize + max_block + clip::LATENCY;
        let mut e = Engine {
            num_channels,
            sample_rate,
            max_block,
            long_stft: stft::StftStage::new(4 * num_channels, MIN_LONG_ORDER, MAX_LONG_ORDER),
            short_stft: stft::StftStage::new(num_channels, MIN_SHORT_ORDER, MAX_SHORT_ORDER),
            clip_stft: stft::StftStage::new(2 * num_channels, MIN_SHORT_ORDER, MAX_SHORT_ORDER),
            sub_oversampler: clip::Oversampler::new(num_channels, MAX_OVERSAMPLE),
            low_dyn: Dynamics::new(Band::Low, num_channels, max_long),
            high_dyn: Dynamics::new(Band::High, num_channels, max_short),
            oversampler: clip::Oversampler::new(num_channels, MAX_OVERSAMPLE),
            detector: onset::OnsetDetector::new(sample_rate),
            ref_delay: Delay::new(num_channels, max_short + extra_max),
            sub_a_delay: Delay::new(num_channels, max_short + extra_max),
            sub_r_delay: Delay::new(num_channels, max_short + extra_max),
            sub_clip_delay: Delay::new(num_channels, max_short),
            proc_delay: Delay::new(num_channels, extra_max),
            driven_delay: Delay::new(num_channels, max_short + clip::LATENCY),
            dry_delay: Delay::new(num_channels, max_long + 2 * max_short + extra_max + clip::LATENCY),
            p_ring: EnvRing::new(ring_len, 1.0),
            t_ring: EnvRing::new(ring_len, 0.0),
            pc_ring: EnvRing::new(ring_len, 0.0),
            pos: 0,
            extra_delay: 0,
            latency: 0,
            timing_scale: 0.0,
            k_dry: loudness::KMeter::new(sample_rate, num_channels, 3.0),
            k_wet: loudness::KMeter::new(sample_rate, num_channels, 3.0),
            sone_wet: sone::SoneMeter::new(sample_rate, num_channels, 100.0),
            sone_dry: sone::SoneMeter::new(sample_rate, num_channels, 100.0),
            match_gain: 1.0,
            gr_spec_db: 0.0,
            gr_post_db: 0.0,
            pending_pk_spec: 0.0,
            pending_clip_on: false,
            protect_vis: 0.0,
            wide: vec![vec![0.0; max_block]; 4 * num_channels],
            a: vec![vec![0.0; max_block]; num_channels],
            refd: vec![vec![0.0; max_block]; num_channels],
            driven: vec![vec![0.0; max_block]; num_channels],
            cl: vec![vec![0.0; max_block]; num_channels],
            clw: vec![vec![0.0; max_block]; 3 * num_channels],
            hf_delay: Delay::new(num_channels, clip::LATENCY),
            hf_env: 0.0,
            hf_hold: 0,
            sub_a: vec![vec![0.0; max_block]; num_channels],
            sub_r: vec![vec![0.0; max_block]; num_channels],
            clsub: vec![vec![0.0; max_block]; num_channels],
            rest_ceil: vec![1.0; max_block],
            sub_env: 0.0,
            sub_hold: 0,
            dry: vec![vec![0.0; max_block]; num_channels],
            tmp_in: vec![0.0; 2 * num_channels],
            tmp_out: vec![0.0; 2 * num_channels],
            cur: Settings::default(),
        };
        let s = Settings::default();
        e.set_windows(s.long_order, s.short_order, s.oversample);
        e
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    /// Reconfigure window sizes / oversampling. Resets all state. Call when they change.
    pub fn set_windows(&mut self, long_order: usize, short_order: usize, oversample: usize) {
        let n_l = 1usize << long_order.clamp(MIN_LONG_ORDER, MAX_LONG_ORDER);
        let n_s = 1usize << short_order.clamp(MIN_SHORT_ORDER, MAX_SHORT_ORDER);
        self.long_stft.set_window_size(n_l);
        self.short_stft.set_window_size(n_s);
        self.clip_stft.set_window_size(n_s);
        self.low_dyn.configure(self.sample_rate, n_l);
        self.high_dyn.configure(self.sample_rate, n_s);
        self.oversampler.set_os(oversample.clamp(1, MAX_OVERSAMPLE));
        self.sub_oversampler.set_os(oversample.clamp(1, MAX_OVERSAMPLE));
        let pre_max = (MAX_PRE_MS / 1000.0 * self.sample_rate).ceil() as usize;
        let l_spec = n_l + n_s;
        self.extra_delay = (pre_max + onset::MAX_LATENCY).saturating_sub(l_spec);
        self.ref_delay.set(n_s + self.extra_delay);
        self.sub_a_delay.set(n_s + self.extra_delay);
        self.sub_r_delay.set(n_s + self.extra_delay);
        self.sub_clip_delay.set(n_s);
        self.hf_delay.set(clip::LATENCY);
        self.proc_delay.set(self.extra_delay);
        self.driven_delay.set(n_s + clip::LATENCY);
        self.latency = l_spec + self.extra_delay + n_s + clip::LATENCY;
        self.dry_delay.set(self.latency);
        self.cur.long_order = long_order;
        self.cur.short_order = short_order;
        self.cur.oversample = oversample;
        self.reset();
    }

    pub fn reset(&mut self) {
        self.long_stft.reset();
        self.short_stft.reset();
        self.clip_stft.reset();
        self.low_dyn.reset();
        self.high_dyn.reset();
        self.oversampler.reset();
        self.sub_oversampler.reset();
        self.sub_env = 0.0;
        self.sub_hold = 0;
        self.hf_env = 0.0;
        self.hf_hold = 0;
        self.hf_delay.reset();
        self.detector.reset();
        self.ref_delay.reset();
        self.sub_a_delay.reset();
        self.sub_r_delay.reset();
        self.sub_clip_delay.reset();
        self.proc_delay.reset();
        self.driven_delay.reset();
        self.dry_delay.reset();
        self.p_ring.clear();
        self.t_ring.clear();
        self.pc_ring.clear();
        self.pos = 0;
        self.timing_scale = 0.0;
        self.k_dry.reset();
        self.k_wet.reset();
        self.sone_wet.reset();
        self.sone_dry.reset();
        self.match_gain = 1.0;
        self.gr_spec_db = 0.0;
        self.gr_post_db = 0.0;
    }

    /// Current loudness-match gain (wet/dry, linear) and K-weighted levels, for meters.
    pub fn meters(&self) -> (f32, f32, f32) {
        (self.match_gain, self.k_dry.lufs(), self.k_wet.lufs())
    }

    /// Perceived loudness in sones: (dry at its own level, processed). The dry meter runs on
    /// the match-gained dry signal, so the dry figure is converted back to unity gain using the
    /// model's ~0.6 power law; it is an estimate when the match gain is far from 1.
    pub fn sones(&self) -> (f32, f32) {
        let g = self.match_gain.max(1e-3);
        (self.sone_dry.sone / g.powf(0.6), self.sone_wet.sone)
    }

    /// Fill analyzer arrays on a log-frequency grid from `f_lo` to `f_hi` Hz: envelope dB, gain dB
    /// and threshold dB. Each point comes from whichever resolution stage owns that frequency.
    pub fn analyzer(&self, f_lo: f32, f_hi: f32, env: &mut [f32], gain: &mut [f32], thr: &mut [f32]) {
        let n = env.len();
        let (l0, l1) = (f_lo.ln(), f_hi.ln());
        for i in 0..n {
            let f = (l0 + (l1 - l0) * i as f32 / (n - 1).max(1) as f32).exp();
            let (e_l, g_l, t_l, w_l) = self.low_dyn.sample_at(f);
            let (e_h, g_h, t_h, w_h) = self.high_dyn.sample_at(f);
            let wl = w_l / (w_l + w_h).max(1e-6);
            env[i] = e_l * wl + e_h * (1.0 - wl);
            gain[i] = g_l + g_h; // stages are in series and each is band-weighted
            thr[i] = t_l * wl + t_h * (1.0 - wl);
        }
    }

    /// Protection activity for display: 1 while hits are being guarded, decaying to 0.
    pub fn protect_activity(&self) -> f32 {
        self.protect_vis
    }

    /// Gain reduction in dB (<= 0) of the spectral (per-bin ceiling) stage and the post soft
    /// clipper, measured on block peaks with a slow release.
    pub fn gain_reduction(&self) -> (f32, f32) {
        (self.gr_spec_db, self.gr_post_db)
    }

    /// Process `len` samples in place. `channels.len()` must equal `num_channels`.
    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize, s: &Settings) {
        assert!(len <= self.max_block);
        let nc = self.num_channels;
        let sr = self.sample_rate;
        if s.long_order != self.cur.long_order || s.short_order != self.cur.short_order || s.oversample != self.cur.oversample {
            self.set_windows(s.long_order, s.short_order, s.oversample);
        }

        // --- dry delay, onset detection, protection ring writes (input timeline) ---
        let pre = (s.pre_ms.clamp(0.0, MAX_PRE_MS) / 1000.0 * sr).round() as usize;
        let hold = (s.hold_ms.max(0.0) / 1000.0 * sr).round() as usize;
        let post = (s.post_ms.max(0.0) / 1000.0 * sr).round() as usize;
        let tb_gain = 10f32.powf(s.trans_boost_db / 20.0) - 1.0;
        let tb_tau = (s.trans_boost_ms.max(0.5) / 1000.0 * sr).max(1.0);
        let pc_hold = (0.008 * sr) as usize;
        let pc_post = (0.020 * sr) as usize;
        for i in 0..len {
            let mut mono = 0.0;
            for ch in 0..nc {
                let x = channels[ch][i];
                self.tmp_in[ch] = x;
                mono += x;
            }
            self.dry_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
            for ch in 0..nc {
                self.dry[ch][i] = self.tmp_out[ch];
            }
            mono /= nc as f32;
            if let Some(t_on) = self.detector.push(mono, s.onset_sens) {
                if s.protect_on {
                    let start = t_on.saturating_sub(pre);
                    for (j, t) in (start..t_on).enumerate() {
                        let u = j as f32 / pre.max(1) as f32; // 0 → 1 approaching the hit
                        let v = 0.5 + 0.5 * (std::f32::consts::PI * u).cos();
                        self.p_ring.min_at(t, v);
                    }
                    for j in 0..hold {
                        self.p_ring.min_at(t_on + j, 0.0);
                    }
                    for j in 0..post {
                        let u = j as f32 / post.max(1) as f32;
                        let v = 0.5 - 0.5 * (std::f32::consts::PI * u).cos();
                        self.p_ring.min_at(t_on + hold + j, v);
                    }
                }
                if tb_gain != 0.0 {
                    let n = (tb_tau * 5.0) as usize;
                    for j in 0..n {
                        self.t_ring.max_at(t_on + j, tb_gain * (-(j as f32) / tb_tau).exp());
                    }
                }
                for j in 0..pc_hold {
                    self.pc_ring.max_at(t_on + j, 1.0);
                }
                for j in 0..pc_post {
                    let u = j as f32 / pc_post.max(1) as f32;
                    self.pc_ring.max_at(t_on + pc_hold + j, 0.5 + 0.5 * (std::f32::consts::PI * u).cos());
                }
            }
        }

        // --- long STFT on [proc.., ref.., sub_proc.., sub_ref..] ---
        // The sub band is split here, at the long resolution, so the split is sharp. The sub
        // channels are zero-phase copies of the proc/ref low band below `sub_hz`; they get the
        // same protection blend as the full signal, so `full - sub` is an exact remainder.
        for ch in 0..nc {
            self.wide[ch][..len].copy_from_slice(&channels[ch][..len]);
            self.wide[nc + ch][..len].copy_from_slice(&channels[ch][..len]);
            self.wide[2 * nc + ch][..len].fill(0.0);
            self.wide[3 * nc + ch][..len].fill(0.0);
        }
        {
            let dyn_s = s.dyn_;
            let low = &mut self.low_dyn;
            let ts = &mut self.timing_scale;
            let n_l = self.long_stft.window_size() as f32;
            let frame_dt = n_l / 4.0 / sr;
            let sub_on = s.sub_mode != SubMode::Off;
            let sub_hz = s.sub_hz.max(20.0);
            let mut refs = self.wide.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
            self.long_stft.process(refs.as_mut(), len, |spectra, nb, mag| {
                let (proc_ref, subs) = spectra.split_at_mut(2 * nc);
                let (first, second) = proc_ref.split_at_mut(nc);
                // mono-below on the reference pair when stereo
                if dyn_s.mono_below_hz > 0.0 && nc == 2 {
                    mono_below(second, nb, sr, dyn_s.mono_below_hz);
                }
                low.process_frame(first, nb, mag, &dyn_s, *ts);
                *ts = (*ts + frame_dt / TIMING_FADE_S).min(1.0);
                if sub_on {
                    let (sub_p, sub_r) = subs.split_at_mut(nc);
                    for k in 0..nb {
                        let f = (k as f32 * sr / n_l).max(1.0);
                        let o = (f / sub_hz).log2();
                        if o >= 0.5 {
                            break;
                        }
                        let w = if o <= -0.5 { 1.0 } else { 0.5 - 0.5 * (std::f32::consts::PI * o).sin() };
                        for ch in 0..nc {
                            sub_p[ch][k] = first[ch][k] * w;
                            sub_r[ch][k] = second[ch][k] * w;
                        }
                    }
                }
            });
        }
        for ch in 0..nc {
            self.a[ch][..len].copy_from_slice(&self.wide[ch][..len]);
            self.refd[ch][..len].copy_from_slice(&self.wide[nc + ch][..len]);
            self.sub_a[ch][..len].copy_from_slice(&self.wide[2 * nc + ch][..len]);
            self.sub_r[ch][..len].copy_from_slice(&self.wide[3 * nc + ch][..len]);
        }

        // --- short STFT (high band) on proc ---
        {
            let dyn_s = s.dyn_;
            let high = &mut self.high_dyn;
            let ts = self.timing_scale;
            let mut refs = self.a.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
            self.short_stft.process(refs.as_mut(), len, |spectra, nb, mag| {
                high.process_frame(spectra, nb, mag, &dyn_s, ts);
            });
        }

        // --- align, protect, boost; then drive ---
        let drive = 10f32.powf(s.drive_db / 20.0);
        let base_t = self.pos; // input time of channels[..][0]
        let l_spec_total = self.long_stft.window_size() + self.short_stft.window_size() + self.extra_delay;
        let mut prot_count = 0usize;
        for i in 0..len {
            for ch in 0..nc {
                self.tmp_in[ch] = self.a[ch][i];
            }
            self.proc_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
            for ch in 0..nc {
                self.a[ch][i] = self.tmp_out[ch];
                self.tmp_in[ch] = self.refd[ch][i];
            }
            self.ref_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
            for ch in 0..nc {
                self.tmp_in[ch] = self.sub_a[ch][i];
            }
            self.sub_a_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[nc..2 * nc]);
            for ch in 0..nc {
                self.sub_a[ch][i] = self.tmp_out[nc + ch];
                self.tmp_in[ch] = self.sub_r[ch][i];
            }
            self.sub_r_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[nc..2 * nc]);
            // input time of the sample now emerging from the spectral path
            let t = (base_t + i).wrapping_sub(l_spec_total);
            let (p, tb) = if base_t + i >= l_spec_total {
                (self.p_ring.take(t), self.t_ring.take(t))
            } else {
                (1.0, 0.0)
            };
            if p < 0.999 {
                prot_count += 1;
            }
            for ch in 0..nc {
                let r = self.tmp_out[ch];
                let y = r + (self.a[ch][i] - r) * p + r * tb;
                self.driven[ch][i] = y * drive;
                self.cl[ch][i] = y * drive;
                let sr_ = self.tmp_out[nc + ch];
                let ys = sr_ + (self.sub_a[ch][i] - sr_) * p + sr_ * tb;
                self.clsub[ch][i] = ys * drive;
            }
        }

        {
            let frac = prot_count as f32 / len as f32;
            self.protect_vis = if frac > self.protect_vis { frac } else { self.protect_vis * 0.9 };
        }

        // --- spectral clip + oversampled clip; driven delayed to match ---
        // The clip STFT runs on [rest.., sub..]. With Sub Split off the sub channels are zeroed
        // in the frequency domain and the rest keeps the full band.
        let ceil = 10f32.powf(s.ceil_db / 20.0);
        let bin_ceil = 10f32.powf(s.bin_ceil_db / 20.0);
        {
            let clip_on = s.clip_on;
            let sub_mode = s.sub_mode;
            // rest = full - sub (exact, both zero-phase and aligned); sub only needs a delay
            for ch in 0..nc {
                for i in 0..len {
                    self.clw[ch][i] = self.cl[ch][i] - self.clsub[ch][i];
                }
            }
            for i in 0..len {
                for ch in 0..nc {
                    self.tmp_in[ch] = self.clsub[ch][i];
                }
                self.sub_clip_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
                for ch in 0..nc {
                    self.clw[nc + ch][i] = self.tmp_out[ch];
                }
            }
            {
                // the clip STFT runs on [rest.., highs..]; highs start as a copy of rest and are
                // separated in the frequency domain (one-octave crossfade at hf_hz)
                {
                    let (rest_sub, hf) = self.clw.split_at_mut(2 * nc);
                    for ch in 0..nc {
                        hf[ch][..len].copy_from_slice(&rest_sub[ch][..len]);
                    }
                }
                let (rest_sub, hf) = self.clw.split_at_mut(2 * nc);
                let (rest, _) = rest_sub.split_at_mut(nc);
                let mut refs = rest.iter_mut().chain(hf.iter_mut()).map(|v| &mut v[..]).collect::<SmallRefs>();
                let hf_on = clip_on && s.hf_preserve;
                let hf_hz = s.hf_hz.max(1000.0);
                let n_clip = self.clip_stft.window_size() as f32;
                self.clip_stft.process(refs.as_mut(), len, |spectra, nb, mag| {
                    let (rest, hf) = spectra.split_at_mut(nc);
                    for k in 0..nb {
                        let f = (k as f32 * sr / n_clip).max(1.0);
                        let o = (f / hf_hz).log2();
                        let w_hf = if !hf_on || o <= -0.5 {
                            0.0
                        } else if o >= 0.5 {
                            1.0
                        } else {
                            0.5 + 0.5 * (std::f32::consts::PI * o).sin()
                        };
                        for ch in 0..nc {
                            let r = &mut rest[ch][k];
                            hf[ch][k] = *r * w_hf;
                            *r *= 1.0 - w_hf;
                            if clip_on && bin_ceil < 0.999 {
                                let m = r.norm() * mag;
                                if m > bin_ceil {
                                    *r *= bin_ceil / m;
                                }
                            }
                        }
                    }
                });
            }
            // highs: plain delay to match the oversampler, and a peak envelope to reserve headroom
            {
                let rel = (-1.0 / (0.08 * sr)).exp();
                let hold_n = (0.010 * sr) as usize;
                for i in 0..len {
                    for ch in 0..nc {
                        self.tmp_in[ch] = self.clw[2 * nc + ch][i];
                    }
                    self.hf_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
                    let mut a = 0.0f32;
                    for ch in 0..nc {
                        self.clw[2 * nc + ch][i] = self.tmp_out[ch];
                        a = a.max(self.tmp_in[ch].abs());
                    }
                    if a >= self.hf_env {
                        self.hf_env = a;
                        self.hf_hold = hold_n;
                    } else if self.hf_hold > 0 {
                        self.hf_hold -= 1;
                    } else {
                        self.hf_env = a + (self.hf_env - a) * rel;
                    }
                    // the highs never reserve more than 3 dB; beyond that the final clamp trims
                    self.rest_ceil[i] = ceil - self.hf_env.min(ceil * 0.29);
                }
            }
            // Separate: the sub gets the per-bin ceiling too, at the clip resolution, via its
            // own pass. Cheap enough and keeps the two paths symmetric in latency (the sub path
            // already carries the clip window delay, so this pass must not add more): we apply
            // it as a plain magnitude cap in the time domain instead (soft clipper below).
            // peak after the spectral stage, on the recombined signal
            let mut pk_spec = 0.0f32;
            for ch in 0..nc {
                for i in 0..len {
                    pk_spec = pk_spec.max((self.clw[ch][i] + self.clw[nc + ch][i] + self.clw[2 * nc + ch][i]).abs());
                }
            }
            // Protect / Separate: the sub's peak envelope (instant attack, 25 ms hold,
            // ~80 ms release) reserves headroom; the rest is clipped into what is left. In
            // Separate the sub is first clipped to its own ceiling, so the reservation is capped.
            let sub_cap = if sub_mode == SubMode::Separate { ceil * 10f32.powf(s.sub_ceil_db.min(0.0) / 20.0) } else { f32::MAX };
            if clip_on && sub_mode != SubMode::Off {
                let rel = (-1.0 / (0.08 * sr)).exp();
                // hold longer than a full period of the lowest sub so the reserved headroom
                // does not ripple at the sub's rate (that would modulate the rest)
                let hold_n = (0.025 * sr) as usize;
                for i in 0..len {
                    let mut a = 0.0f32;
                    for ch in 0..nc {
                        a = a.max(self.clw[nc + ch][i].abs());
                    }
                    if a >= self.sub_env {
                        self.sub_env = a;
                        self.sub_hold = hold_n;
                    } else if self.sub_hold > 0 {
                        self.sub_hold -= 1;
                    } else {
                        self.sub_env = a + (self.sub_env - a) * rel;
                    }
                    self.rest_ceil[i] = (self.rest_ceil[i] - self.sub_env.min(sub_cap)).max(ceil * 0.1);
                }
            }
            {
                let (rest, tail) = self.clw.split_at_mut(nc);
                let (sub, _hf) = tail.split_at_mut(nc);
                let mut rrefs = rest.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
                let mut srefs = sub.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
                if clip_on && (sub_mode != SubMode::Off || s.hf_preserve) {
                    self.oversampler.process_dyn(rrefs.as_mut(), len, ceil, Some(&self.rest_ceil[..len]), s.hardness, s.clip_link);
                } else if clip_on {
                    self.oversampler.process(rrefs.as_mut(), len, ceil, s.hardness, s.clip_link);
                } else {
                    self.oversampler.process(rrefs.as_mut(), len, 1e6, 0.999, 0.0);
                }
                if clip_on && sub_mode == SubMode::Separate {
                    self.sub_oversampler.process(srefs.as_mut(), len, sub_cap, s.hardness, s.clip_link);
                } else {
                    // Off / Protect: plain delay so the two paths stay aligned
                    self.sub_oversampler.process(srefs.as_mut(), len, 1e6, 0.999, 0.0);
                }
            }
            for ch in 0..nc {
                for i in 0..len {
                    let y = self.clw[ch][i] + self.clw[nc + ch][i] + self.clw[2 * nc + ch][i];
                    // the summed bands can exceed the ceiling slightly; hold it with a hard clamp
                    self.cl[ch][i] = if clip_on { y.clamp(-ceil, ceil) } else { y };
                }
            }
            let pk_post = block_peak(&self.cl, nc, len);
            // The post-clip signal is 16 samples later than pk_spec's; negligible for a block meter.
            let gr_post = if clip_on && pk_spec > 1e-6 { (20.0 * (pk_post / pk_spec).log10()).min(0.0) } else { 0.0 };
            self.gr_post_db = if gr_post < self.gr_post_db { gr_post } else { self.gr_post_db + (gr_post - self.gr_post_db) * (len as f32 / (0.3 * sr)).min(1.0) };
            self.pending_pk_spec = pk_spec;
            self.pending_clip_on = clip_on;
        }
        let l_clip = self.clip_stft.window_size() + clip::LATENCY;
        let gain = s.output_gain;
        let mix = s.mix.clamp(0.0, 1.0);
        let mut pk_in = 0.0f32;
        for i in 0..len {
            for ch in 0..nc {
                self.tmp_in[ch] = self.driven[ch][i];
            }
            self.driven_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
            for ch in 0..nc {
                pk_in = pk_in.max(self.tmp_out[ch].abs());
            }
            let t_in = base_t + i;
            let t = t_in.wrapping_sub(l_spec_total + l_clip);
            let pc = if t_in >= l_spec_total + l_clip { self.pc_ring.take(t) } else { 0.0 };
            let amt = if s.clip_on { 1.0 - (1.0 - s.trans_clip.clamp(0.0, 1.0)) * pc } else { 0.0 };
            // meters on aligned dry / wet
            for ch in 0..nc {
                let d = self.tmp_out[ch];
                let c = self.cl[ch][i];
                let wet = d + (c - d) * amt;
                self.cl[ch][i] = wet;
                self.tmp_in[ch] = self.dry[ch][i];
            }
            self.k_dry.push_frame(&self.tmp_in[..nc]);
            for ch in 0..nc {
                self.tmp_in[ch] = self.cl[ch][i];
            }
            self.k_wet.push_frame(&self.tmp_in[..nc]);
            self.sone_wet.push(&self.tmp_in[..nc]);
            for ch in 0..nc {
                self.tmp_in[ch] = self.dry[ch][i] * self.match_gain;
            }
            self.sone_dry.push(&self.tmp_in[..nc]);
            // Servo the match gain so the scaled dry reads the same sones as the processed
            // signal. Loudness grows roughly with gain^0.6, so step in the log domain.
            if self.sone_wet.sone > 0.05 && self.sone_dry.sone > 0.05 {
                let ratio = (self.sone_wet.sone / self.sone_dry.sone).clamp(0.25, 4.0);
                // ~1.5 s time constant: much slower than the meters' 60/300 ms so it cannot hunt
                let step = ratio.ln() / 0.6 / (1.5 * sr);
                self.match_gain = (self.match_gain * step.exp()).clamp(0.05, 20.0);
            }
            for ch in 0..nc {
                let out = if s.matched_bypass {
                    self.dry[ch][i] * self.match_gain
                } else {
                    (self.dry[ch][i] * (1.0 - mix) + self.cl[ch][i] * mix) * gain
                };
                channels[ch][i] = out;
            }
        }
        {
            // spectral-stage GR: block peak after the per-bin ceiling vs the aligned driven input
            let gr_spec = if self.pending_clip_on && pk_in > 1e-6 { (20.0 * (self.pending_pk_spec / pk_in).log10()).min(0.0) } else { 0.0 };
            self.gr_spec_db = if gr_spec < self.gr_spec_db { gr_spec } else { self.gr_spec_db + (gr_spec - self.gr_spec_db) * (len as f32 / (0.3 * sr)).min(1.0) };
        }
        if s.ref_spl != self.sone_wet.ref_spl {
            self.sone_wet.ref_spl = s.ref_spl;
            self.sone_dry.ref_spl = s.ref_spl;
        }
        self.pos += len;
        self.cur = *s;
    }
}

fn block_peak(bufs: &[Vec<f32>], nc: usize, len: usize) -> f32 {
    let mut m = 0.0f32;
    for ch in 0..nc {
        for v in &bufs[ch][..len] {
            m = m.max(v.abs());
        }
    }
    m
}

/// Mono-below applied directly to a stereo spectrum pair (zero-phase, one-octave crossfade).
fn mono_below(spectra: &mut [Vec<Complex32>], nb: usize, sr: f32, mono_hz: f32) {
    if spectra.len() != 2 {
        return;
    }
    let n = (nb - 1) * 2;
    let (a, b) = spectra.split_at_mut(1);
    let (l, r) = (&mut a[0], &mut b[0]);
    for k in 0..nb {
        let f = (k as f32 * sr / n as f32).max(1.0);
        let om = (f / mono_hz).log2();
        let m = if om <= 0.0 { 1.0 } else if om >= 1.0 { break } else { 0.5 + 0.5 * (std::f32::consts::PI * om).cos() };
        let mid = (l[k] + r[k]) * 0.5;
        l[k] = l[k] * (1.0 - m) + mid * m;
        r[k] = r[k] * (1.0 - m) + mid * m;
    }
}

/// Fixed-capacity collection of channel slices (avoids allocating in the process path).
pub struct SmallRefs<'a> {
    items: [Option<&'a mut [f32]>; 8],
    len: usize,
}

impl<'a> SmallRefs<'a> {
    fn as_mut(&mut self) -> &mut [&'a mut [f32]] {
        // SAFETY: the first `len` entries are always `Some`; `Option<&mut [f32]>` has the same
        // layout as `&mut [f32]` (niche optimization), so the reinterpretation is sound.
        unsafe { std::slice::from_raw_parts_mut(self.items.as_mut_ptr() as *mut &'a mut [f32], self.len) }
    }
}

impl<'a> std::iter::FromIterator<&'a mut [f32]> for SmallRefs<'a> {
    fn from_iter<I: IntoIterator<Item = &'a mut [f32]>>(iter: I) -> Self {
        let mut s = SmallRefs { items: [None, None, None, None, None, None, None, None], len: 0 };
        for it in iter {
            assert!(s.len < 8, "OnsetGuard supports at most 2 channels");
            s.items[s.len] = Some(it);
            s.len += 1;
        }
        s
    }
}
