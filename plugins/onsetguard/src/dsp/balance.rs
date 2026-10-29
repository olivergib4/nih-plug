//! Loudness Balance: a slow, zero-phase per-band gain that nudges the long-term spectrum
//! toward equal specific loudness across Bark bands.
//!
//! Why that target: Zwicker loudness is the sum over bands of a compressive function of
//! excitation (N' ~ E^0.23). For a fixed total energy, a concave sum is maximised when the
//! terms are equal, so equal specific loudness across bands is the balance that reads loudest
//! at a given level. It is derived from the hearing model at Reference Level, not from a
//! reference track.
//!
//! The correction is measured on the long-term input spectrum (time constant `speed_s`),
//! limited to +-`range_db` per band, scaled by `strength`, offset so the long-term power is
//! unchanged (it reshapes, it does not turn up), and applied to the processed and reference
//! spectra alike (onset protection never switches tone). Bands below 50 Hz and above 16 kHz
//! are left alone.

use super::eq::PreEq;
use super::sone::{hz_to_bark, specific_loudness, BANDS};
use realfft::num_complex::Complex32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BalanceSettings {
    /// 0..1
    pub strength: f32,
    /// Averaging time constant in seconds.
    pub speed_s: f32,
    /// Maximum per-band correction in dB.
    pub range_db: f32,
}

impl Default for BalanceSettings {
    fn default() -> Self {
        BalanceSettings { strength: 0.0, speed_s: 3.0, range_db: 6.0 }
    }
}

/// Bands whose centre lies in 50 Hz .. 16 kHz take part (Bark ~0.5 .. ~22.5).
const FIRST_BAND: usize = 0;
const LAST_BAND: usize = 22; // inclusive
/// dB of excitation per decade of specific loudness (N' ~ E^0.23 → 10 / 0.23).
const DB_PER_DECADE: f32 = 10.0 / 0.23;

pub struct Balance {
    /// Long-term band power of the input (full-scale sine = 0.5), per Bark band.
    avg_power: [f32; BANDS],
    /// Current per-band correction in dB (already smoothed by the averaging).
    pub gain_db: [f32; BANDS],
    /// Per-bin Bark position (cached per window size).
    bin_bark: Vec<f32>,
    bin_band: Vec<usize>,
    cached_n: usize,
    /// Linear gain per bin for the current frame.
    bin_gain: Vec<f32>,
    /// 1 / |H_eq|^2 per bin, so the measurement is of the signal before the pre-EQ (the user's
    /// EQ then adds to the correction instead of being fought by it).
    eq_inv_pow: Vec<f32>,
    eq_version: u32,
}

impl Balance {
    pub fn new(max_bins: usize) -> Self {
        Balance {
            avg_power: [0.0; BANDS],
            gain_db: [0.0; BANDS],
            bin_bark: vec![0.0; max_bins],
            bin_band: vec![0; max_bins],
            cached_n: 0,
            bin_gain: vec![1.0; max_bins],
            eq_inv_pow: vec![1.0; max_bins],
            eq_version: u32::MAX,
        }
    }

    pub fn reset(&mut self) {
        self.avg_power = [0.0; BANDS];
        self.gain_db = [0.0; BANDS];
        self.bin_gain.fill(1.0);
    }

    fn cache_bins(&mut self, nb: usize, sr: f32) {
        let n = (nb - 1) * 2;
        if self.cached_n == n {
            return;
        }
        self.cached_n = n;
        for k in 0..nb {
            let f = k as f32 * sr / n as f32;
            let z = hz_to_bark(f);
            self.bin_bark[k] = z;
            self.bin_band[k] = (z.floor() as usize).min(BANDS - 1);
        }
        self.eq_version = u32::MAX; // force the EQ response cache to refresh for this size
    }

    fn cache_eq(&mut self, nb: usize, sr: f32, eq: &PreEq) {
        if self.eq_version == eq.version {
            return;
        }
        self.eq_version = eq.version;
        let n = (nb - 1) * 2;
        for k in 0..nb {
            let f = k as f32 * sr / n as f32;
            let m = eq.magnitude_at(f).max(1e-3);
            self.eq_inv_pow[k] = 1.0 / (m * m);
        }
    }

