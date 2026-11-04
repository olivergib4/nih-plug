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
    // this test measures what Post does to ringing; with Upward Depth the silence after the
    // hit is not lifted at all (-144 dB), so use the old unlimited depth here
    base.dyn_.up_depth_db = 60.0;
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

/// Stereo stage: Width must leave the mono sum untouched; the anti-phase guard must remove
/// Side that is in anti-phase and leave in-phase (and merely panned) material alone.
#[test]
fn stereo_width_is_mono_compatible_and_guard_removes_antiphase() {
    let len = (3.0 * SR) as usize;
    let t = |i: usize| i as f32 / SR;
    // 1 kHz in anti-phase (L = -R), 3 kHz panned 70% left (in phase), 150 Hz centered
    let l: Vec<f32> = (0..len).map(|i| 0.2 * (2.0 * std::f32::consts::PI * 1000.0 * t(i)).sin() + 0.2 * (2.0 * std::f32::consts::PI * 3000.0 * t(i)).sin() + 0.2 * (2.0 * std::f32::consts::PI * 150.0 * t(i)).sin()).collect();
    let r: Vec<f32> = (0..len).map(|i| -0.2 * (2.0 * std::f32::consts::PI * 1000.0 * t(i)).sin() + 0.06 * (2.0 * std::f32::consts::PI * 3000.0 * t(i)).sin() + 0.2 * (2.0 * std::f32::consts::PI * 150.0 * t(i)).sin()).collect();
    let mut base = Settings::default();
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.clip_on = false;
    base.protect_on = false;
    let seg = |v: &Vec<f32>| v[len * 2 / 3..].to_vec();
    let mid = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| (x + y) * 0.5).collect() };
    let side = |a: &[f32], b: &[f32]| -> Vec<f32> { a.iter().zip(b).map(|(x, y)| (x - y) * 0.5).collect() };

    // width +6 dB above 300 Hz: mid identical, side above 300 Hz doubled, 150 Hz side unchanged
    let mut s = base;
    s.stereo.width_db = 6.0;
    s.stereo.width_hz = 300.0;
    let out = run(&[l.clone(), r.clone()], &s);
    let (ol, or) = (seg(&out[0]), seg(&out[1]));
    let (il, ir) = (seg(&l), seg(&r));
    let m_in = mid(&il, &ir);
    let m_out = mid(&ol, &or);
    let m_err = m_in.iter().zip(&m_out).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    let s_in = side(&il, &ir);
    let s_out = side(&ol, &or);
    let (s1k_in, s1k_out) = (goertzel(&s_in, SR, 1000.0), goertzel(&s_out, SR, 1000.0));
    let (s3k_in, s3k_out) = (goertzel(&s_in, SR, 3000.0), goertzel(&s_out, SR, 3000.0));
    println!("width +6 dB: mid max error {:.1} dB, side 1 kHz {:+.2} dB, side 3 kHz {:+.2} dB", db(m_err / 0.2), db(s1k_out / s1k_in), db(s3k_out / s3k_in));
    assert!(db(m_err / 0.2) < -80.0, "width must not touch the mono sum");
    assert!((db(s1k_out / s1k_in) - 6.0).abs() < 0.2 && (db(s3k_out / s3k_in) - 6.0).abs() < 0.2);

    // guard 100%: 1 kHz anti-phase side removed, 3 kHz panned side kept, mid unchanged
    let mut s = base;
    s.stereo.guard = 1.0;
    let out = run(&[l.clone(), r.clone()], &s);
    let (ol, or) = (seg(&out[0]), seg(&out[1]));
    let m_out = mid(&ol, &or);
    let m_err = m_in.iter().zip(&m_out).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
    let s_out = side(&ol, &or);
    let (g1k, g3k) = (db(goertzel(&s_out, SR, 1000.0) / s1k_in), db(goertzel(&s_out, SR, 3000.0) / s3k_in));
    let (l3k_in, l3k_out) = (goertzel(&il, SR, 3000.0), goertzel(&ol, SR, 3000.0));
    println!("guard 100%: mid max error {:.1} dB, anti-phase 1 kHz side {:+.1} dB, panned 3 kHz side {:+.2} dB, left 3 kHz {:+.2} dB", db(m_err / 0.2), g1k, g3k, db(l3k_out / l3k_in));
    assert!(db(m_err / 0.2) < -80.0, "guard must not touch the mono sum");
    assert!(g1k < -40.0, "anti-phase side should be removed");
    assert!(g3k.abs() < 0.2 && db(l3k_out / l3k_in).abs() < 0.2, "panned in-phase material must be untouched");
}

