//! Bus clipper: everything before a fixed 1x hard clip at 0 dBFS. There is no output trim and no
//! ceiling control on purpose: the clip is the ceiling, and loudness is shaped by what is sent
//! into it (drive, band leveler, tilt/bell EQ, optional sub split). Zero latency.
//!
//! Signal path: drive → EQ (tilt + bell) → band leveler → [soft knee] → [sub split] → hard clip 0 dBFS.

use crate::dsp::eq::{EqSettings, PreEq};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BusSettings {
    pub drive_db: f32,
    pub tilt_db: f32,
    pub tilt_hz: f32,
    pub bell_on: bool,
    pub bell_hz: f32,
    pub bell_db: f32,
    pub lev_on: bool,
    pub lev_lo_hz: f32,
    pub lev_hi_hz: f32,
    pub lev_thr_db: f32,
    pub lev_ratio: f32,
    pub lev_attack_ms: f32,
    pub lev_release_ms: f32,
    pub sub_on: bool,
    pub sub_hz: f32,
    /// Sub path ceiling relative to 0 dBFS (<= 0).
    pub sub_ceil_db: f32,
    /// Bypass the clip stage (the pre-stage still runs) for A/B listening.
    pub clip_on: bool,
    /// Soft stage before the hard clip: linear below `soft_knee` (0..1 of the ceiling), tanh into
    /// the ceiling above it; `soft_push_db` is the gain from the soft stage into the hard clip.
    pub soft_on: bool,
    pub soft_knee: f32,
    pub soft_push_db: f32,
}

impl Default for BusSettings {
    fn default() -> Self {
        BusSettings {
            drive_db: 0.0,
            tilt_db: 0.0,
            tilt_hz: 1000.0,
            bell_on: false,
            bell_hz: 2000.0,
            bell_db: 3.0,
            lev_on: false,
            lev_lo_hz: 250.0,
            lev_hi_hz: 1000.0,
            lev_thr_db: -22.0,
            lev_ratio: 3.0,
            lev_attack_ms: 3.0,
            lev_release_ms: 80.0,
            sub_on: false,
            sub_hz: 90.0,
            sub_ceil_db: -3.0,
            clip_on: true,
            soft_on: false,
            soft_knee: 0.5,
            soft_push_db: 0.2,
        }
    }
}

