//! Run a raw f32le stereo file through the bus clipper: `chain IN OUT drive_db soft(0/1) knee push_db`
use onsetguard::dsp::busclip::{BusClip, BusSettings};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&a[1]).unwrap();
    let n = bytes.len() / 8;
    let mut l: Vec<f32> = (0..n).map(|i| f32::from_le_bytes(bytes[i * 8..i * 8 + 4].try_into().unwrap())).collect();
    let mut r: Vec<f32> = (0..n).map(|i| f32::from_le_bytes(bytes[i * 8 + 4..i * 8 + 8].try_into().unwrap())).collect();
    let mut s = BusSettings::default();
    s.drive_db = a[3].parse().unwrap();
    s.soft_on = a[4] == "1";
    s.soft_knee = a[5].parse().unwrap();
    s.soft_push_db = a[6].parse().unwrap();
    let mut b = BusClip::new(44100.0, 2);
    b.update(&s);
    b.reset();
    for (cl, cr) in l.chunks_mut(512).zip(r.chunks_mut(512)) {
        let len = cl.len();
        b.process(&mut [cl, cr], len);
    }
    let mut out = Vec::with_capacity(n * 8);
    for i in 0..n {
        out.extend_from_slice(&l[i].to_le_bytes());
        out.extend_from_slice(&r[i].to_le_bytes());
    }
    std::fs::write(&a[2], out).unwrap();
}
