//! Offline hit report, the same numbers the plugin shows live:
//! `cargo run --release --example report -- FILE [--hits HITS] [--sr 44100] [--span A B]`
//! FILE: .wav (any bit depth, mono or stereo) or raw f32le interleaved stereo.
//! HITS: text file, one hit per line: `seconds [clap|other]` (default clap). Without it, hits are
//! detected with the plugin's onset detector and classified automatically.
//! `--span A B`: accumulate only between A and B seconds (default: first clap to last clap).
use onsetguard::analysis::{analyze_buffer, HitKind, Report, BANDS};
use onsetguard::dsp::onset::OnsetDetector;

fn load(path: &str) -> (Vec<f32>, Vec<f32>, f32) {
    if path.to_ascii_lowercase().ends_with(".wav") {
        let mut rd = hound::WavReader::open(path).expect("open wav");
        let spec = rd.spec();
        let ch = spec.channels as usize;
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => rd.samples::<f32>().map(|s| s.unwrap()).collect(),
            hound::SampleFormat::Int => {
                let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
                rd.samples::<i32>().map(|s| s.unwrap() as f32 * scale).collect()
            }
        };
        let n = samples.len() / ch;
        let l: Vec<f32> = (0..n).map(|i| samples[i * ch]).collect();
        let r: Vec<f32> = (0..n).map(|i| samples[i * ch + if ch > 1 { 1 } else { 0 }]).collect();
        (l, r, spec.sample_rate as f32)
    } else {
        let bytes = std::fs::read(path).expect("read raw");
        let n = bytes.len() / 8;
        let mut l = Vec::with_capacity(n);
        let mut r = Vec::with_capacity(n);
        for i in 0..n {
            l.push(f32::from_le_bytes(bytes[i * 8..i * 8 + 4].try_into().unwrap()));
            r.push(f32::from_le_bytes(bytes[i * 8 + 4..i * 8 + 8].try_into().unwrap()));
        }
        (l, r, 0.0)
    }
}

pub fn print_report(label: &str, r: &Report) {
    println!("=== {label}: {} hits, {} claps", r.hits, r.claps);
    println!(
        "  clap level     : {}   (0-3/3-10/10-25/25-60/60-120/120-200 ms, mid dBFS)",
        r.level_db.iter().map(|v| format!("{v:5.1}")).collect::<Vec<_>>().join(" / ")
    );
    println!(
        "  rail profile   : {}   (0-10/10-20/20-40/40-60/60-90/90-120/120-150 ms, % either ch >= 0.999)",
        r.rail_pct.iter().map(|v| format!("{v:3.0}%")).collect::<Vec<_>>().join(" ")
    );
    println!("  clap 5-10k tail: {:.1} dBFS (25-60 ms)", r.tail_db);
    println!(
        "  drums over rest: {}",
        (0..BANDS.len()).map(|b| format!("{}:{:+.1}", BANDS[b].0, r.band_delta(b))).collect::<Vec<_>>().join(" ")
    );
    println!(
        "  rest level     : {}",
        (0..BANDS.len()).map(|b| format!("{}:{:.1}", BANDS[b].0, r.band_rest_db[b])).collect::<Vec<_>>().join(" ")
    );
    println!("  500-6k spread  : {:.1} dB (p90-p10 of 50 ms windows)   250-1k peakiness {:.1} dB (p99-median of 100 ms windows)", r.spread_db, r.lowmid_peak_db);
    println!(
        "  level (all)    : {}",
        (0..BANDS.len()).map(|b| format!("{}:{:.1}", BANDS[b].0, r.band_all_db[b])).collect::<Vec<_>>().join(" ")
    );
    println!(
        "  pinned         : {:.1}% total   {:.1}% inside hits   {:.1}% between",
        r.pinned_pct, r.pinned_hits_pct, r.pinned_rest_pct
    );
    println!("  loudness       : {:.1} phon mean   {:.1} at claps   {:.1} between", r.phon, r.phon_hits, r.phon_between);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut file = None;
    let mut hits_path = None;
    let mut sr_arg = 44100.0f32;
    let mut span = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--hits" => {
                hits_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--sr" => {
                sr_arg = args[i + 1].parse().unwrap();
                i += 2;
            }
            "--span" => {
                span = Some((args[i + 1].parse::<f32>().unwrap(), args[i + 2].parse::<f32>().unwrap()));
                i += 3;
            }
            a => {
                file = Some(a.to_string());
                i += 1;
            }
        }
    }
    let file = file.expect("FILE");
    let (l, r, sr_file) = load(&file);
    let sr = if sr_file > 0.0 { sr_file } else { sr_arg };
    let hits: Vec<(usize, HitKind)> = match hits_path {
        Some(p) => std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .filter_map(|line| {
                let mut it = line.split_whitespace();
                let t: f32 = it.next()?.parse().ok()?;
                let k = match it.next() {
                    Some("other") => HitKind::Other,
                    Some("auto") => HitKind::Auto,
                    _ => HitKind::Clap,
                };
                Some(((t * sr) as usize, k))
            })
            .collect(),
        None => {
            let mut det = OnsetDetector::new(sr);
            let mut out = Vec::new();
            for i in 0..l.len() {
                if let Some(t) = det.push(0.5 * (l[i] + r[i]), 0.5) {
                    out.push((t, HitKind::Auto));
                }
            }
            out
        }
    };
    let span = span.map(|(a, b)| ((a * sr) as usize, (b * sr) as usize));
    let rep = analyze_buffer(&l, &r, sr, &hits, span);
    print_report(&file, &rep);
}