/// Second-order section, transposed direct form II, f64.
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
    fn lowpass(sr: f64, f: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * f / sr;
        let (cw, sw) = (w0.cos(), w0.sin());
        let alpha = sw / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Bq {
            b0: (1.0 - cw) / 2.0 / a0,
            b1: (1.0 - cw) / a0,
            b2: (1.0 - cw) / 2.0 / a0,
            a1: -2.0 * cw / a0,
            a2: (1.0 - alpha) / a0,
            s1: 0.0,
            s2: 0.0,
        }
    }
    fn highpass(sr: f64, f: f64) -> Self {
        let w0 = 2.0 * std::f64::consts::PI * f / sr;
        let (cw, sw) = (w0.cos(), w0.sin());
        let alpha = sw / (2.0 * std::f64::consts::FRAC_1_SQRT_2);
        let a0 = 1.0 + alpha;
        Bq {
            b0: (1.0 + cw) / 2.0 / a0,
            b1: -(1.0 + cw) / a0,
            b2: (1.0 + cw) / 2.0 / a0,
            a1: -2.0 * cw / a0,
            a2: (1.0 - alpha) / a0,
            s1: 0.0,
            s2: 0.0,
        }
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

/// Linkwitz-Riley 4th order 2-way split (two cascaded Butterworth-2 each side); the sum of the
/// two outputs is all-pass flat.
#[derive(Clone, Copy, Default)]
struct Lr4 {
    lp: [Bq; 2],
    hp: [Bq; 2],
}

impl Lr4 {
    fn new(sr: f64, f: f64) -> Self {
        Lr4 { lp: [Bq::lowpass(sr, f); 2], hp: [Bq::highpass(sr, f); 2] }
    }
    #[inline]
    fn split(&mut self, x: f64) -> (f64, f64) {
        let l0 = self.lp[0].run(x);
        let lo = self.lp[1].run(l0);
        let h0 = self.hp[0].run(x);
        let hi = self.hp[1].run(h0);
        (lo, hi)
    }
    fn clear(&mut self) {
        for b in self.lp.iter_mut().chain(self.hp.iter_mut()) {
            b.clear();
        }
    }
}

const MAX_CH: usize = 2;

pub struct BusClip {
    sr: f32,
    nch: usize,
    cur: BusSettings,
    eq: PreEq,
    // leveler
    lev_lo: [Lr4; MAX_CH],
    lev_hi: [Lr4; MAX_CH],
    lev_env: f32,
    lev_att: f32,
    lev_rel: f32,
    // sub split
    sub_x: [Lr4; MAX_CH],
    // smoothed drive
    drive_lin: f32,
    drive_target: f32,
    drive_coef: f32,
    // meters (per processed block)
    pub in_peak: f32,
    pub out_pinned: f32,
    pub lev_gr_db: f32,
}

impl BusClip {
    pub fn new(sample_rate: f32, num_channels: usize) -> Self {
        let sr = sample_rate as f64;
        let s = BusSettings::default();
        let mut b = BusClip {
            sr: sample_rate,
            nch: num_channels.min(MAX_CH),
            cur: BusSettings { drive_db: f32::NAN, ..s },
            eq: PreEq::new(sample_rate, num_channels.min(MAX_CH)),
            lev_lo: [Lr4::new(sr, s.lev_lo_hz as f64); MAX_CH],
            lev_hi: [Lr4::new(sr, s.lev_hi_hz as f64); MAX_CH],
            lev_env: 0.0,
            lev_att: 0.0,
            lev_rel: 0.0,
            sub_x: [Lr4::new(sr, s.sub_hz as f64); MAX_CH],
            drive_lin: 1.0,
            drive_target: 1.0,
            drive_coef: 1.0 - (-1.0f32 / (0.005 * sample_rate)).exp(),
            in_peak: 0.0,
            out_pinned: 0.0,
            lev_gr_db: 0.0,
        };
        b.update(&s);
        b.drive_lin = b.drive_target;
        b
    }

    pub fn reset(&mut self) {
        self.eq.reset();
        for x in self.lev_lo.iter_mut().chain(self.lev_hi.iter_mut()).chain(self.sub_x.iter_mut()) {
            x.clear();
        }
        self.lev_env = 0.0;
        self.drive_lin = self.drive_target;
    }

    pub fn update(&mut self, s: &BusSettings) {
        let sr = self.sr as f64;
        if s.lev_lo_hz != self.cur.lev_lo_hz {
            for x in &mut self.lev_lo {
                *x = Lr4::new(sr, s.lev_lo_hz.clamp(20.0, 20000.0) as f64);
            }
        }
        if s.lev_hi_hz != self.cur.lev_hi_hz {
            for x in &mut self.lev_hi {
                *x = Lr4::new(sr, s.lev_hi_hz.clamp(20.0, 20000.0) as f64);
            }
        }
        if s.sub_hz != self.cur.sub_hz {
            for x in &mut self.sub_x {
                *x = Lr4::new(sr, s.sub_hz.clamp(20.0, 500.0) as f64);
            }
        }
        self.lev_att = 1.0 - (-1.0f32 / (s.lev_attack_ms.max(0.05) * 0.001 * self.sr)).exp();
        self.lev_rel = 1.0 - (-1.0f32 / (s.lev_release_ms.max(1.0) * 0.001 * self.sr)).exp();
        self.drive_target = 10f32.powf(s.drive_db / 20.0);
        let t = s.tilt_db;
        let eqs = EqSettings {
            freq_hz: [s.tilt_hz, s.bell_hz, 2500.0, s.tilt_hz],
            gain_db: [-t * 0.5, if s.bell_on { s.bell_db } else { 0.0 }, 0.0, t * 0.5],
        };
        self.eq.update(&eqs);
        self.cur = *s;
    }

    /// Process `len` samples in place.
    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize) {
        let nch = self.nch.min(channels.len());
        let s = self.cur;
        // drive
        let mut peak = 0.0f32;
        for i in 0..len {
            self.drive_lin += (self.drive_target - self.drive_lin) * self.drive_coef;
            for ch in 0..nch {
                channels[ch][i] *= self.drive_lin;
            }
        }
        // EQ
        self.eq.process(channels, len);
        // band leveler
        let mut gr_min = 0.0f32;
        if s.lev_on {
            let slope = 1.0 - 1.0 / s.lev_ratio.max(1.0);
            for i in 0..len {
                let mut mids = [0.0f64; MAX_CH];
                let mut rest = [0.0f64; MAX_CH];
                let mut det = 0.0f32;
                for ch in 0..nch {
                    let x = channels[ch][i] as f64;
                    let (lo, hi) = self.lev_lo[ch].split(x);
                    let (mid, top) = self.lev_hi[ch].split(hi);
                    mids[ch] = mid;
                    rest[ch] = lo + top;
                    det = det.max(mid.abs() as f32);
                }
                let c = if det > self.lev_env { self.lev_att } else { self.lev_rel };
                self.lev_env += (det - self.lev_env) * c;
                let lvl = 20.0 * self.lev_env.max(1e-6).log10();
                let over = (lvl - s.lev_thr_db).max(0.0);
                let gr = -over * slope;
                gr_min = gr_min.min(gr);
                let g = 10f32.powf(gr / 20.0) as f64;
                for ch in 0..nch {
                    channels[ch][i] = (rest[ch] + mids[ch] * g) as f32;
                }
            }
        }
        self.lev_gr_db = gr_min;
        // sub split + clip
        let mut pinned = 0usize;
        let sub_ceil = 10f32.powf(s.sub_ceil_db.min(0.0) / 20.0);
        let knee = s.soft_knee.clamp(0.05, 0.95);
        let push = 10f32.powf(s.soft_push_db / 20.0);
        for i in 0..len {
            for ch in 0..nch {
                let mut x = channels[ch][i];
                peak = peak.max(x.abs());
                if s.soft_on && s.clip_on {
                    x = soft_knee(x, knee) * push;
                }
                let y = if !s.clip_on {
                    x
                } else if s.sub_on {
                    let (lo, hi) = self.sub_x[ch].split(x as f64);
                    let lo = (lo as f32).clamp(-sub_ceil, sub_ceil);
                    let hi = (hi as f32).clamp(-1.0, 1.0);
                    (lo + hi).clamp(-1.0, 1.0)
                } else {
                    x.clamp(-1.0, 1.0)
                };
                if y.abs() >= 0.999 {
                    pinned += 1;
                }
                channels[ch][i] = y;
            }
        }
        self.in_peak = peak;
        self.out_pinned = if len > 0 { pinned as f32 / (len * nch.max(1)) as f32 } else { 0.0 };
    }
}

