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

pub mod balance;
pub mod clip;
pub mod dynamics;
pub mod eq;
pub mod loudness;
pub mod onset;
pub mod rotate;
pub mod sone;
pub mod stereo;
pub mod stft;

use dynamics::{Band, Dynamics, DynamicsSettings, Weighting};
use realfft::num_complex::Complex32;

pub const MIN_LONG_ORDER: usize = 9;
pub const MAX_LONG_ORDER: usize = 15;
/// Crude playback-system bands for the translation readouts.
pub const PHONE_LO: f32 = 300.0;
pub const PHONE_HI: f32 = 8000.0;
pub const LAPTOP_LO: f32 = 120.0;
pub const LAPTOP_HI: f32 = 15000.0;
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
    /// The sub bypasses the spectral and soft clippers and is summed back clean; its peak
    /// envelope reserves headroom so the rest is clipped into what is left (blended with the
    /// plain clip by `band_protect`).
    Protect,
    /// The sub gets its own clipper at its own ceiling (`sub_ceil_db` below the output
    /// ceiling) and the rest is clipped into the remaining headroom; no intermodulation
    /// between them.
    Separate,
}

#[derive(Clone, Copy, Debug)]
pub struct Settings {
    pub dyn_: DynamicsSettings,
    pub stereo: stereo::StereoSettings,
    pub balance: balance::BalanceSettings,
    pub eq: eq::EqSettings,
    pub rotate: rotate::RotateSettings,
    pub autopilot: AutopilotSettings,
    /// With Clip off, meter the processed side (sones, crest, autopilot) through a simulated
    /// 1x hard clip at Output Ceiling: what a hard clipper downstream will make of it.
    pub meter_thru_clip: bool,
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
    /// Drive applied to detected hits instead of `drive_db` (absolute, dB).
    pub hit_drive_db: f32,
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
    /// How much of the preserved band bypasses the clipper (0 = none of it, 1 = all of it).
    pub hf_amount: f32,
    /// During a detected hit, how much of the preserved band still bypasses the clipper
    /// (0 = the highs of a hit are clipped with everything else, 1 = preserved throughout).
    pub hf_on_hits: f32,
    pub sub_mode: SubMode,
    pub sub_hz: f32,
    /// Blend between "clip everything" (0) and "protected bands" (1): with Sub Split or
    /// Preserve Highs on, how much of that protection is applied. Proportional trade between
    /// band purity and body level; the ceiling holds at every setting.
    pub band_protect: f32,
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
            stereo: stereo::StereoSettings::default(),
            balance: balance::BalanceSettings::default(),
            eq: eq::EqSettings::default(),
            rotate: rotate::RotateSettings::default(),
            autopilot: AutopilotSettings::default(),
            meter_thru_clip: true,
            dyn_: DynamicsSettings {
                weighting: Weighting::Off,
                threshold_db: -24.0,
                center_hz: 1000.0,
                slope_db_oct: -4.5,
                curve_db_oct2: 0.0,
                down_ratio: 3.0,
                up_ratio: 2.0,
                up_range_db: 12.0,
                up_depth_db: 24.0,
                up_zone_hz: [60.0, 300.0, 2000.0, 10000.0],
                up_zone_amt: [1.0; 4],
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
            hit_drive_db: 0.0,
            ceil_db: 0.0,
            hardness: 1.0,
            trans_clip: 1.0,
            clip_link: 1.0,
            oversample: 4,
            hf_preserve: false,
            hf_hz: 8000.0,
            hf_amount: 1.0,
            hf_on_hits: 0.0,
            sub_mode: SubMode::Off,
            sub_hz: 100.0,
            band_protect: 0.6,
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

/// Loudness autopilot: servo the clip drive toward a target loudness, never past a crest floor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AutopilotSettings {
    pub on: bool,
    /// Target perceived loudness of the processed signal, phon (at Reference Level).
    pub target_phon: f32,
    /// The processed crest factor the autopilot must not go below, dB.
    pub crest_floor_db: f32,
}

impl Default for AutopilotSettings {
    fn default() -> Self {
        AutopilotSettings { on: false, target_phon: 100.0, crest_floor_db: 4.0 }
    }
}

/// Distortion readout over the last second.
#[derive(Clone, Copy, Debug, Default)]
pub struct Distortion {
    /// Peak-to-RMS of the dry input, dB.
    pub crest_dry_db: f32,
    /// Peak-to-RMS of the processed output, dB. Drums at a ceiling sit around 4-6; a square is ~1.
    pub crest_wet_db: f32,
    /// Share of processed samples within 0.1 dB of the ceiling, percent.
    pub rail_pct: f32,
}

#[derive(Clone, Copy, Debug, Default)]
struct DistortionAcc {
    dry_sq: f32,
    wet_sq: f32,
    dry_pk: f32,
    wet_pk: f32,
    rail: usize,
    n: usize,
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
    /// Read without clearing (for a stage earlier in the chain than the one that takes).
    #[inline]
    fn peek(&self, t: usize) -> f32 {
        self.buf[t & self.mask]
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
    final_oversampler: clip::Oversampler,
    low_dyn: Dynamics,
    stereo_stage: stereo::StereoStage,
    balance: balance::Balance,
    pre_eq: eq::PreEq,
    rotator: rotate::PhaseRotator,
    /// Per-bin inverse rotation for the reference path (Rotate Hits), cached by key.
    unrot: Vec<Complex32>,
    unrot_key: (usize, f32, f32),
    /// Autopilot: extra drive in dB on top of Clip Drive, learned once a second until it
    /// settles, then held (`auto_locked`) until Autopilot is switched off and on again.
    pub auto_drive_db: f32,
    pub auto_locked: bool,
    auto_was_on: bool,
    auto_settled: u32,
    /// Last three one-second summaries for the autopilot: (peak, sum of squares, samples,
    /// sone sum, sone samples) of the (simulated-clipped) processed signal.
    ap_hist: [(f32, f32, usize, f32, usize); 3],
    ap_sone_sum: f32,
    ap_sone_n: usize,
    /// Last four learned drive values, for the lock test (range < 1 dB = settled).
    ap_drive_hist: [f32; 4],
    ap_ticks: u32,
    high_dyn: Dynamics,
    oversampler: clip::Oversampler,
    detector: onset::OnsetDetector,

    ref_delay: Delay,   // reference after long stage: + short + extra
    sub_a_delay: Delay, // sub (processed) after long stage: + short + extra
    sub_r_delay: Delay, // sub (reference): + short + extra
    sub_clip_delay: Delay, // sub in the clip path: + clip stft window
    rest_clip_delay: Delay, // rest when Preserve Highs is off: replaces the clip STFT
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
    /// Translation readouts: band-limited loudness (phone, laptop), dry at unity and processed.
    sone_tr: [sone::SoneMeter; 4],
    /// Distortion readout: over the last second, peak and sum of squares (dry, processed) and
    /// the number of processed samples within 0.1 dB of the ceiling.
    dist_acc: DistortionAcc,
    pub dist: Distortion,
    match_gain: f32,
    /// Gain reduction meters (dB, <= 0): spectral (per-bin ceiling) stage and post soft clip.
    gr_post_db: f32,
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
    /// Sub band (processed, reference) after the long stage, then the blended driven sub.
    sub_a: Vec<Vec<f32>>,
    sub_r: Vec<Vec<f32>>,
    clsub: Vec<Vec<f32>>,
    /// Protect mode: per-sample ceiling left for the rest after the sub's peak envelope.
    rest_ceil: Vec<f32>,
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
        let ring_len = max_long + 2 * max_short + extra_max + 2 * pre_max + sample_rate as usize + max_block + 2 * clip::LATENCY;
        let mut e = Engine {
            num_channels,
            sample_rate,
            max_block,
            long_stft: stft::StftStage::new(4 * num_channels, MIN_LONG_ORDER, MAX_LONG_ORDER),
            short_stft: stft::StftStage::new(num_channels, MIN_SHORT_ORDER, MAX_SHORT_ORDER),
            clip_stft: stft::StftStage::new(2 * num_channels, MIN_SHORT_ORDER, MAX_SHORT_ORDER),
            sub_oversampler: clip::Oversampler::new(num_channels, MAX_OVERSAMPLE),
            final_oversampler: clip::Oversampler::new(num_channels, MAX_OVERSAMPLE),
            low_dyn: Dynamics::new(Band::Low, num_channels, max_long),
            stereo_stage: stereo::StereoStage::new(max_long / 2 + 1),
            balance: balance::Balance::new(max_long / 2 + 1),
            pre_eq: eq::PreEq::new(sample_rate, num_channels),
            rotator: rotate::PhaseRotator::new(sample_rate, num_channels),
            unrot: vec![Complex32::new(1.0, 0.0); max_long / 2 + 1],
            unrot_key: (0, 0.0, 0.0),
            auto_drive_db: 0.0,
            auto_locked: false,
            auto_was_on: false,
            auto_settled: 0,
            ap_hist: [(0.0, 0.0, 0, 0.0, 0); 3],
            ap_sone_sum: 0.0,
            ap_sone_n: 0,
            ap_drive_hist: [0.0; 4],
            ap_ticks: 0,
            high_dyn: Dynamics::new(Band::High, num_channels, max_short),
            oversampler: clip::Oversampler::new(num_channels, MAX_OVERSAMPLE),
            detector: onset::OnsetDetector::new(sample_rate),
            ref_delay: Delay::new(num_channels, max_short + extra_max),
            sub_a_delay: Delay::new(num_channels, max_short + extra_max),
            sub_r_delay: Delay::new(num_channels, max_short + extra_max),
            sub_clip_delay: Delay::new(num_channels, max_short),
            rest_clip_delay: Delay::new(num_channels, max_short),
            proc_delay: Delay::new(num_channels, extra_max),
            driven_delay: Delay::new(num_channels, max_short + 2 * clip::LATENCY),
            dry_delay: Delay::new(num_channels, max_long + 2 * max_short + extra_max + 2 * clip::LATENCY),
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
            dist_acc: DistortionAcc::default(),
            dist: Distortion::default(),
            sone_tr: {
                let mk = |lo: f32, hi: f32| {
                    let mut m = sone::SoneMeter::new(sample_rate, num_channels, 100.0);
                    m.set_band(lo, hi);
                    m
                };
                [mk(PHONE_LO, PHONE_HI), mk(PHONE_LO, PHONE_HI), mk(LAPTOP_LO, LAPTOP_HI), mk(LAPTOP_LO, LAPTOP_HI)]
            },
            match_gain: 1.0,
            gr_post_db: 0.0,
            pending_clip_on: false,
            protect_vis: 0.0,
            wide: vec![vec![0.0; max_block]; 4 * num_channels],
            a: vec![vec![0.0; max_block]; num_channels],
            refd: vec![vec![0.0; max_block]; num_channels],
            driven: vec![vec![0.0; max_block]; num_channels],
            cl: vec![vec![0.0; max_block]; num_channels],
            clw: vec![vec![0.0; max_block]; 3 * num_channels],
            hf_delay: Delay::new(num_channels, clip::LATENCY),
            sub_a: vec![vec![0.0; max_block]; num_channels],
            sub_r: vec![vec![0.0; max_block]; num_channels],
            clsub: vec![vec![0.0; max_block]; num_channels],
            rest_ceil: vec![1.0; max_block],
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
        self.final_oversampler.set_os(oversample.clamp(1, MAX_OVERSAMPLE));
        let pre_max = (MAX_PRE_MS / 1000.0 * self.sample_rate).ceil() as usize;
        let l_spec = n_l + n_s;
        self.extra_delay = (pre_max + onset::MAX_LATENCY).saturating_sub(l_spec);
        self.ref_delay.set(n_s + self.extra_delay);
        self.sub_a_delay.set(n_s + self.extra_delay);
        self.sub_r_delay.set(n_s + self.extra_delay);
        self.sub_clip_delay.set(n_s);
        self.rest_clip_delay.set(n_s);
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
        self.stereo_stage.reset();
        self.balance.reset();
        self.pre_eq.reset();
        self.rotator.reset();
        self.auto_drive_db = 0.0;
        self.auto_locked = false;
        self.auto_settled = 0;
        self.ap_hist = [(0.0, 0.0, 0, 0.0, 0); 3];
        self.ap_sone_sum = 0.0;
        self.ap_sone_n = 0;
        self.ap_drive_hist = [0.0; 4];
        self.ap_ticks = 0;
        self.short_stft.reset();
        self.clip_stft.reset();
        self.low_dyn.reset();
        self.high_dyn.reset();
        self.oversampler.reset();
        self.sub_oversampler.reset();
        self.final_oversampler.reset();
        self.hf_delay.reset();
        self.detector.reset();
        self.ref_delay.reset();
        self.sub_a_delay.reset();
        self.sub_r_delay.reset();
        self.sub_clip_delay.reset();
        self.rest_clip_delay.reset();
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
        for m in &mut self.sone_tr {
            m.reset();
        }
        self.dist_acc = DistortionAcc::default();
        self.dist = Distortion::default();
        self.match_gain = 1.0;
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

    /// Translation readouts in sones: [phone dry, phone processed, laptop dry, laptop processed].
    pub fn translation_sones(&self) -> [f32; 4] {
        [self.sone_tr[0].sone, self.sone_tr[1].sone, self.sone_tr[2].sone, self.sone_tr[3].sone]
    }

    /// Spectral flatness (0 = tones, 1 = white noise) of the dry (unity) and processed signal.
    pub fn flatness(&self) -> (f32, f32) {
        (self.sone_tr[2].flatness, self.sone_tr[3].flatness)
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
    pub fn gain_reduction(&self) -> f32 {
        self.gr_post_db
    }

    /// Process `len` samples in place. `channels.len()` must equal `num_channels`.
    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize, s: &Settings) {
        assert!(len <= self.max_block);
        let nc = self.num_channels;
        let sr = self.sample_rate;
        if s.long_order != self.cur.long_order || s.short_order != self.cur.short_order || s.oversample != self.cur.oversample {
            self.set_windows(s.long_order, s.short_order, s.oversample);
        }

        // --- pre-EQ: minimum-phase, on the input, before every path ---
        self.pre_eq.update(&s.eq);
        self.pre_eq.process(channels, len);
        self.rotator.process(channels, len, &s.rotate);

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
            let st_s = s.stereo;
            let st = &mut self.stereo_stage;
            let bal_s = s.balance;
            let bal = &mut self.balance;
            let eq_ref = &self.pre_eq;
            let rot_s = s.rotate;
            let unrot = &mut self.unrot;
            let unrot_key = &mut self.unrot_key;
            let ref_spl = s.ref_spl;
            let mut refs = self.wide.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
            self.long_stft.process(refs.as_mut(), len, |spectra, nb, mag| {
                let (proc_ref, subs) = spectra.split_at_mut(2 * nc);
                let (first, second) = proc_ref.split_at_mut(nc);
                // stereo stage (width, anti-phase guard) on both pairs, before anything else
                if nc == 2 && stereo::StereoStage::active(&st_s) {
                    st.process_frame(first, second, nb, sr, frame_dt, &st_s);
                }
                // loudness balance: measured on the input, applied to both pairs
                {
                    let (measure, _) = subs.split_at_mut(nc);
                    for ch in 0..nc {
                        measure[ch][..nb].copy_from_slice(&first[ch][..nb]);
                    }
                    let mut targets = [&mut *first, &mut *second];
                    bal.process_frame(measure, &mut targets, nb, sr, mag, frame_dt, ref_spl, &bal_s, eq_ref);
                    // the sub channels are scratch here; they must go back to silence
                    for ch in 0..nc {
                        measure[ch][..nb].fill(Complex32::default());
                    }
                }
                // Rotate Hits: hand the onset protection an un-rotated reference, so a hit
                // comes out with its original phase while the body stays rotated
                if rot_s.on && rot_s.hits < 0.999 {
                    let amt = 1.0 - rot_s.hits.clamp(0.0, 1.0);
                    let key = (nb, rot_s.freq_hz, amt);
                    if *unrot_key != key {
                        *unrot_key = key;
                        for k in 0..nb {
                            let f = k as f32 * sr / n_l;
                            let ph = -amt * rotate::cascade_phase(f, sr, rot_s.freq_hz);
                            unrot[k] = Complex32::new(ph.cos(), ph.sin());
                        }
                    }
                    for ch in 0..nc {
                        for k in 0..nb {
                            second[ch][k] *= unrot[k];
                        }
                    }
                }
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
        let drive = 10f32.powf((s.drive_db + if s.autopilot.on { self.auto_drive_db } else { 0.0 }) / 20.0);
        // Hit Drive: detected hits get their own drive (the clip-protection envelope decides
        // what is a hit, with its 8 ms hold and 20 ms fade), so Clip Drive only moves the body
        let hit_drive = 10f32.powf(s.hit_drive_db / 20.0);
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
            let pc_here = if base_t + i >= l_spec_total { self.pc_ring.peek(t) } else { 0.0 };
            let drive_t = drive + (hit_drive - drive) * pc_here;
            for ch in 0..nc {
                let r = self.tmp_out[ch];
                let y = r + (self.a[ch][i] - r) * p + r * tb;
                self.driven[ch][i] = y * drive_t;
                self.cl[ch][i] = y * drive_t;
                let sr_ = self.tmp_out[nc + ch];
                let ys = sr_ + (self.sub_a[ch][i] - sr_) * p + sr_ * tb;
                self.clsub[ch][i] = ys * drive_t;
            }
        }

        {
            let frac = prot_count as f32 / len as f32;
            self.protect_vis = if frac > self.protect_vis { frac } else { self.protect_vis * 0.9 };
        }

        // --- clip stage ---
        // Three zero-phase, sample-aligned bands: rest, sub, highs. Two outputs are built:
        //   A = clip(rest + sub + highs)                      "clip everything"
        //   B = clip(rest, ceiling - reserved) + sub' + highs  "protected bands"
        // where sub' is the sub untouched (Protect) or clipped at its own ceiling (Separate),
        // and `reserved` is the peak envelope of what is summed back in. B stays under the
        // ceiling by construction. The output is a linear blend A -> B by Protect Amount, so
        // the trade (band purity vs body level) is proportional and the ceiling always holds.
        let ceil = 10f32.powf(s.ceil_db / 20.0);
        {
            let clip_on = s.clip_on;
            let sub_mode = s.sub_mode;
            let hf_on = clip_on && s.hf_preserve;
            let bands_on = clip_on && (sub_mode != SubMode::Off || hf_on);
            let blend = if bands_on { s.band_protect.clamp(0.0, 1.0) } else { 0.0 };

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
            if hf_on {
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
                let hf_hz = s.hf_hz.max(1000.0);
                let hf_amt = s.hf_amount.clamp(0.0, 1.0);
                let n_clip = self.clip_stft.window_size() as f32;
                self.clip_stft.process(refs.as_mut(), len, |spectra, nb, _mag| {
                    let (rest, hf) = spectra.split_at_mut(nc);
                    for k in 0..nb {
                        let f = (k as f32 * sr / n_clip).max(1.0);
                        let o = (f / hf_hz).log2();
                        let w_hf = if o <= -0.5 {
                            0.0
                        } else if o >= 0.5 {
                            1.0
                        } else {
                            0.5 + 0.5 * (std::f32::consts::PI * o).sin()
                        } * hf_amt;
                        for ch in 0..nc {
                            let r = &mut rest[ch][k];
                            hf[ch][k] = *r * w_hf;
                            *r *= 1.0 - w_hf;
                        }
                    }
                });
            } else {
                // Preserve Highs off: nothing to split, so the rest is just delayed to match
                // (drive -> clipper, nothing in between) and the highs channel is silent
                for i in 0..len {
                    for ch in 0..nc {
                        self.tmp_in[ch] = self.clw[ch][i];
                    }
                    self.rest_clip_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
                    for ch in 0..nc {
                        self.clw[ch][i] = self.tmp_out[ch];
                        self.clw[2 * nc + ch][i] = 0.0;
                    }
                }
            }
            // Highs on Hits: during a detected hit, fold (1 - amount) of the preserved highs
            // back into the rest so they are clipped with everything else. The hit envelope is
            // the clip-protection one (8 ms hold, 20 ms fade), read at this stage's own time.
            let k_hits = 1.0 - s.hf_on_hits.clamp(0.0, 1.0);
            if hf_on && k_hits > 0.0 {
                let n_s = self.clip_stft.window_size();
                let l_here = l_spec_total + n_s;
                for i in 0..len {
                    let t_in = base_t + i;
                    if t_in < l_here {
                        continue;
                    }
                    let pc = self.pc_ring.peek(t_in - l_here);
                    if pc <= 0.0 {
                        continue;
                    }
                    let k = k_hits * pc;
                    for ch in 0..nc {
                        let m = self.clw[2 * nc + ch][i] * k;
                        self.clw[2 * nc + ch][i] -= m;
                        self.clw[ch][i] += m;
                    }
                }
            }
            // peak of the recombined signal before the clippers (for the GR meter)
            let mut pk_pre = 0.0f32;
            for ch in 0..nc {
                for i in 0..len {
                    pk_pre = pk_pre.max((self.clw[ch][i] + self.clw[nc + ch][i] + self.clw[2 * nc + ch][i]).abs());
                }
            }

            // --- A: clip everything (full = rest + sub + highs) ---
            for ch in 0..nc {
                for i in 0..len {
                    self.cl[ch][i] = self.clw[ch][i] + self.clw[nc + ch][i] + self.clw[2 * nc + ch][i];
                }
            }
            {
                let mut refs = self.cl.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
                if clip_on {
                    self.final_oversampler.process(refs.as_mut(), len, ceil, s.hardness, s.clip_link);
                } else {
                    self.final_oversampler.process(refs.as_mut(), len, 1e6, 0.999, 0.0);
                }
            }

            // --- B: protected bands ---
            // Sample-by-sample headroom: the rest is clipped into (ceiling - |sub| - |highs|)
            // at every instant, so rest + sub + highs never exceeds the ceiling and the rest
            // keeps all the level there is room for (at the sub's zero crossings, all of it).
            // The sub is first clipped on its own (at the ceiling in Protect, at Sub Ceiling in
            // Separate), so it can never take more than the whole ceiling. The highs reserve at
            // most 3 dB: on HF-dominant material the highs ARE the hit, and reserving their
            // full level would clip the body into nothing; beyond that the safety clamp trims.
            let sub_cap = if sub_mode == SubMode::Separate { ceil * 10f32.powf(s.sub_ceil_db.min(0.0) / 20.0) } else { ceil };
            if blend > 0.0 {
                for i in 0..len {
                    let mut reserve = 0.0f32;
                    for ch in 0..nc {
                        let sub_v = if sub_mode == SubMode::Off { 0.0 } else { self.clw[nc + ch][i].abs().min(sub_cap) };
                        let hf_v = self.clw[2 * nc + ch][i].abs().min(ceil * 0.29);
                        reserve = reserve.max(sub_v + hf_v);
                    }
                    self.rest_ceil[i] = ceil - reserve;
                }
            }
            // highs: plain delay to match the oversamplers (after A, which used them aligned)
            for i in 0..len {
                for ch in 0..nc {
                    self.tmp_in[ch] = self.clw[2 * nc + ch][i];
                }
                self.hf_delay.tick(&self.tmp_in[..nc], &mut self.tmp_out[..nc]);
                for ch in 0..nc {
                    self.clw[2 * nc + ch][i] = self.tmp_out[ch];
                }
            }
            if blend > 0.0 {
                let (rest, tail) = self.clw.split_at_mut(nc);
                let (sub, _hf) = tail.split_at_mut(nc);
                let mut rrefs = rest.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
                let mut srefs = sub.iter_mut().map(|v| &mut v[..]).collect::<SmallRefs>();
                self.oversampler.process_dyn(rrefs.as_mut(), len, ceil, Some(&self.rest_ceil[..len]), s.hardness, s.clip_link);
                if sub_mode == SubMode::Off {
                    self.sub_oversampler.process(srefs.as_mut(), len, 1e6, 0.999, 0.0);
                } else {
                    self.sub_oversampler.process(srefs.as_mut(), len, sub_cap, s.hardness, s.clip_link);
                }
                // blend: out = (1 - blend) * A + blend * B; B is clamped as FIR-overshoot safety
                let sub_in = sub_mode != SubMode::Off;
                for ch in 0..nc {
                    for i in 0..len {
                        let b = self.clw[ch][i]
                            + if sub_in { self.clw[nc + ch][i] } else { 0.0 }
                            + self.clw[2 * nc + ch][i];
                        let b = b.clamp(-ceil, ceil);
                        self.cl[ch][i] += (b - self.cl[ch][i]) * blend;
                    }
                }
            }

            let pk_post = block_peak(&self.cl, nc, len);
            let gr_post = if clip_on && pk_pre > 1e-6 { (20.0 * (pk_post / pk_pre).log10()).min(0.0) } else { 0.0 };
            self.gr_post_db = if gr_post < self.gr_post_db { gr_post } else { self.gr_post_db + (gr_post - self.gr_post_db) * (len as f32 / (0.3 * sr)).min(1.0) };
            self.pending_clip_on = clip_on;
        }
        let l_clip = self.clip_stft.window_size() + clip::LATENCY;
        let out_ceil = 10f32.powf(s.ceil_db / 20.0);
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
                // the ceiling holds whatever Transient Clip lets through: a hit that bypasses
                // the oversampled clipper is still hard-clamped at the ceiling
                let wet = if s.clip_on { wet.clamp(-out_ceil, out_ceil) } else { wet };
                self.cl[ch][i] = wet;
                self.tmp_in[ch] = self.dry[ch][i];
            }
            self.k_dry.push_frame(&self.tmp_in[..nc]);
            // meter the processed side as a downstream 1x hard clip would see it (Clip off +
            // Meter Thru Clip); the audio itself is untouched
            let sim = !s.clip_on && s.meter_thru_clip;
            for ch in 0..nc {
                let w = self.cl[ch][i];
                self.tmp_in[ch] = if sim { w.clamp(-out_ceil, out_ceil) } else { w };
            }
            self.k_wet.push_frame(&self.tmp_in[..nc]);
            self.sone_wet.push(&self.tmp_in[..nc]);
            self.ap_sone_sum += self.sone_wet.sone;
            self.ap_sone_n += 1;
            self.sone_tr[1].push(&self.tmp_in[..nc]);
            self.sone_tr[3].push(&self.tmp_in[..nc]);
            for ch in 0..nc {
                self.tmp_in[ch] = self.dry[ch][i];
            }
            self.sone_tr[0].push(&self.tmp_in[..nc]);
            self.sone_tr[2].push(&self.tmp_in[..nc]);
            {
                let a = &mut self.dist_acc;
                for ch in 0..nc {
                    let d = self.dry[ch][i];
                    let w = if sim { self.cl[ch][i].clamp(-out_ceil, out_ceil) } else { self.cl[ch][i] };
                    a.dry_sq += d * d;
                    a.wet_sq += w * w;
                    a.dry_pk = a.dry_pk.max(d.abs());
                    a.wet_pk = a.wet_pk.max(w.abs());
                    if w.abs() >= out_ceil * 0.9886 {
                        a.rail += 1;
                    }
                }
                a.n += nc;
                if a.n >= (sr as usize) * nc {
                    let inv = 1.0 / a.n as f32;
                    let (wet_pk_1s, wet_sq_1s, n_1s) = (a.wet_pk, a.wet_sq, a.n);
                    let dry_rms = (a.dry_sq * inv).sqrt();
                    let wet_rms = (a.wet_sq * inv).sqrt();
                    if wet_rms > 1e-5 {
                        self.dist = Distortion {
                            crest_dry_db: 20.0 * (a.dry_pk / dry_rms.max(1e-9)).log10(),
                            crest_wet_db: 20.0 * (a.wet_pk / wet_rms).log10(),
                            rail_pct: 100.0 * a.rail as f32 * inv,
                        };
                    }
                    *a = DistortionAcc::default();
                    // --- autopilot: once a second, step the extra drive toward the target
                    // loudness, never past the crest floor ---
                    // one-second summary into the 3-second history
                    self.ap_hist.rotate_right(1);
                    self.ap_hist[0] = (wet_pk_1s, wet_sq_1s, n_1s, self.ap_sone_sum, self.ap_sone_n);
                    self.ap_sone_sum = 0.0;
                    self.ap_sone_n = 0;
                    if s.autopilot.on {
                        if !self.auto_was_on {
                            // switched on: start a fresh learn
                            self.auto_locked = false;
                            self.auto_settled = 0;
                            self.auto_drive_db = 0.0;
                            self.ap_drive_hist = [0.0; 4];
                            self.ap_ticks = 0;
                        }
                        // 3-second crest and loudness: steadier than one second on sparse drums
                        let (mut pk, mut sq, mut n, mut ss, mut sn) = (0.0f32, 0.0f32, 0usize, 0.0f32, 0usize);
                        for h in &self.ap_hist {
                            pk = pk.max(h.0);
                            sq += h.1;
                            n += h.2;
                            ss += h.3;
                            sn += h.4;
                        }
                        let rms3 = if n > 0 { (sq / n as f32).sqrt() } else { 0.0 };
                        let crest3 = 20.0 * (pk / rms3.max(1e-9)).log10();
                        let sone3 = if sn > 0 { ss / sn as f32 } else { 0.0 };
                        let phon3 = if sone3 > 1e-3 { 40.0 + 10.0 * sone3.log2() } else { 0.0 };
                        let warm = self.ap_hist.iter().all(|h| h.2 > 0);
                        if !self.auto_locked && warm && sone3 > 0.5 {
                            let floor_err = crest3 - s.autopilot.crest_floor_db; // headroom to push
                            let loud_err = s.autopilot.target_phon - phon3;
                            // proportional: half the error per second, at most 2 dB; the floor
                            // always wins; a +-1 phon deadband around the target
                            let step = if floor_err < 0.0 {
                                floor_err.max(-2.0)
                            } else if loud_err > 1.0 {
                                (loud_err * 0.5).min(floor_err * 0.5).min(2.0)
                            } else if loud_err < -1.0 {
                                (loud_err * 0.5).max(-2.0)
                            } else {
                                0.0
                            };
                            // near the floor the measurement lags the drive by up to three
                            // seconds, so damp the last dB to avoid hunting
                            let step = if floor_err.abs() < 1.5 { step * 0.5 } else { step };
                            let next = (self.auto_drive_db + step).clamp(-12.0, 24.0);
                            self.auto_drive_db = next;
                            // settled = the last four learned values span less than 1 dB
                            self.ap_drive_hist.rotate_right(1);
                            self.ap_drive_hist[0] = next;
                            self.ap_ticks += 1;
                            if self.ap_ticks >= 4 {
                                let (mut lo, mut hi) = (f32::MAX, f32::MIN);
                                for v in self.ap_drive_hist {
                                    lo = lo.min(v);
                                    hi = hi.max(v);
                                }
                                if hi - lo < 1.0 {
                                    self.auto_locked = true;
                                }
                            }
                        }
                    } else {
                        self.auto_drive_db = 0.0;
                        self.auto_locked = false;
                        self.auto_settled = 0;
                    }
                    self.auto_was_on = s.autopilot.on;
                }
            }
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
        if s.ref_spl != self.sone_wet.ref_spl {
            self.sone_wet.ref_spl = s.ref_spl;
            self.sone_dry.ref_spl = s.ref_spl;
            for m in &mut self.sone_tr {
                m.ref_spl = s.ref_spl;
            }
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
            assert!(s.len < 8, "Bloater supports at most 2 channels");
            s.items[s.len] = Some(it);
            s.len += 1;
        }
        s
    }
}
