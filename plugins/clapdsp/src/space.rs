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
//! `Growls` and `Sub` delay the signal by `LOOK_MS` (reported as latency) so the gain is already down when
//! the clap arrives. `ClapTail` and `Off` add no latency; `Off` is bit transparent.
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
    /// Sub: stays down until this long after the hit.
    pub sub_hold_ms: f32,
    /// Sub: back to full level this long after the hit.
    pub sub_back_ms: f32,
    /// Clap tail: dip (positive dB) between `tail_from_ms` and `tail_to_ms`.
    pub tail_db: f32,
    pub tail_from_ms: f32,
    pub tail_to_ms: f32,
    /// Clap body: lift (dB) of the centre between `BODY_LO_HZ` and `BODY_HI_HZ` from `body_from_ms` to `body_to_ms`.
    pub body_db: f32,
    /// Clap body: lift (dB) of the sides in the same band and window, so the body can be filled without narrowing it.
    pub body_side_db: f32,
    pub body_from_ms: f32,
    pub body_to_ms: f32,
}

impl Default for SpaceSettings {
    fn default() -> Self {
        SpaceSettings {
            role: SpaceRole::Off,
            thr_db: 12.0,
            split_hz: 150.0,
            depth_db: 24.0,
            back_ms: 250.0,
            sub_hold_ms: 55.0,
            sub_back_ms: 130.0,
            tail_db: 7.0,
            tail_from_ms: 150.0,
            tail_to_ms: 250.0,
            body_db: 0.0,
            body_side_db: 0.0,
            body_from_ms: 20.0,
            body_to_ms: 95.0,
        }
    }
}

/// Lookahead of the `Growls` and `Sub` roles.
pub const LOOK_MS: f32 = 5.0;
/// The reference point of the timings ("the hit") lies this long after the detector fires.
pub const HIT_AFTER_TRIGGER_MS: f32 = 2.9;
/// How far the sub is taken down at the hit.
pub const SUB_DEPTH_DB: f32 = 32.0;
/// The growls return over this long, centred on `back_ms`.
pub const GROWL_RETURN_MS: f32 = 30.0;
pub const RETRIGGER_MS: f32 = 150.0;
/// After a trigger the detector re-arms only once the envelope has fallen this far below the trigger
/// level, so a clap with a long bright tail (a bus that adds reverb or upward compression) counts once.
pub const REARM_DB: f32 = 12.0;
/// Band of the body lift.
pub const BODY_LO_HZ: f32 = 120.0;
pub const BODY_HI_HZ: f32 = 1800.0;
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

