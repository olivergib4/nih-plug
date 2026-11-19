//! Space: clears room around each clap. One instance does one of three jobs, chosen by `role`:
//!
//! * `Growls` (on the growl track, clap on the sidechain): everything above `split_hz` is turned down by
//!   `depth_db` from the clap until `back_ms` after it, then comes straight back. Below the split nothing
//!   changes, so a sub living in the same signal is left alone.
//! * `Sub` (on the sub track, clap on the sidechain): the whole signal drops out at the clap, stays down
//!   until `sub_hold_ms` and is back to full level at `sub_back_ms`.
//! * `ClapTail` (on the clap or its bus, triggered by its own input): everything above `split_hz` is dipped
//!   by `tail_db` between `tail_from_ms` and `tail_to_ms` after the hit, and the centre of the body
//!   (about 150 Hz to 1.5 kHz, mid channel only, with gentle edges) is lifted by `body_db` between `body_from_ms` and
//!   `body_to_ms`. Either can be set to 0 dB.
//!
//! Defaults are the values found by ear against the first hit of the second drop of the reference track:
//! 24 dB / 250 ms for the growls (18 dB or 200 ms already let the clap sink back, more than 24 dB / 250 ms
//! made no further difference), 55 / 130 ms for the sub, 7 dB from 150 to 250 ms for the clap tail.
//!
//! Trigger: the same relative detector as the Attack EQ (mid channel above 2 kHz coming within `thr_db` of
//! the loudest recent hit, at most once every 150 ms and not again until the level has fallen a further
//! 12 dB, so a long bright tail does not count as a second hit), fed from the sidechain for `Growls` and `Sub` and from
//! the input for `ClapTail`. Times in the settings count from the hit, which is `HIT_AFTER_TRIGGER_MS` after
//! the trigger (the detector fires on the first milliseconds of the zap). Feed it the clap on its own: on a
//! clipped drum bus the kick's click is as bright as the clap and triggers it too.
//!
//! `Growls` and `Sub` delay the signal by `LOOK_MS` (reported as latency) so the gain is already down
//! `before_ms` ahead of the clap (40 ms by default: in every bounce where the clap's attack came through
//! at full level, the mix above 150 Hz and the sub were out for about 40 ms before the hit). `ClapTail` and `Off` add no latency; `Off` is bit transparent.
//!
//! The split is complementary: low = LR8 low-pass (two 4th-order Butterworth in series, the same magnitude
//! as the zero-phase 4th-order filter the values were found with), high = input - low, so with the gain at
//! 0 dB the output is the input exactly. While the band is turned down, a sub well below the split stays
//! within 1 dB (the low-pass delays it slightly); an octave above the split the full depth is reached.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceRole {
    Off,
    Growls,
    Sub,
    ClapTail,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpaceSettings {
    pub role: SpaceRole,
    /// Trigger when the 2 kHz+ envelope of the trigger signal comes within this many dB of the loudest recent hit.
    pub thr_db: f32,
    pub split_hz: f32,
    /// Growls: how far the band above the split is turned down (positive dB).
    pub depth_db: f32,
    /// Growls: back to full level this long after the hit.
    pub back_ms: f32,
    /// Growls: after coming back they stay this many dB (positive) under full level until `soft1_until_ms`
    /// after the hit, then `soft2_db` under until `soft2_until_ms`, then full. Each change takes
    /// `SOFT_STEP_MS`, so it can be placed in a gap between two growl hits and is not heard as a swell.
    pub soft1_db: f32,
    pub soft1_until_ms: f32,
    pub soft2_db: f32,
    pub soft2_until_ms: f32,
    /// Growls and Sub: already down this long before the clap (0 = only from the clap on). At most
    /// `LOOK_MS - FADE_MS`.
    pub before_ms: f32,
    /// Sub: stays down until this long after the hit.
    pub sub_hold_ms: f32,
    /// Sub: back to full level this long after the hit.
    pub sub_back_ms: f32,
    /// Clap tail: dip (positive dB) or lift (negative dB) between `tail_from_ms` and `tail_to_ms`.
    pub tail_db: f32,
    pub tail_from_ms: f32,
    pub tail_to_ms: f32,
    /// Clap body: lift (dB) of the centre between `BODY_LO_HZ` and `BODY_HI_HZ` from `body_from_ms` to `body_to_ms`.
    pub body_db: f32,
    /// Clap body: lift (dB) of the sides in the same band and window, so the body can be filled without narrowing it.
    pub body_side_db: f32,
    pub body_from_ms: f32,
    pub body_to_ms: f32,
    /// Clap lean (dB): the left channel's mids come up by this much early in the clap and the right
    /// channel's highs by 0.8 of it late, so the clap starts a little left and ends a little right.
    pub lean_db: f32,
    /// Clap left/right tone split (dB): in the body of the clap (from `TONE_FROM_MS` to `TONE_TO_MS` after the
    /// hit) the left channel is this much louder than the right around `TONE_MID_HZ`, and the right is
    /// `TONE_HIGH_RATIO` of it louder than the left around `TONE_HIGH_HZ`. Half goes on each side, so the
    /// sum of the two stays where it was. Negative mirrors it.
    pub tone_split_db: f32,
    /// Clap tail to centre (dB): from `CENTRE_FROM_MS` to `tail_to_ms` the sides of the tail come down by
    /// this much and both centre and sides are then raised together so the stereo level stays where it
    /// was. The tail folds to mono louder and loses nothing in stereo.
    pub centre_db: f32,
    /// Clap push (dB): the whole signal is raised by this much from `push_from_ms` to `push_to_ms` after
    /// the hit (5 ms up, 30 ms back), so the clap's body goes into the following clip harder.
    pub push_db: f32,
    pub push_from_ms: f32,
    pub push_to_ms: f32,
    /// Clap tail: trigger from the sidechain (the clap on its own) instead of from the input. On a drum
    /// bus the first kick after a silence is the loudest recent hit and would be shaped as if it were a
    /// clap; keyed from the clap's own bus only claps trigger. Silent or missing sidechain: no triggers.
    pub tail_keyed: bool,
}

impl Default for SpaceSettings {
    fn default() -> Self {
        SpaceSettings {
            role: SpaceRole::Off,
            thr_db: 12.0,
            split_hz: 150.0,
            depth_db: 24.0,
            back_ms: 250.0,
            soft1_db: 0.0,
            soft1_until_ms: 375.0,
            soft2_db: 0.0,
            soft2_until_ms: 505.0,
            before_ms: 40.0,
            sub_hold_ms: 55.0,
            sub_back_ms: 130.0,
            tail_db: 7.0,
            tail_from_ms: 150.0,
            tail_to_ms: 250.0,
            body_db: 0.0,
            body_side_db: 0.0,
            body_from_ms: 20.0,
            body_to_ms: 95.0,
            lean_db: 0.0,
            tone_split_db: 0.0,
            centre_db: 0.0,
            push_db: 0.0,
            push_from_ms: 23.0,
            push_to_ms: 63.0,
            tail_keyed: false,
        }
    }
}

