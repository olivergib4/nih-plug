//! 4x oversampling for the nonlinear stage, with minimum-phase FIR filters so nothing rings before
//! a transient. The same 128-tap low-pass (Kaiser design at the base-rate Nyquist, made minimum
//! phase) is used up and down. Delay is a few base-rate samples and frequency dependent; it is
//! not reported as latency.

pub const OS: usize = 4;
const TAPS: usize = 128;
const PHASE_LEN: usize = TAPS / OS;

#[rustfmt::skip]
const H: [f32; TAPS] = [
    5.574907652e-03, 2.045574394e-02, 4.880423102e-02, 9.092776780e-02,
    1.411448806e-01, 1.875647499e-01, 2.149212339e-01, 2.100188722e-01,
    1.678214315e-01, 9.536900414e-02, 1.116330641e-02, -6.054984985e-02,
    -9.874980860e-02, -9.417211639e-02, -5.345682481e-02, 3.332274596e-03,
    5.110298016e-02, 7.044059724e-02, 5.576554731e-02, 1.710437658e-02,
    -2.534269601e-02, -5.119221717e-02, -4.954715958e-02, -2.365156699e-02,
    1.159137820e-02, 3.774735793e-02, 4.232416536e-02, 2.472432666e-02,
    -4.214012557e-03, -2.855044909e-02, -3.587603161e-02, -2.360109388e-02,
    2.996386217e-04, 2.220262054e-02, 3.048103036e-02, 2.163589184e-02,
    1.676551603e-03, -1.774172804e-02, -2.603120753e-02, -1.940724151e-02,
    -2.543353165e-03, 1.453696954e-02, 2.234549754e-02, 1.716776659e-02,
    2.769592327e-03, -1.217701172e-02, -1.925754055e-02, -1.502666660e-02,
    -2.629851267e-03, 1.038986083e-02, 1.663460395e-02, 1.303014616e-02,
    2.291024885e-03, -8.996861632e-03, -1.437677341e-02, -1.119271199e-02,
    -1.855014805e-03, 7.875364752e-03, 1.240845356e-02, 9.517603627e-03,
    1.387771760e-03, -6.942366233e-03, -1.067598170e-02, -8.002216771e-03,
    -9.309377094e-04, 6.141666204e-03, 9.140266320e-03, 6.642385458e-03,
    5.096917357e-04, -5.435762136e-03, -7.772190724e-03, -5.431434385e-03,
    -1.405681376e-04, 4.798216270e-03, 6.551589689e-03, 4.364059879e-03,
    -1.670179243e-04, -4.213094360e-03, -5.464637967e-03, -3.434797361e-03,
    4.105457982e-04, 3.671753710e-03, 4.500997379e-03, 2.635173093e-03,
    -5.913560572e-04, -3.168195927e-03, -3.651360975e-03, -1.957865920e-03,
    7.125465065e-04, 2.700482145e-03, 2.908270992e-03, 1.394139567e-03,
    -7.789891270e-04, -2.267086711e-03, -2.266834198e-03, -9.360060743e-04,
    7.986723872e-04, 1.870661560e-03, 1.719536130e-03, 5.720138624e-04,
    -7.787415966e-04, -1.510354179e-03, -1.260435885e-03, -2.938701125e-04,
    7.276282773e-04, 1.188672980e-03, 8.830269018e-04, 8.975992965e-05,
    -6.527644071e-04, -9.063532040e-04, -5.809294163e-04, 5.030545465e-05,
    5.639657511e-04, 6.640747578e-04, 3.452970273e-04, -1.384603496e-04,
    -4.673061734e-04, -4.598016837e-04, -1.688572234e-04, 1.842108136e-04,
    3.687884513e-04, 2.932311724e-04, 4.262653196e-05, -1.964381982e-04,
    -2.739146603e-04, -1.614792590e-04, 3.917041663e-05, 1.852466905e-04,
];

/// One channel: `up` turns one input sample into `OS` samples, `down` turns `OS` samples back into one.
#[derive(Clone)]
pub struct Oversampler {
    up_hist: [f32; PHASE_LEN],
    up_pos: usize,
    down_hist: [f32; TAPS],
    down_pos: usize,
}