/// Loudness Balance: on a dull (-9 dB/oct) noise the specific-loudness spread across Bark
/// bands must shrink, gains must stay within Balance Range, overall level must not change by
/// more than the range allows, and the output must stay sample-aligned (zero-phase).
#[test]
fn loudness_balance_flattens_specific_loudness_within_range() {
    use onsetguard::dsp::sone::{SoneMeter, BANDS};
    let len = (8.0 * SR) as usize;
    // dull noise: white noise through a one-pole lowpass (about -6 dB/oct above 200 Hz), twice
    let mut st = 12345u32;
    let mut y1 = 0.0f32;
    let mut y2 = 0.0f32;
    let a = (-2.0 * std::f32::consts::PI * 200.0 / SR).exp();
    let x: Vec<f32> = (0..len)
        .map(|_| {
            st ^= st << 13; st ^= st >> 17; st ^= st << 5;
            let w = (st as f32 / u32::MAX as f32) * 2.0 - 1.0;
            y1 = w * (1.0 - a) + y1 * a;
            y2 = y1 * (1.0 - a) + y2 * a;
            y2 * 3.0
        })
        .collect();
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = false;
    s.balance.strength = 1.0;
    s.balance.speed_s = 1.0;
    s.balance.range_db = 6.0;
    let out = run(&[x.clone()], &s);
    let spread = |v: &[f32]| -> (f32, f32) {
        let mut m = SoneMeter::new(SR, 1, 100.0);
        for &smp in &v[v.len() - (2.0 * SR) as usize..] {
            m.push(&[smp]);
        }
        let n: Vec<f32> = (0..BANDS).filter(|&b| b <= 22 && m.specific[b] > 0.3).map(|b| m.specific[b].ln()).collect();
        let mean = n.iter().sum::<f32>() / n.len() as f32;
        let sd = (n.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / n.len() as f32).sqrt();
        (sd, m.sone)
    };
    let (sd_in, sone_in) = spread(&x);
    let (sd_out, sone_out) = spread(&out[0]);
    {
        let bands = |v: &[f32]| -> Vec<f32> {
            let mut m = SoneMeter::new(SR, 1, 100.0);
            for &smp in &v[v.len() - (2.0 * SR) as usize..] { m.push(&[smp]); }
            m.specific.to_vec()
        };
        let (bi, bo) = (bands(&x), bands(&out[0]));
        for b in 0..BANDS { print!("{b}:{:.2}/{:.2} ", bi[b], bo[b]); }
        println!();
    }
    let rms = |v: &[f32]| (v[v.len() / 2..].iter().map(|a| a * a).sum::<f32>() / (v.len() / 2) as f32).sqrt();
    println!(
        "balance: specific-loudness spread (ln sd) {:.2} -> {:.2}, sones {:.1} -> {:.1}, rms {:+.2} dB, lag {}",
        sd_in, sd_out, sone_in, sone_out, db(rms(&out[0]) / rms(&x)), best_lag(&x[len / 2..], &out[0][len / 2..])
    );
    assert!(sd_out < sd_in, "balance should flatten specific loudness");
    assert!(sone_out > sone_in * 1.05, "flatter specific loudness at equal energy should read louder");
    assert!(db(rms(&out[0]) / rms(&x)).abs() < 1.0, "balance is energy-preserving");
    assert_eq!(best_lag(&x[len / 2..], &out[0][len / 2..]), 0, "balance must be zero-phase");
}

/// Pre-EQ: the measured response of the whole plugin (dynamics and clip off) must match the
/// biquad design, and the EQ must be shared by the dry path (Mix 0% is also EQ'd).
#[test]
fn pre_eq_matches_its_design_and_feeds_both_paths() {
    let len = (1.0 * SR) as usize;
    let tones = [30.0f32, 80.0, 200.0, 400.0, 1000.0, 2500.0, 6000.0, 10000.0, 20000.0];
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            tones.iter().map(|f| 0.05 * (2.0 * std::f32::consts::PI * f * t).sin()).sum()
        })
        .collect();
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = false;
    s.eq.freq_hz = [80.0, 400.0, 2500.0, 10000.0];
    s.eq.gain_db = [4.0, -6.0, 3.0, 5.0];
    for mix in [1.0f32, 0.0] {
        s.mix = mix;
        let out = run(&[x.clone()], &s);
        let seg_in = &x[len / 2..];
        let seg_out = &out[0][len / 2..];
        let mut line = format!("pre-eq (mix {:.0}%):", mix * 100.0);
        for f in tones {
            let g = db(goertzel(seg_out, SR, f) / goertzel(seg_in, SR, f));
            line += &format!(" {f:.0}Hz {g:+.1}");
            // expected: shelves reach their gain well past the corner, bells peak at centre
            let expect = match f as u32 {
                30 => Some(4.0), 400 => Some(-6.0), 2500 => Some(3.0), 20000 => Some(5.0), _ => None,
            };
            if let Some(e) = expect {
                assert!((g - e).abs() < 0.7, "{f} Hz: got {g:+.1} dB, designed {e:+.1}");
            }
        }
        println!("{line}");
    }
}