    /// Measure `input` (the spectrum before any gain; `mag` scales |X| to amplitude so that a
    /// full-scale sine has power 0.5) and apply the current correction to every spectrum in
    /// `targets`. `frame_dt` is the hop time.
    #[allow(clippy::too_many_arguments)]
    pub fn process_frame(
        &mut self,
        input: &[Vec<Complex32>],
        targets: &mut [&mut [Vec<Complex32>]],
        nb: usize,
        sr: f32,
        mag: f32,
        frame_dt: f32,
        ref_spl: f32,
        s: &BalanceSettings,
        eq: &PreEq,
    ) {
        self.cache_bins(nb, sr);
        self.cache_eq(nb, sr, eq);
        let nc = input.len().max(1);
        // --- measure: band power of this frame, averaged over channels ---
        let mut p = [0.0f32; BANDS];
        for ch in input {
            for k in 1..nb {
                p[self.bin_band[k]] += ch[k].norm_sqr() * mag * mag * 0.5 * self.eq_inv_pow[k];
            }
        }
        let alpha = (frame_dt / s.speed_s.max(0.1)).min(1.0);
        for b in 0..BANDS {
            let v = p[b] / nc as f32;
            self.avg_power[b] += (v - self.avg_power[b]) * alpha;
        }
        // --- correction toward equal specific loudness ---
        let strength = s.strength.clamp(0.0, 1.0);
        if strength <= 0.0 {
            // ease the gains back to flat
            for g in &mut self.gain_db {
                *g += (0.0 - *g) * alpha;
            }
        } else {
            let n = specific_loudness(&self.avg_power, ref_spl);
            let mut sum = 0.0f32;
            let mut count = 0usize;
            for b in FIRST_BAND..=LAST_BAND {
                if n[b] > 0.02 {
                    sum += n[b].ln();
                    count += 1;
                }
            }
            if count >= 6 {
                // log-mean target: the geometric mean of the active bands' specific loudness
                let target_ln = sum / count as f32;
                let range = s.range_db.max(0.0);
                let mut want = [0.0f32; BANDS];
                for b in FIRST_BAND..=LAST_BAND {
                    if n[b] > 0.02 {
                        let d = (target_ln - n[b].ln()) / std::f32::consts::LN_10 * DB_PER_DECADE;
                        want[b] = (d * strength).clamp(-range, range);
                    }
                }
                // energy-preserving: offset so the long-term power is unchanged, then clamp
                let (mut e_in, mut e_out) = (0.0f32, 0.0f32);
                for b in 0..BANDS {
                    e_in += self.avg_power[b];
                    e_out += self.avg_power[b] * 10f32.powf(want[b] / 10.0);
                }
                let offset = if e_out > 1e-20 { 10.0 * (e_in / e_out).log10() } else { 0.0 };
                for b in 0..BANDS {
                    want[b] = (want[b] + offset).clamp(-range, range);
                }
                for b in 0..BANDS {
                    self.gain_db[b] += (want[b] - self.gain_db[b]) * alpha;
                }
            } else {
                for g in &mut self.gain_db {
                    *g += (0.0 - *g) * alpha;
                }
            }
        }
        // --- per-bin gain: linear interpolation between band centres on the Bark scale ---
        let mut any = false;
        for b in 0..BANDS {
            if self.gain_db[b].abs() > 0.005 {
                any = true;
                break;
            }
        }
        if !any {
            return;
        }
        for k in 0..nb {
            let z = self.bin_bark[k] - 0.5; // band b is centred at Bark b + 0.5
            let g = if z <= 0.0 {
                self.gain_db[0]
            } else if z >= (BANDS - 1) as f32 {
                self.gain_db[BANDS - 1]
            } else {
                let b = z.floor() as usize;
                let t = z - b as f32;
                self.gain_db[b] * (1.0 - t) + self.gain_db[b + 1] * t
            };
            self.bin_gain[k] = 10f32.powf(g / 20.0);
        }
        for spectra in targets.iter_mut() {
            for ch in spectra.iter_mut() {
                for k in 0..nb {
                    ch[k] *= self.bin_gain[k];
                }
            }
        }
    }
}
