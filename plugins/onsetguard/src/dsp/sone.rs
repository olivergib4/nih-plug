//! Perceived loudness in sones, Zwicker-style (ISO 532-1 shape, simplified for real time).
//!
//! Per analysis frame (2048-point FFT, hop 512, Hann):
//! 1. power spectrum → SPL per bin using the reference level (0 dBFS = `ref_spl` dB SPL)
//! 2. outer/middle-ear transfer function (a0, ISO 532-1 table, interpolated)
//! 3. excitation per critical band: power summed over 24 Bark bands (no upward spread; a
//!    simple 0.5-Bark-neighbour smear stands in for masking slopes)
//! 4. specific loudness N' = 0.08 (E_TQ/E_0)^0.23 [ (0.5 + 0.5 E/E_TQ)^0.23 - 1 ] sone/Bark,
//!    clamped at 0, with E_TQ the threshold in quiet per band
//! 5. total loudness = sum N' dz (dz = 1 Bark), smoothed with 60 ms attack / 300 ms release
//!
//! This is a sensation-level model, not a broadcast meter: it needs the playback level, so the
//! numbers are only comparable at one Reference Level setting. Phon = 40 + 10 log2(sone).

use realfft::num_complex::Complex32;
use realfft::{RealFftPlanner, RealToComplex};
use std::sync::Arc;

pub const WIN: usize = 2048;
pub const HOP: usize = 512;
const NB: usize = WIN / 2 + 1;
pub const BANDS: usize = 24;

/// ISO 532-1 Table: a0 (outer/middle ear transfer, dB) at band centre frequencies (Bark 0.5..23.5).
/// Values are the standard's a0 for the lower part (0 dB) rising at high frequencies.
const A0_DB: [f32; BANDS] = [
    0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, -0.5, -1.6, -3.2, -5.4, -5.6, -4.0, -1.5, 2.0, 5.0, 12.0,
    15.0, 15.0, 15.0, 15.0,
];
/// Low-frequency correction on top of a0: the equal-loudness contours (ISO 226) sit well above
/// the 1 kHz level in the first few Bark bands even at listening levels; this is the ~80 phon
/// contour minus 80, per band. Zwicker's full model gets this from its low-band treatment.
const LOW_CORR_DB: [f32; BANDS] = [
    -10.0, -5.0, -3.0, -2.0, -1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
    0.0, 0.0, 0.0,
];
/// Threshold in quiet per band (dB SPL), ISO 532-1 shape.
const LTQ_DB: [f32; BANDS] = [
    30.0, 18.0, 12.0, 8.0, 7.0, 6.0, 5.0, 4.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 3.0, 6.0, 8.0, 12.0, 15.0, 20.0,
    25.0, 30.0, 35.0,
];

#[inline]
pub fn hz_to_bark(f: f32) -> f32 {
    13.0 * (0.00076 * f).atan() + 3.5 * ((f / 7500.0) * (f / 7500.0)).atan()
}

pub struct SoneMeter {
    sample_rate: f32,
    num_channels: usize,
    r2c: Arc<dyn RealToComplex<f32>>,
    scratch: Vec<Complex32>,
    window: Vec<f32>,
    ring: Vec<Vec<f32>>,
    pos: usize,
    since: usize,
    time_buf: Vec<f32>,
    spec: Vec<Complex32>,
    /// Band index per bin.
    bin_band: Vec<usize>,
    band_power: [f32; BANDS],
    /// Smoothed specific loudness per band (sone/Bark).
    pub specific: [f32; BANDS],
    /// Smoothed total loudness in sones.
    pub sone: f32,
    /// 0 dBFS full-scale sine → this SPL.
    pub ref_spl: f32,
    /// Window power normalisation: a full-scale sine → 1.0 power.
    norm: f32,
    /// Bins outside [lo_bin, hi_bin) are ignored: a crude playback-system band limit for the
    /// translation readouts (phone, laptop).
    lo_bin: usize,
    hi_bin: usize,
}