impl Default for Oversampler {
    fn default() -> Self {
        Oversampler { up_hist: [0.0; PHASE_LEN], up_pos: 0, down_hist: [0.0; TAPS], down_pos: 0 }
    }
}

impl Oversampler {
    pub fn reset(&mut self) {
        *self = Oversampler::default();
    }

    #[inline]
    pub fn up(&mut self, x: f32) -> [f32; OS] {
        self.up_pos = (self.up_pos + PHASE_LEN - 1) % PHASE_LEN;
        self.up_hist[self.up_pos] = x;
        let mut out = [0.0f32; OS];
        for (p, o) in out.iter_mut().enumerate() {
            let mut acc = 0.0f32;
            for k in 0..PHASE_LEN {
                acc += H[k * OS + p] * self.up_hist[(self.up_pos + k) % PHASE_LEN];
            }
            *o = acc * OS as f32;
        }
        out
    }

    #[inline]
    pub fn down(&mut self, x: [f32; OS]) -> f32 {
        for v in x {
            self.down_pos = (self.down_pos + TAPS - 1) % TAPS;
            self.down_hist[self.down_pos] = v;
        }
        let mut acc = 0.0f32;
        for (j, h) in H.iter().enumerate() {
            acc += h * self.down_hist[(self.down_pos + j) % TAPS];
        }
        acc
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn through(f: f32, n: usize) -> (Vec<f32>, Vec<f32>) {
        let sr = 44100.0;
        let mut o = Oversampler::default();
        let x: Vec<f32> = (0..n).map(|i| 0.5 * (2.0 * std::f32::consts::PI * f * i as f32 / sr).sin()).collect();
        let y = x.iter().map(|&v| { let u = o.up(v); o.down(u) }).collect();
        (x, y)
    }

    fn rms(v: &[f32]) -> f32 {
        (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt()
    }

    #[test]
    fn passband_is_flat() {
        for f in [50.0, 1000.0, 8000.0, 16000.0] {
            let (x, y) = through(f, 8192);
            let d = 20.0 * (rms(&y[2048..]) / rms(&x[2048..])).log10();
            assert!(d.abs() < 0.2, "{f} Hz: {d} dB");
        }
    }

    #[test]
    fn nothing_comes_out_before_the_input() {
        let mut o = Oversampler::default();
        for _ in 0..64 {
            let u = o.up(0.0);
            assert_eq!(o.down(u), 0.0);
        }
        // an impulse: the output starts at the impulse, never earlier, and most of the energy arrives within a few samples
        let mut out = Vec::new();
        for i in 0..64 {
            let u = o.up(if i == 0 { 1.0 } else { 0.0 });
            out.push(o.down(u));
        }
        let total: f32 = out.iter().map(|v| v * v).sum();
        let early: f32 = out[..8].iter().map(|v| v * v).sum();
        // minimum phase: the energy is front-loaded and what rings does so after the impulse
        assert!(early / total > 0.85, "{}", early / total);
        let peak_at = out.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        assert!(peak_at <= 3, "{peak_at}");
    }

    #[test]
    fn images_are_rejected() {
        // a 1 kHz tone upsampled has images around 43.1/45.1 kHz; check the 4x stream is clean there
        let sr = 44100.0f32;
        let mut o = Oversampler::default();
        let mut up = Vec::new();
        for i in 0..8192 {
            up.extend_from_slice(&o.up(0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin()));
        }
        let up = &up[4096..];
        let probe = |f: f32| {
            let (mut c, mut s) = (0.0f32, 0.0f32);
            for (i, v) in up.iter().enumerate() {
                let ph = 2.0 * std::f32::consts::PI * f * i as f32 / (sr * 4.0);
                c += v * ph.cos();
                s += v * ph.sin();
            }
            (c * c + s * s).sqrt() / up.len() as f32
        };
        let d = 20.0 * (probe(43100.0) / probe(1000.0)).log10();
        assert!(d < -60.0, "{d} dB");
    }
}
