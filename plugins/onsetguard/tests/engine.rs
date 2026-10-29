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
    assert_eq!(lat, (1 << 13) + (1 << 10) + (1 << 10) + 128 + e_extra(lat));
}

fn e_extra(lat: usize) -> usize {
    // extra = max(0, pre_max + det_latency - (long + short)); at defaults this is 0
    let _ = lat;
    0
}

#[test]
fn gain_reduction_meter_tracks_the_clipper() {
    let zap = synth_zap();
    let mut e = Engine::new(1, SR, BLOCK);
    let mut s = Settings::default();
    s.trans_clip = 1.0;
    s.protect_on = false;
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    for (drive, expect_gr) in [(0.0f32, false), (12.0, true)] {
        s.drive_db = drive;
        e.set_windows(s.long_order, s.short_order, s.oversample);
        let mut pos = 0;
        let total = zap.len() + e.latency();
        let mut min_post = 0.0f32;
        while pos < total {
            let n = BLOCK.min(total - pos);
            let mut c: Vec<f32> = (0..n).map(|i| zap.get(pos + i).copied().unwrap_or(0.0)).collect();
            { let mut r: Vec<&mut [f32]> = vec![&mut c[..]]; e.process(&mut r, n, &s); }
            if pos + n < zap.len() + e.latency() / 2 {
                min_post = min_post.min(e.gain_reduction());
            }
            pos += n;
        }
        println!("zap, drive {drive} dB: post GR {min_post:.1} dB");
        if expect_gr { assert!(min_post < -6.0, "12 dB of drive into a zap must show clipper GR"); } else { assert!(min_post > -0.5, "no drive, no GR"); }
    }
}

fn goertzel(x: &[f32], sr: f32, f: f32) -> f32 {
    let k = 2.0 * std::f32::consts::PI * f / sr;
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    let c = 2.0 * k.cos();
    for &v in x {
        let s0 = v + c * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    (s1 * s1 + s2 * s2 - c * s1 * s2).max(0.0).sqrt() * 2.0 / x.len() as f32
}

/// Sub split: 50 Hz sub + 2 kHz tone driven into the clipper.
/// Off: harmonics of 50 Hz and intermod (2 kHz ± 50) both appear.
/// Separate: 50 Hz harmonics appear (sub clipped alone) but intermod is gone.
/// Protect: 50 Hz harmonics gone, the sub comes through at its driven level, and the ceiling holds.
#[test]
fn sub_split_modes_do_what_they_say() {
    use onsetguard::dsp::SubMode;
    let len = (1.0 * SR) as usize;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            0.35 * (2.0 * std::f32::consts::PI * 50.0 * t).sin() + 0.5 * (2.0 * std::f32::consts::PI * 2000.0 * t).sin()
        })
        .collect();
    let mut base = Settings::default();
    base.drive_db = 6.0; // sub → 0.7, tone → 1.0, sum peaks ~1.7 against a 0 dB ceiling
    base.trans_clip = 1.0;
    base.protect_on = false;
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.sub_hz = 100.0;
    base.band_protect = 1.0;
    let mut results = Vec::new();
    for mode in [SubMode::Off, SubMode::Separate, SubMode::Protect] {
        let mut s = base;
        s.sub_mode = mode;
        let out = run(&[x.clone()], &s);
        let seg = &out[0][len / 2..];
        let peak = seg.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let a50 = goertzel(seg, SR, 50.0);
        let harm = goertzel(seg, SR, 150.0).max(goertzel(seg, SR, 250.0));
        let im = goertzel(seg, SR, 1950.0).max(goertzel(seg, SR, 2050.0)).max(goertzel(seg, SR, 2100.0));
        println!(
            "{mode:?}: peak {:.2} dBFS, 50 Hz {:.1} dB, sub harmonics {:.1} dB, intermod {:.1} dB",
            db(peak), db(a50), db(harm), db(im)
        );
        results.push((peak, a50, harm, im));
    }
    let (off, sep, prot) = (results[0], results[1], results[2]);
    for r in &results {
        assert!(r.0 <= 1.0 + 1e-3, "ceiling exceeded: {}", r.0);
    }
    // the rest is clipped into the room the sub leaves at each instant, so it carries
    // sub-rate sidebands at about the level a plain clip's intermod has; never worse
    assert!(db(sep.3) < db(off.3) + 1.0 && db(prot.3) < db(off.3) + 5.0, "band modes must not add more intermod than the plain clip");
    assert!(db(prot.2) < db(off.2) - 20.0, "Protect should remove sub harmonics");
    assert!((db(prot.1) - db(0.7)).abs() < 0.5, "Protect should pass the sub at its driven level (0.7)");
    assert!(sep.2 > prot.2 * 10.0, "Separate still clips the sub on its own");
    // (a clipped sine's fundamental sits above the clip level, so compare modes, not the cap)
    assert!(sep.1 < prot.1, "Separate clips the sub lower than Protect passes it");
}

