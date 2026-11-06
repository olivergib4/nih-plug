//! Run the reference analysis on a file: `cargo run --release --example refcheck -- FILE`
fn main() {
    let path = std::env::args().nth(1).expect("FILE");
    match onsetguard::reference::analyze_file(std::path::Path::new(&path)) {
        Ok(r) => {
            println!("{}: sr {} window from {:.1} s, {} claps, half-bar {:.3} s", r.name, r.sample_rate, r.window_start_s, r.claps, r.period_s);
            let rep = r.report;
            println!("  clap level : {}", rep.level_db.iter().map(|v| format!("{v:5.1}")).collect::<Vec<_>>().join(" / "));
            println!("  rail       : {}", rep.rail_pct.iter().map(|v| format!("{v:3.0}%")).collect::<Vec<_>>().join(" "));
            println!("  drums-rest : {}", (0..5).map(|b| format!("{:+.1}", rep.band_delta(b))).collect::<Vec<_>>().join(" "));
            println!("  rest       : {}", rep.band_rest_db.iter().map(|v| format!("{v:.1}")).collect::<Vec<_>>().join(" "));
            println!("  spread {:.1}  tail {:.1}  pinned {:.1}% ({:.1} in / {:.1} between)  phon {:.1} / {:.1} / {:.1}", rep.spread_db, rep.tail_db, rep.pinned_pct, rep.pinned_hits_pct, rep.pinned_rest_pct, rep.phon, rep.phon_hits, rep.phon_between);
        }
        Err(e) => println!("error: {e}"),
    }
}
