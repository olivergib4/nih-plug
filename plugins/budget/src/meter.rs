//! The measuring half of Budget. One `Meter` sits on one bus (drums, growls or sub) and is told when the
//! clap hits (from the sidechain, or from its own input on the drums). Around every clap it measures its
//! own bus in four windows and reports them in dBFS:
//!
//! * `during`: the level from the hit to 95 ms after it (on the drums this is "the clap level"),
//! * `before`: the last 40 ms before the hit,
//! * `after`: the loudest 30 ms between 95 and 500 ms after the hit,
//! * `rest`: the loudest 30 ms from 500 ms after the hit until 60 ms before the next one.
//!
//! Drums and growls are measured above 150 Hz, the sub over the full band (`Lows`: below 150 Hz, for a
//! bus that carries growls and sub together). The reported value of each
//! window is the loudest of the last four claps (the mean for `during`), so one odd bar does not hide.

pub const HOP_MS: f32 = 10.0;
pub const DURING_MS: f32 = 95.0;
pub const AFTER_END_MS: f32 = 500.0;
pub const BEFORE_MS: f32 = 60.0;
const RING: usize = 64;
const KEEP: usize = 4;
const FLOOR_DB: f32 = -120.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Drums,
    Growls,
    Sub,
    /// The part below 150 Hz of a bus that carries the growls and the sub together.
    Lows,
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    z1: f32,
    z2: f32,
}

impl Biquad {
    fn lowpass(sr: f32, hz: f32, q: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * hz / sr;
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        let a0 = 1.0 + al;
        Biquad { b0: (1.0 - c) / 2.0 / a0, b1: (1.0 - c) / a0, b2: (1.0 - c) / 2.0 / a0, a1: -2.0 * c / a0, a2: (1.0 - al) / a0, z1: 0.0, z2: 0.0 }
    }
    fn highpass(sr: f32, hz: f32, q: f32) -> Self {
        let w = 2.0 * std::f32::consts::PI * hz / sr;
        let (s, c) = w.sin_cos();
        let al = s / (2.0 * q);
        let a0 = 1.0 + al;
        Biquad { b0: (1.0 + c) / 2.0 / a0, b1: -(1.0 + c) / a0, b2: (1.0 + c) / 2.0 / a0, a1: -2.0 * c / a0, a2: (1.0 - al) / a0, z1: 0.0, z2: 0.0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.z1;
        self.z1 = self.b1 * x - self.a1 * y + self.z2;
        self.z2 = self.b2 * x - self.a2 * y;
        y
    }
}

/// 4th-order Butterworth high-pass.
#[derive(Clone, Copy, Default)]
struct Hp4(Biquad, Biquad);

impl Hp4 {
    fn new(sr: f32, hz: f32) -> Self {
        Hp4(Biquad::highpass(sr, hz, 0.541_196_1), Biquad::highpass(sr, hz, 1.306_563))
    }
    /// The same slopes as a low-pass.
    fn low(sr: f32, hz: f32) -> Self {
        Hp4(Biquad::lowpass(sr, hz, 0.541_196_1), Biquad::lowpass(sr, hz, 1.306_563))
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        self.1.run(self.0.run(x))
    }
}

fn to_db(power: f32) -> f32 {
    if power > 1e-12 {
        10.0 * power.log10()
    } else {
        FLOOR_DB
    }
}

/// The last few values of one window.
#[derive(Clone, Copy)]
struct Last {
    v: [f32; KEEP],
    n: usize,
}

impl Last {
    fn new() -> Self {
        Last { v: [FLOOR_DB; KEEP], n: 0 }
    }
    fn push(&mut self, x: f32) {
        self.v.rotate_right(1);
        self.v[0] = x;
        self.n = (self.n + 1).min(KEEP);
    }
    fn max(&self) -> Option<f32> {
        (self.n > 0).then(|| self.v[..self.n].iter().cloned().fold(FLOOR_DB, f32::max))
    }
    fn mean(&self) -> Option<f32> {
        (self.n > 0).then(|| self.v[..self.n].iter().sum::<f32>() / self.n as f32)
    }
}

/// What a meter currently knows, in dBFS. `None` until that window has been measured once.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Readings {
    pub during: Option<f32>,
    pub before: Option<f32>,
    pub after: Option<f32>,
    pub rest: Option<f32>,
}

