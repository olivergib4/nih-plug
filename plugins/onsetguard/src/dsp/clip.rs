//! Oversampled soft clipper (zero-stuff + linear-phase FIR, clip, FIR + decimate).
//! Latency is a constant 16 samples for every oversampling factor.

const HALF: usize = 16; // FIR half-length in input samples → delay = HALF samples
pub const LATENCY: usize = HALF;

pub struct Oversampler {
    num_channels: usize,
    os: usize,
    taps: usize,
    coeffs: Vec<f32>,
    /// Per-channel upsample FIR history (length taps), at the oversampled rate.
    up_hist: Vec<Vec<f32>>,
    up_pos: Vec<usize>,
    down_hist: Vec<Vec<f32>>,
    down_pos: Vec<usize>,
    /// Plain delay for os == 1 so latency stays constant.
    delay: Vec<Vec<f32>>,
    delay_pos: usize,
    /// Per-channel upsampled sample for the current phase.
    stage: Vec<f32>,
}

fn blackman_sinc(taps: usize, fc_norm: f32) -> Vec<f32> {
    let m = (taps - 1) as f32 / 2.0;
    let mut h = vec![0.0f32; taps];
    let mut sum = 0.0;
    for i in 0..taps {
        let x = i as f32 - m;
        let s = if x == 0.0 {
            2.0 * fc_norm
        } else {
            (2.0 * std::f32::consts::PI * fc_norm * x).sin() / (std::f32::consts::PI * x)
        };
        let n = taps as f32 - 1.0;
        let w = 0.42 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n).cos()
            + 0.08 * (4.0 * std::f32::consts::PI * i as f32 / n).cos();
        h[i] = s * w;
        sum += h[i];
    }
    for v in &mut h {
        *v /= sum;
    }
    h
}

impl Oversampler {
    pub fn new(num_channels: usize, max_os: usize) -> Self {
        let max_taps = 2 * HALF * max_os + 1;
        let mut o = Oversampler {
            num_channels,
            os: 1,
            taps: 1,
            coeffs: vec![0.0; max_taps],
            up_hist: vec![vec![0.0; max_taps]; num_channels],
            up_pos: vec![0; num_channels],
            down_hist: vec![vec![0.0; max_taps]; num_channels],
            down_pos: vec![0; num_channels],
            delay: vec![vec![0.0; HALF]; num_channels],
            delay_pos: 0,
            stage: vec![0.0; num_channels],
        };
        o.set_os(max_os);
        o
    }

    pub fn os(&self) -> usize {
        self.os
    }

    pub fn set_os(&mut self, os: usize) {
        let os = os.max(1);
        if os == self.os && self.taps > 1 {
            return;
        }
        self.os = os;
        self.taps = 2 * HALF * os + 1;
        if os > 1 {
            let h = blackman_sinc(self.taps, 0.5 / os as f32 * 0.9);
            self.coeffs[..self.taps].copy_from_slice(&h);
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        for h in &mut self.up_hist {
            h.fill(0.0);
        }
        for h in &mut self.down_hist {
            h.fill(0.0);
        }
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.up_pos.fill(0);
        self.down_pos.fill(0);
        self.delay_pos = 0;
    }

    /// Clip in place. `ceil` linear, `hard` 0..1 (knee start as a fraction of the ceiling),
    /// `link` 0..1: at 1 both channels get the same gain, driven by the louder one, so the
    /// stereo image is untouched; at 0 each channel clips on its own.
    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize, ceil: f32, hard: f32, link: f32) {
        let ceil = ceil.max(1e-4);
        let nc = self.num_channels;
        let link = if nc > 1 { link.clamp(0.0, 1.0) } else { 0.0 };
        if self.os == 1 {
            for i in 0..len {
                let mut peak = 0.0f32;
                for ch in 0..nc {
                    let x = channels[ch][i];
                    let d = &mut self.delay[ch];
                    let y = d[self.delay_pos];
                    d[self.delay_pos] = x;
                    channels[ch][i] = y;
                    peak = peak.max(y.abs());
                }
                let g_link = clip_gain(peak, ceil, hard);
                for ch in 0..nc {
                    let y = channels[ch][i];
                    let g = clip_gain(y.abs(), ceil, hard);
                    channels[ch][i] = y * (g + (g_link - g) * link);
                }
                self.delay_pos = (self.delay_pos + 1) % HALF;
            }
            return;
        }
        let os = self.os;
        let taps = self.taps;
        for i in 0..len {
            for p in 0..os {
                // upsample every channel for this phase
                let mut peak = 0.0f32;
                for ch in 0..nc {
                    let uh = &mut self.up_hist[ch];
                    let up = self.up_pos[ch];
                    uh[up] = if p == 0 { channels[ch][i] * os as f32 } else { 0.0 };
                    let mut acc = 0.0f32;
                    let mut idx = up;
                    for t in 0..taps {
                        acc += uh[idx] * self.coeffs[t];
                        idx = if idx == 0 { taps - 1 } else { idx - 1 };
                    }
                    self.up_pos[ch] = (up + 1) % taps;
                    self.stage[ch] = acc;
                    peak = peak.max(acc.abs());
                }
                let g_link = clip_gain(peak, ceil, hard);
                // clip at the oversampled rate, then push into the downsample FIR
                for ch in 0..nc {
                    let a = self.stage[ch];
                    let g = clip_gain(a.abs(), ceil, hard);
                    let c = a * (g + (g_link - g) * link);
                    let dh = &mut self.down_hist[ch];
                    let dp = self.down_pos[ch];
                    dh[dp] = c;
                    if p == os - 1 {
                        let mut acc2 = 0.0f32;
                        let mut idx2 = dp;
                        for t in 0..taps {
                            acc2 += dh[idx2] * self.coeffs[t];
                            idx2 = if idx2 == 0 { taps - 1 } else { idx2 - 1 };
                        }
                        // guarantee the ceiling after decimation (filter overshoot)
                        channels[ch][i] = acc2.clamp(-ceil, ceil);
                    }
                    self.down_pos[ch] = (dp + 1) % taps;
                }
            }
        }
    }
}

/// Gain (<= 1) that the soft clipper applies to a sample of magnitude `a`.
#[inline]
pub fn clip_gain(a: f32, ceil: f32, hard: f32) -> f32 {
    if a <= 1e-9 {
        return 1.0;
    }
    ceil * soft_clip(a / ceil, hard) / a
}

#[inline]
pub fn soft_clip(y: f32, hard: f32) -> f32 {
    let a = y.abs();
    let hard = hard.clamp(0.0, 0.999);
    if a <= hard {
        return y;
    }
    let r = (1.0 - hard).max(1e-6);
    y.signum() * (hard + r * ((a - hard) / r).tanh())
}