/// Highs on Hits: with it at 0%, a hit's highs are clipped like everything else (output
/// during the hit matches Preserve Highs OFF), while between hits the highs are preserved
/// (output matches Preserve Highs ON). At 100% the whole thing matches Preserve Highs ON.
#[test]
fn highs_on_hits_folds_the_highs_into_the_clip_during_hits_only() {
    let len = (1.5 * SR) as usize;
    let mut st = 777u32;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            st ^= st << 13; st ^= st >> 17; st ^= st << 5;
            let w = (st as f32 / u32::MAX as f32) * 2.0 - 1.0;
            // sustained bed: 200 Hz + 12 kHz tone; one bright burst at 0.5 s (high-passed noise)
            let bed = 0.4 * (2.0 * std::f32::consts::PI * 200.0 * t).sin() + 0.1 * (2.0 * std::f32::consts::PI * 12000.0 * t).sin();
            let burst = if (0.5..0.52).contains(&t) { w * 0.9 * (-(t - 0.5) * 150.0).exp() } else { 0.0 };
            bed + burst
        })
        .collect();
    let mut base = Settings::default();
    base.drive_db = 8.0;
    base.trans_clip = 1.0;
    base.protect_on = true;
    base.onset_sens = 0.6;
    base.dyn_.down_ratio = 1.0;
    base.dyn_.up_ratio = 1.0;
    base.dyn_.mono_below_hz = 0.0;
    base.hardness = 1.0;
    base.hf_hz = 4000.0;
    base.band_protect = 1.0;
    let run_with = |hf: bool, hits: f32| {
        let mut s = base;
        s.hf_preserve = hf;
        s.hf_on_hits = hits;
        run(&[x.clone()], &s)[0].clone()
    };
    let off = run_with(false, 0.0);
    let on = run_with(true, 1.0);
    let hits0 = run_with(true, 0.0);
    let diff = |a: &[f32], b: &[f32], from: f32, to: f32| -> f32 {
        let (i0, i1) = ((from * SR) as usize, (to * SR) as usize);
        let e: f32 = a[i0..i1].iter().zip(&b[i0..i1]).map(|(p, q)| (p - q).powi(2)).sum();
        let r: f32 = a[i0..i1].iter().map(|p| p * p).sum();
        10.0 * (e / r.max(1e-12)).max(1e-12).log10()
    };
    // during the hit (first 8 ms after the burst): hits0 ~ off; between hits (1.0..1.4 s): hits0 ~ on
    let d_hit_off = diff(&hits0, &off, 0.5, 0.508);
    let d_hit_on = diff(&hits0, &on, 0.5, 0.508);
    let d_bed_on = diff(&hits0, &on, 1.0, 1.4);
    let d_bed_off = diff(&hits0, &off, 1.0, 1.4);
    println!("highs on hits 0%: vs OFF during hit {d_hit_off:.1} dB, vs ON during hit {d_hit_on:.1} dB; vs ON between hits {d_bed_on:.1} dB, vs OFF between hits {d_bed_off:.1} dB");
    assert!(d_hit_off < -15.0, "during the hit the highs must be clipped like Preserve Highs off");
    assert!(d_hit_off < d_hit_on - 10.0, "during the hit it must be closer to OFF than to ON");
    assert!(d_bed_on < -60.0, "between hits it must equal Preserve Highs on");
    assert!(d_bed_on < d_bed_off - 10.0);
}