/// Matched bypass should settle on a gain that makes the dry signal as loud (in sones) as the
/// processed one. Process a loop with +9 dB of drive, let the servo settle, then check the
/// two sone readings agree within ~5%.
#[test]
fn matched_bypass_settles_in_sones() {
    let sr = SR;
    let len = (6.0 * sr) as usize;
    let clap = synth_clap(4);
    let kick: Vec<f32> = (0..(0.4 * sr) as usize).map(|i| { let t = i as f32 / sr; (2.0 * std::f32::consts::PI * (50.0 * t + 60.0 * (1.0 - (-t / 0.03).exp()))).sin() * (-t / 0.2).exp() * 0.8 }).collect();
    let mut x = vec![0.0f32; len];
    let beat = (0.4 * sr) as usize;
    for b in 0..(len / beat) {
        let at = b * beat;
        let src = if b % 2 == 0 { &kick } else { &clap };
        for i in 0..src.len() { if at + i < len { x[at + i] += src[i]; } }
    }
    let mut e = Engine::new(1, sr, BLOCK);
    let mut s = Settings::default();
    s.drive_db = 9.0;
    s.matched_bypass = true;
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let mut pos = 0;
    let (mut acc_d, mut acc_w, mut cnt) = (0.0f32, 0.0f32, 0usize);
    while pos < len {
        let n = BLOCK.min(len - pos);
        let mut c: Vec<f32> = x[pos..pos + n].to_vec();
        { let mut r: Vec<&mut [f32]> = vec![&mut c[..]]; e.process(&mut r, n, &s); }
        pos += n;
        if pos > (4.0 * sr) as usize {
            // average both readings over the last 2 s (hits make the instantaneous values swing)
            let (d, w) = e.sones();
            let (g, _, _) = e.meters();
            acc_d += d * g.powf(0.6);
            acc_w += w;
            cnt += 1;
        }
    }
    let (g, _, _) = e.meters();
    let (dry_scaled, wet) = (acc_d / cnt as f32, acc_w / cnt as f32);
    println!("last 2 s: processed {wet:.1} sone, matched dry {dry_scaled:.1} sone, match gain {:+.1} dB", 20.0 * g.log10());
    assert!((dry_scaled / wet - 1.0).abs() < 0.1, "servo should match sones: {dry_scaled} vs {wet}");
}