pub struct Meter {
    sr: f32,
    hop: usize,
    pub role: Role,
    pub thr_db: f32,
    /// Gain (linear) that still follows on this bus after the meter: its fader, the drive of a later clip.
    pub gain_after: f32,
    /// A clip at full scale follows after that gain (the bus clip the growls and the sub run into), so the
    /// levels are read as they leave that clip and not as they enter it.
    pub clip_after: bool,
    band: [Hp4; 2],
    det: Hp4,
    env: f32,
    env_rel: f32,
    peak: f32,
    peak_decay: f32,
    /// Samples since the last trigger (saturating); `usize::MAX` = no clap yet.
    since: usize,
    retrig: usize,
    acc: f32,
    acc_n: usize,
    ring: [f32; RING],
    ring_n: usize,
    /// Hops finished since the last trigger.
    hops: usize,
    during_acc: f32,
    during_n: usize,
    during_done: bool,
    after_max: f32,
    rest_max: f32,
    rest_fifo: [f32; 8],
    rest_fifo_n: usize,
    during: Last,
    before: Last,
    after: Last,
    rest: Last,
    pub triggers: u32,
    /// Samples since the sidechain last carried signal.
    side_quiet: usize,
}

impl Meter {
    pub fn new(sr: f32, role: Role) -> Self {
        Meter {
            sr,
            hop: ((sr * HOP_MS / 1000.0).round() as usize).max(1),
            role,
            thr_db: 12.0,
            gain_after: 1.0,
            clip_after: false,
            band: if role == Role::Lows { [Hp4::low(sr, 150.0); 2] } else { [Hp4::new(sr, 150.0); 2] },
            det: Hp4::new(sr, 2000.0),
            env: 0.0,
            env_rel: (-1.0 / (0.005 * sr)).exp(),
            peak: 0.0,
            peak_decay: (-1.0 / (3.0 * sr)).exp(),
            since: usize::MAX,
            retrig: (0.3 * sr) as usize,
            acc: 0.0,
            acc_n: 0,
            ring: [0.0; RING],
            ring_n: 0,
            hops: 0,
            during_acc: 0.0,
            during_n: 0,
            during_done: true,
            after_max: 0.0,
            rest_max: 0.0,
            rest_fifo: [0.0; 8],
            rest_fifo_n: 0,
            during: Last::new(),
            before: Last::new(),
            after: Last::new(),
            rest: Last::new(),
            triggers: 0,
            side_quiet: usize::MAX / 2,
        }
    }

    pub fn reset(&mut self) {
        *self = Meter { thr_db: self.thr_db, gain_after: self.gain_after, clip_after: self.clip_after, ..Meter::new(self.sr, self.role) };
    }

    pub fn readings(&self) -> Readings {
        // nothing is current when the claps have stopped
        if self.since == usize::MAX || self.since > (4.0 * self.sr) as usize {
            return Readings::default();
        }
        Readings { during: self.during.mean(), before: self.before.max(), after: self.after.max(), rest: self.rest.max() }
    }

    /// Mean power of the last `n` finished hops.
    fn recent(&self, n: usize) -> f32 {
        let n = n.min(self.ring_n).max(1);
        self.ring[..n].iter().sum::<f32>() / n as f32
    }

    fn on_trigger(&mut self) {
        let had_cycle = self.since != usize::MAX && self.since <= (4.0 * self.sr) as usize;
        if had_cycle {
            if self.hops as f32 * HOP_MS > DURING_MS + 30.0 {
                self.after.push(to_db(self.after_max));
            }
            if self.rest_max > 0.0 {
                self.rest.push(to_db(self.rest_max));
            }
        }
        // the last four finished hops: 40 to 50 ms, so a duck that starts 60 ms ahead is fully inside
        if self.ring_n >= 4 {
            self.before.push(to_db(self.recent(4)));
        }
        self.since = 0;
        self.hops = 0;
        self.acc = 0.0;
        self.acc_n = 0;
        self.during_acc = 0.0;
        self.during_n = 0;
        self.during_done = false;
        self.after_max = 0.0;
        self.rest_max = 0.0;
        self.rest_fifo_n = 0;
        self.triggers = self.triggers.wrapping_add(1);
    }

    fn on_hop(&mut self, power: f32) {
        self.ring.rotate_right(1);
        self.ring[0] = power;
        self.ring_n = (self.ring_n + 1).min(RING);
        if self.since == usize::MAX {
            return;
        }
        self.hops += 1;
        let t = self.hops as f32 * HOP_MS; // end of this hop, from the trigger
        if t <= DURING_MS + 0.5 * HOP_MS {
            self.during_acc += power;
            self.during_n += 1;
        } else if !self.during_done {
            self.during_done = true;
            if self.during_n > 0 {
                self.during.push(to_db(self.during_acc / self.during_n as f32));
            }
        }
        let lvl30 = self.recent(3);
        if t >= DURING_MS + 30.0 && t <= AFTER_END_MS {
            self.after_max = self.after_max.max(lvl30);
        } else if t > AFTER_END_MS + 30.0 {
            // only count what turns out to lie more than 60 ms before the next clap
            let keep = (BEFORE_MS / HOP_MS) as usize;
            if self.rest_fifo_n == keep {
                self.rest_max = self.rest_max.max(self.rest_fifo[keep - 1]);
            }
            self.rest_fifo.rotate_right(1);
            self.rest_fifo[0] = lvl30;
            self.rest_fifo_n = (self.rest_fifo_n + 1).min(keep);
        }
    }

