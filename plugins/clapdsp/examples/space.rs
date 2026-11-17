//! Run a raw f32le stereo file through the Space section:
//! `space IN OUT role(growls|sub|tail) [SIDE|-] [depth_db back_ms sub_hold_ms sub_back_ms tail_db tail_from_ms tail_to_ms split_hz thr_db body_db body_from_ms body_to_ms body_side_db lean_db before_ms centre_db]`
//! SIDE is the sidechain (the clap) as raw f32le stereo, "-" for none. The output is delayed by the lookahead
//! for the growls and sub roles, as in the plugin before the host compensates. Trigger positions (samples in
//! the input) go to stderr.
use clapdsp::space::{Space, SpaceRole, SpaceSettings};

fn read(p: &str) -> (Vec<f32>, Vec<f32>) {
    let bytes = std::fs::read(p).unwrap();
    let n = bytes.len() / 8;
    let l = (0..n).map(|i| f32::from_le_bytes(bytes[i * 8..i * 8 + 4].try_into().unwrap())).collect();
    let r = (0..n).map(|i| f32::from_le_bytes(bytes[i * 8 + 4..i * 8 + 8].try_into().unwrap())).collect();
    (l, r)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (mut l, mut r) = read(&a[1]);
    let n = l.len();
    let mut s = SpaceSettings::default();
    s.role = match a[3].as_str() {
        "growls" => SpaceRole::Growls,
        "sub" => SpaceRole::Sub,
        "tail" => SpaceRole::ClapTail,
        _ => SpaceRole::Off,
    };
    let side = if a.len() > 4 && a[4] != "-" { Some(read(&a[4])) } else { None };
    let f = |i: usize, d: f32| a.get(i).map(|v| v.parse().unwrap()).unwrap_or(d);
    s.depth_db = f(5, s.depth_db);
    s.back_ms = f(6, s.back_ms);
    s.sub_hold_ms = f(7, s.sub_hold_ms);
    s.sub_back_ms = f(8, s.sub_back_ms);
    s.tail_db = f(9, s.tail_db);
    s.tail_from_ms = f(10, s.tail_from_ms);
    s.tail_to_ms = f(11, s.tail_to_ms);
    s.split_hz = f(12, s.split_hz);
    s.thr_db = f(13, s.thr_db);
    s.body_db = f(14, s.body_db);
    s.body_from_ms = f(15, s.body_from_ms);
    s.body_to_ms = f(16, s.body_to_ms);
    s.body_side_db = f(17, s.body_side_db);
    s.lean_db = f(18, s.lean_db);
    s.before_ms = f(19, s.before_ms);
    s.centre_db = f(20, s.centre_db);
    let mut sp = Space::new(44100.0);
    sp.update(&s);
    sp.reset();
    let mut seen = 0;
    let mut pos = 0;
    while pos < n {
        // short blocks so the trigger positions can be reported to within a few samples
        let len = (n - pos).min(16);
        let sc = side.as_ref().map(|(sl, sr)| [&sl[pos..pos + len], &sr[pos..pos + len]]);
        sp.process(&mut [&mut l[pos..pos + len], &mut r[pos..pos + len]], sc, len);
        if sp.triggers != seen {
            seen = sp.triggers;
            eprintln!("{}", pos);
        }
        pos += len;
    }
    let mut out = Vec::with_capacity(n * 8);
    for i in 0..n {
        out.extend_from_slice(&l[i].to_le_bytes());
        out.extend_from_slice(&r[i].to_le_bytes());
    }
    std::fs::write(&a[2], out).unwrap();
}
