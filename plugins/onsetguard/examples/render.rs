//! Offline render through the engine: `render in.raw out.raw [key=value ...]`
//! (raw = f32le interleaved stereo at 44.1 kHz). Keys: up, down, thr, range, depth, drive,
//! hit, clip (0/1), smooth, long, short, protect (0/1), attack, release.
use onsetguard::dsp::{Engine, Settings};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bytes = std::fs::read(&args[1]).expect("read");
    let n = bytes.len() / 8;
    let mut input = vec![vec![0.0f32; n]; 2];
    for i in 0..n {
        input[0][i] = f32::from_le_bytes(bytes[i * 8..i * 8 + 4].try_into().unwrap());
        input[1][i] = f32::from_le_bytes(bytes[i * 8 + 4..i * 8 + 8].try_into().unwrap());
    }
    let mut s = Settings::default();
    s.clip_on = false;
    s.dyn_.up_ratio = 1.0;
    s.dyn_.down_ratio = 1.0;
    for kv in &args[3..] {
        let (k, v) = kv.split_once('=').expect("key=value");
        let f: f32 = v.parse().expect("number");
        match k {
            "up" => s.dyn_.up_ratio = f,
            "down" => s.dyn_.down_ratio = f,
            "thr" => s.dyn_.threshold_db = f,
            "range" => s.dyn_.up_range_db = f,
            "depth" => s.dyn_.up_depth_db = f,
            "drive" => s.drive_db = f,
            "hit" => s.hit_drive_db = f,
            "clip" => s.clip_on = f > 0.5,
            "smooth" => s.dyn_.smooth_oct = f,
            "long" => s.long_order = f as usize,
            "short" => s.short_order = f as usize,
            "protect" => s.protect_on = f > 0.5,
            "attack" => s.dyn_.attack_ms = f,
            "body" => s.dyn_.body_ms = f,
            "release" => s.dyn_.release_ms = f,
            _ => panic!("unknown key {k}"),
        }
    }
    let sr = 44100.0;
    let block = 512;
    let mut e = Engine::new(2, sr, block);
    e.set_windows(s.long_order, s.short_order, s.oversample);
    let lat = e.latency();
    let total = n + lat;
    let mut out = vec![vec![0.0f32; total]; 2];
    let mut pos = 0;
    while pos < total {
        let len = block.min(total - pos);
        let mut chunks: Vec<Vec<f32>> =
            (0..2).map(|ch| (0..len).map(|i| input[ch].get(pos + i).copied().unwrap_or(0.0)).collect()).collect();
        {
            let mut refs: Vec<&mut [f32]> = chunks.iter_mut().map(|c| &mut c[..]).collect();
            e.process(&mut refs, len, &s);
        }
        for ch in 0..2 {
            out[ch][pos..pos + len].copy_from_slice(&chunks[ch]);
        }
        pos += len;
    }
    let mut ob = Vec::with_capacity(n * 8);
    for i in 0..n {
        ob.extend_from_slice(&out[0][lat + i].to_le_bytes());
        ob.extend_from_slice(&out[1][lat + i].to_le_bytes());
    }
    std::fs::write(&args[2], ob).expect("write");
}