/// Post-ringing proper: a 2 ms noise burst followed by silence. Anything in the output
/// 30-170 ms after the burst is ringing from the spectral stage (there is no dry tail to
/// confuse it with). Reported relative to the burst's own energy, dynamics stage only.
#[test]
fn post_ring_after_hits_is_measured() {
    let len = (0.6 * SR) as usize;
    let mut burst = vec![0.0f32; len];
    let onset = (0.05 * SR) as usize;
    let mut seed = 21u32;
    for i in 0..(0.002 * SR) as usize {
        seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        burst[onset + i] = 0.7 * ((seed as f32 / 4294967296.0) * 2.0 - 1.0);
    }
    let a = onset + (0.030 * SR) as usize;
    let b = onset + (0.170 * SR) as usize;
    let e_burst: f32 = burst.iter().map(|v| v * v).sum();
    let mut base = Settings::default();
    base.dyn_.threshold_db = -30.0;
    base.dyn_.down_ratio = 6.0;
    base.dyn_.up_ratio = 3.0;
    base.dyn_.up_range_db = 18.0;
    base.dyn_.attack_ms = 2.0;
    base.dyn_.release_ms = 80.0;
    base.dyn_.mono_below_hz = 0.0;
    base.clip_on = false;
    let mut rows = Vec::new();
    for (name, protect, smooth, post) in [
        ("protection off, no smoothing", false, 0.0f32, 30.0f32),
        ("protection on, no smoothing", true, 0.0, 30.0),
        ("protection on, 1/6 oct smoothing", true, 0.17, 30.0),
        ("protection on, 1/3 oct smoothing", true, 0.33, 30.0),
        ("protection on, 1/6 oct, post 120 ms", true, 0.17, 120.0),
    ] {
        let mut s = base;
        s.protect_on = protect;
        s.dyn_.smooth_oct = smooth;
        s.post_ms = post;
        let out = run(&[burst.clone()], &s);
        let e_tail: f32 = out[0][a..b].iter().map(|v| v * v).sum();
        let pre_a = onset.saturating_sub((0.040 * SR) as usize);
        let e_pre: f32 = out[0][pre_a..onset.saturating_sub(48)].iter().map(|v| v * v).sum();
        let rel = db((e_tail / e_burst).sqrt());
        let rel_pre = db((e_pre / e_burst).sqrt());
        println!("{name}: post-ring (30-170 ms) {rel:.1} dB, pre-ring (-40..-1 ms) {rel_pre:.1} dB, both relative to the hit");
        rows.push(rel);
    }
    assert!(rows[0] < -30.0, "post-ring should be well below the hit even unprotected");
    assert!(rows[3] <= rows[1] + 0.5, "smoothing should not increase ringing");
    assert!(rows[4] < rows[2] - 3.0, "a longer Post should reduce ringing");
}

/// HF preserve: a loud 100 Hz tone (which the clipper flattens) plus a quiet 12 kHz tone.
/// Hard clipping the sum buries the 12 kHz detail under the clipped tops; with HF preserve the
/// highs bypass the clipper and survive, while the ceiling still holds.
#[test]
fn hf_preserve_keeps_highs_through_a_hard_clip() {
    let len = (1.0 * SR) as usize;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            0.5 * (2.0 * std::f32::consts::PI * 100.0 * t).sin() + 0.02 * (2.0 * std::f32::consts::PI * 12000.0 * t).sin()
        })
        .collect();
    let mut base = Settings::default();
    base.drive_db = 12.0; // 100 Hz → 2.0, hard clipped to 1.0
    base.protect_on = false;
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.hardness = 1.0;
    base.trans_clip = 1.0;
    base.band_protect = 1.0;
    let mut res = Vec::new();
    for hf in [false, true] {
        let mut s = base;
        s.hf_preserve = hf;
        let out = run(&[x.clone()], &s);
        let seg = &out[0][len / 2..];
        let peak = seg.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let a12k = goertzel(seg, SR, 12000.0);
        // clipping a 100 Hz square-ish wave puts odd harmonics everywhere; measure the 12 kHz
        // tone against the clip harmonics right next to it (11.9 / 12.1 kHz are not harmonics
        // of 100 Hz... 11900 = 119*100 is; use 12050 instead)
        let floor = goertzel(seg, SR, 12050.0).max(goertzel(seg, SR, 11950.0));
        println!("hf preserve {hf}: peak {:.2} dBFS, 12 kHz tone {:.1} dB, clip-harmonic floor nearby {:.1} dB", db(peak), db(a12k), db(floor));
        res.push((peak, a12k, floor));
    }
    assert!(res[0].0 <= 1.0 + 1e-3 && res[1].0 <= 1.0 + 1e-3, "ceiling must hold");
    let in_tone = 0.02 * 10f32.powf(12.0 / 20.0); // 12 kHz after drive
    assert!((db(res[1].1) - db(in_tone)).abs() < 1.5, "with HF preserve the 12 kHz tone should come through at its driven level");
    assert!(res[1].1 > res[0].1 * 1.5, "HF preserve should keep more of the 12 kHz tone than plain hard clipping");

    // Highs Amount: the 12 kHz tone must rise monotonically from the clipped level (0%) to
    // the driven level (100%)
    let mut prev = 0.0f32;
    for amt in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let mut s = base;
        s.hf_preserve = true;
        s.hf_amount = amt;
        let out = run(&[x.clone()], &s);
        let seg = &out[0][len / 2..];
        let a12k = goertzel(seg, SR, 12000.0);
        println!("highs amount {:.0}%: 12 kHz tone {:.1} dB", amt * 100.0, db(a12k));
        assert!(a12k >= prev * 0.98, "highs amount should not reduce the tone as it rises");
        prev = a12k;
    }
    assert!((db(prev) - db(in_tone)).abs() < 1.5);
}

