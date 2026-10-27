//! Real-time zero-phase STFT stage: sqrt-Hann analysis and synthesis windows, hop = N/4.
//! All channels of one frame are handed to the callback together, so cross-channel work
//! (mono-below, stereo linking) is possible. Latency is exactly `window_size` samples.
//! Nothing allocates after `new()` as long as `window_size <= max_window_size`.

use realfft::num_complex::Complex32;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub struct Plan {
    pub r2c: Arc<dyn RealToComplex<f32>>,
    pub c2r: Arc<dyn ComplexToReal<f32>>,
}

pub struct StftStage {
    num_channels: usize,
    max_window: usize,
    window_size: usize,
    hop: usize,
    /// Analysis/synthesis window (sqrt-Hann) for the current size.
    window: Vec<f32>,
    /// Overlap-add normalization (1 / sum of window^2 across hops).
    ola_norm: f32,
    /// Plans indexed by `order - min_order`.
    plans: Vec<Plan>,
    min_order: usize,

    /// Per-channel input ring buffers, length `max_window`.
    in_ring: Vec<Vec<f32>>,
    /// Per-channel output accumulation ring buffers, length `2 * max_window`.
    out_ring: Vec<Vec<f32>>,
    /// Write position shared by all channels; counts samples since start.
    pos: usize,
    /// Samples accumulated since the last frame.
    since_frame: usize,

    /// Scratch: per-channel time-domain frame and spectra.
    time_scratch: Vec<Vec<f32>>,
    spectra: Vec<Vec<Complex32>>,
    fft_scratch: Vec<Complex32>,
}

impl StftStage {
    pub fn new(num_channels: usize, min_order: usize, max_order: usize) -> Self {
        let max_window = 1 << max_order;
        let mut planner = RealFftPlanner::<f32>::new();
        let plans = (min_order..=max_order)
            .map(|o| Plan {
                r2c: planner.plan_fft_forward(1 << o),
                c2r: planner.plan_fft_inverse(1 << o),
            })
            .collect::<Vec<_>>();
        let scratch_len = plans
            .iter()
            .map(|p| p.r2c.get_scratch_len().max(p.c2r.get_scratch_len()))
            .max()
            .unwrap_or(0);

        let mut s = StftStage {
            num_channels,
            max_window,
            window_size: 0,
            hop: 0,
            window: vec![0.0; max_window],
            ola_norm: 1.0,
            plans,
            min_order,
            in_ring: vec![vec![0.0; max_window]; num_channels],
            out_ring: vec![vec![0.0; 2 * max_window]; num_channels],
            pos: 0,
            since_frame: 0,
            time_scratch: vec![vec![0.0; max_window]; num_channels],
            spectra: vec![vec![Complex32::default(); max_window / 2 + 1]; num_channels],
            fft_scratch: vec![Complex32::default(); scratch_len],
        };
        s.set_window_size(1 << min_order);
        s
    }

    pub fn num_channels(&self) -> usize {
        self.num_channels
    }

    pub fn window_size(&self) -> usize {
        self.window_size
    }

    pub fn num_bins(&self) -> usize {
        self.window_size / 2 + 1
    }

    /// Latency in samples.
    pub fn latency(&self) -> usize {
        self.window_size
    }

    /// Change the window size (must be a power of two within the planned range). Resets state.
    pub fn set_window_size(&mut self, n: usize) {
        assert!(n <= self.max_window && n.is_power_of_two());
        self.window_size = n;
        self.hop = n / 4;
        for (i, w) in self.window[..n].iter_mut().enumerate() {
            let hann = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            *w = hann.sqrt();
        }
        let mut norm = 0.0f32;
        let mut i = 0;
        while i < n {
            norm += self.window[i] * self.window[i];
            i += self.hop;
        }
        self.ola_norm = 1.0 / norm;
        self.reset();
    }

