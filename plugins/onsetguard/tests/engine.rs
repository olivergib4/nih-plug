//! Offline verification of the OnsetGuard engine: identity, alignment, onset protection,
//! clipper ceiling. Run with `cargo test -p onsetguard --release -- --nocapture`.

use onsetguard::dsp::{Engine, Settings};

const SR: f32 = 48000.0;
const BLOCK: usize = 256;

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// Run the engine over `input` (per channel) with the given settings, in BLOCK-sized chunks,
/// and return the output with latency removed (aligned to the input).
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
        let mut chunks: Vec<Vec<f32>> = (0..nc)
            .map(|ch| (0..n).map(|i| input[ch].get(pos + i).copied().unwrap_or(0.0)).collect())
            .collect();
        {
            let mut refs: Vec<&mut [f32]> = chunks.iter_mut().map(|c| &mut c[..]).collect();
            e.process(&mut refs, n, s);
        }
        for ch in 0..nc {
            out[ch][pos..pos + n].copy_from_slice(&chunks[ch]);
        }
        pos += n;
    }
    out.into_iter().map(|c| c[lat..].to_vec()).collect()
}

fn synth_clap(seed: u32) -> Vec<f32> {
    let sr = SR;
    let mut s = seed;
    let mut rnd = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s as f32 / 4294967296.0) * 2.0 - 1.0
    };
    let len = (0.6 * sr) as usize;
    let mut x = vec![0.0f32; len];
    let t0 = 0.05;
    for b in [t0, t0 + 0.009, t0 + 0.019, t0 + 0.031] {
        let st = (b * sr) as usize;
        for i in 0..(0.008 * sr) as usize {
            if st + i < len {
                x[st + i] += rnd() * (-(i as f32) / (0.0025 * sr)).exp();
            }
        }
    }
    let st = ((t0 + 0.03) * sr) as usize;
    for i in 0..len - st {
        x[st + i] += rnd() * 0.5 * (-(i as f32) / (0.09 * sr)).exp();
    }
    let (mut lp, mut hp, mut prev) = (0.0f32, 0.0f32, 0.0f32);
    let a_lp = (-2.0 * std::f32::consts::PI * 6000.0 / sr).exp();
    let a_hp = (-2.0 * std::f32::consts::PI * 500.0 / sr).exp();
    let mut y = vec![0.0f32; len];
    for i in 0..len {
        lp = x[i] * (1.0 - a_lp) + lp * a_lp;
        hp = a_hp * (hp + lp - prev);
        prev = lp;
        y[i] = hp;
    }
    let m = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    for v in &mut y {
        *v = *v / m * 0.7;
    }
    y
}

fn synth_zap() -> Vec<f32> {
    let len = (0.4 * SR) as usize;
    let p0 = (0.05 * SR) as usize;
    let mut y = vec![0.0f32; len];
    let mut ph = 0.0f32;
    for i in p0..len {
        let t = (i - p0) as f32 / SR;
        let f = 60.0 + 2500.0 * (-t / 0.045).exp();
        ph += 2.0 * std::f32::consts::PI * f / SR;
        y[i] = ph.sin() * (-t / 0.25).exp() * 0.7;
    }
    y
}

fn first_onset(x: &[f32]) -> usize {
    x.iter().position(|v| v.abs() > 1e-4).unwrap_or(0)
}

/// RMS of (wet - dry) in the 10 ms before the onset, in dB relative to the dry peak.
fn pre_ring_db(dry: &[f32], wet: &[f32], onset: usize) -> f32 {
    let pre = (0.010 * SR) as usize;
    let a = onset.saturating_sub(pre);
    let b = onset.saturating_sub((0.001 * SR) as usize);
    let mut s = 0.0;
    for i in a..b {
        s += (wet[i] - dry[i]).powi(2);
    }
    let peak = dry.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    db((s / (b - a) as f32).sqrt()) - db(peak)
}

/// Residual (wet - dry) over the first 5 ms after the onset, relative to dry energy, in dB.
fn transient_null_db(dry: &[f32], wet: &[f32], onset: usize) -> f32 {
    let n = (0.005 * SR) as usize;
    let (mut num, mut den) = (0.0f32, 0.0f32);
    for i in onset..onset + n {
        num += (wet[i] - dry[i]).powi(2);
        den += dry[i].powi(2);
    }
    db((num / den.max(1e-12)).sqrt())
}

