//! Live spectrum: mid channel, 4096-point Hann FFT every 1024 samples, read out on 1/6-octave
//! log-spaced points from 20 Hz to 20 kHz. Keeps a fast display curve (ballistics in dB) and a
//! long-term power average (since the last `clear`), which is what the reference is compared to.
//! Allocation-free after construction.

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const FFT: usize = 4096;
pub const HOP: usize = 1024;
/// 1/6-octave points from 20 Hz to 20 kHz (~10 octaves).
pub const NPTS: usize = 61;
pub const F_LO: f32 = 20.0;
pub const F_HI: f32 = 20000.0;
/// Display tilt: dB per octave around 1 kHz, so a pink-ish mix reads roughly flat.
pub const TILT_DB_OCT: f32 = 4.5;

pub fn point_freq(k: usize) -> f32 {
    F_LO * (F_HI / F_LO).powf(k as f32 / (NPTS - 1) as f32)
}

pub fn tilt_db(f: f32) -> f32 {
    TILT_DB_OCT * (f / 1000.0).log2()
}

pub struct Spectrum {
    sr: f32,
    fft: Arc<dyn RealToComplex<f32>>,
    window: Vec<f32>,
    ring: Vec<f32>,
    pos: usize,
    since: usize,
    tbuf: Vec<f32>,
    spec: Vec<Complex32>,
    scratch: Vec<Complex32>,
    /// Bin range [lo, hi) and weights per point.
    ranges: Vec<(usize, usize)>,
    norm: f32,
    /// Fast display curve, dBFS per point (untilted).
    pub fast_db: [f32; NPTS],
    sum_pow: [f64; NPTS],
    frames: u64,
}

impl Spectrum {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT);
        let window: Vec<f32> = (0..FFT).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT as f32).cos()).collect();
        let wsum: f32 = window.iter().sum();
        let nb = FFT / 2 + 1;
        let per_bin = sample_rate / FFT as f32;
        let half = 2f32.powf(1.0 / 12.0); // half of a 1/6 octave
        let ranges = (0..NPTS)
            .map(|k| {
                let f = point_freq(k);
                let lo = ((f / half / per_bin).floor() as usize).min(nb - 1);
                let hi = ((f * half / per_bin).ceil() as usize).clamp(lo + 1, nb);
                (lo, hi)
            })
            .collect();
        let scratch = vec![Complex32::default(); fft.get_scratch_len()];
        Spectrum {
            sr: sample_rate,
            fft,
            window,
            ring: vec![0.0; FFT],
            pos: 0,
            since: 0,
            tbuf: vec![0.0; FFT],
            spec: vec![Complex32::default(); nb],
            scratch,
            ranges,
            // full-scale sine → 0 dB in its band
            norm: 2.0 / wsum,
            fast_db: [-120.0; NPTS],
            sum_pow: [0.0; NPTS],
            frames: 0,
        }
    }

    pub fn sample_rate(&self) -> f32 {
        self.sr
    }

    pub fn clear(&mut self) {
        self.sum_pow = [0.0; NPTS];
        self.frames = 0;
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.since = 0;
        self.fast_db = [-120.0; NPTS];
        self.clear();
    }

    /// Push one mid sample; `average` adds the frame to the long-term average. Returns true when
    /// a new frame was computed.
    #[inline]
    pub fn push(&mut self, x: f32, average: bool) -> bool {
        self.ring[self.pos] = x;
        self.pos = (self.pos + 1) % FFT;
        self.since += 1;
        if self.since < HOP {
            return false;
        }
        self.since = 0;
        for i in 0..FFT {
            self.tbuf[i] = self.ring[(self.pos + i) % FFT] * self.window[i];
        }
        if self.fft.process_with_scratch(&mut self.tbuf, &mut self.spec, &mut self.scratch).is_err() {
            return false;
        }
        for k in 0..NPTS {
            let (lo, hi) = self.ranges[k];
            // band power: sum of bin powers (a sine reads its own level; noise reads band power)
            let mut p = 0.0f32;
            for b in lo..hi {
                let m = self.spec[b].norm() * self.norm;
                p += m * m;
            }
            // Hann main lobe spreads a sine over ~1.5 bins of power: normalise so a sine reads 0 dB
            let p = p / 1.5;
            if average {
                self.sum_pow[k] += p as f64;
            }
            let db = 10.0 * p.max(1e-14).log10();
            let cur = self.fast_db[k];
            // ballistics: quick rise, ~300 ms fall at 43 frames/s
            let c = if db > cur { 0.5 } else { 0.12 };
            self.fast_db[k] = cur + (db - cur) * c;
        }
        if average {
            self.frames += 1;
        }
        true
    }

    /// Long-term average since `clear`, dBFS per point (untilted). NaN before the first frame.
    pub fn average_db(&self, out: &mut [f32; NPTS]) {
        for k in 0..NPTS {
            out[k] = if self.frames == 0 { f32::NAN } else { (10.0 * (self.sum_pow[k] / self.frames as f64).max(1e-14).log10()) as f32 };
        }
    }

    pub fn frames(&self) -> u64 {
        self.frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sine_reads_its_level_in_its_band() {
        let sr = 44100.0;
        let mut s = Spectrum::new(sr);
        // a point frequency so the sine sits at a point centre
        let k = 40;
        let f = point_freq(k);
        for i in 0..(sr as usize) {
            s.push(0.5 * (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin(), true);
        }
        let mut avg = [0.0; NPTS];
        s.average_db(&mut avg);
        assert!((avg[k] + 6.0).abs() < 1.5, "{} dB at {f} Hz", avg[k]);
        assert!(avg[k - 6] < avg[k] - 30.0, "leak an octave down: {}", avg[k - 6]);
        assert!((s.fast_db[k] - avg[k]).abs() < 1.0);
    }
}
