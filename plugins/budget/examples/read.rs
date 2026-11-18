//! Reads raw interleaved stereo f32 files and prints what a Budget meter would show.
//! Usage: read <role: drums|growls|sub> <main.f32> [side.f32]
use budget::meter::{Meter, Role};
fn load(p: &str) -> (Vec<f32>, Vec<f32>) {
    let b = std::fs::read(p).unwrap();
    let v: Vec<f32> = b.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    (v.iter().step_by(2).cloned().collect(), v.iter().skip(1).step_by(2).cloned().collect())
}
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let role = match a[1].as_str() {
        "drums" => Role::Drums,
        "growls" => Role::Growls,
        _ => Role::Sub,
    };
    let (l, r) = load(&a[2]);
    let side = a.get(3).map(|p| load(p));
    let mut m = Meter::new(44100.0, role);
    // two passes over the file, so the last-four-claps values are settled
    for _ in 0..2 {
        for k in (0..l.len()).step_by(512) {
            let e = (k + 512).min(l.len());
            let sc = side.as_ref().map(|(sl, sr)| [&sl[k..e], &sr[k..e]]);
            m.process([&l[k..e], &r[k..e]], sc);
        }
    }
    println!("{} triggers {} {:?}", a[1], m.triggers, m.readings());
}