    pub fn reset(&mut self) {
        for r in &mut self.in_ring {
            r.fill(0.0);
        }
        for r in &mut self.out_ring {
            r.fill(0.0);
        }
        self.pos = 0;
        self.since_frame = 0;
    }

    /// Process `len` samples in place for every channel. `frame_fn` is called once per frame
    /// with all channels' spectra (`[channel][bin]`) and the frequency-domain magnitude scale is
    /// such that a full-scale sine reads ~1.0 at its bin after multiplying by `mag_scale`.
    /// Note: `frame_fn` may be called zero or several times per call.
    pub fn process<F>(&mut self, channels: &mut [&mut [f32]], len: usize, mut frame_fn: F)
    where
        F: FnMut(&mut [Vec<Complex32>], usize, f32),
    {
        debug_assert_eq!(channels.len(), self.num_channels);
        let n = self.window_size;
        let out_len = 2 * self.max_window;
        for i in 0..len {
            let wp = self.pos % n;
            // write input, read output (delayed by N), clear slot for future accumulation
            for ch in 0..self.num_channels {
                let x = channels[ch][i];
                self.in_ring[ch][wp] = x;
                let rp = self.pos % out_len;
                channels[ch][i] = self.out_ring[ch][rp];
                self.out_ring[ch][rp] = 0.0;
            }
            self.pos += 1;
            self.since_frame += 1;
            if self.since_frame == self.hop {
                self.since_frame = 0;
                self.run_frame(&mut frame_fn);
            }
        }
    }

    fn run_frame<F>(&mut self, frame_fn: &mut F)
    where
        F: FnMut(&mut [Vec<Complex32>], usize, f32),
    {
        let n = self.window_size;
        let nb = n / 2 + 1;
        // The frame covers the last N input samples: ring positions (pos - N .. pos)
        let start = self.pos; // == end of frame in absolute samples; ring index (pos - N + i) % N == (pos + i) % N
        for ch in 0..self.num_channels {
            let ring = &self.in_ring[ch];
            let ts = &mut self.time_scratch[ch];
            for i in 0..n {
                ts[i] = ring[(start + i) % n] * self.window[i];
            }
        }
        // sqrt-Hann coherent gain is 2/pi, so a sine of amplitude A gives |bin| = A*n/pi.
        let mag_scale = std::f32::consts::PI / n as f32;
        {
            let plan = &self.plans[self.window_size.trailing_zeros() as usize - self.min_order];
            for ch in 0..self.num_channels {
                plan.r2c
                    .process_with_scratch(
                        &mut self.time_scratch[ch][..n],
                        &mut self.spectra[ch][..nb],
                        &mut self.fft_scratch,
                    )
                    .expect("forward FFT");
            }
        }
        frame_fn(&mut self.spectra, nb, mag_scale);
        {
            let plan = &self.plans[self.window_size.trailing_zeros() as usize - self.min_order];
            for ch in 0..self.num_channels {
                // realfft requires DC/Nyquist imaginary parts to be zero
                self.spectra[ch][0].im = 0.0;
                self.spectra[ch][nb - 1].im = 0.0;
                plan.c2r
                    .process_with_scratch(
                        &mut self.spectra[ch][..nb],
                        &mut self.time_scratch[ch][..n],
                        &mut self.fft_scratch,
                    )
                    .expect("inverse FFT");
            }
        }
        // Overlap-add into the output ring. The frame's first sample corresponds to absolute
        // input time (pos - N); output is read at absolute time t for input time t - N, so the
        // frame's output lands at ring positions (pos - N + N + i) = (pos + i).
        let out_len = 2 * self.max_window;
        let inv_n = 1.0 / n as f32; // realfft inverse is unnormalized
        let g = self.ola_norm * inv_n;
        for ch in 0..self.num_channels {
            let ts = &self.time_scratch[ch];
            let out = &mut self.out_ring[ch];
            for i in 0..n {
                out[(self.pos + i) % out_len] += ts[i] * self.window[i] * g;
            }
        }
    }
}
