//! Oversampled soft clipper (zero-stuff + linear-phase FIR, clip, FIR + decimate).
//!
//! Latency is a constant `LATENCY` (= 2 * HALF) samples for every oversampling factor: the
//! upsample and the downsample FIR each delay by HALF input samples, and the decimated output
//! is taken on the input grid (phase 0), so the delay is an exact integer and the clipped
//! signal stays sample-aligned with everything that bypasses it. At 1x a plain delay line
//! matches it.
//!
//! The upsampler is polyphase (only the non-zero stuffed samples are multiplied), so a long
//! FIR is affordable: HALF = 64 gives a transition band of about 2 kHz at 48 kHz, i.e. flat
//! to ~22 kHz instead of rolling off from 17 kHz as a 16-tap half did. At 1x the clipper is
//! bit-identical to a plain sample-level hard clip of the same signal.

const HALF: usize = 64; // FIR half-length in input samples
pub const LATENCY: usize = 2 * HALF;
const HIST: usize = 2 * HALF + 1; // input-rate history for the polyphase upsampler

pub struct Oversampler {
    num_channels: usize,
    os: usize,
    taps: usize,
    coeffs: Vec<f32>,
    /// Per-channel input-rate history for the polyphase upsample FIR (length HIST).
    in_hist: Vec<Vec<f32>>,
    in_pos: Vec<usize>,
    /// Per-channel downsample FIR history (length taps), at the oversampled rate.
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
            in_hist: vec![vec![0.0; HIST]; num_channels],
            in_pos: vec![0; num_channels],
            down_hist: vec![vec![0.0; max_taps]; num_channels],
            down_pos: vec![0; num_channels],
            delay: vec![vec![0.0; LATENCY]; num_channels],
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
            // cutoff at 0.96 x the input Nyquist: with HALF = 64 the Blackman transition is
            // ~2 kHz wide at 48 kHz, so the passband is flat to ~22 kHz and the stopband
            // starts at the input Nyquist
            let h = blackman_sinc(self.taps, 0.5 / os as f32 * 0.96);
            self.coeffs[..self.taps].copy_from_slice(&h);
        }
        self.reset();
    }

    pub fn reset(&mut self) {
        for h in &mut self.in_hist {
            h.fill(0.0);
        }
        for h in &mut self.down_hist {
            h.fill(0.0);
        }
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.in_pos.fill(0);
        self.down_pos.fill(0);
        self.delay_pos = 0;
    }

    /// Clip in place. `ceil` linear, `hard` 0..1 (knee start as a fraction of the ceiling),
    /// `link` 0..1: at 1 both channels get the same gain, driven by the louder one, so the
    /// stereo image is untouched; at 0 each channel clips on its own.
    pub fn process(&mut self, channels: &mut [&mut [f32]], len: usize, ceil: f32, hard: f32, link: f32) {
        self.process_dyn(channels, len, ceil, None, hard, link);
    }

    /// Like [`process`](Self::process) but with an optional per-sample ceiling (`ceil_t[i]`,
    /// linear, at the input rate) that overrides `ceil`. Used to reserve headroom for a band that
    /// is summed back in later.
    pub fn process_dyn(&mut self, channels: &mut [&mut [f32]], len: usize, ceil: f32, ceil_t: Option<&[f32]>, hard: f32, link: f32) {
        let ceil_fixed = ceil.max(1e-4);
        let nc = self.num_channels;
        let link = if nc > 1 { link.clamp(0.0, 1.0) } else { 0.0 };
        if self.os == 1 {
            for i in 0..len {
                let ceil = ceil_t.map_or(ceil_fixed, |c| c[i].max(1e-4));
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
                self.delay_pos = (self.delay_pos + 1) % LATENCY;
            }
            return;
        }
        let os = self.os;
        let taps = self.taps;
        let gain = os as f32;
        for i in 0..len {
            let ceil = ceil_t.map_or(ceil_fixed, |c| c[i].max(1e-4));
            // push the new input sample into every channel's input-rate history
            for ch in 0..nc {
                let ip = self.in_pos[ch];
                self.in_hist[ch][ip] = channels[ch][i];
            }
            for p in 0..os {
                // polyphase upsample: y[i*os + p] = sum_m h[p + m*os] * x[i - m]
                let mut peak = 0.0f32;
                for ch in 0..nc {
                    let ih = &self.in_hist[ch];
                    let mut idx = self.in_pos[ch];
                    let mut acc = 0.0f32;
                    let mut t = p;
                    while t < taps {
                        acc += ih[idx] * self.coeffs[t];
                        idx = if idx == 0 { HIST - 1 } else { idx - 1 };
                        t += os;
                    }
                    let a = acc * gain;
                    self.stage[ch] = a;
                    peak = peak.max(a.abs());
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
                    // decimate at phase 0: the output grid then coincides with the input grid
                    // and the two FIR delays add to an exact 2 * HALF input samples
                    if p == 0 {
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
            for ch in 0..nc {
                self.in_pos[ch] = (self.in_pos[ch] + 1) % HIST;
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
