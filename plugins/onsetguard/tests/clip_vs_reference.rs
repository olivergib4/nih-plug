//! Internal clip stage vs a plain sample-level hard clip of the same driven signal.
use onsetguard::dsp::{Engine, Settings};

const SR: f32 = 48000.0;
const BLOCK: usize = 256;

fn db(x: f32) -> f32 { 20.0 * x.max(1e-12).log10() }

fn run(input: &[Vec<f32>], s: &Settings) -> Vec<Vec<f32>> {
    let nc = input.len();
    let mut e = Engine::new(nc, SR, BLOCK);
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let lat = e.latency();
    let len = input[0].len();
    let total = len + lat;
    let mut out = vec![vec![0.0f32; total]; nc];
    let mut pos = 0;
    while pos < total {
        let n = BLOCK.min(total - pos);
        let mut chunks: Vec<Vec<f32>> = (0..nc).map(|ch| (0..n).map(|i| input[ch].get(pos + i).copied().unwrap_or(0.0)).collect()).collect();
        { let mut refs: Vec<&mut [f32]> = chunks.iter_mut().map(|c| &mut c[..]).collect(); e.process(&mut refs, n, s); }
        for ch in 0..nc { out[ch][pos..pos + n].copy_from_slice(&chunks[ch]); }
        pos += n;
    }
    out.into_iter().map(|c| c[lat..].to_vec()).collect()
}

fn rms(x: &[f32]) -> f32 { (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt() }

/// band energy (dB) via a crude DFT over a window
fn band_db(x: &[f32], lo: f32, hi: f32) -> f32 {
    let n = 4096;
    let mut acc = 0.0f64;
    let mut frames = 0;
    let mut pos = 0;
    while pos + n <= x.len() {
        let seg = &x[pos..pos + n];
        for k in ((lo / SR * n as f32) as usize)..((hi / SR * n as f32) as usize) {
            let (mut re, mut im) = (0.0f64, 0.0f64);
            for (i, v) in seg.iter().enumerate() {
                let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
                let ph = 2.0 * std::f64::consts::PI * k as f64 * i as f64 / n as f64;
                re += *v as f64 * w * ph.cos();
                im -= *v as f64 * w * ph.sin();
            }
            acc += re * re + im * im;
        }
        frames += 1;
        pos += n * 2;
    }
    10.0 * (acc / frames as f64).max(1e-30).log10() as f32
}

fn seeded(seed: u32) -> impl FnMut() -> f32 {
    let mut st = seed;
    move || { st ^= st << 13; st ^= st >> 17; st ^= st << 5; (st as f32 / u32::MAX as f32) * 2.0 - 1.0 }
}

/// 2 s of "riddim-ish" material: 50 Hz sub, a mid growl (sawtooth-ish harmonics), 4 claps
/// (bright noise bursts) and a hat pattern — broadband up to 20 kHz.
fn material() -> Vec<f32> {
    let len = (2.0 * SR) as usize;
    let mut r = seeded(99);
    let mut x = vec![0.0f32; len];
    for i in 0..len {
        let t = i as f32 / SR;
        let sub = 0.45 * (2.0 * std::f32::consts::PI * 50.0 * t).sin();
        let mut growl = 0.0;
        for h in 1..=24 { growl += (2.0 * std::f32::consts::PI * 110.0 * h as f32 * t + (h as f32).powi(2)).sin() / h as f32; }
        growl *= 0.18;
        let hat_phase = (t * 8.0).fract();
        let hat = if hat_phase < 0.03 { r() * 0.25 * (1.0 - hat_phase / 0.03) } else { 0.0 };
        let clap_phase = (t * 2.0 + 0.5).fract();
        let clap = if clap_phase < 0.12 { r() * 0.6 * (-clap_phase * 40.0).exp() } else { 0.0 };
        x[i] = sub + growl + hat + clap;
    }
    x
}

#[test]
fn internal_clip_vs_sample_hard_clip() {
    let x = material();
    let drive = 10f32.powf(8.0 / 20.0);
    let reference: Vec<f32> = x.iter().map(|v| (v * drive).clamp(-1.0, 1.0)).collect();
    let mut base = Settings::default();
    base.protect_on = false;
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.bin_ceil_db = 0.0;
    base.drive_db = 8.0;
    base.ceil_db = 0.0;
    base.hardness = 1.0;
    base.trans_clip = 1.0;
    base.clip_on = true;
    let bands = [(20.0, 200.0), (200.0, 2000.0), (2000.0, 8000.0), (8000.0, 14000.0), (14000.0, 17000.0), (17000.0, 20000.0), (20000.0, 24000.0)];
    let fmt = |y: &[f32]| -> String {
        let mut s = format!("rms {:6.2} dB  peak {:5.2}", db(rms(y)), db(y.iter().fold(0.0f32, |m, v| m.max(v.abs()))));
        for (lo, hi) in bands { s += &format!("  {:.0}-{:.0}k:{:6.1}", lo / 1000.0, hi / 1000.0, band_db(y, lo, hi)); }
        s
    };
    println!("dry*drive (unclipped): {}", fmt(&x.iter().map(|v| v * drive).collect::<Vec<_>>()));
    println!("reference 1x hard clip: {}", fmt(&reference));
    for os in [1usize, 2, 4, 8] {
        let mut s = base;
        s.oversample = os;
        let out = run(&[x.clone()], &s);
        println!("internal clip {os}x:        {}", fmt(&out[0]));
    }
}