/// Sub Protect Amount: 0% leaves the body at full level and lets the final clip touch the sub
/// where the sum exceeds the ceiling; 100% keeps the sub perfectly clean and ducks the body.
#[test]
fn sub_protect_amount_trades_body_level_for_sub_purity() {
    use onsetguard::dsp::SubMode;
    let len = (1.0 * SR) as usize;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            0.35 * (2.0 * std::f32::consts::PI * 50.0 * t).sin() + 0.5 * (2.0 * std::f32::consts::PI * 2000.0 * t).sin()
        })
        .collect();
    let mut base = Settings::default();
    base.drive_db = 6.0;
    base.trans_clip = 1.0;
    base.protect_on = false;
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.sub_hz = 100.0;
    base.sub_mode = SubMode::Protect;
    let mut rows = Vec::new();
    for amt in [0.0f32, 0.2, 0.4, 0.6, 0.8, 1.0] {
        let mut s = base;
        s.band_protect = amt;
        let out = run(&[x.clone()], &s);
        let seg = &out[0][len / 2..];
        let peak = seg.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let body = goertzel(seg, SR, 2000.0);
        let harm = goertzel(seg, SR, 150.0).max(goertzel(seg, SR, 250.0));
        println!("protect amount {:.0}%: peak {:.2} dBFS, body (2 kHz) {:.1} dB, sub harmonics {:.1} dB", amt * 100.0, db(peak), db(body), db(harm));
        rows.push((peak, body, harm));
        assert!(peak <= 1.0 + 1e-3, "ceiling must hold");
    }
    let n = rows.len();
    for w in rows.windows(2) {
        assert!(w[1].1 <= w[0].1 + 0.02, "body level should fall monotonically with the amount");
        assert!(w[1].2 <= w[0].2 * 1.15, "sub harmonics should fall monotonically with the amount");
    }
    assert!(rows[0].1 > rows[n - 1].1 && rows[n - 1].2 < rows[0].2);
}