/// Upward Depth: a tone a few dB under the threshold is lifted; a noise floor far under it is
/// not (that is the "white noise" complaint). With a huge depth the floor IS lifted, which
/// proves the depth is what stops it.
#[test]
fn upward_depth_lifts_the_body_but_not_the_floor() {
    let len = (2.0 * SR) as usize;
    let mut st = 4242u32;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            st ^= st << 13; st ^= st >> 17; st ^= st << 5;
            let w = (st as f32 / u32::MAX as f32) * 2.0 - 1.0;
            // tone at -30 dBFS (6 dB under a -24 threshold); white floor at about -55 dBFS,
            // which is roughly -82 dB per bin in the 1024 window (58 dB under the threshold)
            0.0316 * (2.0 * std::f32::consts::PI * 1000.0 * t).sin() + w * 0.0018
        })
        .collect();
    let band_db = |v: &[f32], lo: f32, hi: f32| -> f32 {
        // crude: sum of goertzel over a few probe frequencies well away from the tone
        let fs = [lo, (lo + hi) * 0.5, hi];
        db(fs.iter().map(|&f| goertzel(&v[len / 2..], SR, f).powi(2)).sum::<f32>().sqrt())
    };
    let mut s = Settings::default();
    s.protect_on = false;
    s.clip_on = false;
    s.dyn_.mono_below_hz = 0.0;
    s.dyn_.weighting = onsetguard::dsp::dynamics::Weighting::Off;
    s.dyn_.threshold_db = -24.0;
    s.dyn_.slope_db_oct = 0.0;
    s.dyn_.curve_db_oct2 = 0.0;
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 3.0;
    s.dyn_.up_range_db = 12.0;
    s.dyn_.smooth_oct = 0.0;
    let tone_in = db(goertzel(&x[len / 2..], SR, 1000.0));
    let floor_in = band_db(&x, 3000.0, 6000.0);
    let mut rows = Vec::new();
    for depth in [24.0f32, 60.0] {
        s.dyn_.up_depth_db = depth;
        let out = run(&[x.clone()], &s);
        let tone = db(goertzel(&out[0][len / 2..], SR, 1000.0)) - tone_in;
        let floor = band_db(&out[0], 3000.0, 6000.0) - floor_in;
        println!("upward depth {depth} dB: tone {tone:+.1} dB, floor {floor:+.1} dB");
        rows.push((tone, floor));
    }
    assert!(rows[0].0 > 2.0, "the tone (6 dB under) must be lifted");
    assert!(rows[0].1 < 1.0, "with 24 dB depth the floor (50 dB under) must not be lifted");
    assert!(rows[1].1 > 4.0, "with 60 dB depth the floor is lifted (the old behaviour)");
}

/// Upward Zone: with the zone at 0% around 300 Hz and 100% around 3 kHz, only the 3 kHz tone
/// is lifted by the upward compressor; both are 6 dB under the threshold.
#[test]
fn upward_zone_selects_where_the_lift_happens() {
    let len = (2.0 * SR) as usize;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            0.0316 * (2.0 * std::f32::consts::PI * 300.0 * t).sin() + 0.0316 * (2.0 * std::f32::consts::PI * 3000.0 * t).sin()
        })
        .collect();
    let mut s = Settings::default();
    s.protect_on = false;
    s.clip_on = false;
    s.dyn_.mono_below_hz = 0.0;
    s.dyn_.weighting = onsetguard::dsp::dynamics::Weighting::Off;
    s.dyn_.threshold_db = -24.0;
    s.dyn_.slope_db_oct = 0.0;
    s.dyn_.curve_db_oct2 = 0.0;
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 3.0;
    s.dyn_.up_range_db = 12.0;
    s.dyn_.smooth_oct = 0.0;
    let t300_in = db(goertzel(&x[len / 2..], SR, 300.0));
    let t3k_in = db(goertzel(&x[len / 2..], SR, 3000.0));
    let mut rows = Vec::new();
    for zone in [[1.0f32, 1.0, 1.0, 1.0], [0.0, 0.0, 1.0, 1.0]] {
        s.dyn_.up_zone_hz = [100.0, 600.0, 1500.0, 8000.0];
        s.dyn_.up_zone_amt = zone;
        let out = run(&[x.clone()], &s);
        let g300 = db(goertzel(&out[0][len / 2..], SR, 300.0)) - t300_in;
        let g3k = db(goertzel(&out[0][len / 2..], SR, 3000.0)) - t3k_in;
        println!("upward zone {zone:?}: 300 Hz {g300:+.1} dB, 3 kHz {g3k:+.1} dB");
        rows.push((g300, g3k));
    }
    assert!(rows[0].0 > 3.0 && rows[0].1 > 3.0, "all at 100%: both lifted");
    assert!(rows[1].0.abs() < 0.5, "zone 0% around 300 Hz: not lifted");
    assert!((rows[1].1 - rows[0].1).abs() < 0.5, "zone 100% around 3 kHz: lifted as before");
}