impl SoneMeter {
    pub fn new(sample_rate: f32, num_channels: usize, ref_spl: f32) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(WIN);
        let scratch = vec![Complex32::default(); r2c.get_scratch_len()];
        let window: Vec<f32> = (0..WIN)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos())
            .collect();
        let wsum: f32 = window.iter().sum();
        let bin_band = (0..NB)
            .map(|k| {
                let f = k as f32 * sample_rate / WIN as f32;
                (hz_to_bark(f).floor() as usize).min(BANDS - 1)
            })
            .collect();
        SoneMeter {
            sample_rate,
            num_channels,
            r2c,
            scratch,
            window,
            ring: vec![vec![0.0; WIN]; num_channels],
            pos: 0,
            since: 0,
            time_buf: vec![0.0; WIN],
            spec: vec![Complex32::default(); NB],
            bin_band,
            band_power: [0.0; BANDS],
            specific: [0.0; BANDS],
            sone: 0.0,
            ref_spl,
            norm: (2.0 / wsum) * (2.0 / wsum) / 2.0, // |X|^2 scale so a full-scale sine → 0.5 (RMS^2)
            lo_bin: 1,
            hi_bin: NB,
        }
    }

    /// Restrict the analysis to `lo_hz..hi_hz` (a playback-system band limit).
    pub fn set_band(&mut self, lo_hz: f32, hi_hz: f32) {
        let per_bin = self.sample_rate / WIN as f32;
        self.lo_bin = ((lo_hz / per_bin).ceil() as usize).clamp(1, NB);
        self.hi_bin = ((hi_hz / per_bin).floor() as usize).clamp(self.lo_bin, NB);
    }

    pub fn reset(&mut self) {
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.pos = 0;
        self.since = 0;
        self.specific = [0.0; BANDS];
        self.sone = 0.0;
    }

    /// Push one multi-channel sample. Loudness is updated every HOP samples.
    #[inline]
    pub fn push(&mut self, samples: &[f32]) {
        let p = self.pos % WIN;
        for ch in 0..self.num_channels {
            self.ring[ch][p] = samples[ch];
        }
        self.pos += 1;
        self.since += 1;
        if self.since >= HOP {
            self.since = 0;
            self.analyze();
        }
    }

    fn analyze(&mut self) {
        self.band_power = [0.0; BANDS];
        let start = self.pos; // ring index (pos + i) % WIN == sample (pos - WIN + i)
        for ch in 0..self.num_channels {
            for i in 0..WIN {
                self.time_buf[i] = self.ring[ch][(start + i) % WIN] * self.window[i];
            }
            self.r2c
                .process_with_scratch(&mut self.time_buf, &mut self.spec, &mut self.scratch)
                .ok();
            for k in self.lo_bin..self.hi_bin {
                let p = self.spec[k].norm_sqr() * self.norm;
                self.band_power[self.bin_band[k]] += p;
            }
        }
        // average over channels (a stereo signal of two identical channels is one source)
        let inv_ch = 1.0 / self.num_channels as f32;
        let mut p = self.band_power;
        for v in &mut p {
            *v *= inv_ch;
        }
        let n_now = specific_loudness(&p, self.ref_spl);
        // smooth (60 ms attack / 300 ms release) and sum
        let frame_dt = HOP as f32 / self.sample_rate;
        let a_c = (-frame_dt / 0.06).exp();
        let r_c = (-frame_dt / 0.30).exp();
        let mut total = 0.0f32;
        for b in 0..BANDS {
            let n = n_now[b];
            let s = self.specific[b];
            self.specific[b] = if n > s { n + (s - n) * a_c } else { n + (s - n) * r_c };
            total += self.specific[b];
        }
        self.sone = total;
    }

    pub fn phon(&self) -> f32 {
        if self.sone <= 1e-4 {
            0.0
        } else {
            40.0 + 10.0 * self.sone.log2()
        }
    }
}

/// Specific loudness per Bark band (sone/Bark) from per-band power (full-scale sine = 0.5)
/// at a given reference level. Steps 2-4 of the module description.
pub fn specific_loudness(band_power: &[f32; BANDS], ref_spl: f32) -> [f32; BANDS] {
    // reference: power 0.5 (full-scale sine) ↔ ref_spl dB SPL
    let e0_db = ref_spl - 10.0 * 0.5f32.log10();
    let mut excitation_db = [0.0f32; BANDS];
    for b in 0..BANDS {
        let p = band_power[b];
        // a0 is the attenuation through the outer/middle ear (ISO 532-1 sign convention):
        // negative around 3-4 kHz (ear-canal resonance), positive above 10 kHz
        excitation_db[b] = 10.0 * p.max(1e-20).log10() + e0_db - A0_DB[b] + LOW_CORR_DB[b];
    }
    // Masking spread (Zwicker): each band's excitation leaks downward at 27 dB/Bark and
    // upward with a level-dependent slope, s = 24 + 230/fc - 0.2 L dB/Bark (flatter when
    // loud), which is what makes loudness grow faster than the power law at high levels.
    let mut spread_pow = [0.0f32; BANDS];
    for b in 0..BANDS {
        let l = excitation_db[b];
        if l < -20.0 {
            continue;
        }
        let fc = 100.0 * (b as f32 + 0.5); // rough band centre in Hz for the slope formula
        let up = (24.0 + 230.0 / fc.max(50.0) - 0.2 * l).clamp(5.0, 40.0);
        for j in 0..BANDS {
            let d = j as f32 - b as f32;
            let att = if d >= 0.0 { up * d } else { 27.0 * -d };
            spread_pow[j] += 10f32.powf((l - att) / 10.0);
        }
    }
    let mut out = [0.0f32; BANDS];
    for b in 0..BANDS {
        let e_db = 10.0 * spread_pow[b].max(1e-20).log10();
        let ltq = LTQ_DB[b];
        // Zwicker: N' = 0.08 (E_TQ/E_0)^0.23 [ (0.5 + 0.5 E/E_TQ)^0.23 - 1 ]
        let e_tq_ratio = 10f32.powf((e_db - ltq) / 10.0); // E / E_TQ
        let e_tq_e0 = 10f32.powf(ltq / 10.0); // E_TQ / E_0 (E_0 = 0 dB SPL)
        let n = 0.08 * e_tq_e0.powf(0.23) * ((0.5 + 0.5 * e_tq_ratio).powf(0.23) - 1.0);
        out[b] = n.max(0.0);
    }
    out
}
