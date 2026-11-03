//! Phase rotation for crest reduction (the broadcast "phase rotator"): a cascade of
//! first-order all-pass sections. The magnitude spectrum is untouched; only the phase relation
//! between harmonics changes, which turns one-sided (asymmetric) waveforms - snares, vocals,
//! many synths - into more symmetric ones with a lower peak at the same RMS. Lower peak at
//! the same loudness means more drive before the clipper bites.
//!
//! This is the one stage that changes phase on purpose, so it is off by default: with it on,
//! the processed signal no longer lines up sample-for-sample with the original outside the
//! plugin (inside, Mix and Matched Bypass still compare like with like, because it sits at the
//! input before every path).

pub const STAGES: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RotateSettings {
    pub on: bool,
    /// Corner frequency of the all-pass sections, Hz.
    pub freq_hz: f32,
    /// How much of the rotation a detected hit keeps (0 = the transient is handed out
    /// un-rotated by the onset protection, 1 = rotated like everything else).
    pub hits: f32,
}

impl Default for RotateSettings {
    fn default() -> Self {
        RotateSettings { on: false, freq_hz: 200.0, hits: 0.0 }
    }
}

pub struct PhaseRotator {
    sample_rate: f32,
    num_channels: usize,
    a: f32,
    cur_hz: f32,
    /// Per channel, per stage: previous input and output.
    x1: Vec<[f32; STAGES]>,
    y1: Vec<[f32; STAGES]>,
}

impl PhaseRotator {
    pub fn new(sample_rate: f32, num_channels: usize) -> Self {
        let mut r = PhaseRotator {
            sample_rate,
            num_channels,
            a: 0.0,
            cur_hz: 0.0,
            x1: vec![[0.0; STAGES]; num_channels],
            y1: vec![[0.0; STAGES]; num_channels],
        };
        r.set_freq(200.0);
        r
    }

    pub fn reset(&mut self) {
        for v in &mut self.x1 {
            v.fill(0.0);
        }
        for v in &mut self.y1 {
            v.fill(0.0);
        }
    }

    fn set_freq(&mut self, hz: f32) {
        let hz = hz.clamp(20.0, self.sample_rate * 0.45);
        if hz == self.cur_hz {
            return;
        }
        self.cur_hz = hz;
        let t = (std::f32::consts::PI * hz / self.sample_rate).tan();
        self.a = (t - 1.0) / (t + 1.0);
    }

    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize, s: &RotateSettings) {
        if !s.on {
            return;
        }
        self.set_freq(s.freq_hz);
        let a = self.a;
        for ch in 0..self.num_channels.min(channels.len()) {
            let x1 = &mut self.x1[ch];
            let y1 = &mut self.y1[ch];
            for i in 0..len {
                let mut x = channels[ch][i];
                for st in 0..STAGES {
                    // first-order all-pass: y = a*x + x1 - a*y1
                    let y = a * x + x1[st] - a * y1[st];
                    x1[st] = x;
                    y1[st] = y;
                    x = y;
                }
                channels[ch][i] = x;
            }
        }
    }
}

/// Phase response (radians) of the whole cascade at `hz`, for undoing the rotation in the
/// frequency domain: multiply a bin by exp(-j * phase) to un-rotate it.
pub fn cascade_phase(hz: f32, sample_rate: f32, freq_hz: f32) -> f32 {
    let fc = freq_hz.clamp(20.0, sample_rate * 0.45);
    let t = (std::f32::consts::PI * fc / sample_rate).tan();
    let a = (t - 1.0) / (t + 1.0);
    let w = 2.0 * std::f32::consts::PI * hz / sample_rate;
    // H(z) = (a + z^-1) / (1 + a z^-1)
    let (c, sn) = (w.cos(), w.sin());
    let num = realfft::num_complex::Complex32::new(a + c, -sn);
    let den = realfft::num_complex::Complex32::new(1.0 + a * c, -a * sn);
    (num / den).arg() * STAGES as f32
}