/// Lookahead of the `Growls` and `Sub` roles: fixed, so the latency does not move with the "before" time.
pub const LOOK_MS: f32 = 64.0;
/// The gain goes down over this long, ending `before_ms` ahead of the clap.
pub const FADE_MS: f32 = 5.0;
/// The reference point of the timings ("the hit") lies this long after the detector fires.
pub const HIT_AFTER_TRIGGER_MS: f32 = 2.9;
/// How far the sub is taken down at the hit.
pub const SUB_DEPTH_DB: f32 = 32.0;
/// The growls return over this long, centred on `back_ms`.
pub const GROWL_RETURN_MS: f32 = 30.0;
/// Each change of the softer level after the return takes this long.
pub const SOFT_STEP_MS: f32 = 20.0;
pub const RETRIGGER_MS: f32 = 150.0;
/// After a trigger the detector re-arms only once the envelope has fallen this far below the trigger
/// level, so a clap with a long bright tail (a bus that adds reverb or upward compression) counts once.
pub const REARM_DB: f32 = 12.0;
/// Band of the body lift: the response of a 3rd-order Butterworth band-pass run forwards and backwards
/// (no phase shift), as a linear-phase filter of `2 * BODY_FIR_HALF_MS` so it can run in real time. The
/// lifted band is then in phase with the rest at every frequency: a filter with phase shift, added back,
/// takes away above the band what it adds inside it (1.4 dB at 3 to 8 kHz with the old one).
pub const BODY_LO_HZ: f32 = 150.0;
pub const BODY_HI_HZ: f32 = 1500.0;
/// Half length of the body filter, which is also the latency of the `ClapTail` role.
pub const BODY_FIR_HALF_MS: f32 = 20.0;
/// Lean: left mids from 15 to 60 ms, right highs from 70 to 180 ms. The corners sit outside the bands that
/// are meant to move (600 Hz to 1.5 kHz, and 1.5 kHz up) because the gentle filters only reach the full
/// amount about an octave inside their corners.
pub const LEAN_MID_LO_HZ: f32 = 420.0;
pub const LEAN_MID_HI_HZ: f32 = 2100.0;
pub const LEAN_HIGH_LO_HZ: f32 = 1000.0;
pub const LEAN_HIGH_HI_HZ: f32 = 14000.0;
pub const LEAN_RIGHT_RATIO: f32 = 0.8;
/// Left/right tone split: two bells per channel, and the part of the clap they act on.
pub const TONE_MID_HZ: f32 = 950.0;
pub const TONE_MID_Q: f32 = 1.0;
pub const TONE_HIGH_HZ: f32 = 3600.0;
pub const TONE_HIGH_Q: f32 = 0.8;
pub const TONE_HIGH_RATIO: f32 = 0.6;
pub const TONE_FROM_MS: f32 = 12.0;
pub const TONE_TO_MS: f32 = 105.0;
const TONE_FADE_IN_MS: f32 = 8.0;
const TONE_FADE_OUT_MS: f32 = 20.0;
/// Tail to centre: band, start (the end is `tail_to_ms`) and fades.
pub const CENTRE_LO_HZ: f32 = 400.0;
pub const CENTRE_HI_HZ: f32 = 7000.0;
pub const CENTRE_FROM_MS: f32 = 85.0;
const CENTRE_FADE_MS: f32 = 25.0;
/// The level match never lifts by more than this.
const CENTRE_MAX_LIFT_DB: f32 = 6.0;
const BODY_FADE_IN_MS: f32 = 12.0;
const BODY_FADE_OUT_MS: f32 = 30.0;
const TAIL_FADE_IN_MS: f32 = 30.0;
const TAIL_FADE_OUT_MS: f32 = 20.0;

#[derive(Clone, Copy, Default)]
struct Bq {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    s1: f64,
    s2: f64,
}

impl Bq {
    fn lowpass(&mut self, sr: f64, f0: f64, q: f64) {
        let w = 2.0 * std::f64::consts::PI * f0 / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / (2.0 * q);
        let a0 = 1.0 + al;
        self.b0 = (1.0 - c) / 2.0 / a0;
        self.b1 = (1.0 - c) / a0;
        self.b2 = (1.0 - c) / 2.0 / a0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - al) / a0;
    }
    fn highpass(&mut self, sr: f64, f0: f64, q: f64) {
        let w = 2.0 * std::f64::consts::PI * f0 / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / (2.0 * q);
        let a0 = 1.0 + al;
        self.b0 = (1.0 + c) / 2.0 / a0;
        self.b1 = -(1.0 + c) / a0;
        self.b2 = (1.0 + c) / 2.0 / a0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - al) / a0;
    }
    /// A bell of `gain_db` at `f0`.
    fn peaking(&mut self, sr: f64, f0: f64, q: f64, gain_db: f64) {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * f0 / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / (2.0 * q);
        let a0 = 1.0 + al / a;
        self.b0 = (1.0 + al * a) / a0;
        self.b1 = -2.0 * c / a0;
        self.b2 = (1.0 - al * a) / a0;
        self.a1 = -2.0 * c / a0;
        self.a2 = (1.0 - al / a) / a0;
    }
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b0 * x + self.s1;
        self.s1 = self.b1 * x - self.a1 * y + self.s2;
        self.s2 = self.b2 * x - self.a2 * y;
        y
    }
    fn clear(&mut self) {
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
}

/// The two section Qs of a 4th-order Butterworth filter.
const BW4_Q: [f64; 2] = [0.541_196_100_146_197, 1.306_562_964_876_376_4];

/// Linear-phase band filter (odd length, symmetric) with its own input history.
struct Fir {
    taps: Vec<f32>,
    hist: Vec<f32>,
    pos: usize,
}

impl Fir {
    /// Zero-phase 3rd-order Butterworth band-pass (magnitude of the forwards-backwards run), `half` taps
    /// each side of the centre, Hann windowed.
    fn bandpass(sr: f32, lo: f32, hi: f32, half: usize) -> Fir {
        let n = 2 * half + 1;
        let m = 8192usize.max((4 * n).next_power_of_two());
        let wl = (std::f64::consts::PI * lo as f64 / sr as f64).tan();
        let wh = (std::f64::consts::PI * (hi as f64).min(0.45 * sr as f64) / sr as f64).tan();
        let (w0sq, bw) = (wl * wh, wh - wl);
        // desired response on the grid 0..m/2
        let mag: Vec<f64> = (0..=m / 2)
            .map(|k| {
                if k == 0 || k == m / 2 {
                    return 0.0;
                }
                let w = (std::f64::consts::PI * k as f64 / m as f64).tan();
                let x = (w * w - w0sq) / (bw * w);
                1.0 / (1.0 + x.powi(6))
            })
            .collect();
        let mut taps = vec![0.0f32; n];
        for (i, t) in taps.iter_mut().enumerate() {
            let d = i as f64 - half as f64;
            let mut acc = mag[0] + mag[m / 2] * (std::f64::consts::PI * d).cos();
            for (k, g) in mag.iter().enumerate().take(m / 2).skip(1) {
                acc += 2.0 * g * (2.0 * std::f64::consts::PI * k as f64 * d / m as f64).cos();
            }
            let win = 0.5 + 0.5 * (std::f64::consts::PI * d / (half as f64 + 1.0)).cos();
            *t = (acc / m as f64 * win) as f32;
        }
        Fir { taps, hist: vec![0.0; n], pos: 0 }
    }
    fn clear(&mut self) {
        self.hist.fill(0.0);
        self.pos = 0;
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let n = self.taps.len();
        self.hist[self.pos] = x;
        // newest sample at `pos`, going back in time towards lower indices (wrapping)
        let (a, b) = self.hist.split_at(self.pos + 1);
        let mut acc = 0.0f32;
        // taps are symmetric, so the direction of the walk does not matter
        for (h, t) in a.iter().rev().zip(self.taps.iter()) {
            acc += h * t;
        }
        for (h, t) in b.iter().rev().zip(self.taps[a.len()..].iter()) {
            acc += h * t;
        }
        self.pos += 1;
        if self.pos >= n {
            self.pos = 0;
        }
        acc
    }
}

#[inline]
fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub struct Space {
    sr: f32,
    cur: SpaceSettings,
    /// LR8 low-pass per channel: BW4 twice.
    lp: [[Bq; 4]; 2],
    /// Body band of the left and of the right channel.
    body_fir: [Fir; 2],
    body_gain: f32,
    side_gain: f32,
    /// Delay of the `ClapTail` role (half the body filter).
    clook: usize,
    cdelay: [Vec<f32>; 2],
    cpos: usize,
    /// Lean: mid band of the left channel, high band of the right.
    lean_l: [Bq; 2],
    lean_r: [Bq; 2],
    lean_l_gain: f32,
    /// Tone split: mid and high bell of the left and of the right channel, and how far it is faded in.
    tone_l: [Bq; 2],
    tone_r: [Bq; 2],
    tone_w: f32,
    lean_r_gain: f32,
    /// Tail to centre: band of the mid and of the side channel, their smoothed powers, the weight.
    cen_m: [Bq; 2],
    cen_s: [Bq; 2],
    cen_pm: f32,
    cen_ps: f32,
    cen_w: f32,
    cen_pow: f32,
    push_gain: f32,
    det: Bq,
    env: f32,
    env_rel: f32,
    peak: f32,
    peak_rel: f32,
    armed: bool,
    /// Loudest level of the hit that triggered last (its first 20 ms).
    trig_level: f32,
    /// Samples since the last trigger (large when idle).
    since: usize,
    look: usize,
    delay: [Vec<f32>; 2],
    dpos: usize,
    gain: f32,
    slew: f32,
    /// Number of triggers so far (for the editor and tests).
    pub triggers: u32,
    /// Lowest gain applied in the last block, dB (0 when idle).
    pub gain_db: f32,
}