    /// `main`: this bus, `side`: the clap (may be absent). Nothing is changed in the audio.
    pub fn process(&mut self, main: [&[f32]; 2], side: Option<[&[f32]; 2]>) {
        let n = main[0].len();
        let rel = 10f32.powf(-self.thr_db / 20.0);
        for i in 0..n {
            let (l, r) = (main[0][i], main[1][i]);
            // the clap comes from the sidechain; the drums fall back to their own signal when it is silent
            let mut d = 0.0;
            if let Some(sc) = side {
                d = 0.5 * (sc[0][i] + sc[1][i]);
                if d.abs() > 1e-5 {
                    self.side_quiet = 0;
                } else {
                    self.side_quiet = self.side_quiet.saturating_add(1);
                }
            }
            let side_live = side.is_some() && self.side_quiet < (2.0 * self.sr) as usize;
            if !side_live {
                d = if self.role == Role::Drums { 0.5 * (l + r) } else { 0.0 };
            }
            let h = self.det.run(d).abs();
            self.env = h.max(self.env * self.env_rel);
            let before_peak = self.peak * self.peak_decay;
            self.peak = before_peak.max(self.env);
            if self.since != usize::MAX {
                self.since = self.since.saturating_add(1).min(usize::MAX - 1);
            }
            let free = self.since == usize::MAX || self.since >= self.retrig;
            if free && self.env > 1e-3 && self.env >= before_peak * rel && self.env >= self.peak * rel {
                self.on_trigger();
            }
            // the level as it will be after what still follows on this bus
            let (mut l, mut r) = (l * self.gain_after, r * self.gain_after);
            if self.clip_after {
                l = l.clamp(-1.0, 1.0);
                r = r.clamp(-1.0, 1.0);
            }
            let p = if self.role == Role::Sub {
                0.5 * (l * l + r * r)
            } else {
                let (a, b) = (self.band[0].run(l), self.band[1].run(r));
                0.5 * (a * a + b * b)
            };
            self.acc += p;
            self.acc_n += 1;
            if self.acc_n >= self.hop {
                let power = self.acc / self.acc_n as f32;
                self.acc = 0.0;
                self.acc_n = 0;
                self.on_hop(power);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const SR: f32 = 44100.0;
    fn ms(t: f32) -> usize {
        (t * SR / 1000.0) as usize
    }
    /// A bright 60 ms burst every `period_ms`, starting at `first_ms`.
    fn claps(n: usize, first_ms: f32, period_ms: f32, amp: f32) -> Vec<f32> {
        let mut x = vec![0.0; n];
        let mut seed = 1u32;
        let mut at = ms(first_ms);
        while at + ms(60.0) < n {
            for i in 0..ms(60.0) {
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let noise = (seed >> 9) as f32 / (1u32 << 22) as f32 - 1.0;
                x[at + i] = amp * noise * (-(i as f32) / ms(25.0) as f32).exp();
            }
            at += ms(period_ms);
        }
        x
    }
    fn tone(n: usize, hz: f32, gain: impl Fn(f32) -> f32) -> Vec<f32> {
        (0..n).map(|i| gain(i as f32 * 1000.0 / SR) * (2.0 * std::f32::consts::PI * hz * i as f32 / SR).sin()).collect()
    }
    fn run(role: Role, main: &[f32], side: Option<&[f32]>) -> Meter {
        let mut m = Meter::new(SR, role);
        for (k, c) in main.chunks(512).enumerate() {
            let s = side.map(|s| &s[k * 512..k * 512 + c.len()]);
            m.process([c, c], s.map(|s| [s, s]));
        }
        m
    }

    #[test]
    fn growls_are_read_in_the_four_windows_around_each_clap() {
        let n = ms(9000.0);
        let period = 1714.0;
        let side = claps(n, 500.0, period, 0.8);
        // a 1 kHz "growl": -40 dB from 60 ms before each clap to 250 ms after, -6 dB from 250 to 500 ms,
        // -12 dB for the rest of the bar
        let growl = tone(n, 1000.0, |t| {
            let ph = (t - 500.0).rem_euclid(period);
            if ph < 250.0 || ph > period - 60.0 {
                0.01
            } else if ph < 500.0 {
                0.5
            } else {
                0.25
            }
        });
        let m = run(Role::Growls, &growl, Some(&side));
        assert!(m.triggers >= 4, "{}", m.triggers);
        let r = m.readings();
        // a sine of amplitude a reads 20*log10(a) - 3 dB
        assert!((r.after.unwrap() - (-6.0 - 3.0)).abs() < 0.5, "after {:?}", r.after);
        assert!((r.rest.unwrap() - (-12.0 - 3.0)).abs() < 0.5, "rest {:?}", r.rest);
        assert!((r.before.unwrap() - (-40.0 - 3.0)).abs() < 1.0, "before {:?}", r.before);
        assert!((r.during.unwrap() - (-40.0 - 3.0)).abs() < 1.0, "during {:?}", r.during);
    }

    #[test]
    fn the_drums_find_the_clap_themselves_and_report_its_level() {
        let n = ms(8000.0);
        let x = claps(n, 400.0, 1714.0, 0.8);
        let m = run(Role::Drums, &x, None);
        assert!(m.triggers >= 4, "{}", m.triggers);
        let want = {
            let at = ms(400.0 + 1714.0 * 2.0);
            let seg = &x[at..at + ms(95.0)];
            10.0 * (seg.iter().map(|v| v * v).sum::<f32>() / seg.len() as f32).log10()
        };
        let got = m.readings().during.unwrap();
        assert!((got - want).abs() < 1.5, "{got:.1} vs {want:.1}");
    }

    #[test]
    fn readings_go_away_when_the_claps_stop_and_other_buses_do_not_trigger_themselves() {
        let n = ms(9000.0);
        let mut side = claps(n, 500.0, 1714.0, 0.8);
        for v in side[ms(3000.0)..].iter_mut() {
            *v = 0.0;
        }
        let growl = tone(n, 1000.0, |_| 0.3);
        let m = run(Role::Growls, &growl, Some(&side));
        assert_eq!(m.triggers, 2);
        assert_eq!(m.readings(), Readings::default());
        // a bright growl with no sidechain at all never counts as a clap
        let m = run(Role::Growls, &tone(n, 4000.0, |t| if (t as usize / 200) % 2 == 0 { 0.5 } else { 0.0 }), None);
        assert_eq!(m.triggers, 0);
    }

    #[test]
    fn a_fader_and_a_clip_after_the_meter_are_taken_into_account() {
        let n = ms(9000.0);
        let side = claps(n, 500.0, 1714.0, 0.8);
        // a 1 kHz tone 6 dB over full scale, then a -3 dB fader, then a clip at full scale: a square-ish wave
        let growl = tone(n, 1000.0, |_| 2.0);
        let mut m = Meter::new(SR, Role::Growls);
        m.gain_after = 10f32.powf(-3.0 / 20.0);
        m.clip_after = true;
        for (k, c) in growl.chunks(512).enumerate() {
            let s = &side[k * 512..k * 512 + c.len()];
            m.process([c, c], Some([s, s]));
        }
        let rest = m.readings().rest.unwrap();
        // a sine clipped 3 dB over full scale has an RMS between a sine's (-3 dB) and a square's (0 dB)
        assert!(rest > -2.0 && rest < 0.0, "{rest:.2}");
        // without the clip the same signal reads 6 - 3 - 3 = 0 dB
        let mut m = Meter::new(SR, Role::Growls);
        m.gain_after = 10f32.powf(-3.0 / 20.0);
        for (k, c) in growl.chunks(512).enumerate() {
            let s = &side[k * 512..k * 512 + c.len()];
            m.process([c, c], Some([s, s]));
        }
        assert!((m.readings().rest.unwrap() - 0.0).abs() < 0.3);
    }

    #[test]
    fn one_bus_with_growls_and_sub_is_split_at_150_hz() {
        let n = ms(9000.0);
        let side = claps(n, 500.0, 1714.0, 0.8);
        let g = tone(n, 1000.0, |_| 0.25);
        let sub = tone(n, 50.0, |_| 0.5);
        let bus: Vec<f32> = g.iter().zip(&sub).map(|(a, b)| a + b).collect();
        let hi = run(Role::Growls, &bus, Some(&side)).readings().rest.unwrap();
        let lo = run(Role::Lows, &bus, Some(&side)).readings().rest.unwrap();
        assert!((hi - (-12.0 - 3.0)).abs() < 0.3, "{hi:.2}");
        assert!((lo - (-6.0 - 3.0)).abs() < 0.3, "{lo:.2}");
    }
}
