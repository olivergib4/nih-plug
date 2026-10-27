//! Per-bin spectral dynamics for one STFT resolution, restricted to one frequency band
//! (low or high, crossfaded over one octave around the crossover).

use realfft::num_complex::Complex32;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Band {
    Low,
    High,
}

/// Frequency weighting applied to the detector (what the compressor "hears"), not the audio.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Weighting {
    /// Flat: every bin counts by its raw magnitude.
    Off,
    /// BS.1770 K-weighting: +4 dB shelf above ~1.7 kHz, high-pass below ~40 Hz.
    K,
    /// Equal-loudness (ISO 226 shape at ~80 phon): the detector sees roughly what the ear
    /// weighs, so lows are controlled harder and presence is left alone.
    EqualLoudness,
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicsSettings {
    pub weighting: Weighting,
    pub threshold_db: f32,
    pub center_hz: f32,
    pub slope_db_oct: f32,
    pub curve_db_oct2: f32,
    pub down_ratio: f32,
    pub up_ratio: f32,
    pub up_range_db: f32,
    pub attack_ms: f32,
    pub release_ms: f32,
    pub smooth_oct: f32,
    pub stereo_link: f32,
    pub crossover_hz: f32,
    pub mono_below_hz: f32,
}

pub struct Dynamics {
    band: Band,
    num_channels: usize,
    sample_rate: f32,
    window_size: usize,
    num_bins: usize,
    /// Cached settings used to build the per-bin tables.
    cached: Option<DynamicsSettings>,

    thresholds_db: Vec<f32>,
    band_weight: Vec<f32>,
    /// Detector weight per bin (linear), from `weighting`.
    det_weight: Vec<f32>,
    mono_weight: Vec<f32>,
    envelopes: Vec<Vec<f32>>, // [channel][bin], linear magnitude
    gain_db: Vec<f32>,
    gain_smooth: Vec<f32>,
    /// For the analyzer: channel-averaged weighted envelope (dB) and applied gain (dB) per bin.
    vis_env_db: Vec<f32>,
    vis_gain_db: Vec<f32>,
    prefix: Vec<f64>,
    /// First bin index treated as audio (skips DC/near-DC).
    first_bin: usize,
}

#[inline]
fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}
#[inline]
fn lin(d: f32) -> f32 {
    10f32.powf(d / 20.0)
}

impl Dynamics {
    pub fn new(band: Band, num_channels: usize, max_window: usize) -> Self {
        let nb = max_window / 2 + 1;
        Dynamics {
            band,
            num_channels,
            sample_rate: 48000.0,
            window_size: 0,
            num_bins: 0,
            cached: None,
            thresholds_db: vec![0.0; nb],
            band_weight: vec![0.0; nb],
            det_weight: vec![1.0; nb],
            mono_weight: vec![0.0; nb],
            envelopes: vec![vec![1e-4; nb]; num_channels],
            gain_db: vec![0.0; nb],
            gain_smooth: vec![0.0; nb],
            vis_env_db: vec![-100.0; nb],
            vis_gain_db: vec![0.0; nb],
            prefix: vec![0.0; nb + 1],
            first_bin: 1,
        }
    }

    pub fn configure(&mut self, sample_rate: f32, window_size: usize) {
        self.sample_rate = sample_rate;
        self.window_size = window_size;
        self.num_bins = window_size / 2 + 1;
        self.cached = None;
        self.first_bin = ((20.0 * window_size as f32 / sample_rate).floor() as usize + 1).max(1);
        self.reset();
    }

    pub fn reset(&mut self) {
        for e in &mut self.envelopes {
            e.fill(1e-4);
        }
    }