impl Space {
    pub fn new(sample_rate: f32) -> Self {
        let look = (LOOK_MS * sample_rate / 1000.0).round() as usize;
        let clook = (BODY_FIR_HALF_MS * sample_rate / 1000.0).round() as usize;
        let mut s = Space {
            sr: sample_rate,
            cur: SpaceSettings { split_hz: f32::NAN, ..Default::default() },
            lp: [[Bq::default(); 4]; 2],
            body_fir: [Fir::bandpass(sample_rate, BODY_LO_HZ, BODY_HI_HZ, clook), Fir::bandpass(sample_rate, BODY_LO_HZ, BODY_HI_HZ, clook)],
            body_gain: 1.0,
            side_gain: 1.0,
            clook,
            cdelay: [vec![0.0; clook.max(1)], vec![0.0; clook.max(1)]],
            cpos: 0,
            lean_l: [Bq::default(); 2],
            lean_r: [Bq::default(); 2],
            lean_l_gain: 1.0,
            tone_l: [Bq::default(); 2],
            tone_r: [Bq::default(); 2],
            tone_w: 0.0,
            lean_r_gain: 1.0,
            cen_m: [Bq::default(); 2],
            cen_s: [Bq::default(); 2],
            cen_pm: 0.0,
            cen_ps: 0.0,
            cen_w: 0.0,
            push_gain: 1.0,
            // 20 ms power average
            cen_pow: 1.0 - (-1.0f32 / (0.020 * sample_rate)).exp(),
            det: Bq::default(),
            env: 0.0,
            env_rel: (-1.0f32 / (0.005 * sample_rate)).exp(),
            peak: 0.0,
            peak_rel: 10f32.powf(-2.0 / 20.0 / sample_rate),
            armed: true,
            trig_level: 0.0,
            since: usize::MAX / 2,
            look,
            delay: [vec![0.0; look.max(1)], vec![0.0; look.max(1)]],
            dpos: 0,
            gain: 1.0,
            // 0.3 ms: only there so a retrigger in the middle of a curve cannot click
            slew: 1.0 - (-1.0f32 / (0.0003 * sample_rate)).exp(),
            triggers: 0,
            gain_db: 0.0,
        };
        s.det.highpass(sample_rate as f64, 2000.0, std::f64::consts::FRAC_1_SQRT_2);
        let q = std::f64::consts::FRAC_1_SQRT_2;
        s.lean_l[0].highpass(sample_rate as f64, LEAN_MID_LO_HZ as f64, q);
        s.lean_l[1].lowpass(sample_rate as f64, LEAN_MID_HI_HZ as f64, q);
        s.lean_r[0].highpass(sample_rate as f64, LEAN_HIGH_LO_HZ as f64, q);
        s.lean_r[1].lowpass(sample_rate as f64, (LEAN_HIGH_HI_HZ as f64).min(0.45 * sample_rate as f64), q);
        s.cen_m[0].highpass(sample_rate as f64, CENTRE_LO_HZ as f64, q);
        s.cen_m[1].lowpass(sample_rate as f64, (CENTRE_HI_HZ as f64).min(0.45 * sample_rate as f64), q);
        s.cen_s = s.cen_m;
        s.update(&SpaceSettings::default());
        s
    }

    /// Latency in samples for the given role.
    pub fn latency(&self, role: SpaceRole) -> u32 {
        match role {
            SpaceRole::Growls | SpaceRole::Sub => self.look as u32,
            SpaceRole::ClapTail => self.clook as u32,
            SpaceRole::Off => 0,
        }
    }

    pub fn reset(&mut self) {
        for ch in self.lp.iter_mut() {
            for b in ch.iter_mut() {
                b.clear();
            }
        }
        self.det.clear();
        for b in self.lean_l.iter_mut().chain(self.lean_r.iter_mut()) {
            b.clear();
        }
        for f in self.body_fir.iter_mut() {
            f.clear();
        }
        for d in self.cdelay.iter_mut() {
            d.fill(0.0);
        }
        self.cpos = 0;
        self.lean_l_gain = 1.0;
        self.lean_r_gain = 1.0;
        for b in self.tone_l.iter_mut().chain(self.tone_r.iter_mut()) {
            b.clear();
        }
        self.tone_w = 0.0;
        for b in self.cen_m.iter_mut().chain(self.cen_s.iter_mut()) {
            b.clear();
        }
        self.cen_pm = 0.0;
        self.cen_ps = 0.0;
        self.cen_w = 0.0;
        self.push_gain = 1.0;
        self.body_gain = 1.0;
        self.side_gain = 1.0;
        self.env = 0.0;
        self.peak = 0.0;
        self.armed = true;
        self.trig_level = 0.0;
        self.since = usize::MAX / 2;
        for d in self.delay.iter_mut() {
            d.fill(0.0);
        }
        self.dpos = 0;
        self.gain = 1.0;
        self.gain_db = 0.0;
    }

    pub fn update(&mut self, s: &SpaceSettings) {
        if s.split_hz != self.cur.split_hz {
            let f = s.split_hz.clamp(20.0, 2000.0) as f64;
            for ch in self.lp.iter_mut() {
                for (i, b) in ch.iter_mut().enumerate() {
                    b.lowpass(self.sr as f64, f, BW4_Q[i % 2]);
                }
            }
        }
        if s.tone_split_db != self.cur.tone_split_db {
            let (sr, d) = (self.sr as f64, s.tone_split_db as f64);
            let h = d * TONE_HIGH_RATIO as f64;
            self.tone_l[0].peaking(sr, TONE_MID_HZ as f64, TONE_MID_Q as f64, 0.5 * d);
            self.tone_l[1].peaking(sr, TONE_HIGH_HZ as f64, TONE_HIGH_Q as f64, -0.5 * h);
            self.tone_r[0].peaking(sr, TONE_MID_HZ as f64, TONE_MID_Q as f64, -0.5 * d);
            self.tone_r[1].peaking(sr, TONE_HIGH_HZ as f64, TONE_HIGH_Q as f64, 0.5 * h);
            for b in self.tone_l.iter_mut().chain(self.tone_r.iter_mut()) {
                b.clear();
            }
        }
        if s.role != self.cur.role {
            // the delay only runs for some roles: start it empty so nothing stale comes out
            for d in self.delay.iter_mut() {
                d.fill(0.0);
            }
            self.dpos = 0;
            self.since = usize::MAX / 2;
            self.gain = 1.0;
        }
        self.cur = *s;
    }

    /// Target gain `u` ms after the trigger point in the (possibly delayed) signal.
    #[inline]
    fn curve(&self, u: f32) -> f32 {
        let s = &self.cur;
        // time from the hit
        let t = u - HIT_AFTER_TRIGGER_MS;
        match s.role {
            SpaceRole::Off => 1.0,
            SpaceRole::Growls => {
                let d = -s.depth_db.abs();
                let r0 = s.back_ms - 0.5 * GROWL_RETURN_MS;
                let r1 = s.back_ms + 0.5 * GROWL_RETURN_MS;
                let b = s.before_ms.clamp(0.0, LOOK_MS - FADE_MS);
                // the softer levels after the return: `a1` until `u1`, `a2` until `u2`, then full
                let u1 = s.soft1_until_ms;
                let a1 = if s.soft1_db > 0.0 && u1 > r1 { -s.soft1_db } else { 0.0 };
                let u2 = s.soft2_until_ms.max(u1 + SOFT_STEP_MS);
                let a2 = if a1 < 0.0 && s.soft2_db > 0.0 { -s.soft2_db } else { 0.0 };
                let h = 0.5 * SOFT_STEP_MS;
                if u <= -(b + FADE_MS) {
                    1.0
                } else if u < -b {
                    db_to_gain(d * (u + b + FADE_MS) / FADE_MS)
                } else if t <= r0 {
                    db_to_gain(d)
                } else if t < r1 {
                    db_to_gain(a1 + (d - a1) * (r1 - t) / GROWL_RETURN_MS)
                } else if a1 == 0.0 {
                    1.0
                } else if t <= u1 - h {
                    db_to_gain(a1)
                } else if t < u1 + h {
                    db_to_gain(a1 + (a2 - a1) * (t - (u1 - h)) / SOFT_STEP_MS)
                } else if a2 == 0.0 || t >= u2 + h {
                    1.0
                } else if t <= u2 - h {
                    db_to_gain(a2)
                } else {
                    db_to_gain(a2 * (u2 + h - t) / SOFT_STEP_MS)
                }
            }
            SpaceRole::Sub => {
                let d = -SUB_DEPTH_DB;
                // down over the lookahead and the first milliseconds of the hit
                let down_end = HIT_AFTER_TRIGGER_MS + 3.0;
                let hold = s.sub_hold_ms.max(down_end);
                let back = s.sub_back_ms.max(hold + 1.0);
                let b = s.before_ms.clamp(0.0, LOOK_MS - FADE_MS);
                // with no "before" time the fall runs into the first milliseconds of the hit, as it always did
                let (f0, f1) = if b > 0.0 { (-(b + FADE_MS), -b) } else { (-FADE_MS, down_end) };
                if u <= f0 || t >= back {
                    1.0
                } else if u < f1 {
                    db_to_gain(d * (u - f0) / (f1 - f0))
                } else if t <= hold {
                    db_to_gain(d)
                } else {
                    // smoothstep in amplitude from the floor back to 1
                    let x = (t - hold) / (back - hold);
                    let w = x * x * (3.0 - 2.0 * x);
                    let g0 = db_to_gain(d);
                    g0 + (1.0 - g0) * w
                }
            }
            SpaceRole::ClapTail => {
                // positive dips the tail, negative lifts it
                let d = -s.tail_db;
                let from = s.tail_from_ms;
                let to = s.tail_to_ms.max(from + 1.0);
                let a0 = from - TAIL_FADE_IN_MS;
                let o0 = to - 0.75 * TAIL_FADE_OUT_MS;
                let o1 = to + 0.25 * TAIL_FADE_OUT_MS;
                if t <= a0 || t >= o1 {
                    1.0
                } else if t < from {
                    db_to_gain(d * (t - a0) / TAIL_FADE_IN_MS)
                } else if t <= o0 {
                    db_to_gain(d)
                } else {
                    db_to_gain(d * (o1 - t) / (o1 - o0))
                }
            }
        }
    }

