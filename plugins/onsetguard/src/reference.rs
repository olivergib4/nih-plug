//! Reference track analysis: decode an audio file, find its loudest 24 s, detect the backbeat
//! claps (2-8 kHz flux hits that sit on a regular half-bar grid), and run the same analysis the
//! plugin runs live. Offline, allocating, meant for a background thread.
//!
//! The detector is a port of the offline scripts the reference numbers were measured with
//! (claptest.py `claps`, gridclaps.py `grid_claps`): STFT 1024/256 Hann, positive spectral flux of
//! log1p(1e3·|Z|) over 2-8 kHz, threshold median + 1.5·std, peaks at least 300 ms apart, hits
//! whose 30-90 Hz band jumps by >= 3 dB are kicks and dropped; then the half-bar period
//! (1.2-2.4 s) and phase that collect the most hits, keeping hits within 30 ms of that grid.

use crate::analysis::{analyze_buffer, HitKind, Report};
use crate::dsp::butter::{butter_bandpass, filtfilt};
use realfft::num_complex::Complex32;
use realfft::RealFftPlanner;
use serde::{Deserialize, Serialize};

pub const WINDOW_S: f32 = 24.0;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReferenceData {
    pub name: String,
    pub sample_rate: f32,
    /// Start of the analysed window in the file, seconds.
    pub window_start_s: f32,
    pub claps: usize,
    /// Half-bar period found, seconds (0 if no grid).
    pub period_s: f32,
    pub report: Report,
}

/// Decode any file symphonia understands into stereo f32 (mono duplicated).
pub fn decode(path: &std::path::Path) -> Result<(Vec<f32>, Vec<f32>, f32), String> {
    use symphonia::core::codecs::DecoderOptions;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;
    use symphonia::core::probe::Hint;

    let file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let probed = symphonia::default::get_probe()
        .format(&hint, mss, &FormatOptions::default(), &MetadataOptions::default())
        .map_err(|e| format!("unsupported file: {e}"))?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|t| t.codec_params.codec != symphonia::core::codecs::CODEC_TYPE_NULL)
        .ok_or("no audio track")?;
    let track_id = track.id;
    let sr = track.codec_params.sample_rate.ok_or("unknown sample rate")? as f32;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder: {e}"))?;
    let mut l = Vec::new();
    let mut r = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(symphonia::core::errors::Error::ResetRequired) => break,
            Err(e) => {
                if l.is_empty() {
                    return Err(format!("read: {e}"));
                }
                break;
            }
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(d) => d,
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decode: {e}")),
        };
        let spec = *decoded.spec();
        let nch = spec.channels.count();
        let mut buf = symphonia::core::audio::SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buf.copy_interleaved_ref(decoded);
        let s = buf.samples();
        let n = s.len() / nch.max(1);
        for i in 0..n {
            l.push(s[i * nch]);
            r.push(s[i * nch + if nch > 1 { 1 } else { 0 }]);
        }
    }
    if l.is_empty() {
        return Err("no audio decoded".into());
    }
    Ok((l, r, sr))
}

/// Loudest `secs` window (mono mean square, 1 s steps): returns the start sample.
pub fn loudest_window(l: &[f32], r: &[f32], sr: f32, secs: f32) -> usize {
    let n = (secs * sr) as usize;
    if l.len() <= n {
        return 0;
    }
    let step = sr as usize;
    let mut best = -1.0f64;
    let mut bi = 0;
    // prefix sums of the mono power for O(1) windows
    let mut ps = Vec::with_capacity(l.len() + 1);
    ps.push(0.0f64);
    for i in 0..l.len() {
        let m = 0.5 * (l[i] + r[i]);
        ps.push(ps[i] + (m * m) as f64);
    }
    let mut i = 0;
    while i + n <= l.len() {
        let v = ps[i + n] - ps[i];
        if v > best {
            best = v;
            bi = i;
        }
        i += step;
    }
    bi
}

const WIN: usize = 1024;
const HOP: usize = 256;

