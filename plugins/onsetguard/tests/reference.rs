//! Reference pipeline on a synthetic track: kicks on the beat, a noise clap on the backbeat,
//! a growl bed. The grid detector must find the claps (not the kicks) and the half-bar period.
use onsetguard::reference::{analyze, flux_hits, grid_claps};

fn synth(sr: f32, secs: f32, bpm: f32) -> (Vec<f32>, Vec<f32>) {
    let n = (secs * sr) as usize;
    let beat = 60.0 / bpm;
    let mut l = vec![0.0f32; n];
    let mut r = vec![0.0f32; n];
    let mut seed = 7u32;
    let mut prev = 0.0f32;
    let mut rnd = || {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    for i in 0..n {
        let t = i as f32 / sr;
        let tb = t % beat;
        let beat_idx = (t / beat).floor() as i32;
        // growl bed: 110 Hz saw-ish + harmonics, sidechain-ducked under the kick and the clap
        let hit_beat = beat_idx % 4 == 0 || beat_idx % 4 == 2;
        let duck = if hit_beat { 0.9 - 0.7 * (-tb / 0.05).exp() } else { 0.9 };
        let mut g = 0.0;
        for h in 1..8 {
            g += (2.0 * std::f32::consts::PI * 110.0 * h as f32 * t).sin() / h as f32;
        }
        let growl = 0.35 * g * duck;
        // half-time: kick on beat 1, clap on beat 3 (once per bar): 55 Hz decaying sine, 150 ms
        let kick = if beat_idx % 4 == 0 && tb < 0.15 { (2.0 * std::f32::consts::PI * 55.0 * tb).sin() * (-tb * 20.0).exp() * 0.9 } else { 0.0 };
        // clap on beat 3: 120 ms high-passed noise burst (a real clap has no sub)
        let w = rnd();
        let hp = w - prev;
        prev = w;
        let clap = if beat_idx % 4 == 2 && tb < 0.12 { hp * (-tb * 12.0).exp() * 0.6 } else { 0.0 };
        let x = growl + kick + clap;
        l[i] = x.clamp(-1.0, 1.0);
        r[i] = (x + 0.005 * rnd()).clamp(-1.0, 1.0);
    }
    (l, r)
}

#[test]
fn grid_finds_backbeat_claps() {
    let sr = 44100.0;
    let bpm = 140.0;
    let (l, r) = synth(sr, 40.0, bpm);
    let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
    let hits = flux_hits(&mono, sr);
    assert!(hits.len() >= 20, "flux hits {}", hits.len());
    let (claps, period) = grid_claps(&hits);
    let half_bar = 4.0 * 60.0 / bpm; // one clap per bar in half-time
    assert!((period - half_bar).abs() < 0.01, "period {period} vs {half_bar}; hits {:?}", hits);
    assert!(claps.len() >= 18, "claps {}", claps.len());
    // every clap must sit on an odd beat
    for &t in &claps {
        let beat = t / (60.0 / bpm);
        let frac = beat - beat.round();
        assert!(frac.abs() < 0.08, "clap at {t} is off the beat ({frac})");
        assert!(beat.round() as i32 % 4 == 2, "clap at {t} is not on beat 3");
    }
    let rep = analyze("synth", &l, &r, sr).expect("analysis");
    assert!(rep.claps >= 10, "{rep:?}");
    assert!((rep.period_s - half_bar).abs() < 0.01);
    assert!(rep.report.band_delta(4) > 3.0, "clap band should dominate hits: {:?}", rep.report);
}
