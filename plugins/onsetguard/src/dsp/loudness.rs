//! K-weighted (BS.1770-style) running loudness, used for the loudness-matched bypass and meters.

#[derive(Clone, Copy)]
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
}

#[derive(Clone, Copy, Default)]
struct State {
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

fn high_shelf(sr: f32, f0: f32, gain_db: f32, q: f32) -> Biquad {
    let a = 10f32.powf(gain_db / 40.0);
    let w = 2.0 * std::f32::consts::PI * f0 / sr;
    let (s, c) = w.sin_cos();
    let al = s / (2.0 * q);
    let sa = 2.0 * a.sqrt() * al;
    let a0 = (a + 1.0) - (a - 1.0) * c + sa;
    Biquad {
        b0: (a * ((a + 1.0) + (a - 1.0) * c + sa)) / a0,
        b1: (-2.0 * a * ((a - 1.0) + (a + 1.0) * c)) / a0,
        b2: (a * ((a + 1.0) + (a - 1.0) * c - sa)) / a0,
        a1: (2.0 * ((a - 1.0) - (a + 1.0) * c)) / a0,
        a2: ((a + 1.0) - (a - 1.0) * c - sa) / a0,
    }
}

fn high_pass(sr: f32, f0: f32, q: f32) -> Biquad {
    let w = 2.0 * std::f32::consts::PI * f0 / sr;
    let (s, c) = w.sin_cos();
    let al = s / (2.0 * q);
    let a0 = 1.0 + al;
    Biquad {
        b0: ((1.0 + c) / 2.0) / a0,
        b1: (-(1.0 + c)) / a0,
        b2: ((1.0 + c) / 2.0) / a0,
        a1: (-2.0 * c) / a0,
        a2: (1.0 - al) / a0,
    }
}

#[inline]
fn run(b: &Biquad, s: &mut State, x: f32) -> f32 {
    let y = b.b0 * x + b.b1 * s.x1 + b.b2 * s.x2 - b.a1 * s.y1 - b.a2 * s.y2;
    s.x2 = s.x1;
    s.x1 = x;
    s.y2 = s.y1;
    s.y1 = y;
    y
}

/// Running K-weighted mean square over all channels with an exponential window.
pub struct KMeter {
    shelf: Biquad,
    hp: Biquad,
    st: Vec<[State; 2]>,
    coef: f32,
    pub mean_square: f32,
}

impl KMeter {
    pub fn new(sample_rate: f32, num_channels: usize, window_s: f32) -> Self {
        KMeter {
            shelf: high_shelf(sample_rate, 1681.97, 3.9998, 0.7072),
            hp: high_pass(sample_rate, 38.135, 0.5003),
            st: vec![[State::default(); 2]; num_channels],
            coef: (-1.0 / (window_s * sample_rate)).exp(),
            mean_square: 0.0,
        }
    }

    pub fn reset(&mut self) {
        for s in &mut self.st {
            *s = [State::default(); 2];
        }
        self.mean_square = 0.0;
    }

    #[inline]
    pub fn push_frame(&mut self, samples: &[f32]) {
        let mut sum = 0.0;
        for (ch, &x) in samples.iter().enumerate() {
            let s = &mut self.st[ch];
            let y0 = run(&self.shelf, &mut s[0], x);
            let y = run(&self.hp, &mut s[1], y0);
            sum += y * y;
        }
        self.mean_square = sum + (self.mean_square - sum) * self.coef;
    }

    pub fn lufs(&self) -> f32 {
        -0.691 + 10.0 * self.mean_square.max(1e-12).log10()
    }
}

/// Magnitude response of a biquad at frequency `f` (Hz) for sample rate `sr`, in dB.
fn biquad_mag_db(b: &Biquad, sr: f32, f: f32) -> f32 {
    let w = 2.0 * std::f32::consts::PI * f / sr;
    let (c1, s1) = (w.cos(), w.sin());
    let (c2, s2) = ((2.0 * w).cos(), (2.0 * w).sin());
    // H(z) = (b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2), z = e^{jw}
    let nr = b.b0 + b.b1 * c1 + b.b2 * c2;
    let ni = -(b.b1 * s1 + b.b2 * s2);
    let dr = 1.0 + b.a1 * c1 + b.a2 * c2;
    let di = -(b.a1 * s1 + b.a2 * s2);
    10.0 * ((nr * nr + ni * ni) / (dr * dr + di * di).max(1e-20)).max(1e-20).log10()
}

/// K-weighting (BS.1770) gain in dB at frequency `f` for sample rate `sr`.
pub fn k_weight_db(sr: f32, f: f32) -> f32 {
    let sh = high_shelf(sr, 1681.97, 3.9998, 0.7072);
    let hp = high_pass(sr, 38.135, 0.5003);
    biquad_mag_db(&sh, sr, f) + biquad_mag_db(&hp, sr, f)
}