    /// Target gain of the body band `t` ms after the hit (`ClapTail` role).
    #[inline]
    fn body_curve(&self, t: f32, amount_db: f32) -> f32 {
        let s = &self.cur;
        if amount_db == 0.0 {
            return 1.0;
        }
        let from = s.body_from_ms;
        let to = s.body_to_ms.max(from + 1.0);
        let a0 = from - BODY_FADE_IN_MS + 2.0;
        let a1 = from + 2.0;
        let o1 = to + BODY_FADE_OUT_MS;
        if t <= a0 || t >= o1 {
            1.0
        } else if t < a1 {
            db_to_gain(amount_db * (t - a0) / BODY_FADE_IN_MS)
        } else if t <= to {
            db_to_gain(amount_db)
        } else {
            db_to_gain(amount_db * (o1 - t) / BODY_FADE_OUT_MS)
        }
    }

    /// A dB-linear trapezoid: 0 dB until `a0`, `amount_db` from `a1` to `o0`, 0 dB again from `o1`.
    #[inline]
    fn trapezoid(t: f32, amount_db: f32, a0: f32, a1: f32, o0: f32, o1: f32) -> f32 {
        if amount_db == 0.0 || t <= a0 || t >= o1 {
            1.0
        } else if t < a1 {
            db_to_gain(amount_db * (t - a0) / (a1 - a0))
        } else if t <= o0 {
            db_to_gain(amount_db)
        } else {
            db_to_gain(amount_db * (o1 - t) / (o1 - o0))
        }
    }

