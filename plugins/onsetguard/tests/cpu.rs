use onsetguard::dsp::{Engine, Settings, SubMode};
#[test]
fn realtime_factor() {
    for (os, sub, hf) in [(4usize, SubMode::Off, false), (8, SubMode::Protect, true)] {
        let mut s = Settings::default();
        s.oversample = os; s.sub_mode = sub; s.hf_preserve = hf;
        let mut e = Engine::new(2, 48000.0, 256);
        e.set_windows(s.long_order, s.short_order, s.oversample);
        let secs = 10.0;
        let n = (secs * 48000.0) as usize;
        let mut l = vec![0.0f32; 256]; let mut r = vec![0.0f32; 256];
        let t0 = std::time::Instant::now();
        let mut ph = 0.0f32;
        for _ in 0..n / 256 {
            for i in 0..256 { ph += 0.01; l[i] = (ph).sin() * 0.9; r[i] = (ph * 1.01).sin() * 0.9; }
            let mut refs = [&mut l[..], &mut r[..]];
            e.process(&mut refs, 256, &s);
        }
        let el = t0.elapsed().as_secs_f32();
        println!("os {os} sub {sub:?} hf {hf}: {secs} s of stereo in {el:.2} s -> {:.1}% of one core", el / secs * 100.0);
    }
}
