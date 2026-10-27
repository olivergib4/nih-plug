//! Spectral-flux onset detector on the mono sum, 512-point FFT, hop 128, with an adaptive
//! threshold. Emits onset times as absolute sample indices (in the detector's input timeline).
//! Detection latency: at most `WIN + HOP * LOOK` samples after the true onset.

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const WIN: usize = 512;
pub const HOP: usize = 128;
const NB: usize = WIN / 2 + 1;
const RING: usize = 2 * WIN;
/// Frames of lookahead used for the local peak test.
const LOOK: usize = 1;
/// Maximum detection latency in samples (used by the host-side latency budget).
pub const MAX_LATENCY: usize = WIN + HOP * (LOOK + 1);

pub struct OnsetDetector {
    sample_rate: f32,
    r2c: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<Complex32>,
    window: Vec<f32>,
    ring: Vec<f32>,
    pos: usize,
    since: usize,
    time_buf: Vec<f32>,
    spec: Vec<Complex32>,
    prev_db: Vec<f32>,
    cur_db: Vec<f32>,
    /// Flux history ring for the adaptive threshold (~150 ms each side) and peak test.
    flux: Vec<f32>,
    flux_len: usize,
    flux_pos: usize,
    frame_idx: usize,
    max_flux: f32,
    last_onset_frame: i64,
    min_gap_frames: usize,
    mean_half: usize,
}

impl OnsetDetector {
    pub fn new(sample_rate: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(WIN);
        let scratch = vec![Complex32::default(); r2c.get_scratch_len()];
        let window = (0..WIN)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos())
            .collect();
        let mean_half = ((0.15 * sample_rate / HOP as f32).round() as usize).max(2);
        let flux_len = (mean_half * 2 + 2).next_power_of_two();
        OnsetDetector {
            sample_rate,
            r2c,
            scratch,
            window,
            ring: vec![0.0; RING],
            pos: 0,
            since: 0,
            time_buf: vec![0.0; WIN],
            spec: vec![Complex32::default(); NB],
            prev_db: vec![-100.0; NB],
            cur_db: vec![-100.0; NB],
            flux: vec![0.0; flux_len],
            flux_len,
            flux_pos: 0,
            frame_idx: 0,
            max_flux: 0.0,
            last_onset_frame: -1_000_000,
            min_gap_frames: ((0.04 * sample_rate / HOP as f32).round() as usize).max(1),
            mean_half,
        }
    }

    pub fn reset(&mut self) {
        self.ring.fill(0.0);
        self.pos = 0;
        self.since = 0;
        self.prev_db.fill(-100.0);
        self.flux.fill(0.0);
        self.flux_pos = 0;
        self.frame_idx = 0;
        self.max_flux = 0.0;
        self.last_onset_frame = -1_000_000;
    }

    /// Feed one sample. Returns `Some(onset_time)` (absolute sample index, same timeline as the
    /// samples fed so far) when an onset is confirmed. `sens` in 0..1.
    #[inline]
    pub fn push(&mut self, x: f32, sens: f32) -> Option<usize> {
        self.ring[self.pos % RING] = x;
        self.pos += 1;
        self.since += 1;
        if self.since < HOP {
            return None;
        }
        self.since = 0;
        self.analyze_frame(sens)
    }

    fn analyze_frame(&mut self, sens: f32) -> Option<usize> {
        // frame covers absolute samples [pos - WIN, pos)
        let start = self.pos + RING - WIN;
        for i in 0..WIN {
            self.time_buf[i] = self.ring[(start + i) % RING] * self.window[i];
        }
        self.r2c
            .process_with_scratch(&mut self.time_buf, &mut self.spec, &mut self.scratch)
            .ok();
        let mut f = 0.0f32;
        for k in 1..NB {
            let m = self.spec[k].norm() * (4.0 / WIN as f32);
            let d = 20.0 * m.max(1e-9).log10();
            self.cur_db[k] = d;
            let diff = d - self.prev_db[k];
            if diff > 0.0 && d > -70.0 {
                f += diff * (0.5 + k as f32 / NB as f32);
            }
        }
        std::mem::swap(&mut self.prev_db, &mut self.cur_db);
        let f = f / NB as f32;
        self.max_flux = (self.max_flux * 0.9995).max(f);
        self.flux[self.flux_pos % self.flux_len] = f;
        self.flux_pos += 1;
        self.frame_idx += 1;

        // Evaluate the frame LOOK frames back as a candidate peak (needs one future frame).
        if self.frame_idx <= LOOK + 1 {
            return None;
        }
        let c = self.flux_pos - 1 - LOOK; // index into flux history of the candidate
        let get = |i: usize| self.flux[i % self.flux_len];
        let v = get(c);
        if v <= get(c - 1) || v < get(c + LOOK) {
            return None;
        }
        // local mean over the past window (causal; the future half isn't available in real time)
        let n = self.mean_half.min(c);
        let mut s = 0.0;
        for i in (c - n)..=c {
            s += get(i);
        }
        let mean = s / (n + 1) as f32;
        let k = 1.5 + (1.0 - sens.clamp(0.0, 1.0)) * 4.0;
        let cand_frame = (self.frame_idx - 1 - LOOK) as i64;
        if v > mean * k + 0.02 * self.max_flux
            && v > 0.08 * self.max_flux
            && cand_frame - self.last_onset_frame >= self.min_gap_frames as i64
        {
            self.last_onset_frame = cand_frame;
            // Candidate frame ended at absolute sample (pos - LOOK*HOP) and covers the WIN
            // samples before that; refine to the max 2 ms RMS rise inside it. The ring holds
            // the last 2*WIN samples so the whole frame is still available.
            let frame_end = self.pos - LOOK * HOP;
            let frame_start = frame_end.saturating_sub(WIN);
            let l = ((self.sample_rate * 0.002).round() as usize).max(4);
            let mut best = frame_end.saturating_sub(HOP);
            let mut best_rise = -1.0f32;
            let mut eprev = 0.0f32;
            let mut st = frame_start;
            while st + l <= frame_end {
                let mut e = 0.0f32;
                for q in 0..l {
                    let t = st + q;
                    if t + RING >= self.pos {
                        let x = self.ring[t % RING];
                        e += x * x;
                    }
                }
                let e = (e / l as f32).sqrt();
                let rise = e - eprev;
                if rise > best_rise {
                    best_rise = rise;
                    best = st;
                }
                eprev = e;
                st += l / 2;
            }
            return Some(best);
        }
        None
    }
}