    fn rebuild_tables(&mut self, s: &DynamicsSettings) {
        let n = self.window_size as f32;
        let xo = s.crossover_hz.max(20.0);
        let mono = s.mono_below_hz;
        for k in 0..self.num_bins {
            let f = (k as f32 * self.sample_rate / n).max(1.0);
            let oct = (f / s.center_hz.max(1.0)).log2();
            self.thresholds_db[k] = s.threshold_db + s.slope_db_oct * oct + s.curve_db_oct2 * oct * oct;
            // cosine crossfade over one octave centred on the crossover
            let o = (f / xo).log2();
            let wl = if o <= -0.5 {
                1.0
            } else if o >= 0.5 {
                0.0
            } else {
                0.5 - 0.5 * (std::f32::consts::PI * o).sin()
            };
            let mut w = match self.band {
                Band::Low => wl,
                Band::High => 1.0 - wl,
            };
            if f < 10.0 || f > self.sample_rate * 0.49 {
                w = 0.0;
            }
            self.band_weight[k] = w;
            self.det_weight[k] = lin(weighting_db(s.weighting, self.sample_rate, f));
            // mono-below: full below mono_hz, crossfade over one octave above it
            self.mono_weight[k] = if mono <= 0.0 {
                0.0
            } else {
                let om = (f / mono).log2();
                if om <= 0.0 {
                    1.0
                } else if om >= 1.0 {
                    0.0
                } else {
                    0.5 + 0.5 * (std::f32::consts::PI * om).cos()
                }
            };
        }
        self.cached = Some(*s);
    }

    /// Apply to one frame. `spectra[channel][bin]`, `mag_scale` from the STFT stage.
    /// `timing_scale` (0..1) shortens attack/release right after a reset so envelopes settle.
    pub fn process_frame(
        &mut self,
        spectra: &mut [Vec<Complex32>],
        num_bins: usize,
        mag_scale: f32,
        s: &DynamicsSettings,
        timing_scale: f32,
    ) {
        debug_assert_eq!(num_bins, self.num_bins);
        let needs = match &self.cached {
            None => true,
            Some(c) => {
                c.threshold_db != s.threshold_db
                    || c.center_hz != s.center_hz
                    || c.slope_db_oct != s.slope_db_oct
                    || c.curve_db_oct2 != s.curve_db_oct2
                    || c.crossover_hz != s.crossover_hz
                    || c.mono_below_hz != s.mono_below_hz
                    || c.weighting != s.weighting
            }
        };
        if needs {
            self.rebuild_tables(s);
        }

        // Mono-below (only meaningful in the low band stage, which sees the lowest resolution)
        if self.band == Band::Low && s.mono_below_hz > 0.0 && self.num_channels == 2 {
            let (a, b) = spectra.split_at_mut(1);
            let (l, r) = (&mut a[0], &mut b[0]);
            for k in 0..num_bins {
                let m = self.mono_weight[k];
                if m <= 0.0 {
                    continue;
                }
                let mid = (l[k] + r[k]) * 0.5;
                l[k] = l[k] * (1.0 - m) + mid * m;
                r[k] = r[k] * (1.0 - m) + mid * m;
            }
        }

        let hop = self.window_size as f32 / 4.0;
        let frame_dt = hop / self.sample_rate;
        let atk = (s.attack_ms * timing_scale).max(0.01) / 1000.0;
        let rel = (s.release_ms * timing_scale).max(0.01) / 1000.0;
        let a_c = (-frame_dt / atk).exp();
        let r_c = (-frame_dt / rel).exp();
        let down_k = 1.0 - 1.0 / s.down_ratio.max(1.0);
        let up_k = 1.0 - 1.0 / s.up_ratio.max(1.0);

        // envelopes
        for ch in 0..self.num_channels {
            let env = &mut self.envelopes[ch];
            let sp = &spectra[ch];
            for k in 0..num_bins {
                let m = sp[k].norm() * mag_scale;
                let e = env[k];
                env[k] = if m > e { m + (e - m) * a_c } else { m + (e - m) * r_c };
            }
        }

        // per channel gain with stereo link on the detector
        let link = s.stereo_link.clamp(0.0, 1.0);
        for ch in 0..self.num_channels {
            for k in 0..num_bins {
                let mut e = self.envelopes[ch][k];
                if self.num_channels > 1 && link > 0.0 {
                    let mut mx = e;
                    for o in 0..self.num_channels {
                        mx = mx.max(self.envelopes[o][k]);
                    }
                    e = e + (mx - e) * link;
                }
                let e_db = db(e * self.det_weight[k]);
                if ch == 0 {
                    self.vis_env_db[k] = e_db;
                } else {
                    self.vis_env_db[k] = self.vis_env_db[k].max(e_db);
                }
                let over = e_db - self.thresholds_db[k];
                let mut g = 0.0;
                if k >= self.first_bin {
                    if over > 0.0 {
                        g = -over * down_k;
                    } else if s.up_ratio > 1.0 && e_db > -90.0 {
                        g = (-over).min(s.up_range_db) * up_k;
                    }
                }
                self.gain_db[k] = g * self.band_weight[k];
            }
            self.smooth(num_bins, s.smooth_oct);
            let sp = &mut spectra[ch];
            for k in 0..num_bins {
                sp[k] *= lin(self.gain_smooth[k]);
            }
            if ch == 0 {
                self.vis_gain_db[..num_bins].copy_from_slice(&self.gain_smooth[..num_bins]);
            } else {
                for k in 0..num_bins {
                    self.vis_gain_db[k] = 0.5 * (self.vis_gain_db[k] + self.gain_smooth[k]);
                }
            }
        }
    }