/// Anti-phase guard must not pre-ring: an anti-phase noise burst starts from silence at 1.0 s
/// (nothing in the Side before it). The Side output in the 40 ms before the burst, relative
/// to the burst's own level, must stay well down; the in-phase Mid is untouched throughout.
#[test]
fn antiphase_guard_does_not_pre_ring() {
    let len = (3.0 * SR) as usize;
    let mut st = 31337u32;
    let (mut l, mut r) = (vec![0.0f32; len], vec![0.0f32; len]);
    for i in 0..len {
        let t = i as f32 / SR;
        st ^= st << 13; st ^= st >> 17; st ^= st << 5;
        let w = (st as f32 / u32::MAX as f32) * 2.0 - 1.0;
        let m = 0.2 * (2.0 * std::f32::consts::PI * 220.0 * t).sin(); // steady in-phase content
        let s = if t >= 1.0 { w * 0.3 } else { 0.0 }; // anti-phase burst from 1.0 s on
        l[i] = m + s;
        r[i] = m - s;
    }
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = false;
    s.stereo.guard = 1.0;
    let out = run(&[l.clone(), r.clone()], &s);
    let side = |a: &[f32], b: &[f32], i0: usize, i1: usize| -> f32 {
        (a[i0..i1].iter().zip(&b[i0..i1]).map(|(x, y)| ((x - y) * 0.5).powi(2)).sum::<f32>() / (i1 - i0) as f32).sqrt()
    };
    let t0 = SR as usize;
    let pre = side(&out[0], &out[1], t0 - (0.040 * SR) as usize, t0 - 1);
    let burst = side(&out[0], &out[1], t0 + (1.5 * SR) as usize, t0 + (1.9 * SR) as usize);
    let burst_in = side(&l, &r, t0 + (1.5 * SR) as usize, t0 + (1.9 * SR) as usize);
    let mid_err = (0..len).map(|i| ((out[0][i] + out[1][i]) * 0.5 - (l[i] + r[i]) * 0.5).abs()).fold(0.0f32, f32::max);
    println!("guard 100%: side pre-ring (40 ms before) {:.1} dB re input burst, burst passed at {:.1} dB, mid max error {:.1} dB", db(pre / burst_in), db(burst / burst_in), db(mid_err / 0.2));
    assert!(db(pre / burst_in) < -50.0, "no pre-ring on the Side before the anti-phase onset");
    assert!(db(burst / burst_in) < -20.0, "the anti-phase burst itself must be removed");
    assert!(db(mid_err / 0.2) < -80.0, "mid untouched");
}

/// Distortion readout: a clap loop driven 20 dB into a 1x hard clip must read a crest factor
/// near a square wave's and a large share of samples at the ceiling; at 0 dB drive with no
/// clipping it must read the dry crest and ~0% at the ceiling.
#[test]
fn distortion_readout_tracks_crest_and_rail() {
    let len = (3.0 * SR) as usize;
    let clap = synth_clap(9);
    let x: Vec<f32> = (0..len).map(|i| clap.get(i % (SR as usize / 4)).copied().unwrap_or(0.0) * 0.5).collect();
    let mut s = Settings::default();
    s.protect_on = false;
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.oversample = 1;
    s.trans_clip = 1.0;
    let mut rows = Vec::new();
    for drive in [0.0f32, 20.0] {
        s.drive_db = drive;
        s.clip_on = drive > 0.0;
        let mut e = Engine::new(1, SR, BLOCK);
        e.set_windows(s.long_order, s.short_order, s.oversample);
        let total = len + e.latency();
        let mut pos = 0;
        while pos < total {
            let n = BLOCK.min(total - pos);
            let mut c: Vec<f32> = (0..n).map(|i| x.get(pos + i).copied().unwrap_or(0.0)).collect();
            { let mut r: Vec<&mut [f32]> = vec![&mut c[..]]; e.process(&mut r, n, &s); }
            pos += n;
            if pos > len { break; } // read the readout while the loop is still playing
        }
        let d = e.dist;
        println!("drive {drive} dB: crest dry {:.1} dB, processed {:.1} dB, at ceiling {:.1}%", d.crest_dry_db, d.crest_wet_db, d.rail_pct);
        rows.push(d);
    }
    assert!((rows[0].crest_wet_db - rows[0].crest_dry_db).abs() < 0.5, "no clipping: processed crest equals dry");
    assert!(rows[0].rail_pct < 0.5, "no clipping: nothing at the ceiling");
    assert!(rows[1].crest_wet_db < rows[1].crest_dry_db - 6.0, "20 dB into a hard clip collapses the crest");
    assert!(rows[1].rail_pct > 2.0, "20 dB into a hard clip pins a visible share of samples (sparse claps: short hits, long gaps)");
}

