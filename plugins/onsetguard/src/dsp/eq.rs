//! Pre-EQ: four minimum-phase biquads (RBJ cookbook) at the plugin input, before both the dry
//! and the processed paths, so Mix and Matched Bypass compare like with like. Minimum phase
//! means no pre-ringing by construction; the shaped signal is what the detector and the
//! upward compressor see.
//!
//! Node 1 is a low shelf, nodes 2 and 3 are bells one octave wide, node 4 is a high shelf.

pub const NODES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EqSettings {
    pub freq_hz: [f32; NODES],
    pub gain_db: [f32; NODES],
}

impl Default for EqSettings {
    fn default() -> Self {
        EqSettings { freq_hz: [80.0, 400.0, 2500.0, 10000.0], gain_db: [0.0; NODES] }
    }
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

impl Biquad {
    fn identity() -> Self {
        Biquad { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 }
    }

    /// Magnitude response at normalized angular frequency `w` (radians/sample).
    fn magnitude(&self, w: f32) -> f32 {
        let (c1, s1) = (w.cos(), w.sin());
        let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im).max(1e-20)).sqrt()
    }
}

fn low_shelf(sr: f32, f: f32, gain_db: f32) -> Biquad {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let s = 1.0f32; // shelf slope
    let alpha = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
    let sq = 2.0 * a.sqrt() * alpha;
    let a0 = (a + 1.0) + (a - 1.0) * cw + sq;
    Biquad {
        b0: (a * ((a + 1.0) - (a - 1.0) * cw + sq)) / a0,
        b1: (2.0 * a * ((a - 1.0) - (a + 1.0) * cw)) / a0,
        b2: (a * ((a + 1.0) - (a - 1.0) * cw - sq)) / a0,
        a1: (-2.0 * ((a - 1.0) + (a + 1.0) * cw)) / a0,
        a2: ((a + 1.0) + (a - 1.0) * cw - sq) / a0,
    }
}

fn high_shelf(sr: f32, f: f32, gain_db: f32) -> Biquad {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    let s = 1.0f32;
    let alpha = sw / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).sqrt();
    let sq = 2.0 * a.sqrt() * alpha;
    let a0 = (a + 1.0) - (a - 1.0) * cw + sq;
    Biquad {
        b0: (a * ((a + 1.0) + (a - 1.0) * cw + sq)) / a0,
        b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * cw)) / a0,
        b2: (a * ((a + 1.0) + (a - 1.0) * cw - sq)) / a0,
        a1: (2.0 * ((a - 1.0) - (a + 1.0) * cw)) / a0,
        a2: ((a + 1.0) - (a - 1.0) * cw - sq) / a0,
    }
}

fn peak(sr: f32, f: f32, gain_db: f32, bw_oct: f32) -> Biquad {
    let a = 10f32.powf(gain_db / 40.0);
    let w0 = 2.0 * std::f32::consts::PI * f / sr;
    let (cw, sw) = (w0.cos(), w0.sin());
    // bandwidth in octaves -> alpha (RBJ)
    let alpha = sw * ((2f32.ln() / 2.0 * bw_oct * w0 / sw).sinh());
    let a0 = 1.0 + alpha / a;
    Biquad {
        b0: (1.0 + alpha * a) / a0,
        b1: (-2.0 * cw) / a0,
        b2: (1.0 - alpha * a) / a0,
        a1: (-2.0 * cw) / a0,
        a2: (1.0 - alpha / a) / a0,
    }
}

pub struct PreEq {
    sample_rate: f32,
    num_channels: usize,
    cur: EqSettings,
    coefs: [Biquad; NODES],
    /// Direct form II transposed state, per channel per node.
    z1: Vec<[f32; NODES]>,
    z2: Vec<[f32; NODES]>,
    /// Bumped whenever the coefficients change, so caches of the response can refresh.
    pub version: u32,
}

impl PreEq {
    pub fn new(sample_rate: f32, num_channels: usize) -> Self {
        let mut e = PreEq {
            sample_rate,
            num_channels,
            cur: EqSettings { freq_hz: [0.0; NODES], gain_db: [f32::NAN; NODES] },
            coefs: [Biquad::identity(); NODES],
            z1: vec![[0.0; NODES]; num_channels],
            z2: vec![[0.0; NODES]; num_channels],
            version: 0,
        };
        e.update(&EqSettings::default());
        e
    }

    pub fn reset(&mut self) {
        for z in &mut self.z1 {
            z.fill(0.0);
        }
        for z in &mut self.z2 {
            z.fill(0.0);
        }
    }

    pub fn active(&self) -> bool {
        self.cur.gain_db.iter().any(|g| g.abs() > 0.005)
    }

    pub fn update(&mut self, s: &EqSettings) {
        if *s == self.cur {
            return;
        }
        self.cur = *s;
        let sr = self.sample_rate;
        let nyq = sr * 0.49;
        for n in 0..NODES {
            let f = s.freq_hz[n].clamp(20.0, nyq);
            let g = s.gain_db[n].clamp(-24.0, 24.0);
            self.coefs[n] = if g.abs() < 0.005 {
                Biquad::identity()
            } else {
                match n {
                    0 => low_shelf(sr, f, g),
                    NODES_LAST => high_shelf(sr, f, g),
                    _ => peak(sr, f, g, 1.0),
                }
            };
        }
        self.version = self.version.wrapping_add(1);
    }

    /// Magnitude response (linear) at `hz`.
    pub fn magnitude_at(&self, hz: f32) -> f32 {
        let w = 2.0 * std::f32::consts::PI * hz / self.sample_rate;
        self.coefs.iter().map(|c| c.magnitude(w)).product()
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize) {
        if !self.active() {
            return;
        }
        for ch in 0..self.num_channels.min(channels.len()) {
            let z1 = &mut self.z1[ch];
            let z2 = &mut self.z2[ch];
            for i in 0..len {
                let mut x = channels[ch][i];
                for n in 0..NODES {
                    let c = &self.coefs[n];
                    let y = c.b0 * x + z1[n];
                    z1[n] = c.b1 * x - c.a1 * y + z2[n];
                    z2[n] = c.b2 * x - c.a2 * y;
                    x = y;
                }
                channels[ch][i] = x;
            }
        }
    }
}

const NODES_LAST: usize = NODES - 1;
