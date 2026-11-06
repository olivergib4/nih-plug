//! Butterworth band-pass design in second-order sections, the same construction as
//! `scipy.signal.butter(N, [lo, hi], 'band', output='sos')`: analog prototype poles, low-pass to
//! band-pass transform, bilinear transform with frequency pre-warping. Used by the analysis so the
//! plugin's band levels match the offline (Python) measurements the metrics were calibrated on.
//! f64 throughout: the 20 Hz edge of the lowest band puts poles very close to the unit circle.

use realfft::num_complex::Complex64;

#[derive(Clone, Copy, Debug)]
pub struct Sos {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

/// A cascade of sections with its own state (transposed direct form II), one channel.
#[derive(Clone)]
pub struct SosChain {
    secs: Vec<Sos>,
    state: Vec<[f64; 2]>,
}

impl SosChain {
    pub fn new(secs: Vec<Sos>) -> Self {
        let n = secs.len();
        SosChain { secs, state: vec![[0.0; 2]; n] }
    }

    pub fn reset(&mut self) {
        for s in &mut self.state {
            *s = [0.0; 2];
        }
    }

    #[inline]
    pub fn process(&mut self, x: f64) -> f64 {
        let mut v = x;
        for (s, st) in self.secs.iter().zip(self.state.iter_mut()) {
            let y = s.b0 * v + st[0];
            st[0] = s.b1 * v - s.a1 * y + st[1];
            st[1] = s.b2 * v - s.a2 * y;
            v = y;
        }
        v
    }
}

/// `order`-th order Butterworth band-pass between `lo` and `hi` Hz (giving `order` sections, i.e.
/// 2·order poles), sections ordered with the first section carrying the overall gain.
pub fn butter_bandpass(order: usize, lo: f64, hi: f64, sr: f64) -> Vec<Sos> {
    let n = order;
    // analog prototype (unit cutoff), left half-plane poles
    let proto: Vec<Complex64> = (0..n)
        .map(|k| {
            let th = std::f64::consts::PI * (2 * k + n + 1) as f64 / (2 * n) as f64;
            Complex64::new(th.cos(), th.sin())
        })
        .collect();
    // pre-warp
    let fs2 = 2.0 * sr;
    let w1 = fs2 * (std::f64::consts::PI * lo / sr).tan();
    let w2 = fs2 * (std::f64::consts::PI * hi / sr).tan();
    let bw = w2 - w1;
    let wo = (w1 * w2).sqrt();
    // low-pass -> band-pass: each pole becomes a pair
    let mut p_bp: Vec<Complex64> = Vec::with_capacity(2 * n);
    for &p in &proto {
        let pl = p * (bw / 2.0);
        let root = (pl * pl - wo * wo).sqrt();
        p_bp.push(pl + root);
        p_bp.push(pl - root);
    }
    // analog zeros: n at the origin; gain k = bw^n
    let k_analog = bw.powi(n as i32);
    // bilinear: poles
    let p_z: Vec<Complex64> = p_bp.iter().map(|&p| (fs2 + p) / (fs2 - p)).collect();
    // zeros: n at +1 (from the origin) and n at -1 (degree completion)
    // gain: k * real(prod(fs2 - z) / prod(fs2 - p)) with z = 0 (n of them)
    let mut num = Complex64::new(1.0, 0.0);
    let mut den = Complex64::new(1.0, 0.0);
    for _ in 0..n {
        num *= fs2;
    }
    for &p in &p_bp {
        den *= fs2 - p;
    }
    let k_z = k_analog * (num / den).re;
    // pair conjugate poles: p_bp was built as (pl + root, pl - root) per prototype pole; the
    // conjugate pairs are between prototype poles k and n-1-k. Simpler: group by taking each pole
    // with positive imaginary part and pairing it with its conjugate.
    let mut used = vec![false; p_z.len()];
    let mut secs = Vec::with_capacity(n);
    for i in 0..p_z.len() {
        if used[i] {
            continue;
        }
        let p = p_z[i];
        used[i] = true;
        // find closest conjugate
        let mut best = None;
        let mut best_d = f64::MAX;
        for j in 0..p_z.len() {
            if used[j] {
                continue;
            }
            let d = (p_z[j] - p.conj()).norm();
            if d < best_d {
                best_d = d;
                best = Some(j);
            }
        }
        let (a1, a2) = match best {
            Some(j) if best_d < 1e-6 => {
                used[j] = true;
                (-(2.0 * p.re), p.norm_sqr())
            }
            _ => {
                // real pole (odd order or on-axis): first order section
                (-p.re, 0.0)
            }
        };
        // numerator (1 - z^-1)(1 + z^-1) = 1 - z^-2
        secs.push(Sos { b0: 1.0, b1: 0.0, b2: -1.0, a1, a2 });
    }
    // real poles come in pairs for even order band-pass; if two first-order sections exist, they
    // would each have needed one zero; the pairing above only produces real poles when the
    // conjugate search fails, which for these designs does not happen.
    if let Some(first) = secs.first_mut() {
        first.b0 *= k_z;
        first.b1 *= k_z;
        first.b2 *= k_z;
    }
    secs
}

/// Zero-phase filtering (forward, then backward), like `scipy.signal.sosfiltfilt` with an odd
/// extension of `pad` samples at both ends to settle the transients. Offline use only.
pub fn filtfilt(secs: &[Sos], x: &[f32], pad: usize) -> Vec<f32> {
    let n = x.len();
    if n == 0 {
        return Vec::new();
    }
    let pad = pad.min(n - 1);
    let mut ext: Vec<f64> = Vec::with_capacity(n + 2 * pad);
    let x0 = x[0] as f64;
    let xn = x[n - 1] as f64;
    for i in (1..=pad).rev() {
        ext.push(2.0 * x0 - x[i] as f64);
    }
    ext.extend(x.iter().map(|&v| v as f64));
    for i in 1..=pad {
        ext.push(2.0 * xn - x[n - 1 - i] as f64);
    }
    let mut ch = SosChain::new(secs.to_vec());
    for v in ext.iter_mut() {
        *v = ch.process(*v);
    }
    ch.reset();
    for v in ext.iter_mut().rev() {
        *v = ch.process(*v);
    }
    ext[pad..pad + n].iter().map(|&v| v as f32).collect()
}

/// Magnitude response of a cascade at `f` Hz.
pub fn magnitude(secs: &[Sos], f: f64, sr: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * f / sr;
    let z1 = Complex64::new(w.cos(), -w.sin());
    let z2 = z1 * z1;
    let mut h = Complex64::new(1.0, 0.0);
    for s in secs {
        h *= (s.b0 + s.b1 * z1 + s.b2 * z2) / (1.0 + s.a1 * z1 + s.a2 * z2);
    }
    h.norm()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bandpass_edges_are_minus_3db() {
        let sr = 44100.0;
        for (lo, hi) in [(2000.0, 8000.0), (250.0, 1000.0), (20.0, 250.0), (5000.0, 10000.0)] {
            let s = butter_bandpass(4, lo, hi, sr);
            assert_eq!(s.len(), 4);
            let m_lo = 20.0 * magnitude(&s, lo, sr).log10();
            let m_hi = 20.0 * magnitude(&s, hi, sr).log10();
            let m_mid = 20.0 * magnitude(&s, (lo * hi).sqrt(), sr).log10();
            assert!((m_lo + 3.01).abs() < 0.05, "{lo}-{hi}: lo edge {m_lo}");
            assert!((m_hi + 3.01).abs() < 0.05, "{lo}-{hi}: hi edge {m_hi}");
            assert!(m_mid.abs() < 0.05, "{lo}-{hi}: centre {m_mid}");
            let m_far = 20.0 * magnitude(&s, lo / 4.0, sr).log10();
            assert!(m_far < -40.0, "{lo}-{hi}: stop {m_far}");
        }
    }

    #[test]
    fn chain_is_stable_and_matches_response() {
        let sr = 44100.0;
        let s = butter_bandpass(4, 500.0, 6000.0, sr);
        let mut c = SosChain::new(s.clone());
        let f = 2000.0;
        let mut p = 0.0f64;
        let n = 44100;
        let mut y = vec![0.0; n];
        for i in 0..n {
            let x = (2.0 * std::f64::consts::PI * f * i as f64 / sr).sin();
            y[i] = c.process(x);
        }
        for v in &y[n / 2..] {
            p += v * v;
        }
        let rms = (p / (n / 2) as f64).sqrt() * 2f64.sqrt();
        let expect = magnitude(&s, f, sr);
        assert!((rms - expect).abs() < 1e-3, "{rms} vs {expect}");
    }
}