fn best_lag(a: &[f32], b: &[f32]) -> i32 {
    let mut best = f32::MIN;
    let mut lag_best = 0;
    for lag in -64i32..=64 {
        let mut c = 0.0;
        for i in 64..a.len() - 64 {
            c += a[i] * b[(i as i32 + lag) as usize];
        }
        if c > best {
            best = c;
            lag_best = lag;
        }
    }
    lag_best
}

#[test]
fn identity_when_everything_is_off() {
    let clap = synth_clap(7);
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = false;
    let out = run(&[clap.clone()], &s);
    let err = clap.iter().zip(&out[0]).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    println!("identity max abs error: {err:e}");
    assert!(err < 1e-4, "identity error {err}");
}

#[test]
fn zero_phase_alignment_on_zap() {
    let zap = synth_zap();
    let mut s = Settings::default();
    s.clip_on = false;
    s.dyn_.mono_below_hz = 0.0;
    let out = run(&[zap.clone()], &s);
    let lag = best_lag(&zap, &out[0]);
    println!("zap best lag: {lag}");
    assert_eq!(lag, 0);
}

#[test]
fn onset_protection_removes_pre_ring_and_keeps_transient() {
    let clap = synth_clap(7);
    let onset = first_onset(&clap);
    let mut hard = Settings::default();
    hard.dyn_.threshold_db = -30.0;
    hard.dyn_.down_ratio = 6.0;
    hard.dyn_.up_ratio = 3.0;
    hard.dyn_.up_range_db = 18.0;
    hard.dyn_.attack_ms = 2.0;
    hard.dyn_.release_ms = 80.0;
    hard.dyn_.smooth_oct = 0.0;
    hard.dyn_.mono_below_hz = 0.0;
    hard.clip_on = false;

    let mut off = hard;
    off.protect_on = false;
    let o_off = run(&[clap.clone()], &off);
    let pr_off = pre_ring_db(&clap, &o_off[0], onset);
    let tn_off = transient_null_db(&clap, &o_off[0], onset);

    let mut on = hard;
    on.protect_on = true;
    let o_on = run(&[clap.clone()], &on);
    let pr_on = pre_ring_db(&clap, &o_on[0], onset);
    let tn_on = transient_null_db(&clap, &o_on[0], onset);

    println!("pre-ring  off {pr_off:.1} dB  on {pr_on:.1} dB");
    println!("transient off {tn_off:.1} dB  on {tn_on:.1} dB");
    assert!(pr_on < pr_off - 15.0, "protection should cut pre-ring by >15 dB");
    assert!(tn_on < -40.0, "transient should null against dry with protection on");
}

#[test]
fn clipper_never_exceeds_ceiling() {
    let clap = synth_clap(11);
    let zap = synth_zap();
    let len = clap.len().max(zap.len());
    let mut l = vec![0.0f32; len];
    let mut r = vec![0.0f32; len];
    for i in 0..len {
        l[i] = clap.get(i).copied().unwrap_or(0.0) * 1.0 + zap.get(i).copied().unwrap_or(0.0) * 0.9;
        r[i] = clap.get(i).copied().unwrap_or(0.0) * 1.1 + zap.get(i).copied().unwrap_or(0.0) * 0.8;
    }
    for os in [1usize, 2, 4, 8] {
        let mut s = Settings::default();
        s.drive_db = 12.0;
        s.trans_clip = 1.0;
        s.ceil_db = 0.0;
        s.oversample = os;
        let out = run(&[l.clone(), r.clone()], &s);
        let peak = out.iter().flat_map(|c| c.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        println!("os {os}: peak {:.2} dBFS", db(peak));
        assert!(peak <= 1.0 + 1e-4, "os {os} exceeded ceiling: {peak}");
        let in_peak = l.iter().chain(r.iter()).fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.9, "clipper output should reach the ceiling (in peak {in_peak})");
    }
}

#[test]
fn latency_is_reported_and_stable() {
    let mut e = Engine::new(2, SR, BLOCK);
    let s = Settings::default();
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let lat = e.latency();
    println!("latency at defaults: {lat} samples = {:.1} ms", lat as f32 / SR * 1000.0);
    assert_eq!(lat, (1 << 13) + (1 << 10) + (1 << 10) + 16 + e_extra(lat));
}

fn e_extra(lat: usize) -> usize {
    // extra = max(0, pre_max + det_latency - (long + short)); at defaults this is 0
    let _ = lat;
    0
}