/// Positive spectral flux over 2-8 kHz per STFT hop (frame k vs k+1), scipy-compatible framing.
pub fn flux_curve(mono: &[f32], sr: f32) -> Vec<f32> {
    let nb = WIN / 2 + 1;
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(WIN);
    let window: Vec<f32> = (0..WIN).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / WIN as f32).cos()).collect();
    let wsum: f32 = window.iter().sum();
    // scipy stft: zero boundary padding of WIN/2 on both sides, frames every HOP
    let pad = WIN / 2;
    let total = mono.len() + 2 * pad;
    let frames = if total >= WIN { (total - WIN) / HOP + 1 } else { 0 };
    let per_bin = sr / WIN as f32;
    let k0 = (2000.0 / per_bin).ceil() as usize;
    let k1 = ((8000.0 / per_bin).floor() as usize).min(nb - 1); // bins with f < 8000
    let mut tb = vec![0.0f32; WIN];
    let mut spec = vec![Complex32::default(); nb];
    let mut scratch = vec![Complex32::default(); fft.get_scratch_len()];
    let mut prev: Vec<f32> = vec![0.0; nb];
    let mut cur: Vec<f32> = vec![0.0; nb];
    let mut flux: Vec<f32> = Vec::with_capacity(frames);
    for f in 0..frames {
        let start = f * HOP;
        for i in 0..WIN {
            let t = start + i;
            let x = if t >= pad && t - pad < mono.len() { mono[t - pad] } else { 0.0 };
            tb[i] = x * window[i];
        }
        fft.process_with_scratch(&mut tb, &mut spec, &mut scratch).ok();
        for k in 0..nb {
            cur[k] = (1e3 * spec[k].norm() / wsum).ln_1p();
        }
        if f > 0 {
            let mut s = 0.0;
            for k in k0..k1 {
                let d = cur[k] - prev[k];
                if d > 0.0 {
                    s += d;
                }
            }
            flux.push(s);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    flux
}

/// Flux hits in seconds (relative to `mono`), kicks excluded.
pub fn flux_hits(mono: &[f32], sr: f32) -> Vec<f32> {
    let flux = flux_curve(mono, sr);
    if flux.len() < 3 {
        return Vec::new();
    }
    let mean = flux.iter().sum::<f32>() / flux.len() as f32;
    let std = (flux.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / flux.len() as f32).sqrt();
    let mut sorted = flux.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = if sorted.len() % 2 == 1 { sorted[sorted.len() / 2] } else { 0.5 * (sorted[sorted.len() / 2 - 1] + sorted[sorted.len() / 2]) };
    let thr = med + 1.5 * std;
    // peaks; flux[j] compares frames j and j+1; python time index = (j + 2) * HOP / sr
    let mut cand: Vec<(f32, f32)> = Vec::new();
    for j in 1..flux.len() - 1 {
        if flux[j] > flux[j - 1] && flux[j] >= flux[j + 1] && flux[j] > thr {
            cand.push((flux[j], (j + 2) as f32 * HOP as f32 / sr));
        }
    }
    cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut out: Vec<f32> = Vec::new();
    for (_, t) in cand {
        if out.iter().all(|o| (t - o).abs() > 0.3) {
            out.push(t);
        }
    }
    out.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let dur = mono.len() as f32 / sr;
    out.retain(|&t| t > 0.1 && t < dur - 0.5);
    // kick exclusion: 30-90 Hz band energy 0-60 ms vs -60..-10 ms, zero-phase like the offline
    // script (a causal filter at 30 Hz delays the band by tens of ms and would move a level step
    // at the hit into the "after" window)
    let secs = butter_bandpass(4, 30.0, 90.0, sr as f64);
    let sub = filtfilt(&secs, mono, (0.5 * sr) as usize);
    let e = |a: usize, b: usize| -> f32 {
        if b <= a || b > sub.len() {
            return 0.0;
        }
        (sub[a..b].iter().map(|v| v * v).sum::<f32>() / (b - a) as f32).sqrt().max(1e-12)
    };
    let ms = |m: f32| (m * sr / 1000.0) as usize;
    out.retain(|&t| {
        let i = (t * sr) as usize;
        if i < ms(60.0) {
            return false;
        }
        let after = e(i, i + ms(60.0));
        let before = e(i - ms(60.0), i - ms(10.0));
        20.0 * (after / before).log10() < 3.0
    });
    out
}

/// Half-bar grid: period in [1.2, 2.4) s (1 ms steps) and phase (20 ms bins) that collect the
/// most hits; returns the hits within 30 ms of that grid and the period.
pub fn grid_claps(hits: &[f32]) -> (Vec<f32>, f32) {
    if hits.len() < 3 {
        return (hits.to_vec(), 0.0);
    }
    let mut best = (0usize, 0.0f32, 0.0f32);
    let mut p = 1.2f32;
    while p < 2.4 {
        let nbins = (p / 0.02) as usize + 1;
        let mut h = vec![0usize; nbins];
        for &t in hits {
            let ph = t.rem_euclid(p);
            let k = ((ph / p) * nbins as f32) as usize;
            h[k.min(nbins - 1)] += 1;
        }
        let (k, &c) = h.iter().enumerate().max_by_key(|(_, &c)| c).unwrap();
        if c > best.0 {
            let w = p / nbins as f32;
            best = (c, p, (k as f32 + 0.5) * w);
        }
        p += 0.001;
    }
    let (_, period, phi) = best;
    let on: Vec<f32> = hits
        .iter()
        .cloned()
        .filter(|&t| {
            let d = ((t - phi + period / 2.0).rem_euclid(period)) - period / 2.0;
            d.abs() < 0.03
        })
        .collect();
    (on, period)
}

/// Full reference analysis of a decoded stereo buffer.
pub fn analyze(name: &str, l: &[f32], r: &[f32], sr: f32) -> Result<ReferenceData, String> {
    let n = (WINDOW_S * sr) as usize;
    let start = loudest_window(l, r, sr, WINDOW_S);
    let end = (start + n).min(l.len());
    // pre-roll so the filters are settled at the window start
    let pre = (0.5 * sr) as usize;
    let a = start.saturating_sub(pre);
    let lw: Vec<f32> = l[a..end].iter().map(|v| v.clamp(-1.0, 1.0)).collect();
    let rw: Vec<f32> = r[a..end].iter().map(|v| v.clamp(-1.0, 1.0)).collect();
    let off = start - a;
    let mono: Vec<f32> = lw[off..].iter().zip(&rw[off..]).map(|(a, b)| 0.5 * (a + b)).collect();
    let hits = flux_hits(&mono, sr);
    let (claps, period) = grid_claps(&hits);
    if claps.len() < 2 {
        return Err(format!("found only {} backbeat hits in the loudest {WINDOW_S:.0} s", claps.len()));
    }
    let mut list: Vec<(usize, HitKind)> = Vec::new();
    for &t in &claps {
        list.push((off + (t * sr) as usize, HitKind::Clap));
    }
    for w in claps.windows(2) {
        list.push((off + ((w[0] + w[1]) * 0.5 * sr) as usize, HitKind::Other));
    }
    list.sort_by_key(|h| h.0);
    let span = (list.iter().filter(|h| h.1 == HitKind::Clap).map(|h| h.0).min().unwrap(), list.iter().filter(|h| h.1 == HitKind::Clap).map(|h| h.0).max().unwrap());
    let report = analyze_buffer(&lw, &rw, sr, &list, Some(span));
    Ok(ReferenceData { name: name.to_string(), sample_rate: sr, window_start_s: start as f32 / sr, claps: claps.len(), period_s: period, report })
}

pub fn analyze_file(path: &std::path::Path) -> Result<ReferenceData, String> {
    let (l, r, sr) = decode(path)?;
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("reference").to_string();
    analyze(&name, &l, &r, sr)
}
