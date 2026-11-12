//! Print the six simple-view gauges for a mix against a reference, as the plugin would show them:
//! `cargo run --release --example guards -- MIX REFERENCE`
use onsetguard::guard::gauges;
use onsetguard::reference::analyze_file;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let live = analyze_file(std::path::Path::new(&a[1])).expect("mix");
    let reference = analyze_file(std::path::Path::new(&a[2])).expect("reference");
    println!("{}  vs  {}", live.name, reference.name);
    for g in gauges(&live.report, Some(&reference.report)) {
        let w = 40usize;
        let mut bar: Vec<char> = vec!['.'; w];
        for k in 0..w {
            let x = (k as f32 + 0.5) / w as f32;
            if x >= g.zone.0 && x <= g.zone.1 {
                bar[k] = '#';
            }
        }
        if !g.pos.is_nan() {
            bar[((g.pos * w as f32) as usize).min(w - 1)] = '|';
        }
        println!("  {:<11} [{}]  {:?}: {}", g.name, bar.iter().collect::<String>(), g.status, g.text);
    }
    let (l, r) = (&live.report, &reference.report);
    println!("  sub crest {:.2} (ref {:.2}), flattened {:.1}/s (ref {:.1}); width between hits 250-1k {:+.1} (ref {:+.1}), 4-10k {:+.1} (ref {:+.1})", l.sub_crest, r.sub_crest, l.sub_flat_per_s, r.sub_flat_per_s, l.width_rest_db[2], r.width_rest_db[2], l.width_rest_db[4], r.width_rest_db[4]);
}
