//! Across the clip-relevant settings, with Clip on, the output must never exceed the ceiling
//! and must actually reach it (no headroom left).
use onsetguard::dsp::{Engine, Settings, SubMode};
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
fn seeded(seed: u32) -> impl FnMut() -> f32 {
    let mut st = seed;
    move || { st ^= st << 13; st ^= st >> 17; st ^= st << 5; (st as f32 / u32::MAX as f32) * 2.0 - 1.0 }
}
fn material(seed: u32) -> Vec<f32> {
    let len = (1.0 * SR) as usize;
    let mut r = seeded(seed);
    (0..len).map(|i| {
        let t = i as f32 / SR;
        let sub = 0.45 * (2.0 * std::f32::consts::PI * 50.0 * t).sin();
        let mut growl = 0.0;
        for h in 1..=24 { growl += (2.0 * std::f32::consts::PI * 110.0 * h as f32 * t + (h as f32).powi(2)).sin() / h as f32; }
        let hat_phase = (t * 8.0).fract();
        let hat = if hat_phase < 0.03 { r() * 0.25 * (1.0 - hat_phase / 0.03) } else { 0.0 };
        let clap_phase = (t * 2.0 + 0.5).fract();
        let clap = if clap_phase < 0.12 { r() * 0.6 * (-clap_phase * 40.0).exp() } else { 0.0 };
        sub + growl * 0.18 + hat + clap
    }).collect()
}
#[test]
fn clip_on_never_exceeds_ceiling_and_reaches_it() {
    let l = material(1);
    let r = material(2);
    let mut worst: Vec<(f32, String)> = Vec::new();
    for &ceil_db in &[0.0f32] {
    for &os in &[1usize, 4] {
    for &protect in &[false, true] {
    for &trans_clip in &[1.0f32, 0.5, 0.0] {
    for &sub in &[SubMode::Off, SubMode::Protect, SubMode::Separate] {
    for &hf in &[false, true] {
    for &amt in &[0.0f32, 0.6, 1.0] {
    for &hard in &[1.0f32, 0.7] {
        if !hf && sub == SubMode::Off && amt != 0.6 { continue; }
        let mut s = Settings::default();
        s.clip_on = true; s.drive_db = 10.0; s.ceil_db = ceil_db; s.oversample = os;
        s.protect_on = protect; s.trans_clip = trans_clip; s.sub_mode = sub;
        s.hf_preserve = hf; s.band_protect = amt; s.hardness = hard;
        s.bin_ceil_db = 0.0;
        let out = run(&[l.clone(), r.clone()], &s);
        let ceil = 10f32.powf(ceil_db / 20.0);
        let pk = out.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
        let desc = format!("ceil {ceil_db} os {os} protect {protect} trans_clip {trans_clip} sub {sub:?} hf {hf} amt {amt} hard {hard}");
        if pk > ceil * (1.0 + 1e-6) { worst.push((db(pk / ceil), format!("OVER by {:+.2} dB: {desc}", db(pk / ceil)))); }
        if pk < ceil * 0.999 { worst.push((db(pk / ceil), format!("HEADROOM {:+.2} dB: {desc}", db(pk / ceil)))); }
    }}}}}}}}
    for (_, w) in &worst { println!("{w}"); }
    assert!(worst.is_empty(), "{} settings combinations fail", worst.len());
}
