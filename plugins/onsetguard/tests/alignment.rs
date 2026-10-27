use onsetguard::dsp::{Engine, Settings};

#[test]
fn impulse_alignment() {
    let sr = 48000.0; let block = 256;
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0; s.dyn_.up_ratio = 1.0; s.dyn_.mono_below_hz = 0.0; s.clip_on = false; s.protect_on = false;
    let mut e = Engine::new(1, sr, block);
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let lat = e.latency();
    let total = lat + 4096;
    let mut out = Vec::new();
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let mut c: Vec<f32> = (0..n).map(|i| if pos + i == 1000 { 1.0 } else { 0.0 }).collect();
        { let mut r: Vec<&mut [f32]> = vec![&mut c[..]]; e.process(&mut r, n, &s); }
        out.extend_from_slice(&c);
        pos += n;
    }
    let (mut mi, mut mv) = (0, 0.0f32);
    for (i, v) in out.iter().enumerate() { if v.abs() > mv { mv = v.abs(); mi = i; } }
    println!("latency {lat}, impulse in at 1000, peak out at {mi} (delta {}), value {mv}", mi as i64 - 1000);
    assert_eq!(mi, 1000 + lat);
}

#[test]
fn mono_below_sums_out_of_phase_sub() {
    // L = -R at 60 Hz (cancels in mono), in-phase 1 kHz. After mono-below at 120 Hz the 60 Hz
    // component should be gone from both channels while 1 kHz survives.
    let block = 256; let sr = 48000.0;
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0; s.dyn_.up_ratio = 1.0; s.clip_on = false; s.protect_on = false; s.dyn_.mono_below_hz = 120.0;
    let mut e = Engine::new(2, sr, block);
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let lat = e.latency();
    let total = lat + 48000;
    let mut outl = Vec::new();
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        let mut l: Vec<f32> = (0..n).map(|i| { let t = (pos + i) as f32 / sr; 0.5 * (2.0 * std::f32::consts::PI * 60.0 * t).sin() + 0.3 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() }).collect();
        let mut r: Vec<f32> = (0..n).map(|i| { let t = (pos + i) as f32 / sr; -0.5 * (2.0 * std::f32::consts::PI * 60.0 * t).sin() + 0.3 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() }).collect();
        { let mut refs: Vec<&mut [f32]> = vec![&mut l[..], &mut r[..]]; e.process(&mut refs, n, &s); }
        outl.extend_from_slice(&l);
        pos += n;
    }
    let seg = &outl[lat + 24000..lat + 48000];
    let goertzel = |f: f32| { let k = 2.0 * std::f32::consts::PI * f / sr; let (mut s1, mut s2) = (0.0f32, 0.0f32); for &x in seg { let s0 = x + 2.0 * k.cos() * s1 - s2; s2 = s1; s1 = s0; } ((s1 * s1 + s2 * s2 - 2.0 * k.cos() * s1 * s2).sqrt() * 2.0 / seg.len() as f32) };
    let a60 = goertzel(60.0); let a1k = goertzel(1000.0);
    println!("60 Hz: {:.4} (in 0.5)   1 kHz: {:.4} (in 0.3)", a60, a1k);
    assert!(a60 < 0.01);
    assert!((a1k - 0.3).abs() < 0.03);
}
