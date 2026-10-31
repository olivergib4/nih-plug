//! Does "clean, frequency-peaked content" read louder than "a wall of noise"? Measured with
//! the plugin's sone model at EQUAL PEAK (0 dBFS), before and after a hard clip.
use onsetguard::dsp::sone::SoneMeter;
const SR: f32 = 48000.0;
fn db(x: f32) -> f32 { 20.0 * x.max(1e-12).log10() }
fn sones(x: &[f32]) -> f32 {
    let mut m = SoneMeter::new(SR, 1, 100.0);
    for &v in x { m.push(&[v]); }
    m.sone
}
fn flatness(x: &[f32]) -> f32 {
    // spectral flatness (Wiener entropy) over 4096-point frames, 0 = tone, 1 = white
    let n = 4096;
    let mut planner = realfft::RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n);
    let mut acc_log = 0.0f64; let mut acc_lin = 0.0f64; let mut cnt = 0usize;
    let mut pos = n;
    while pos + n <= x.len() {
        let mut buf: Vec<f32> = (0..n).map(|i| x[pos + i] * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos())).collect();
        let mut spec = r2c.make_output_vec();
        r2c.process(&mut buf, &mut spec).unwrap();
        for k in 4..n / 2 { let p = spec[k].norm_sqr().max(1e-20) as f64; acc_log += p.ln(); acc_lin += p; cnt += 1; }
        pos += n;
    }
    ((acc_log / cnt as f64).exp() / (acc_lin / cnt as f64)) as f32
}
fn norm_peak(mut x: Vec<f32>) -> Vec<f32> { let p = x.iter().fold(0.0f32, |m, v| m.max(v.abs())); for v in &mut x { *v /= p; } x }
#[test]
fn peaky_content_vs_wall_of_noise_at_equal_peak() {
    let len = (3.0 * SR) as usize;
    let mut st = 99u32;
    let noise: Vec<f32> = (0..len).map(|_| { st ^= st << 13; st ^= st >> 17; st ^= st << 5; (st as f32 / u32::MAX as f32) * 2.0 - 1.0 }).collect();
    // pink-ish noise: one-pole lowpass at 500 Hz + a bit of white
    let a = (-2.0 * std::f32::consts::PI * 500.0 / SR).exp();
    let mut y = 0.0f32;
    let pink: Vec<f32> = noise.iter().map(|w| { y = w * (1.0 - a) + y * a; y * 4.0 + w * 0.15 }).collect();
    // "clean peaked" content: 12 harmonics of 110 Hz with 1/h amplitudes (a saw-like growl)
    let saw: Vec<f32> = (0..len).map(|i| { let t = i as f32 / SR; (1..=12).map(|h| (2.0 * std::f32::consts::PI * 110.0 * h as f32 * t).sin() / h as f32).sum() }).collect();
    // the same growl buried in noise 10 dB under it (a "wall")
    let wall: Vec<f32> = saw.iter().zip(&pink).map(|(s, p)| s + p * 0.25).collect();
    let cases = [("white noise", noise), ("pink-ish noise", pink), ("12-harmonic growl (peaky)", saw), ("growl + noise bed (wall)", wall)];
    println!("{:<28} {:>9} {:>9} {:>8} | {:>9} {:>9} {:>8}", "equal peak 0 dBFS", "rms dB", "sones", "flat", "clip+6 rms", "sones", "flat");
    for (name, x) in cases {
        let x = norm_peak(x);
        let rms = db((x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt());
        let s = sones(&x);
        let f = flatness(&x);
        let g = 10f32.powf(6.0 / 20.0);
        let c: Vec<f32> = x.iter().map(|v| (v * g).clamp(-1.0, 1.0)).collect();
        let rms_c = db((c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt());
        println!("{name:<28} {rms:>9.1} {s:>9.1} {f:>8.3} | {rms_c:>10.1} {:>9.1} {:>8.3}", sones(&c), flatness(&c));
    }
}
