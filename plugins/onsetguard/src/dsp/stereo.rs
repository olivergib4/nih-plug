//! Zero-phase stereo stage, applied per bin in the long STFT, to the processed and the
//! reference spectra alike (so onset protection never switches the stereo image).
//!
//! * Width: the Side signal above `width_hz` (one-octave crossfade) is scaled by `width_db`.
//!   The mono sum is the Mid, which is untouched, so any width setting is mono-compatible by
//!   construction.
//! * Anti-phase guard: per bin, the (time-smoothed) normalized correlation
//!   rho = 2 Re(L R*) / (|L|^2 + |R|^2) is measured on the processed spectrum; where it is
//!   below the correlation floor (anti-phase when the floor is 0) the Side is scaled down by
//!   up to `guard`, with the gain
//!   followed slowly (200 ms) and smoothed across 1/3 octave so it never rings. Anti-phase
//!   content disappears on a mono sum and eats clipper headroom for nothing. The narrowing is
//!   energy-preserving: what the Side loses the Mid gains (capped at +6 dB), so the stereo
//!   loudness holds and the mono sum rises by the recovered energy.

use realfft::num_complex::Complex32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoSettings {
    /// Side gain applied above `width_hz`, in dB (0 = unchanged).
    pub width_db: f32,
    pub width_hz: f32,
    /// 0..1: how much of the anti-phase Side is removed.
    pub guard: f32,
    /// Correlation floor (0..1): Side is also pulled down where the L/R correlation is below
    /// this (weakly correlated "fake wide" content that sums 3 dB down in mono). 0 = anti-phase
    /// only.
    pub floor: f32,
}

impl Default for StereoSettings {
    fn default() -> Self {
        StereoSettings { width_db: 0.0, width_hz: 300.0, guard: 0.0, floor: 0.0 }
    }
}

/// Correlation time constant: the image is steady, hits are not, so follow slowly.
const RHO_TAU_S: f32 = 0.2;
/// The guard gain is smoothed across this many octaves (energy-weighted), so it stays a
/// short filter and cannot ring before hits.
const GUARD_SMOOTH_OCT: f32 = 1.0 / 3.0;

pub struct StereoStage {
    /// Smoothed per-bin correlation of the processed spectrum (-1..1).
    rho: Vec<f32>,
    /// Scratch: raw guard gain, bin energy, prefix sums of energy*gain and energy.
    gain: Vec<f32>,
    energy: Vec<f32>,
    pre_wg: Vec<f64>,
    pre_w: Vec<f64>,
    /// Mid gain that puts the removed Side energy back into the Mid (energy-preserving
    /// narrowing), from frequency-smoothed |M|^2 and |S|^2.
    mid_gain: Vec<f32>,
    pre_em: Vec<f64>,
    pre_es: Vec<f64>,
}

impl StereoStage {
    pub fn new(max_bins: usize) -> Self {
        StereoStage {
            rho: vec![1.0; max_bins],
            gain: vec![1.0; max_bins],
            energy: vec![0.0; max_bins],
            pre_wg: vec![0.0; max_bins + 1],
            pre_w: vec![0.0; max_bins + 1],
            mid_gain: vec![1.0; max_bins],
            pre_em: vec![0.0; max_bins + 1],
            pre_es: vec![0.0; max_bins + 1],
        }
    }

    pub fn reset(&mut self) {
        self.rho.fill(1.0);
        self.gain.fill(1.0);
        self.mid_gain.fill(1.0);
    }

    pub fn active(s: &StereoSettings) -> bool {
        s.width_db.abs() > 0.01 || s.guard > 0.001
    }

