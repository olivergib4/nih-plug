//! The audio path (clip stage, onset detector, analyser incl. report()) must not allocate after
//! construction, and the output must stay inside ±1.
use onsetguard::analysis::{Analyzer, HitKind};
use onsetguard::dsp::busclip::{BusClip, BusSettings};
use onsetguard::dsp::onset::OnsetDetector;
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counting;
static COUNT: AtomicUsize = AtomicUsize::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.load(Ordering::Relaxed) {
            COUNT.fetch_add(1, Ordering::Relaxed);
        }
        System.alloc(l)
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        System.dealloc(p, l)
    }
}
#[global_allocator]
static A: Counting = Counting;

#[test]
fn audio_path_does_not_allocate() {
    let sr = 48000.0;
    let block = 512;
    let mut bus = BusClip::new(sr, 2);
    let mut s = BusSettings::default();
    s.drive_db = 9.0;
    s.lev_on = true;
    s.sub_on = true;
    s.bell_on = true;
    s.tilt_db = 2.0;
    bus.update(&s);
    let mut det = OnsetDetector::new(sr);
    let mut an = Analyzer::new(sr);
    let mut l = vec![0.0f32; block];
    let mut r = vec![0.0f32; block];
    let mut t = 0usize;
    let mut hits = 0;
    let mut reports = 0;
    ARMED.store(true, Ordering::Relaxed);
    for blk in 0..(sr as usize * 12 / block) {
        for i in 0..block {
            let n = (t + i) as f32;
            let burst = if (t + i) % 24000 < 1500 { ((n * 12.9898).sin() * 43758.5).fract() * 0.9 } else { 0.0 };
            let x = (n * 2.0 * std::f32::consts::PI * 55.0 / sr).sin() * 0.5 + (n * 2.0 * std::f32::consts::PI * 700.0 / sr).sin() * 0.3 + burst;
            l[i] = x;
            r[i] = -0.8 * x;
        }
        {
            let mut ch: [&mut [f32]; 2] = [&mut l[..], &mut r[..]];
            bus.process(&mut ch, block);
        }
        for i in 0..block {
            assert!(l[i].abs() <= 1.0 && r[i].abs() <= 1.0 && l[i].is_finite());
            an.push(l[i], r[i]);
            if let Some(at) = det.push(0.5 * (l[i] + r[i]), 0.5) {
                an.onset(at, HitKind::Auto);
                hits += 1;
            }
        }
        if blk % 8 == 7 {
            let rep = an.report();
            reports += 1;
            assert!(rep.pinned_pct >= 0.0);
        }
        t += block;
    }
    ARMED.store(false, Ordering::Relaxed);
    assert!(hits >= 20, "hits {hits}");
    assert!(reports > 10);
    let rep = an.report();
    assert!(rep.claps > 5, "{rep:?}");
    assert_eq!(COUNT.load(Ordering::Relaxed), 0, "allocations in the audio path");
}
