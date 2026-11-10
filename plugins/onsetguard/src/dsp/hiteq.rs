//! Attack EQ: a bell + high shelf that act only in a time window after each hit (default 8 to 60 ms), so the
//! first milliseconds (the zap) and the body of the hit stay untouched. Zero latency: the window starts after
//! the trigger, so nothing has to be known in advance. Trigger: the mid channel above 2 kHz rising to within
//! `thr_db` of the loudest recent hit (a peak follower falling 2 dB/s), at most once every 150 ms. The
//! threshold is relative so it does not depend on the level of the bus; a kick, whose top end is far below the
//! clap's, stays under it.
//!
//! out = x + w(t) * (eq(x) - x); the filters run all the time so their state is always settled.

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HitEqSettings {
    pub on: bool,
    /// Trigger when the 2 kHz+ envelope comes within this many dB of the loudest recent hit.
    pub thr_db: f32,
    pub start_ms: f32,
    pub len_ms: f32,
    pub bell_hz: f32,
    pub bell_db: f32,
    pub shelf_hz: f32,
    pub shelf_db: f32,
}

impl Default for HitEqSettings {
    fn default() -> Self {
        HitEqSettings { on: false, thr_db: 12.0, start_ms: 8.8, len_ms: 52.0, bell_hz: 2000.0, bell_db: -4.0, shelf_hz: 5000.0, shelf_db: 4.0 }
    }
}