    /// `proc` and `refr` are [L, R] spectra; `frame_dt` the hop time in seconds.
    pub fn process_frame(
        &mut self,
        proc: &mut [Vec<Complex32>],
        refr: &mut [Vec<Complex32>],
        nb: usize,
        sr: f32,
        frame_dt: f32,
        s: &StereoSettings,
    ) {
        if proc.len() != 2 || refr.len() != 2 {
            return;
        }
        let n = (nb - 1) * 2;
        let g_w = 10f32.powf(s.width_db / 20.0);
        let width_hz = s.width_hz.max(20.0);
        let guard = s.guard.clamp(0.0, 1.0);
        let floor = s.floor.clamp(0.0, 0.95);
        let alpha = (frame_dt / RHO_TAU_S).min(1.0);
        let (pl, pr) = proc.split_at_mut(1);
        let (rl, rr) = refr.split_at_mut(1);
        let (pl, pr, rl, rr) = (&mut pl[0], &mut pr[0], &mut rl[0], &mut rr[0]);

        // --- guard: per-bin correlation (slow), raw gain, then energy-weighted smoothing
        // across frequency so the gain curve is a short filter ---
        if guard > 0.0 {
            for k in 0..nb {
                let l = pl[k];
                let r = pr[k];
                let e = l.norm_sqr() + r.norm_sqr();
                let rho_now = if e > 1e-18 { 2.0 * (l * r.conj()).re / e } else { self.rho[k] };
                let rho = self.rho[k] + (rho_now - self.rho[k]) * alpha;
                self.rho[k] = rho;
                // deficit below the floor, scaled so full anti-phase (rho = -1) = full guard
                let deficit = ((floor - rho) / (floor + 1.0)).clamp(0.0, 1.0);
                self.gain[k] = 1.0 - guard * deficit;
                self.energy[k] = e;
            }
            self.pre_wg[0] = 0.0;
            self.pre_w[0] = 0.0;
            for k in 0..nb {
                let w = self.energy[k] as f64 + 1e-12;
                self.pre_wg[k + 1] = self.pre_wg[k] + w * self.gain[k] as f64;
                self.pre_w[k + 1] = self.pre_w[k] + w;
            }
            // frequency-smoothed |M|^2 and |S|^2 for the energy-preserving mid gain
            self.pre_em[0] = 0.0;
            self.pre_es[0] = 0.0;
            for k in 0..nb {
                let m = (pl[k] + pr[k]) * 0.5;
                let sd = (pl[k] - pr[k]) * 0.5;
                self.pre_em[k + 1] = self.pre_em[k] + m.norm_sqr() as f64;
                self.pre_es[k + 1] = self.pre_es[k] + sd.norm_sqr() as f64;
            }
            let half = 2f32.powf(GUARD_SMOOTH_OCT / 2.0) - 1.0;
            for k in 0..nb {
                let hw = ((k as f32 * half) as usize).max(2);
                let lo = k.saturating_sub(hw);
                let hi = (k + hw + 1).min(nb);
                let wsum = self.pre_w[hi] - self.pre_w[lo];
                let g = if wsum > 0.0 { ((self.pre_wg[hi] - self.pre_wg[lo]) / wsum) as f32 } else { 1.0 };
                self.gain[k] = g;
                // what the Side loses, the Mid gains: |M'|^2 = |M|^2 + |S|^2 (1 - g^2), capped at
                // +6 dB so a near-empty Mid cannot explode
                let em = self.pre_em[hi] - self.pre_em[lo];
                let es = self.pre_es[hi] - self.pre_es[lo];
                self.mid_gain[k] = if em > 1e-18 && g < 0.999 {
                    (((em + es * (1.0 - (g * g) as f64)) / em).sqrt() as f32).min(2.0)
                } else {
                    1.0
                };
            }
        }

        for k in 0..nb {
            let f = (k as f32 * sr / n as f32).max(1.0);
            let o = (f / width_hz).log2();
            let w = if o <= -0.5 {
                0.0
            } else if o >= 0.5 {
                1.0
            } else {
                0.5 + 0.5 * (std::f32::consts::PI * o).sin()
            };
            let g_width = 1.0 + (g_w - 1.0) * w;
            let (g_guard, g_mid) = if guard > 0.0 { (self.gain[k], self.mid_gain[k]) } else { (1.0, 1.0) };
            let g_side = g_width * g_guard;
            if (g_side - 1.0).abs() < 1e-6 && (g_mid - 1.0).abs() < 1e-6 {
                continue;
            }
            for (a, b) in [(&mut *pl, &mut *pr), (&mut *rl, &mut *rr)] {
                let mid = (a[k] + b[k]) * 0.5 * g_mid;
                let side = (a[k] - b[k]) * 0.5 * g_side;
                a[k] = mid + side;
                b[k] = mid - side;
            }
        }
    }
}