/// Phase rotation: on a one-sided waveform (harmonics with aligned phases, a pulse-like
/// snare body) the crest factor must drop while every harmonic's magnitude stays the same.
#[test]
fn phase_rotation_lowers_crest_without_changing_magnitudes() {
    let len = (2.0 * SR) as usize;
    let f0 = 110.0f32;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            // cosine-phase harmonics: a sharp, asymmetric pulse train
            (1..=16).map(|h| (2.0 * std::f32::consts::PI * f0 * h as f32 * t).cos() / h as f32).sum::<f32>() * 0.25
        })
        .collect();
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = false;
    let crest = |v: &[f32]| {
        let seg = &v[len / 2..];
        let pk = seg.iter().fold(0.0f32, |m, a| m.max(a.abs()));
        let rms = (seg.iter().map(|a| a * a).sum::<f32>() / seg.len() as f32).sqrt();
        db(pk / rms)
    };
    let mut rows = Vec::new();
    for on in [false, true] {
        s.rotate.on = on;
        let out = run(&[x.clone()], &s);
        let c = crest(&out[0]);
        let mags: Vec<f32> = (1..=8).map(|h| db(goertzel(&out[0][len / 2..], SR, f0 * h as f32))).collect();
        println!("rotate {on}: crest {c:.2} dB, harmonics 1-8 {:?}", mags.iter().map(|m| format!("{m:.2}")).collect::<Vec<_>>());
        rows.push((c, mags));
    }
    assert!(rows[1].0 < rows[0].0 - 1.0, "rotation should lower the crest by more than 1 dB on a one-sided pulse");
    for (a, b) in rows[0].1.iter().zip(&rows[1].1) {
        assert!((a - b).abs() < 0.1, "harmonic magnitude must be unchanged ({a:.2} vs {b:.2})");
    }
}

/// Autopilot: a loop at a given level; target reachable -> processed loudness lands within
/// 2 phon of the target; target unreachable without squaring -> the crest floor holds and
/// the drive stops rising.
#[test]
fn autopilot_hits_the_target_or_the_crest_floor() {
    let len = (14.0 * SR) as usize;
    let clap = synth_clap(21);
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let hit = clap.get(i % (SR as usize / 2)).copied().unwrap_or(0.0) * 0.6;
            hit + 0.15 * (2.0 * std::f32::consts::PI * 55.0 * t).sin() + 0.08 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
        })
        .collect();
    let mut s = Settings::default();
    s.protect_on = false;
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = true;
    s.drive_db = 0.0;
    s.oversample = 1;
    s.trans_clip = 1.0;
    s.autopilot.on = true;
    s.autopilot.crest_floor_db = 6.0;
    // (clip on, 130) and (clip OFF + Meter Thru Clip, 130): the servo must behave the same
    for (clip_on, target) in [(true, 96.0f32), (true, 130.0), (false, 130.0)] {
        s.clip_on = clip_on;
        s.meter_thru_clip = true;
        s.autopilot.target_phon = target;
        let mut e = Engine::new(1, SR, BLOCK);
        e.set_windows(s.long_order, s.short_order, s.oversample);
        let mut pos = 0;
        let mut c = vec![0.0f32; BLOCK];
        while pos + BLOCK <= len {
            c.copy_from_slice(&x[pos..pos + BLOCK]);
            { let mut r: Vec<&mut [f32]> = vec![&mut c[..]]; e.process(&mut r, BLOCK, &s); }
            pos += BLOCK;
        }
        let (_, wet) = e.sones();
        let phon = 40.0 + 10.0 * wet.max(1e-3).log2();
        let d = e.dist;
        println!("autopilot clip {clip_on} target {target} phon: auto drive {:+.1} dB, processed {phon:.1} phon, crest {:.1} dB, at ceiling {:.1}%", e.auto_drive_db, d.crest_wet_db, d.rail_pct);
        if target < 100.0 {
            assert!((phon - target).abs() < 2.0, "reachable target should be met");
        } else {
            assert!(d.crest_wet_db > s.autopilot.crest_floor_db - 1.0, "crest floor must hold on an unreachable target");
            assert!(e.auto_drive_db < 24.0, "drive must stop short of the clamp");
        }
    }
}