    /// Sample the analyzer state at frequency `f` (Hz): (envelope dB, gain dB, threshold dB,
    /// band weight). Linear interpolation between bins.
    pub fn sample_at(&self, f: f32) -> (f32, f32, f32, f32) {
        if self.num_bins < 2 {
            return (-100.0, 0.0, 0.0, 0.0);
        }
        let pos = f * self.window_size as f32 / self.sample_rate;
        let k = (pos.floor() as usize).min(self.num_bins - 2);
        let t = (pos - k as f32).clamp(0.0, 1.0);
        let lerp = |a: &[f32]| a[k] + (a[k + 1] - a[k]) * t;
        (lerp(&self.vis_env_db), lerp(&self.vis_gain_db), lerp(&self.thresholds_db), lerp(&self.band_weight))
    }

    /// Octave-proportional moving average of `gain_db` into `gain_smooth`.
    fn smooth(&mut self, nb: usize, oct: f32) {
        if oct <= 0.0 {
            self.gain_smooth[..nb].copy_from_slice(&self.gain_db[..nb]);
            return;
        }
        self.prefix[0] = 0.0;
        for k in 0..nb {
            self.prefix[k + 1] = self.prefix[k] + self.gain_db[k] as f64;
        }
        let hi = 2f32.powf(oct / 2.0);
        let lo = 2f32.powf(-oct / 2.0);
        for k in 0..nb {
            let mut a = (k as f32 * lo).floor() as usize;
            let mut b = (k as f32 * hi).ceil() as usize;
            if b - a < 2 {
                a = k.saturating_sub(1);
                b = k + 1;
            }
            b = b.min(nb - 1);
            self.gain_smooth[k] = ((self.prefix[b + 1] - self.prefix[a]) / (b - a + 1) as f64) as f32;
        }
    }
}

/// Detector weighting in dB at frequency `f`.
pub fn weighting_db(w: Weighting, sr: f32, f: f32) -> f32 {
    match w {
        Weighting::Off => 0.0,
        Weighting::K => crate::dsp::loudness::k_weight_db(sr, f),
        Weighting::EqualLoudness => {
            // Inverse of an 80-phon equal-loudness contour, normalised to 0 dB at 1 kHz.
            // Piecewise-linear in log-frequency from ISO 226:2003 (80 phon), interpolated.
            const F: [f32; 12] = [20.0, 31.5, 50.0, 80.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 12500.0];
            // SPL needed for 80 phon at each frequency, minus 80 (so 1 kHz = 0).
            const SPL: [f32; 12] = [38.6, 29.2, 20.0, 12.7, 7.3, 2.6, 0.3, 0.0, -1.3, -5.7, 6.0, 12.1];
            let lf = f.clamp(20.0, 12500.0).ln();
            let mut i = 0;
            while i + 2 < F.len() && lf > F[i + 1].ln() {
                i += 1;
            }
            let (a, b) = (F[i].ln(), F[i + 1].ln());
            let t = ((lf - a) / (b - a)).clamp(0.0, 1.0);
            let needed = SPL[i] + (SPL[i + 1] - SPL[i]) * t;
            // The ear needs `needed` dB more at this frequency to sound as loud, so it hears it
            // that much quieter: weight the detector by -needed.
            -needed
        }
    }
}