pub const FADE_IN_MS: f32 = 1.0;
pub const FADE_OUT_MS: f32 = 15.0;
pub const RETRIGGER_MS: f32 = 150.0;
const BELL_Q: f64 = 1.0;

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
    fn set(&mut self, b: [f64; 3], a: [f64; 3]) {
        self.b0 = b[0] / a[0];
        self.b1 = b[1] / a[0];
        self.b2 = b[2] / a[0];
        self.a1 = a[1] / a[0];
        self.a2 = a[2] / a[0];
    }
    fn peaking(&mut self, sr: f64, f0: f64, gain_db: f64, q: f64) {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * f0 / sr;
        let al = w.sin() / (2.0 * q);
        self.set([1.0 + al * a, -2.0 * w.cos(), 1.0 - al * a], [1.0 + al / a, -2.0 * w.cos(), 1.0 - al / a]);
    }
    fn high_shelf(&mut self, sr: f64, f0: f64, gain_db: f64) {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * f0 / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / 2.0 * 2f64.sqrt();
        let sq = 2.0 * a.sqrt() * al;
        self.set(
            [a * ((a + 1.0) + (a - 1.0) * c + sq), -2.0 * a * ((a - 1.0) + (a + 1.0) * c), a * ((a + 1.0) + (a - 1.0) * c - sq)],
            [(a + 1.0) - (a - 1.0) * c + sq, 2.0 * ((a - 1.0) - (a + 1.0) * c), (a + 1.0) - (a - 1.0) * c - sq],
        );
    }
    fn highpass(&mut self, sr: f64, f0: f64) {
        let w = 2.0 * std::f64::consts::PI * f0 / sr;
        let (c, s) = (w.cos(), w.sin());
        let al = s / 2f64.sqrt();
        self.set([(1.0 + c) / 2.0, -(1.0 + c), (1.0 + c) / 2.0], [1.0 + al, -2.0 * c, 1.0 - al]);
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

pub struct HitEq {
    sr: f32,
    cur: HitEqSettings,
    bell: [Bq; 2],
    shelf: [Bq; 2],
    det: Bq,
    env: f32,
    env_rel: f32,
    peak: f32,
    peak_rel: f32,
    above: bool,
    /// Samples since the last trigger (large when idle).
    since: usize,
    /// Number of triggers so far (for meters/tests).
    pub triggers: u32,
}

impl HitEq {
    pub fn new(sample_rate: f32) -> Self {
        let mut h = HitEq {
            sr: sample_rate,
            cur: HitEqSettings { bell_hz: f32::NAN, shelf_hz: f32::NAN, ..Default::default() },
            bell: [Bq::default(); 2],
            shelf: [Bq::default(); 2],
            det: Bq::default(),
            env: 0.0,
            env_rel: (-1.0f32 / (0.005 * sample_rate)).exp(),
            peak: 0.0,
            peak_rel: 10f32.powf(-2.0 / 20.0 / sample_rate),
            above: false,
            since: usize::MAX / 2,
            triggers: 0,
        };
        h.det.highpass(sample_rate as f64, 2000.0);
        h.update(&HitEqSettings::default());
        h
    }

    pub fn reset(&mut self) {
        for b in self.bell.iter_mut().chain(self.shelf.iter_mut()) {
            b.clear();
        }
        self.det.clear();
        self.env = 0.0;
        self.peak = 0.0;
        self.above = false;
        self.since = usize::MAX / 2;
    }

    pub fn update(&mut self, s: &HitEqSettings) {
        if s.bell_hz != self.cur.bell_hz || s.bell_db != self.cur.bell_db {
            for b in &mut self.bell {
                b.peaking(self.sr as f64, s.bell_hz.clamp(20.0, 20000.0) as f64, s.bell_db as f64, BELL_Q);
            }
        }
        if s.shelf_hz != self.cur.shelf_hz || s.shelf_db != self.cur.shelf_db {
            for b in &mut self.shelf {
                b.high_shelf(self.sr as f64, s.shelf_hz.clamp(200.0, 18000.0) as f64, s.shelf_db as f64);
            }
        }
        self.cur = *s;
    }

    /// Window weight `since` samples after a trigger.
    #[inline]
    fn weight(&self, since: usize) -> f32 {
        let t = since as f32 * 1000.0 / self.sr;
        let s = self.cur;
        let a0 = s.start_ms - FADE_IN_MS;
        let end = s.start_ms + s.len_ms;
        if t <= a0 || t >= end + FADE_OUT_MS {
            0.0
        } else if t < s.start_ms {
            (t - a0) / FADE_IN_MS
        } else if t <= end {
            1.0
        } else {
            1.0 - (t - end) / FADE_OUT_MS
        }
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize) {
        let nch = channels.len().min(2);
        if nch == 0 {
            return;
        }
        let s = self.cur;
        let rel = 10f32.powf(-s.thr_db.abs() / 20.0);
        let retrig = (RETRIGGER_MS * self.sr / 1000.0) as usize;
        for i in 0..len {
            let mid = if nch > 1 { 0.5 * (channels[0][i] + channels[1][i]) } else { channels[0][i] };
            let d = (self.det.run(mid as f64) as f32).abs();
            self.env = if d > self.env { d } else { self.env * self.env_rel };
            self.peak = (self.peak * self.peak_rel).max(self.env);
            let now_above = self.env > 1e-3 && self.env >= self.peak * rel;
            if now_above && !self.above && self.since >= retrig {
                self.since = 0;
                self.triggers = self.triggers.wrapping_add(1);
            }
            self.above = now_above;
            let w = if s.on { self.weight(self.since) } else { 0.0 };
            for ch in 0..nch {
                let x = channels[ch][i] as f64;
                let y = self.shelf[ch].run(self.bell[ch].run(x));
                if w > 0.0 {
                    channels[ch][i] = (x + w as f64 * (y - x)) as f32;
                }
            }
            self.since = self.since.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acts_only_in_the_window_after_a_hit() {
        let sr = 44100.0;
        let mut h = HitEq::new(sr);
        let mut s = HitEqSettings::default();
        s.on = true;
        h.update(&s);
        // silence, then a 300 ms burst of a 6 kHz + 300 Hz mix starting at sample 4410
        let n = 44100;
        let mut l: Vec<f32> = (0..n)
            .map(|i| {
                if i >= 4410 && i < 4410 + 13230 {
                    let t = i as f32 / sr;
                    0.4 * (2.0 * std::f32::consts::PI * 6000.0 * t).sin() + 0.4 * (2.0 * std::f32::consts::PI * 300.0 * t).sin()
                } else {
                    0.0
                }
            })
            .collect();
        let orig = l.clone();
        let mut r = l.clone();
        h.process(&mut [&mut l, &mut r], n);
        assert_eq!(h.triggers, 1);
        let ms = |m: f32| 4410 + (m * sr / 1000.0) as usize;
        let diff = |a: usize, b: usize| l[a..b].iter().zip(&orig[a..b]).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(diff(0, ms(6.5)) < 1e-6, "zap window changed: {}", diff(0, ms(6.5)));
        assert!(diff(ms(20.0), ms(50.0)) > 0.1, "no change in the attack window");
        assert!(diff(ms(80.0), n) < 1e-6, "body changed: {}", diff(ms(80.0), n));
        // inside the window the 6 kHz part is up about 2.3 dB (shelf midpoint is at 5 kHz, bell takes a little off)
        let seg = &l[ms(20.0)..ms(50.0)];
        let hf: f32 = seg.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        let seg0 = &orig[ms(20.0)..ms(50.0)];
        let hf0: f32 = seg0.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        let d = 10.0 * (hf / hf0).log10();
        assert!(d > 1.8 && d < 2.8, "{d}");
    }

    #[test]
    fn off_is_bit_transparent() {
        let sr = 44100.0;
        let mut h = HitEq::new(sr);
        let mut l: Vec<f32> = (0..4096).map(|i| ((i * 7919) % 1000) as f32 / 500.0 - 1.0).collect();
        let orig = l.clone();
        let mut r = l.clone();
        h.process(&mut [&mut l, &mut r], 4096);
        assert_eq!(l, orig);
    }
}