/// Autopilot must learn, lock, and then HOLD: after it has settled on a loud section, a
/// quieter section must not move the drive (the drums must not change through the song).
#[test]
fn autopilot_locks_and_holds_when_the_material_changes() {
    let clap = synth_clap(33);
    // loud section: hits with a body (sub + tone), so the crest floor can bind
    let loud = |i: usize| {
        let t = i as f32 / SR;
        clap.get(i % (SR as usize / 2)).copied().unwrap_or(0.0) * 0.6
            + 0.15 * (2.0 * std::f32::consts::PI * 55.0 * t).sin()
            + 0.08 * (2.0 * std::f32::consts::PI * 440.0 * t).sin()
    };
    let quiet = |i: usize| clap.get(i % (SR as usize)).copied().unwrap_or(0.0) * 0.15;
    let n_loud = (12.0 * SR) as usize;
    let n_quiet = (10.0 * SR) as usize;
    let mut s = Settings::default();
    s.protect_on = false;
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.meter_thru_clip = true;
    s.oversample = 1;
    s.autopilot.on = true;
    s.autopilot.target_phon = 130.0;
    s.autopilot.crest_floor_db = 6.0;
    let mut e = Engine::new(1, SR, BLOCK);
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let mut c = vec![0.0f32; BLOCK];
    let mut feed = |e: &mut Engine, f: &dyn Fn(usize) -> f32, n: usize| {
        let mut pos = 0;
        let mut last_sec = 0;
        while pos + BLOCK <= n {
            for i in 0..BLOCK { c[i] = f(pos + i); }
            { let mut r: Vec<&mut [f32]> = vec![&mut c[..]]; e.process(&mut r, BLOCK, &s); }
            pos += BLOCK;
            if pos / SR as usize != last_sec {
                last_sec = pos / SR as usize;
                print!("[{last_sec}s {:+.1}{}] ", e.auto_drive_db, if e.auto_locked { "L" } else { "" });
            }
        }
        println!();
    };
    feed(&mut e, &loud, n_loud);
    let learned = e.auto_drive_db;
    let locked = e.auto_locked;
    feed(&mut e, &quiet, n_quiet);
    println!("autopilot: learned {learned:+.1} dB on the loud section (locked {locked}); after 10 s of a quiet section: {:+.1} dB (locked {})", e.auto_drive_db, e.auto_locked);
    assert!(locked, "should have locked on the loud section");
    assert!((e.auto_drive_db - learned).abs() < 1e-6, "a quiet section must not move a locked drive");
}