    /// Process `len` samples of `channels` in place. `side` is the sidechain (the clap) for the `Growls`
    /// and `Sub` roles; `ClapTail` triggers from `channels` itself.
    pub fn process(&mut self, channels: &mut [&mut [f32]], side: Option<[&[f32]; 2]>, len: usize) {
        let nch = channels.len().min(2);
        let role = self.cur.role;
        if nch == 0 || role == SpaceRole::Off {
            self.gain_db = 0.0;
            return;
        }
        let rel = 10f32.powf(-self.cur.thr_db.abs() / 20.0);
        let rearm = rel * 10f32.powf(-REARM_DB / 20.0);
        let retrig = (RETRIGGER_MS * self.sr / 1000.0) as usize;
        let hit_len = (0.020 * self.sr) as usize;
        let keyed = matches!(role, SpaceRole::Growls | SpaceRole::Sub);
        let delayed = keyed || role == SpaceRole::ClapTail;
        let look = if keyed { self.look } else { self.clook };
        let split = matches!(role, SpaceRole::Growls | SpaceRole::ClapTail);
        let ms_per_sample = 1000.0 / self.sr;
        let mut min_gain = 1.0f32;
        for i in 0..len {
            // trigger
            let trig = if keyed || (role == SpaceRole::ClapTail && self.cur.tail_keyed) {
                match side {
                    Some(sc) => 0.5 * (sc[0][i] + sc[1][i]),
                    None => 0.0,
                }
            } else if nch > 1 {
                0.5 * (channels[0][i] + channels[1][i])
            } else {
                channels[0][i]
            };
            let d = (self.det.run(trig as f64) as f32).abs();
            self.env = if d > self.env { d } else { self.env * self.env_rel };
            self.peak = (self.peak * self.peak_rel).max(self.env);
            if self.env < self.peak * rearm {
                self.armed = true;
            }
            if self.since < hit_len {
                self.trig_level = self.trig_level.max(self.env);
            }
            // fire when re-armed, or when something 12 dB louder than the last hit arrives (a steady
            // background that happened to trigger must not make the detector miss the clap)
            let louder = self.env >= self.trig_level * 4.0;
            if (self.armed || louder) && self.since >= retrig && self.env > 1e-3 && self.env >= self.peak * rel {
                self.armed = false;
                self.since = 0;
                self.trig_level = self.env;
                self.triggers = self.triggers.wrapping_add(1);
            }

            // gain for the sample that leaves the delay now
            let u = if delayed {
                (self.since as f32 - look as f32) * ms_per_sample
            } else {
                self.since as f32 * ms_per_sample
            };
            let target = if self.since > (self.sr as usize) * 4 { 1.0 } else { self.curve(u) };
            self.gain += (target - self.gain) * self.slew;
            if (self.gain - target).abs() < 1e-6 {
                self.gain = target;
            }
            let g = self.gain;
            min_gain = min_gain.min(g);
            // body lift: the band-passed mid and side channels are added back, scaled
            let mut body_add = 0.0f32;
            let mut side_add = 0.0f32;
            if role == SpaceRole::ClapTail {
                let idle = self.since > (self.sr as usize) * 4;
                let t = u - HIT_AFTER_TRIGGER_MS;
                let bt = if idle { 1.0 } else { self.body_curve(t, self.cur.body_db) };
                let st = if idle || nch < 2 { 1.0 } else { self.body_curve(t, self.cur.body_side_db) };
                self.body_gain += (bt - self.body_gain) * self.slew;
                if (self.body_gain - bt).abs() < 1e-6 {
                    self.body_gain = bt;
                }
                self.side_gain += (st - self.side_gain) * self.slew;
                if (self.side_gain - st).abs() < 1e-6 {
                    self.side_gain = st;
                }
                // the band of what comes in now lines up with what leaves the delay now
                let bl = self.body_fir[0].run(channels[0][i]);
                let (bp, sbp) = if nch > 1 {
                    let br = self.body_fir[1].run(channels[1][i]);
                    (0.5 * (bl + br), 0.5 * (bl - br))
                } else {
                    (bl, 0.0)
                };
                if self.body_gain != 1.0 {
                    body_add = (self.body_gain - 1.0) * bp;
                }
                if self.side_gain != 1.0 {
                    side_add = (self.side_gain - 1.0) * sbp;
                }
            }

            for ch in 0..nch {
                let x = if keyed {
                    let y = self.delay[ch][self.dpos];
                    self.delay[ch][self.dpos] = channels[ch][i];
                    y
                } else if delayed {
                    let y = self.cdelay[ch][self.cpos];
                    self.cdelay[ch][self.cpos] = channels[ch][i];
                    y
                } else {
                    channels[ch][i]
                };
                channels[ch][i] = if split {
                    let mut low = x as f64;
                    for b in self.lp[ch].iter_mut() {
                        low = b.run(low);
                    }
                    // the side band goes to the two channels with opposite sign
                    let add = body_add + if ch == 0 { side_add } else { -side_add };
                    if g == 1.0 && !keyed {
                        x + add
                    } else {
                        (low + g as f64 * (x as f64 - low)) as f32 + add
                    }
                } else {
                    g * x
                };
            }
            // tail to centre, on what the body and tail stages produced: sides of the tail down, then both
            // up by whatever keeps the band's stereo power where it was
            if role == SpaceRole::ClapTail && nch > 1 {
                let idle = self.since > (self.sr as usize) * 4;
                let t = u - HIT_AFTER_TRIGGER_MS;
                let amount = self.cur.centre_db.abs();
                let to = self.cur.tail_to_ms.max(CENTRE_FROM_MS + 1.0);
                let wt = if idle || amount == 0.0 || t <= CENTRE_FROM_MS - CENTRE_FADE_MS || t >= to + CENTRE_FADE_MS {
                    0.0
                } else if t < CENTRE_FROM_MS {
                    (t - (CENTRE_FROM_MS - CENTRE_FADE_MS)) / CENTRE_FADE_MS
                } else if t <= to {
                    1.0
                } else {
                    (to + CENTRE_FADE_MS - t) / CENTRE_FADE_MS
                };
                self.cen_w += (wt - self.cen_w) * self.slew;
                if (self.cen_w - wt).abs() < 1e-6 {
                    self.cen_w = wt;
                }
                let mid = 0.5 * (channels[0][i] + channels[1][i]);
                let side = 0.5 * (channels[0][i] - channels[1][i]);
                let mh = self.cen_m[0].run(mid as f64);
                let mb = self.cen_m[1].run(mh) as f32;
                let sh = self.cen_s[0].run(side as f64);
                let sb = self.cen_s[1].run(sh) as f32;
                self.cen_pm += (mb * mb - self.cen_pm) * self.cen_pow;
                self.cen_ps += (sb * sb - self.cen_ps) * self.cen_pow;
                if self.cen_w > 0.0 {
                    let g = db_to_gain(-amount);
                    let k = ((self.cen_pm + self.cen_ps) / (self.cen_pm + g * g * self.cen_ps + 1e-20)).sqrt().clamp(1.0, db_to_gain(CENTRE_MAX_LIFT_DB));
                    let m_add = self.cen_w * (k - 1.0) * mb;
                    let s_add = self.cen_w * (g * k - 1.0) * sb;
                    channels[0][i] += m_add + s_add;
                    channels[1][i] += m_add - s_add;
                }
            }
            // lean, on what the body and tail stages produced: the left channel's mids early, the right
            // channel's highs late
            if role == SpaceRole::ClapTail && nch > 1 {
                let idle = self.since > (self.sr as usize) * 4;
                let t = u - HIT_AFTER_TRIGGER_MS;
                let lean = self.cur.lean_db;
                let lt = if idle { 1.0 } else { Self::trapezoid(t, lean, 5.0, 15.0, 60.0, 90.0) };
                let rt = if idle { 1.0 } else { Self::trapezoid(t, lean * LEAN_RIGHT_RATIO, 40.0, 70.0, 180.0, 230.0) };
                self.lean_l_gain += (lt - self.lean_l_gain) * self.slew;
                if (self.lean_l_gain - lt).abs() < 1e-6 {
                    self.lean_l_gain = lt;
                }
                self.lean_r_gain += (rt - self.lean_r_gain) * self.slew;
                if (self.lean_r_gain - rt).abs() < 1e-6 {
                    self.lean_r_gain = rt;
                }
                let lh = self.lean_l[0].run(channels[0][i] as f64);
                let lb = self.lean_l[1].run(lh) as f32;
                let rh = self.lean_r[0].run(channels[1][i] as f64);
                let rb = self.lean_r[1].run(rh) as f32;
                if self.lean_l_gain != 1.0 {
                    channels[0][i] += (self.lean_l_gain - 1.0) * lb;
                }
                if self.lean_r_gain != 1.0 {
                    channels[1][i] += (self.lean_r_gain - 1.0) * rb;
                }
            }

            // left/right tone split, on what the stages above produced: each channel is cross-faded to its
            // own pair of bells for the body of the clap
            if role == SpaceRole::ClapTail && nch > 1 && self.cur.tone_split_db != 0.0 {
                let idle = self.since > (self.sr as usize) * 4;
                let t = u - HIT_AFTER_TRIGGER_MS;
                let wt = if idle || t <= TONE_FROM_MS || t >= TONE_TO_MS + TONE_FADE_OUT_MS {
                    0.0
                } else if t < TONE_FROM_MS + TONE_FADE_IN_MS {
                    (t - TONE_FROM_MS) / TONE_FADE_IN_MS
                } else if t <= TONE_TO_MS {
                    1.0
                } else {
                    (TONE_TO_MS + TONE_FADE_OUT_MS - t) / TONE_FADE_OUT_MS
                };
                self.tone_w += (wt - self.tone_w) * self.slew;
                if (self.tone_w - wt).abs() < 1e-6 {
                    self.tone_w = wt;
                }
                let (xl, xr) = (channels[0][i], channels[1][i]);
                let ml = self.tone_l[0].run(xl as f64);
                let yl = self.tone_l[1].run(ml) as f32;
                let mr = self.tone_r[0].run(xr as f64);
                let yr = self.tone_r[1].run(mr) as f32;
                if self.tone_w > 0.0 {
                    channels[0][i] = xl + self.tone_w * (yl - xl);
                    channels[1][i] = xr + self.tone_w * (yr - xr);
                }
            }

            // push: everything up for the length of the body, last so it scales what the other stages made
            if role == SpaceRole::ClapTail {
                let idle = self.since > (self.sr as usize) * 4;
                let t = u - HIT_AFTER_TRIGGER_MS;
                let from = self.cur.push_from_ms;
                let to = self.cur.push_to_ms.max(from + 6.0);
                let pt = if idle { 1.0 } else { Self::trapezoid(t, self.cur.push_db, from, from + 5.0, to, to + 30.0) };
                self.push_gain += (pt - self.push_gain) * self.slew;
                if (self.push_gain - pt).abs() < 1e-6 {
                    self.push_gain = pt;
                }
                if self.push_gain != 1.0 {
                    for ch in channels.iter_mut() {
                        ch[i] *= self.push_gain;
                    }
                }
            }

            if keyed {
                self.dpos += 1;
                if self.dpos >= self.look.max(1) {
                    self.dpos = 0;
                }
            } else if delayed {
                self.cpos += 1;
                if self.cpos >= self.clook.max(1) {
                    self.cpos = 0;
                }
            }
            self.since = self.since.saturating_add(1);
        }
        self.gain_db = 20.0 * min_gain.max(1e-6).log10();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 44100.0;

    fn ms(m: f32) -> usize {
        (m * SR / 1000.0).round() as usize
    }

    /// A clap-like burst (bright noise, 60 ms) at `at`, silence elsewhere.
    fn clap(n: usize, at: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                if i >= at && i < at + ms(60.0) {
                    let x = ((i as f32 * 12.9898).sin() * 43758.5).fract();
                    0.8 * x
                } else {
                    0.0
                }
            })
            .collect()
    }

    fn tone(n: usize, hz: f32, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * hz * i as f32 / SR).sin()).collect()
    }

    fn rms_db(x: &[f32]) -> f32 {
        10.0 * (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32 + 1e-20).log10()
    }

    fn run(role: SpaceRole, main: &[f32], side: &[f32]) -> (Vec<f32>, Space) {
        run_with(SpaceSettings { role, ..Default::default() }, main, side)
    }

    /// The `ClapTail` role delays by half its body filter: line the output up with the input again (the
    /// last samples, which have not come out yet, are taken from the input).
    fn undelay(sp: &Space, y: &mut Vec<f32>, x: &[f32]) {
        if sp.cur.role != SpaceRole::ClapTail {
            return;
        }
        let d = sp.latency(SpaceRole::ClapTail) as usize;
        let n = y.len();
        y.copy_within(d.., 0);
        y[n - d..].copy_from_slice(&x[n - d..]);
    }

    fn run_with(s: SpaceSettings, main: &[f32], side: &[f32]) -> (Vec<f32>, Space) {
        let mut sp = Space::new(SR);
        sp.update(&s);
        sp.reset();
        let mut l = main.to_vec();
        let mut r = main.to_vec();
        let mut pos = 0;
        while pos < l.len() {
            let len = (l.len() - pos).min(512);
            let (cl, cr) = (&mut l[pos..pos + len], &mut r[pos..pos + len]);
            sp.process(&mut [cl, cr], Some([&side[pos..pos + len], &side[pos..pos + len]]), len);
            pos += len;
        }
        undelay(&sp, &mut l, main);
        (l, sp)
    }

    #[test]
    fn off_is_bit_transparent_and_has_no_latency() {
        let n = 8192;
        let x: Vec<f32> = (0..n).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let side = clap(n, 2000);
        let (y, sp) = run(SpaceRole::Off, &x, &side);
        assert_eq!(x, y);
        assert_eq!(sp.latency(SpaceRole::Off), 0);
        assert_eq!(sp.latency(SpaceRole::ClapTail), ms(BODY_FIR_HALF_MS) as u32);
        assert_eq!(sp.latency(SpaceRole::Growls), ms(LOOK_MS) as u32);
    }

    #[test]
    fn growls_are_down_24_db_until_250_ms_and_the_low_end_is_untouched() {
        let n = ms(900.0);
        let at = ms(200.0);
        let side = clap(n, at);
        let look = ms(LOOK_MS);
        let hit = at + look + ms(HIT_AFTER_TRIGGER_MS);
        // a growl-like 1 kHz tone
        let (y, sp) = run(SpaceRole::Growls, &tone(n, 1000.0, 0.5), &side);
        assert_eq!(sp.triggers, 1);
        let full = rms_db(&y[ms(80.0)..ms(150.0)]);
        // already down when the clap arrives (the clap sits `look` later in the delayed signal)
        let at_hit = rms_db(&y[at + look..at + look + ms(10.0)]);
        assert!((full - at_hit - 24.0).abs() < 1.0, "at the hit {:.1} dB down", full - at_hit);
        let held = rms_db(&y[hit + ms(100.0)..hit + ms(230.0)]);
        assert!((full - held - 24.0).abs() < 0.5, "held {:.1} dB down", full - held);
        let back = rms_db(&y[hit + ms(270.0)..hit + ms(400.0)]);
        assert!((full - back).abs() < 0.2, "back {:.2} dB off", full - back);
        // a 50 Hz sub in the same signal passes (within 1 dB: the low-pass delays it a little, so a small
        // part of it lands in the band that is turned down)
        let (ys, _) = run(SpaceRole::Growls, &tone(n, 50.0, 0.5), &side);
        let sub_full = rms_db(&ys[ms(80.0)..ms(160.0)]);
        let sub_held = rms_db(&ys[hit + ms(40.0)..hit + ms(200.0)]);
        assert!((sub_full - sub_held).abs() < 1.0, "sub moved {:.2} dB", sub_full - sub_held);
        // an octave above the split the full depth is reached
        let (yh, _) = run(SpaceRole::Growls, &tone(n, 300.0, 0.5), &side);
        let d300 = rms_db(&yh[ms(80.0)..ms(160.0)]) - rms_db(&yh[hit + ms(40.0)..hit + ms(200.0)]);
        assert!(d300 > 22.0, "300 Hz only {d300:.1} dB down");
    }

    #[test]
    fn sub_drops_at_the_hit_and_is_back_by_130_ms() {
        let n = ms(700.0);
        let at = ms(200.0);
        let side = clap(n, at);
        let hit = at + ms(LOOK_MS) + ms(HIT_AFTER_TRIGGER_MS);
        let (y, _) = run_with(SpaceSettings { role: SpaceRole::Sub, before_ms: 0.0, ..Default::default() }, &tone(n, 45.0, 0.8), &side);
        let peak = |a: usize, b: usize| y[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let full = peak(ms(100.0), ms(180.0));
        // full level until 5 ms before the trigger point
        assert!((peak(hit - ms(40.0), hit - ms(10.0)) / full - 1.0).abs() < 0.02);
        // down by 32 dB from just after the hit to 55 ms
        let down = peak(hit + ms(5.0), hit + ms(54.0));
        assert!(20.0 * (down / full).log10() < -30.0, "{:.1} dB", 20.0 * (down / full).log10());
        // back at 130 ms
        assert!((peak(hit + ms(135.0), hit + ms(200.0)) / full - 1.0).abs() < 0.02);
    }

    #[test]
    fn growls_and_sub_are_already_down_40_ms_before_the_clap() {
        let n = ms(900.0);
        let at = ms(300.0);
        let side = clap(n, at);
        let hit = at + ms(LOOK_MS) + ms(HIT_AFTER_TRIGGER_MS);
        for (role, hz, depth) in [(SpaceRole::Growls, 1000.0, 24.0), (SpaceRole::Sub, 45.0, 32.0)] {
            let (y, _) = run(role, &tone(n, hz, 0.5), &side);
            let peak = |a: usize, b: usize| y[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            let full = peak(ms(100.0), ms(200.0));
            // untouched until 50 ms before the clap
            assert!((peak(hit - ms(90.0), hit - ms(50.0)) / full - 1.0).abs() < 0.02, "{role:?} moved early");
            // fully down from 40 ms before it
            let down = 20.0 * (peak(hit - ms(38.0), hit) / full).log10();
            assert!((down + depth).abs() < 1.5, "{role:?} {down:.1} dB before the clap");
            // with the time at 0 the 40 ms before the clap stay at full level
            let (y0, _) = run_with(SpaceSettings { role, before_ms: 0.0, ..Default::default() }, &tone(n, hz, 0.5), &side);
            let p0 = y0[hit - ms(40.0)..hit - ms(12.0)].iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!((p0 / full - 1.0).abs() < 0.02, "{role:?} moved with before = 0");
        }
    }

    #[test]
    fn clap_tail_dips_7_db_between_150_and_250_ms_only() {
        let n = ms(700.0);
        let at = ms(100.0);
        // a clap with a long 3 kHz tail on the same track
        let mut x = clap(n, at);
        for (i, v) in x.iter_mut().enumerate() {
            if i >= at {
                *v += 0.2 * (2.0 * std::f32::consts::PI * 3000.0 * i as f32 / SR).sin();
            }
        }
        let side = vec![0.0; n];
        let (y, sp) = run(SpaceRole::ClapTail, &x, &side);
        assert_eq!(sp.triggers, 1);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        // untouched through the body of the clap
        assert_eq!(&x[..hit + ms(115.0)], &y[..hit + ms(115.0)]);
        let d = rms_db(&x[hit + ms(160.0)..hit + ms(230.0)]) - rms_db(&y[hit + ms(160.0)..hit + ms(230.0)]);
        assert!((d - 7.0).abs() < 0.3, "{d:.2}");
        let after = rms_db(&x[hit + ms(270.0)..hit + ms(400.0)]) - rms_db(&y[hit + ms(270.0)..hit + ms(400.0)]);
        assert!(after.abs() < 0.05, "{after:.3}");
    }

    #[test]
    fn a_negative_tail_dip_lifts_the_tail_and_leaves_the_body_alone() {
        let n = ms(900.0);
        let at = ms(100.0);
        let mut x = clap(n, at);
        for (i, v) in x.iter_mut().enumerate() {
            if i >= at {
                *v += 0.05 * (2.0 * std::f32::consts::PI * 3000.0 * i as f32 / SR).sin();
            }
        }
        let side = vec![0.0; n];
        let s = SpaceSettings { role: SpaceRole::ClapTail, tail_db: -10.0, tail_from_ms: 110.0, tail_to_ms: 300.0, ..Default::default() };
        let (y, sp) = run_with(s, &x, &side);
        assert_eq!(sp.triggers, 1);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let body = rms_db(&y[hit..hit + ms(75.0)]) - rms_db(&x[hit..hit + ms(75.0)]);
        assert!(body.abs() < 0.05, "{body:.3}");
        let d = rms_db(&y[hit + ms(120.0)..hit + ms(280.0)]) - rms_db(&x[hit + ms(120.0)..hit + ms(280.0)]);
        assert!((d - 10.0).abs() < 0.3, "{d:.2}");
        let after = rms_db(&y[hit + ms(330.0)..hit + ms(500.0)]) - rms_db(&x[hit + ms(330.0)..hit + ms(500.0)]);
        assert!(after.abs() < 0.05, "{after:.3}");
    }

    #[test]
    fn growls_come_back_softer_in_two_steps_and_then_reach_full_level() {
        let n = ms(1200.0);
        let at = ms(100.0);
        let trig = clap(n, at);
        let x: Vec<f32> = (0..n).map(|i| 0.3 * (2.0 * std::f32::consts::PI * 2000.0 * i as f32 / SR).sin()).collect();
        let s = SpaceSettings { role: SpaceRole::Growls, soft1_db: 4.5, soft1_until_ms: 375.0, soft2_db: 2.0, soft2_until_ms: 505.0, ..Default::default() };
        let (y, sp) = run_with(s, &x, &trig);
        assert_eq!(sp.triggers, 1);
        let hit = at + ms(LOOK_MS) + ms(HIT_AFTER_TRIGGER_MS);
        let lvl = |a: f32, b: f32| rms_db(&y[hit + ms(a)..hit + ms(b)]) - rms_db(&x[hit + ms(a)..hit + ms(b)]);
        assert!((lvl(100.0, 220.0) + 24.0).abs() < 0.5, "{:.2}", lvl(100.0, 220.0));
        assert!((lvl(280.0, 355.0) + 4.5).abs() < 0.2, "{:.2}", lvl(280.0, 355.0));
        assert!((lvl(395.0, 485.0) + 2.0).abs() < 0.2, "{:.2}", lvl(395.0, 485.0));
        assert!(lvl(530.0, 700.0).abs() < 0.05, "{:.3}", lvl(530.0, 700.0));
    }

    #[test]
    fn keyed_clap_tail_ignores_a_kick_on_its_input() {
        // input: a bright click ("kick") at 100 ms after silence, the clap at 957 ms; sidechain: the clap only
        let n = ms(1800.0);
        let mut x = vec![0.0f32; n];
        let mut side = vec![0.0f32; n];
        for (at, to_side) in [(ms(100.0), false), (ms(957.0), true)] {
            for i in at..(at + ms(300.0)).min(n) {
                let t = (i - at) as f32 / SR;
                let noise = ((i as f32 * 12.9898).sin() * 43758.5).fract();
                let v = 0.9 * noise * 10f32.powf(-(30.0 / 0.150) * t / 20.0);
                x[i] += v;
                if to_side {
                    side[i] += v;
                }
            }
        }
        let mut st = SpaceSettings { role: SpaceRole::ClapTail, body_db: 8.0, push_db: 6.0, ..SpaceSettings::default() };
        let run_with = |st: &SpaceSettings| {
            let mut sp = Space::new(SR);
            sp.update(st);
            let mut l = x.clone();
            let mut r = x.clone();
            sp.process(&mut [&mut l[..], &mut r[..]], Some([&side[..], &side[..]]), n);
            (l, sp.triggers)
        };
        let (own, own_trigs) = run_with(&st);
        st.tail_keyed = true;
        let (key, key_trigs) = run_with(&st);
        assert_eq!(own_trigs, 2, "from its own input the kick triggers too");
        assert_eq!(key_trigs, 1, "keyed: only the clap");
        let look = ms(BODY_FIR_HALF_MS);
        // keyed: the kick passes unchanged (delayed by the latency)
        let a = ms(100.0) + look;
        let d = rms_db(&key[a..a + ms(90.0)]) - rms_db(&x[ms(100.0)..ms(190.0)]);
        assert!(d.abs() < 0.05, "kick changed by {d:.2} dB");
        // the clap is shaped the same either way
        let c = ms(957.0) + look;
        let e: f32 = (c..c + ms(300.0)).map(|i| (own[i] - key[i]).abs()).fold(0.0, f32::max);
        assert!(e < 1e-4, "clap differs by {e}");
    }

    #[test]
    fn a_long_bright_tail_does_not_retrigger() {
        // a clap whose tail is still only 8 dB down after 150 ms, as on a bus that adds reverb
        let n = ms(1800.0);
        let mut x = vec![0.0f32; n];
        for at in [ms(100.0), ms(957.0)] {
            for i in at..(at + ms(600.0)).min(n) {
                let t = (i - at) as f32 / SR;
                let noise = ((i as f32 * 12.9898).sin() * 43758.5).fract();
                x[i] += 0.9 * noise * 10f32.powf(-(8.0 / 0.150) * t / 20.0);
            }
        }
        let side = vec![0.0; n];
        let (y, sp) = run(SpaceRole::ClapTail, &x, &side);
        assert_eq!(sp.triggers, 2, "one trigger per clap");
        let hit = ms(100.0) + ms(HIT_AFTER_TRIGGER_MS);
        let d = rms_db(&x[hit + ms(160.0)..hit + ms(230.0)]) - rms_db(&y[hit + ms(160.0)..hit + ms(230.0)]);
        assert!((d - 7.0).abs() < 0.5, "tail dip {d:.2} dB");
    }

    #[test]
    fn body_lift_raises_the_centre_of_the_body_only() {
        let n = ms(900.0);
        // the steady tone below triggers once when it starts; the clap comes well after that
        let at = ms(400.0);
        let trig = clap(n, at);
        // a 600 Hz tone under the clap: in phase (centre) on one run, out of phase (sides) on the other
        let t600 = tone(n, 600.0, 0.1);
        let run_ms = |side_sign: f32| {
            let mut sp = Space::new(SR);
            sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, body_db: 8.0, ..Default::default() });
            sp.reset();
            let mut l: Vec<f32> = trig.iter().zip(&t600).map(|(a, b)| a + b).collect();
            let mut r: Vec<f32> = trig.iter().zip(&t600).map(|(a, b)| a + side_sign * b).collect();
            let (l0, r0) = (l.clone(), r.clone());
            let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
            (l0, r0, l, r)
        };
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let band = |x: &[f32]| {
            // crude 600 Hz probe: correlate with the tone
            let w = &t600[hit + ms(30.0)..hit + ms(90.0)];
            let y = &x[hit + ms(30.0)..hit + ms(90.0)];
            y.iter().zip(w).map(|(a, b)| a * b).sum::<f32>() / w.iter().map(|b| b * b).sum::<f32>()
        };
        let (l0, _, l, _) = run_ms(1.0);
        let lift = 20.0 * (band(&l) / band(&l0)).log10();
        assert!((lift - 8.0).abs() < 1.0, "centre lifted {lift:.2} dB");
        // untouched from the first clap-free moment before the hit to the body window, and after it
        assert_eq!(&l0[at - ms(20.0)..hit + ms(8.0)], &l[at - ms(20.0)..hit + ms(8.0)]);
        assert_eq!(&l0[hit + ms(130.0)..], &l[hit + ms(130.0)..]);
        // a side-only 600 Hz tone is not lifted
        let (l0, r0, l, r) = run_ms(-1.0);
        let s0: Vec<f32> = l0.iter().zip(&r0).map(|(a, b)| 0.5 * (a - b)).collect();
        let s1: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a - b)).collect();
        assert!((20.0 * (band(&s1) / band(&s0)).log10()).abs() < 0.1);
    }

    #[test]
    fn side_lift_raises_the_sides_of_the_body_and_leaves_the_centre() {
        let n = ms(900.0);
        let at = ms(400.0);
        let trig = clap(n, at);
        let t600 = tone(n, 600.0, 0.1);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, body_db: 0.0, body_side_db: 6.0, ..Default::default() });
        sp.reset();
        // the clap in the centre, the tone on the sides only
        let mut l: Vec<f32> = trig.iter().zip(&t600).map(|(a, b)| a + b).collect();
        let mut r: Vec<f32> = trig.iter().zip(&t600).map(|(a, b)| a - b).collect();
        let (l0, r0) = (l.clone(), r.clone());
        let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let probe = |x: &[f32]| {
            let w = &t600[hit + ms(30.0)..hit + ms(90.0)];
            x[hit + ms(30.0)..hit + ms(90.0)].iter().zip(w).map(|(a, b)| a * b).sum::<f32>() / w.iter().map(|b| b * b).sum::<f32>()
        };
        let side = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| 0.5 * (x - y)).collect() };
        let mid = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)).collect() };
        let lift = 20.0 * (probe(&side(&l, &r)) / probe(&side(&l0, &r0))).log10();
        assert!((lift - 6.0).abs() < 1.0, "sides lifted {lift:.2} dB");
        // the centre (the clap itself) is bit for bit what went in... up to rounding of the mid/side sum
        let (m0, m1) = (mid(&l0, &r0), mid(&l, &r));
        for i in 0..n {
            assert!((m0[i] - m1[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn tail_to_centre_moves_the_tail_inwards_at_the_same_stereo_level() {
        let n = ms(1000.0);
        let at = ms(400.0);
        let trig = clap(n, at);
        // a tail that is as loud on the sides as in the centre: 1 kHz in the centre, 1.5 kHz on the sides
        let c = tone(n, 1000.0, 0.05);
        let w = tone(n, 1500.0, 0.05);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, centre_db: 4.0, ..Default::default() });
        sp.reset();
        let mut l: Vec<f32> = (0..n).map(|i| trig[i] + c[i] + w[i]).collect();
        let mut r: Vec<f32> = (0..n).map(|i| trig[i] + c[i] - w[i]).collect();
        let (l0, r0) = (l.clone(), r.clone());
        let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let (i0, i1) = (hit + ms(120.0), hit + ms(240.0));
        let pw = |a: &[f32], b: &[f32], sign: f32| -> f32 { (i0..i1).map(|i| (0.5 * (a[i] + sign * b[i])).powi(2)).sum::<f32>() };
        let (m0, s0, m1, s1) = (pw(&l0, &r0, 1.0), pw(&l0, &r0, -1.0), pw(&l, &r, 1.0), pw(&l, &r, -1.0));
        let up = 10.0 * (m1 / m0).log10();
        let down = 10.0 * (s1 / s0).log10();
        let total = 10.0 * ((m1 + s1) / (m0 + s0)).log10();
        // sides 4 dB under the centre's change, the sum where it was
        assert!((up - down - 4.0).abs() < 0.6, "centre {up:.2} dB, sides {down:.2} dB");
        assert!(up > 1.0 && down < -1.5, "centre {up:.2} dB, sides {down:.2} dB");
        assert!(total.abs() < 0.4, "stereo level moved {total:.2} dB");
        // the hit, the body and everything after the tail are what went in
        let diff = |a: &[f32], b: &[f32], i0: usize, i1: usize| (i0..i1).map(|i| (a[i] - b[i]).abs()).fold(0.0f32, f32::max);
        // (the tones starting at sample 0 count as a first, quiet hit: look after its window)
        assert!(diff(&l0, &l, ms(320.0), hit + ms(55.0)) == 0.0, "before: {}", diff(&l0, &l, ms(320.0), hit + ms(55.0)));
        assert!(diff(&l0, &l, hit + ms(285.0), n) == 0.0, "after: {}", diff(&l0, &l, hit + ms(285.0), n));
        assert!(diff(&r0, &r, hit + ms(285.0), n) == 0.0);
    }

    #[test]
    fn body_lift_is_exact_in_the_band_and_takes_nothing_away_above_it() {
        let n = ms(900.0);
        let at = ms(400.0);
        let trig = clap(n, at);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        for (hz, want) in [(400.0, 8.0), (900.0, 8.0), (5000.0, 0.0), (9000.0, 0.0), (40.0, 0.0)] {
            let t = tone(n, hz, 0.05);
            let mut sp = Space::new(SR);
            sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, body_db: 8.0, body_side_db: 8.0, ..Default::default() });
            sp.reset();
            let x: Vec<f32> = (0..n).map(|i| trig[i] + t[i]).collect();
            let (mut l, mut r) = (x.clone(), x.clone());
            let (li, ri) = (l.clone(), r.clone());
            sp.process(&mut [&mut l, &mut r], None, n);
            undelay(&sp, &mut l, &li);
            undelay(&sp, &mut r, &ri);
            // after the burst has ended, while the lift is still fully on
            let (i0, i1) = (hit + ms(65.0), hit + ms(93.0));
            let got = 20.0 * ((l[i0..i1].iter().zip(&t[i0..i1]).map(|(p, q)| p * q).sum::<f32>() / t[i0..i1].iter().map(|q| q * q).sum::<f32>()).abs()).log10();
            assert!((got - want).abs() < 0.25, "{hz} Hz: {got:.2} dB, wanted {want}");
        }
    }

    #[test]
    fn push_raises_everything_for_the_length_of_the_body() {
        let n = ms(900.0);
        let at = ms(400.0);
        let trig = clap(n, at);
        let low = tone(n, 300.0, 0.05);
        let high = tone(n, 6000.0, 0.05);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, push_db: 6.0, ..Default::default() });
        sp.reset();
        let x: Vec<f32> = (0..n).map(|i| trig[i] + low[i] + high[i]).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let probe = |y: &[f32], w: &[f32], a: f32, b: f32| {
            let (i0, i1) = (hit + ms(a), hit + ms(b));
            20.0 * ((y[i0..i1].iter().zip(&w[i0..i1]).map(|(p, q)| p * q).sum::<f32>() / x[i0..i1].iter().zip(&w[i0..i1]).map(|(p, q)| p * q).sum::<f32>()).abs()).log10()
        };
        for w in [&low, &high] {
            assert!((probe(&l, w, 30.0, 62.0) - 6.0).abs() < 0.3, "{:.2}", probe(&l, w, 30.0, 62.0));
            assert!((probe(&r, w, 30.0, 62.0) - 6.0).abs() < 0.3);
        }
        // the hit itself and everything after the body are what went in
        // (the tones starting at sample 0 count as a first, quiet hit: look after its window)
        assert!(x[ms(320.0)..hit + ms(22.0)] == l[ms(320.0)..hit + ms(22.0)], "moved before the push");
        assert!(x[hit + ms(100.0)..] == l[hit + ms(100.0)..], "moved after the push");
    }

    #[test]
    fn lean_lifts_left_mids_early_and_right_highs_late() {
        let n = ms(900.0);
        let at = ms(400.0);
        let trig = clap(n, at);
        let mid = tone(n, 1000.0, 0.05);
        let high = tone(n, 4000.0, 0.05);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, lean_db: 2.5, ..Default::default() });
        sp.reset();
        let x: Vec<f32> = (0..n).map(|i| trig[i] + mid[i] + high[i]).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let probe = |y: &[f32], w: &[f32], a: f32, b: f32| {
            let (i0, i1) = (hit + ms(a), hit + ms(b));
            20.0 * ((y[i0..i1].iter().zip(&w[i0..i1]).map(|(p, q)| p * q).sum::<f32>() / x[i0..i1].iter().zip(&w[i0..i1]).map(|(p, q)| p * q).sum::<f32>()).abs()).log10()
        };
        // early: left mids up 2.5 dB, right untouched
        let lm = probe(&l, &mid, 20.0, 58.0);
        assert!((lm - 2.5).abs() < 0.7, "left mids {lm:.2} dB");
        assert!(probe(&r, &mid, 20.0, 38.0).abs() < 0.05);
        // late: right highs up 2 dB, left untouched
        let rh = probe(&r, &high, 80.0, 175.0);
        assert!((rh - 2.0).abs() < 0.7, "right highs {rh:.2} dB");
        assert!(probe(&l, &high, 100.0, 175.0).abs() < 0.05);
        // and nothing is left of it afterwards
        assert_eq!(&x[hit + ms(240.0)..], &l[hit + ms(240.0)..]);
        assert_eq!(&x[hit + ms(240.0)..], &r[hit + ms(240.0)..]);
    }

    #[test]
    fn tone_split_makes_the_left_louder_in_the_mids_and_the_right_in_the_highs_in_the_body_only() {
        let n = ms(900.0);
        let at = ms(400.0);
        let trig = clap(n, at);
        let mid = tone(n, 950.0, 0.05);
        let high = tone(n, 3600.0, 0.05);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::ClapTail, tail_db: 0.0, tone_split_db: 3.0, ..Default::default() });
        sp.reset();
        let x: Vec<f32> = (0..n).map(|i| trig[i] + mid[i] + high[i]).collect();
        let (mut l, mut r) = (x.clone(), x.clone());
        let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
        let hit = at + ms(HIT_AFTER_TRIGGER_MS);
        let probe = |y: &[f32], w: &[f32], a: f32, b: f32| {
            let (i0, i1) = (hit + ms(a), hit + ms(b));
            20.0 * ((y[i0..i1].iter().zip(&w[i0..i1]).map(|(p, q)| p * q).sum::<f32>() / x[i0..i1].iter().zip(&w[i0..i1]).map(|(p, q)| p * q).sum::<f32>()).abs()).log10()
        };
        let (lm, rm) = (probe(&l, &mid, 40.0, 100.0), probe(&r, &mid, 40.0, 100.0));
        assert!((lm - 1.5).abs() < 0.4 && (rm + 1.5).abs() < 0.4, "mids left {lm:.2} right {rm:.2} dB");
        let (lh, rh) = (probe(&l, &high, 40.0, 100.0), probe(&r, &high, 40.0, 100.0));
        assert!((lh + 0.9).abs() < 0.4 && (rh - 0.9).abs() < 0.4, "highs left {lh:.2} right {rh:.2} dB");
        // the first 12 ms after the hit and everything from 200 ms on are as they came in
        assert!(l[hit..hit + ms(10.0)] == x[hit..hit + ms(10.0)], "moved at the hit");
        let late = hit + ms(200.0);
        let err = l[late..].iter().zip(&x[late..]).chain(r[late..].iter().zip(&x[late..])).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1e-4, "still moving after the body: {err}");
    }

    #[test]
    fn nothing_happens_without_a_sidechain() {
        let n = ms(500.0);
        let x = tone(n, 1000.0, 0.5);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::Growls, ..Default::default() });
        sp.reset();
        let mut l = x.clone();
        let mut r = x.clone();
        let (li, ri) = (l.clone(), r.clone());
        sp.process(&mut [&mut l, &mut r], None, n);
        undelay(&sp, &mut l, &li);
        undelay(&sp, &mut r, &ri);
        assert_eq!(sp.triggers, 0);
        let look = ms(LOOK_MS);
        // only the delay
        for i in look..n {
            assert!((l[i] - x[i - look]).abs() < 1e-6);
        }
    }
}
