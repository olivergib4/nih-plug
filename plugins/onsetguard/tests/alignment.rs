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

/// Clip link: with decorrelated stereo content, independent clipping changes the L/R
/// correlation (the image moves on every hit); a linked gain keeps it where the dry signal had it.
#[test]
fn clip_link_preserves_stereo_image() {
    use onsetguard::dsp::clip::Oversampler;
    let sr = 48000.0f32;
    let n = 8192;
    let mut seed = 9u32;
    let mut rnd = move || { seed = seed.wrapping_mul(1664525).wrapping_add(1013904223); (seed as f32 / 4294967296.0) * 2.0 - 1.0 };
    let mut l0 = vec![0.0f32; n];
    let mut r0 = vec![0.0f32; n];
    for i in 0..n {
        let t = i as f32 / sr;
        let shared = 1.5 * (2.0 * std::f32::consts::PI * 70.0 * t).sin() * (-(t % 0.5) / 0.12).exp();
        l0[i] = shared + 0.35 * rnd();
        r0[i] = shared + 0.35 * rnd();
    }
    let corr = |l: &[f32], r: &[f32]| -> f32 {
        let (mut lr, mut ll, mut rr) = (0.0f32, 0.0f32, 0.0f32);
        for i in 512..n { lr += l[i] * r[i]; ll += l[i] * l[i]; rr += r[i] * r[i]; }
        lr / (ll * rr).sqrt()
    };
    let dry = corr(&l0, &r0);
    let mut out = Vec::new();
    for link in [0.0f32, 1.0] {
        let mut o = Oversampler::new(2, 8);
        o.set_os(4);
        let (mut l, mut r) = (l0.clone(), r0.clone());
        { let mut refs: Vec<&mut [f32]> = vec![&mut l[..], &mut r[..]]; o.process(&mut refs, n, 1.0, 0.6, link); }
        let c = corr(&l, &r);
        println!("link {link}: L/R correlation {c:.4} (dry {dry:.4})");
        out.push(c);
    }
    // Unlinked clipping changes the image (here it collapses the tops of both channels onto the
    // ceiling, raising correlation); linked gain keeps the dry image.
    assert!((out[1] - dry).abs() < (out[0] - dry).abs(), "linked should stay closer to dry");
    assert!((out[1] - dry).abs() < 0.01, "linked clipping should keep the dry correlation");
}

#[test]
fn weighting_curves_have_the_expected_shape() {
    use onsetguard::dsp::dynamics::{weighting_db, Weighting};
    let sr = 48000.0;
    for (name, w) in [("K", Weighting::K), ("EqualLoudness", Weighting::EqualLoudness)] {
        let v: Vec<(f32, f32)> = [30.0f32, 60.0, 100.0, 250.0, 1000.0, 3000.0, 8000.0, 12000.0].iter().map(|&f| (f, weighting_db(w, sr, f))).collect();
        println!("{name}: {}", v.iter().map(|(f, d)| format!("{f:.0}Hz {d:+.1}")).collect::<Vec<_>>().join("  "));
    }
    // K: ~0 dB at 1 kHz, about +4 dB at high frequencies, strong cut below 40 Hz
    assert!(weighting_db(Weighting::K, sr, 1000.0).abs() < 0.8); // the shelf is already rising at 1 kHz
    assert!((weighting_db(Weighting::K, sr, 8000.0) - 4.0).abs() < 0.6);
    assert!(weighting_db(Weighting::K, sr, 30.0) < -3.0);
    // Equal loudness: 0 at 1 kHz, lows strongly down, 3-4 kHz slightly up
    assert!(weighting_db(Weighting::EqualLoudness, sr, 1000.0).abs() < 0.1);
    assert!(weighting_db(Weighting::EqualLoudness, sr, 50.0) < -15.0);
    assert!(weighting_db(Weighting::EqualLoudness, sr, 3500.0) > 2.0);
    assert!(weighting_db(Weighting::Off, sr, 50.0) == 0.0);
}