#[inline]
fn db_to_gain(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

pub struct Space {
    sr: f32,
    cur: SpaceSettings,
    /// LR8 low-pass per channel: BW4 twice.
    lp: [[Bq; 4]; 2],
    /// Body band of the mid channel: 2nd-order high-pass then low-pass.
    body_bp: [Bq; 2],
    body_gain: f32,
    /// The same band of the side channel.
    side_bp: [Bq; 2],
    side_gain: f32,
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
        let mut s = Space {
            sr: sample_rate,
            cur: SpaceSettings { split_hz: f32::NAN, ..Default::default() },
            lp: [[Bq::default(); 4]; 2],
            body_bp: [Bq::default(); 2],
            body_gain: 1.0,
            side_bp: [Bq::default(); 2],
            side_gain: 1.0,
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
        s.body_bp[0].highpass(sample_rate as f64, BODY_LO_HZ as f64, std::f64::consts::FRAC_1_SQRT_2);
        s.body_bp[1].lowpass(sample_rate as f64, BODY_HI_HZ as f64, std::f64::consts::FRAC_1_SQRT_2);
        s.side_bp = s.body_bp;
        s.update(&SpaceSettings::default());
        s
    }

    /// Latency in samples for the given role.
    pub fn latency(&self, role: SpaceRole) -> u32 {
        match role {
            SpaceRole::Growls | SpaceRole::Sub => self.look as u32,
            _ => 0,
        }
    }

    pub fn reset(&mut self) {
        for ch in self.lp.iter_mut() {
            for b in ch.iter_mut() {
                b.clear();
            }
        }
        self.det.clear();
        for b in self.body_bp.iter_mut().chain(self.side_bp.iter_mut()) {
            b.clear();
        }
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
                if u <= -LOOK_MS || t >= r1 {
                    1.0
                } else if u < 0.0 {
                    db_to_gain(d * (u + LOOK_MS) / LOOK_MS)
                } else if t <= r0 {
                    db_to_gain(d)
                } else {
                    db_to_gain(d * (r1 - t) / GROWL_RETURN_MS)
                }
            }
            SpaceRole::Sub => {
                let d = -SUB_DEPTH_DB;
                // down over the lookahead and the first milliseconds of the hit
                let down_end = HIT_AFTER_TRIGGER_MS + 3.0;
                let hold = s.sub_hold_ms.max(down_end);
                let back = s.sub_back_ms.max(hold + 1.0);
                if u <= -LOOK_MS || t >= back {
                    1.0
                } else if u < down_end {
                    db_to_gain(d * (u + LOOK_MS) / (down_end + LOOK_MS))
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
                let d = -s.tail_db.abs();
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
        let delayed = matches!(role, SpaceRole::Growls | SpaceRole::Sub);
        let split = matches!(role, SpaceRole::Growls | SpaceRole::ClapTail);
        let ms_per_sample = 1000.0 / self.sr;
        let mut min_gain = 1.0f32;
        for i in 0..len {
            // trigger
            let trig = if delayed {
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
                (self.since as f32 - self.look as f32) * ms_per_sample
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
                let (mid, side) = if nch > 1 {
                    (0.5 * (channels[0][i] + channels[1][i]), 0.5 * (channels[0][i] - channels[1][i]))
                } else {
                    (channels[0][i], 0.0)
                };
                let hp = self.body_bp[0].run(mid as f64);
                let bp = self.body_bp[1].run(hp) as f32;
                let shp = self.side_bp[0].run(side as f64);
                let sbp = self.side_bp[1].run(shp) as f32;
                if self.body_gain != 1.0 {
                    body_add = (self.body_gain - 1.0) * bp;
                }
                if self.side_gain != 1.0 {
                    side_add = (self.side_gain - 1.0) * sbp;
                }
            }

            for ch in 0..nch {
                let x = if delayed {
                    let y = self.delay[ch][self.dpos];
                    self.delay[ch][self.dpos] = channels[ch][i];
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
                    if g == 1.0 && !delayed {
                        x + add
                    } else {
                        (low + g as f64 * (x as f64 - low)) as f32 + add
                    }
                } else {
                    g * x
                };
            }
            if delayed {
                self.dpos += 1;
                if self.dpos >= self.look.max(1) {
                    self.dpos = 0;
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
        let mut sp = Space::new(SR);
        let s = SpaceSettings { role, ..Default::default() };
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
        assert_eq!(sp.latency(SpaceRole::ClapTail), 0);
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
        let full = rms_db(&y[ms(20.0)..ms(150.0)]);
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
        let sub_full = rms_db(&ys[ms(40.0)..ms(160.0)]);
        let sub_held = rms_db(&ys[hit + ms(40.0)..hit + ms(200.0)]);
        assert!((sub_full - sub_held).abs() < 1.0, "sub moved {:.2} dB", sub_full - sub_held);
        // an octave above the split the full depth is reached
        let (yh, _) = run(SpaceRole::Growls, &tone(n, 300.0, 0.5), &side);
        let d300 = rms_db(&yh[ms(40.0)..ms(160.0)]) - rms_db(&yh[hit + ms(40.0)..hit + ms(200.0)]);
        assert!(d300 > 22.0, "300 Hz only {d300:.1} dB down");
    }

    #[test]
    fn sub_drops_at_the_hit_and_is_back_by_130_ms() {
        let n = ms(700.0);
        let at = ms(200.0);
        let side = clap(n, at);
        let hit = at + ms(LOOK_MS) + ms(HIT_AFTER_TRIGGER_MS);
        let (y, _) = run(SpaceRole::Sub, &tone(n, 45.0, 0.8), &side);
        let peak = |a: usize, b: usize| y[a..b].iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let full = peak(ms(60.0), ms(150.0));
        // full level until 5 ms before the trigger point
        assert!((peak(hit - ms(40.0), hit - ms(10.0)) / full - 1.0).abs() < 0.02);
        // down by 32 dB from just after the hit to 55 ms
        let down = peak(hit + ms(5.0), hit + ms(54.0));
        assert!(20.0 * (down / full).log10() < -30.0, "{:.1} dB", 20.0 * (down / full).log10());
        // back at 130 ms
        assert!((peak(hit + ms(135.0), hit + ms(200.0)) / full - 1.0).abs() < 0.02);
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
            sp.process(&mut [&mut l, &mut r], None, n);
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
        sp.process(&mut [&mut l, &mut r], None, n);
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
    fn nothing_happens_without_a_sidechain() {
        let n = ms(500.0);
        let x = tone(n, 1000.0, 0.5);
        let mut sp = Space::new(SR);
        sp.update(&SpaceSettings { role: SpaceRole::Growls, ..Default::default() });
        sp.reset();
        let mut l = x.clone();
        let mut r = x.clone();
        sp.process(&mut [&mut l, &mut r], None, n);
        assert_eq!(sp.triggers, 0);
        let look = ms(LOOK_MS);
        // only the delay
        for i in look..n {
            assert!((l[i] - x[i - look]).abs() < 1e-6);
        }
    }
}