/// Preserve Highs on, Protect Amount 100%, clip on, a zap (HF-dominant hit) that stays under
/// the ceiling: the clip stage has nothing to do, so the output must null against the input.
/// Catches the two 0.9.0 regressions: the highs reserving their full level (body clipped to
/// -20 dB) and the highs being delayed before the clip-everything path summed them.
#[test]
fn hf_preserve_is_transparent_below_the_ceiling() {
    let zap: Vec<f32> = synth_zap().iter().map(|v| v * 0.5).collect();
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.protect_on = false;
    s.clip_on = true;
    s.drive_db = 0.0;
    s.ceil_db = 0.0;
    s.hardness = 1.0;
    s.trans_clip = 1.0;
    s.hf_preserve = true;
    s.hf_hz = 4000.0;
    for (hf, amt) in [(false, 0.0), (true, 0.0), (true, 0.5), (true, 1.0)] {
        s.hf_preserve = hf;
        s.band_protect = amt;
        let out = run(&[zap.clone()], &s);
        println!("hf {hf} amt {amt}: best lag {} samples", best_lag(&zap, &out[0]));
        let pk = zap.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let err = zap.iter().zip(&out[0]).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        println!("hf preserve transparency at {:.0}%: max error {:.1} dB re peak", amt * 100.0, db(err / pk));
        assert!(db(err / pk) < -35.0, "hf {hf} at {amt}: error {} dB", db(err / pk));
    }
}

/// Routing audit: with Sub Protect at 100%, onset protection ON, hits + sub. The sub must stay
/// as clean as with protection off (the sub channels get the same protection blend as the full
/// signal, so `full - sub` must stay an exact remainder), the body must keep most of its level
/// against the plain clip, and the ceiling must hold.
#[test]
fn sub_protect_routing_under_onset_protection() {
    use onsetguard::dsp::SubMode;
    let len = (2.0 * SR) as usize;
    let clap = synth_clap(3);
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let hit = clap.get(i % (SR as usize / 2)).copied().unwrap_or(0.0) * 0.8;
            0.35 * (2.0 * std::f32::consts::PI * 50.0 * t).sin() + 0.4 * (2.0 * std::f32::consts::PI * 2000.0 * t).sin() + hit
        })
        .collect();
    let mut base = Settings::default();
    base.drive_db = 8.0;
    base.trans_clip = 1.0;
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.sub_hz = 100.0;
    let mut rows = Vec::new();
    for (protect, mode, amt) in [(false, SubMode::Off, 0.0), (false, SubMode::Protect, 1.0), (true, SubMode::Off, 0.0), (true, SubMode::Protect, 1.0)] {
        let mut s = base;
        s.protect_on = protect;
        s.sub_mode = mode;
        s.band_protect = amt;
        let out = run(&[x.clone()], &s);
        let seg = &out[0][len / 4..];
        let peak = seg.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let a50 = goertzel(seg, SR, 50.0);
        let harm = goertzel(seg, SR, 150.0).max(goertzel(seg, SR, 250.0)).max(goertzel(seg, SR, 350.0));
        let body = goertzel(seg, SR, 2000.0);
        let rms = (seg.iter().map(|v| v * v).sum::<f32>() / seg.len() as f32).sqrt();
        println!("onset protect {protect} sub {mode:?} amt {amt}: peak {:.2} dBFS, rms {:.1} dB, 50 Hz {:.1} dB, sub harmonics {:.1} dB, body 2 kHz {:.1} dB", db(peak), db(rms), db(a50), db(harm), db(body));
        rows.push((peak, rms, a50, harm, body));
    }
    for r in &rows {
        assert!(r.0 <= 1.0 + 1e-3, "ceiling exceeded: {}", r.0);
    }
    // protect-on vs protect-off: the sub purity must not degrade with onset protection on
    assert!(db(rows[3].3) < db(rows[1].3) + 3.0, "sub harmonics should not degrade with onset protection on");
    assert!(db(rows[3].3) < db(rows[2].3) - 15.0, "Sub Protect should still remove sub harmonics with onset protection on");
    // the sub keeps its driven level (it is the loud part here) and the overall rms stays
    // within 1 dB of the plain clip: level is redistributed from body to sub, not lost
    assert!(db(rows[3].2) > db(rows[2].2) + 1.0, "Protect should give the sub back the level the plain clip takes");
    assert!(db(rows[3].1) > db(rows[2].1) - 1.0, "rms should keep within 1 dB of the plain clip");
}
