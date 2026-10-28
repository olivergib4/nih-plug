use onsetguard::dsp::sone::SoneMeter;

const SR: f32 = 48000.0;

fn measure(f: f32, dbfs: f32, ref_spl: f32) -> (f32, f32) {
    let mut m = SoneMeter::new(SR, 1, ref_spl);
    let amp = 10f32.powf(dbfs / 20.0);
    let n = (1.5 * SR) as usize;
    for i in 0..n {
        let t = i as f32 / SR;
        m.push(&[amp * (2.0 * std::f32::consts::PI * f * t).sin()]);
    }
    (m.sone, m.phon())
}

#[test]
fn one_khz_at_40_phon_is_one_sone_and_10_db_doubles_loudness() {
    // Definition: a 1 kHz tone at 40 dB SPL = 1 sone = 40 phon.
    let (s40, p40) = measure(1000.0, -60.0, 100.0); // 100 - 60 = 40 dB SPL
    let (s50, _) = measure(1000.0, -50.0, 100.0);
    let (s60, _) = measure(1000.0, -40.0, 100.0);
    let (s80, p80) = measure(1000.0, -20.0, 100.0);
    println!("1 kHz: 40 dB SPL → {s40:.2} sone ({p40:.1} phon); 50 → {s50:.2}; 60 → {s60:.2}; 80 → {s80:.2} ({p80:.1} phon)");
    assert!((s40 - 1.0).abs() < 0.25, "40 dB SPL at 1 kHz should be ~1 sone");
    assert!((s50 / s40 - 2.0).abs() < 0.4, "+10 dB should roughly double loudness");
    assert!((s60 / s50 - 2.0).abs() < 0.4);
    assert!((p80 - 80.0).abs() < 5.0, "80 dB SPL at 1 kHz should read ~80 phon");
}

#[test]
fn low_frequencies_are_quieter_than_1khz_at_equal_spl() {
    let (s1k, _) = measure(1000.0, -30.0, 100.0);
    let (s60, _) = measure(60.0, -30.0, 100.0);
    let (s3k, _) = measure(3000.0, -30.0, 100.0);
    println!("70 dB SPL: 60 Hz {s60:.2} sone, 1 kHz {s1k:.2} sone, 3 kHz {s3k:.2} sone");
    assert!(s60 < s1k * 0.7, "60 Hz should be clearly quieter than 1 kHz at the same SPL");
    assert!(s3k > s1k * 0.8, "3 kHz should be at least as loud as 1 kHz");
}

#[test]
fn broadband_is_louder_than_a_tone_of_equal_power() {
    // Critical-band summation: energy spread across many bands sounds louder than the same
    // energy in one band. White noise at -20 dBFS RMS vs a sine at -20 dBFS RMS.
    let mut tone = SoneMeter::new(SR, 1, 100.0);
    let mut noise = SoneMeter::new(SR, 1, 100.0);
    let n = (1.5 * SR) as usize;
    let mut seed = 3u32;
    let amp = 10f32.powf(-20.0 / 20.0);
    for i in 0..n {
        let t = i as f32 / SR;
        tone.push(&[amp * 2f32.sqrt() * (2.0 * std::f32::consts::PI * 1000.0 * t).sin()]);
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        let u = (seed as f32 / 4294967296.0) * 2.0 - 1.0; // uniform, rms = 1/sqrt(3)
        noise.push(&[amp * 3f32.sqrt() * u]);
    }
    println!("-20 dBFS RMS: tone {:.1} sone, white noise {:.1} sone", tone.sone, noise.sone);
    assert!(noise.sone > tone.sone * 1.5);
}