/// Linear below `knee`, tanh from the knee into 1.0 above it (continuous value and slope).
#[inline]
pub fn soft_knee(x: f32, knee: f32) -> f32 {
    let a = x.abs();
    if a <= knee {
        x
    } else {
        (knee + (1.0 - knee) * ((a - knee) / (1.0 - knee)).tanh()).copysign(x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(f: f32, sr: f32, n: usize, amp: f32) -> Vec<f32> {
        (0..n).map(|i| amp * (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()).collect()
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn lr4_sums_flat() {
        let sr = 44100.0;
        for f in [50.0, 200.0, 1000.0, 5000.0, 12000.0] {
            let mut x = Lr4::new(sr, 500.0);
            let n = 44100;
            let mut out = Vec::with_capacity(n);
            for i in 0..n {
                let v = (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin();
                let (a, b) = x.split(v);
                out.push((a + b) as f32);
            }
            let r = rms(&out[n / 2..]) * 2f32.sqrt();
            assert!((20.0 * r.log10()).abs() < 0.05, "{f} Hz: {r}");
        }
    }

    #[test]
    fn unity_below_clip_and_hard_ceiling() {
        let sr = 44100.0;
        let mut b = BusClip::new(sr, 2);
        let n = 8192;
        let mut l = sine(1000.0, sr, n, 0.5);
        let mut r = l.clone();
        let orig = l.clone();
        b.process(&mut [&mut l, &mut r], n);
        for i in 0..n {
            assert!((l[i] - orig[i]).abs() < 1e-5);
        }
        let mut s = BusSettings::default();
        s.drive_db = 12.0;
        b.update(&s);
        b.reset();
        let mut l = sine(1000.0, sr, n, 0.5);
        let mut r = l.clone();
        b.process(&mut [&mut l, &mut r], n);
        assert!(l.iter().all(|v| v.abs() <= 1.0));
        assert!(b.out_pinned > 0.5, "{}", b.out_pinned);
        assert!(b.in_peak > 1.9, "{}", b.in_peak);
    }

    #[test]
    fn soft_stage_is_transparent_below_knee_and_bounded() {
        assert_eq!(soft_knee(0.3, 0.5), 0.3);
        assert_eq!(soft_knee(-0.5, 0.5), -0.5);
        // continuous and monotonic through the knee, never above 1
        let mut prev = 0.0;
        for i in 0..4000 {
            let x = i as f32 * 0.005;
            let y = soft_knee(x, 0.5);
            assert!(y >= prev - 1e-6 && y <= 1.0, "{x} -> {y}");
            assert!((y - prev).abs() < 0.006);
            prev = y;
        }
        assert!(soft_knee(20.0, 0.5) > 0.9999);
        // in the chain: +0.2 dB push makes heavily driven input reach the hard ceiling
        let sr = 44100.0;
        let mut b = BusClip::new(sr, 2);
        let mut s = BusSettings::default();
        s.soft_on = true;
        s.drive_db = 12.0;
        b.update(&s);
        b.reset();
        let n = 8192;
        let mut l = sine(1000.0, sr, n, 0.5);
        let mut r = l.clone();
        b.process(&mut [&mut l, &mut r], n);
        assert!(l.iter().all(|v| v.abs() <= 1.0));
        assert!(b.out_pinned > 0.2, "{}", b.out_pinned);
        // quiet material is untouched
        s.drive_db = 0.0;
        b.update(&s);
        b.reset();
        let mut l = sine(1000.0, sr, n, 0.3);
        let mut r = l.clone();
        let orig = l.clone();
        b.process(&mut [&mut l, &mut r], n);
        assert!(l.iter().zip(&orig).skip(1024).all(|(a, o)| (a - o * 10f32.powf(0.2 / 20.0)).abs() < 1e-3));
    }

    #[test]
    fn leveler_acts_in_band_only() {
        let sr = 44100.0;
        let mut s = BusSettings::default();
        s.lev_on = true;
        s.lev_thr_db = -20.0;
        s.lev_ratio = 4.0;
        let n = 44100;
        for (f, expect_gr) in [(500.0, true), (4000.0, false)] {
            let mut b = BusClip::new(sr, 2);
            b.update(&s);
            let mut l = sine(f, sr, n, 0.5); // -6 dBFS peak, 14 dB over threshold
            let mut r = l.clone();
            b.process(&mut [&mut l, &mut r], n);
            let out = rms(&l[n / 2..]);
            let inp = 0.5 / 2f32.sqrt();
            let d = 20.0 * (out / inp).log10();
            if expect_gr {
                assert!(d < -8.0 && d > -12.0, "{f} Hz: {d} dB, gr {}", b.lev_gr_db);
            } else {
                assert!(d.abs() < 0.3, "{f} Hz: {d} dB");
            }
        }
    }

    #[test]
    fn tilt_is_antisymmetric() {
        let sr = 44100.0;
        let mut s = BusSettings::default();
        s.tilt_db = 6.0;
        let mut b = BusClip::new(sr, 2);
        b.update(&s);
        let lo = b.eq.magnitude_at(100.0);
        let hi = b.eq.magnitude_at(10000.0);
        let mid = b.eq.magnitude_at(1000.0);
        assert!((20.0 * lo.log10() + 3.0).abs() < 0.5, "{lo}");
        assert!((20.0 * hi.log10() - 3.0).abs() < 0.5, "{hi}");
        assert!((20.0 * mid.log10()).abs() < 0.3, "{mid}");
    }
}