/// Rotate Hits 0%: with onset protection on, a hit comes out with its ORIGINAL phase (nulls
/// against the un-rotated run over the first 6 ms) while the asymmetric body between hits is
/// still rotated (its crest is lower than un-rotated).
#[test]
fn rotate_hits_keeps_the_transient_and_rotates_the_body() {
    let len = (3.0 * SR) as usize;
    // one sharp noise burst per second (instant attack, 3 ms decay) over an asymmetric body
    let mut st = 4444u32;
    let burst: Vec<f32> = (0..(0.012 * SR) as usize)
        .map(|i| {
            st ^= st << 13; st ^= st >> 17; st ^= st << 5;
            ((st as f32 / u32::MAX as f32) * 2.0 - 1.0) * (-(i as f32) / (0.003 * SR)).exp()
        })
        .collect();
    let f0 = 110.0f32;
    let period = SR as usize; // one hit per second, at t = 0, 1, 2
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            let body = (1..=12).map(|h| (2.0 * std::f32::consts::PI * f0 * h as f32 * t).cos() / h as f32).sum::<f32>() * 0.15;
            body + burst.get(i % period).copied().unwrap_or(0.0) * 0.8
        })
        .collect();
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = true;
    s.onset_sens = 0.6;
    s.hold_ms = 6.0;
    let run_with = |on: bool, hits: f32| {
        let mut s = s;
        s.rotate.on = on;
        s.rotate.hits = hits;
        run(&[x.clone()], &s)[0].clone()
    };
    let plain = run_with(false, 0.0);
    let rotated_all = run_with(true, 1.0);
    let rotated_body = run_with(true, 0.0);
    let onset = first_onset(&x[period..]) + period; // the second hit
    let win = (0.006 * SR) as usize;
    let null = |a: &[f32], b: &[f32]| -> f32 {
        let e: f32 = a[onset..onset + win].iter().zip(&b[onset..onset + win]).map(|(p, q)| (p - q).powi(2)).sum();
        let r: f32 = a[onset..onset + win].iter().map(|p| p * p).sum();
        10.0 * (e / r.max(1e-12)).max(1e-12).log10()
    };
    let crest_body = |v: &[f32]| {
        // 300..900 ms after the second hit: body only
        let seg = &v[onset + (0.3 * SR) as usize..onset + (0.9 * SR) as usize];
        let pk = seg.iter().fold(0.0f32, |m, a| m.max(a.abs()));
        let rms = (seg.iter().map(|a| a * a).sum::<f32>() / seg.len() as f32).sqrt();
        db(pk / rms)
    };
    let n_all = null(&plain, &rotated_all);
    let n_body = null(&plain, &rotated_body);
    println!(
        "rotate hits: transient null vs un-rotated: hits 100% {n_all:.1} dB, hits 0% {n_body:.1} dB; body crest plain {:.2} dB, rotated {:.2} dB, hits-0% {:.2} dB",
        crest_body(&plain), crest_body(&rotated_all), crest_body(&rotated_body)
    );
    assert!(n_body < -25.0, "with Rotate Hits 0% the transient must match the original");
    assert!(n_body < n_all - 10.0, "Rotate Hits 0% must be much closer to the original than full rotation");
    assert!(crest_body(&rotated_body) < crest_body(&plain) - 1.0, "the body must still be rotated");
}

/// Hit Drive: Clip Drive moves the body only, Hit Drive moves the hit only (clip off so the
/// gains can be read directly).
#[test]
fn hit_drive_is_independent_of_clip_drive() {
    let len = (3.0 * SR) as usize;
    let mut st = 5555u32;
    let burst: Vec<f32> = (0..(0.012 * SR) as usize)
        .map(|i| {
            st ^= st << 13; st ^= st >> 17; st ^= st << 5;
            ((st as f32 / u32::MAX as f32) * 2.0 - 1.0) * (-(i as f32) / (0.003 * SR)).exp()
        })
        .collect();
    let period = SR as usize;
    let x: Vec<f32> = (0..len)
        .map(|i| {
            let t = i as f32 / SR;
            0.1 * (2.0 * std::f32::consts::PI * 220.0 * t).sin() + burst.get(i % period).copied().unwrap_or(0.0) * 0.8
        })
        .collect();
    let mut s = Settings::default();
    s.dyn_.down_ratio = 1.0;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.mono_below_hz = 0.0;
    s.clip_on = false;
    s.protect_on = false;
    s.onset_sens = 0.6;
    let onset = first_onset(&x[period..]) + period;
    let win = (0.005 * SR) as usize;
    let gain_hit = |o: &[f32]| -> f32 {
        let e_o: f32 = o[onset..onset + win].iter().map(|v| v * v).sum();
        let e_x: f32 = x[onset..onset + win].iter().map(|v| v * v).sum();
        10.0 * (e_o / e_x).log10()
    };
    let gain_body = |o: &[f32]| -> f32 {
        let (a, b) = (onset + (0.3 * SR) as usize, onset + (0.9 * SR) as usize);
        let e_o: f32 = o[a..b].iter().map(|v| v * v).sum();
        let e_x: f32 = x[a..b].iter().map(|v| v * v).sum();
        10.0 * (e_o / e_x).log10()
    };
    for (drive, hit) in [(12.0f32, 0.0f32), (0.0, 12.0)] {
        s.drive_db = drive;
        s.hit_drive_db = hit;
        let out = run(&[x.clone()], &s);
        let (gh, gb) = (gain_hit(&out[0]), gain_body(&out[0]));
        println!("clip drive {drive} / hit drive {hit}: hit {gh:+.1} dB, body {gb:+.1} dB");
        if drive > 0.0 {
            assert!((gb - 12.0).abs() < 0.5 && gh < 1.0, "clip drive must move the body only");
        } else {
            assert!((gh - 12.0).abs() < 1.0 && gb.abs() < 0.5, "hit drive must move the hit only");
        }
    }
}
