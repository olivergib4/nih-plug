//! Offline loudness readout for raw f32le interleaved stereo files (44.1 kHz):
//! `cargo run --release --example measure -- a.raw b.raw ...`
//! Prints, per file: mean sone / phon (stereo), mono-sum sone, peak, crest, % at ceiling,
//! and the mean specific loudness per Bark band.
use onsetguard::dsp::sone::{SoneMeter, BANDS, HOP};

fn main() {
    let sr = 44100.0;
    let refl = 100.0;
    let onsets: Vec<f32> = std::env::var("ONSETS")
        .ok()
        .map(|p| std::fs::read_to_string(p).unwrap().lines().filter_map(|l| l.trim().parse().ok()).collect())
        .unwrap_or_default();
    for path in std::env::args().skip(1) {
        let bytes = std::fs::read(&path).expect("read");
        let n = bytes.len() / 8;
        let mut st = SoneMeter::new(sr, 2, refl);
        let mut mo = SoneMeter::new(sr, 1, refl);
        let mut ph = SoneMeter::new(sr, 2, refl);
        ph.set_band(300.0, 8000.0);
        let mut lp = SoneMeter::new(sr, 2, refl);
        lp.set_band(120.0, 15000.0);
        let (mut sum_p, mut sum_l) = (0.0f64, 0.0f64);
        let (mut sum_s, mut sum_m, mut cnt) = (0.0f64, 0.0f64, 0usize);
        let mut spec = [0.0f64; BANDS];
        let mut peak = 0.0f32;
        let mut sq = 0.0f64;
        let mut rail = 0usize;
        let mut bands_ok = 0usize;
        let mut trace: Vec<f32> = Vec::new();
        let mut trace_m: Vec<f32> = Vec::new();
        for i in 0..n {
            let l = f32::from_le_bytes(bytes[i * 8..i * 8 + 4].try_into().unwrap());
            let r = f32::from_le_bytes(bytes[i * 8 + 4..i * 8 + 8].try_into().unwrap());
            peak = peak.max(l.abs()).max(r.abs());
            sq += (l * l + r * r) as f64 * 0.5;
            if l.abs() >= 0.999 || r.abs() >= 0.999 {
                rail += 1;
            }
            st.push(&[l, r]);
            mo.push(&[(l + r) * 0.5]);
            ph.push(&[l, r]);
            lp.push(&[l, r]);
            if i % HOP == 0 {
                trace.push(st.sone);
                trace_m.push(mo.sone);
            }
            if i % HOP == 0 && i > sr as usize / 2 {
                sum_s += st.sone as f64;
                sum_m += mo.sone as f64;
                sum_p += ph.sone as f64;
                sum_l += lp.sone as f64;
                cnt += 1;
                for b in 0..BANDS {
                    spec[b] += st.specific[b] as f64;
                }
                bands_ok += 1;
            }
        }
        let ms = sum_s / cnt as f64;
        let mm = sum_m / cnt as f64;
        let rms = (sq / n as f64).sqrt();
        println!(
            "{path}: stereo {:.1} sone ({:.1} phon)  mono {:.1} sone ({:.1} phon)  peak {:.2} dBFS  rms {:.1} dBFS  crest {:.1} dB  at-ceiling {:.1}%",
            ms,
            40.0 + 10.0 * ms.log2(),
            mm,
            40.0 + 10.0 * mm.log2(),
            20.0 * peak.log10(),
            20.0 * rms.log10(),
            20.0 * (peak as f64 / rms).log10(),
            100.0 * rail as f64 / (2 * n) as f64
        );
        if !onsets.is_empty() {
            // sone 30 ms after each hit (meter attack 60 ms, so this is the hit rising) and at the
            // midpoint between hits (body)
            let at = |t: f32, tr: &Vec<f32>| tr[((t * sr) as usize / HOP).min(tr.len() - 1)];
            let mut hit: Vec<f32> = onsets.iter().map(|&o| at(o + 0.03, &trace)).collect();
            let mut hit_m: Vec<f32> = onsets.iter().map(|&o| at(o + 0.03, &trace_m)).collect();
            let mut gap: Vec<f32> = onsets.windows(2).map(|w| at((w[0] + w[1]) * 0.5, &trace)).collect();
            for v in [&mut hit, &mut hit_m, &mut gap] {
                v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            }
            let med = |v: &Vec<f32>| v[v.len() / 2];
            println!(
                "  at hits: stereo {:.1} sone ({:.1} phon)  mono {:.1} sone  | between hits: {:.1} sone ({:.1} phon)",
                med(&hit),
                40.0 + 10.0 * med(&hit).log2(),
                med(&hit_m),
                med(&gap),
                40.0 + 10.0 * med(&gap).log2()
            );
        }
        println!(
            "  translation: phone(300-8k) {:.1} phon  laptop(120-15k) {:.1} phon",
            40.0 + 10.0 * (sum_p / cnt as f64).log2(),
            40.0 + 10.0 * (sum_l / cnt as f64).log2()
        );
        let s: Vec<String> = spec.iter().map(|v| format!("{:.2}", v / bands_ok as f64)).collect();
        println!("  specific: {}", s.join(" "));
    }
}
