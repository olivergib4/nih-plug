use onsetguard::dsp::{Engine, Settings};
use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

struct Counting;
static COUNT: AtomicUsize = AtomicUsize::new(0);
static ARMED: AtomicBool = AtomicBool::new(false);
static SIZES_N: AtomicUsize = AtomicUsize::new(0);
static SIZES: [AtomicUsize; 16] = [const { AtomicUsize::new(0) }; 16];
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { if ARMED.load(Ordering::Relaxed) { COUNT.fetch_add(1, Ordering::Relaxed); let i = SIZES_N.fetch_add(1, Ordering::Relaxed); if i < 16 { SIZES[i].store(l.size(), Ordering::Relaxed); } } System.alloc(l) }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { System.dealloc(p, l) }
}
#[global_allocator]
static A: Counting = Counting;

fn drive(e: &mut Engine, s: &Settings, nc: usize, blocks: usize, block: usize) -> f32 {
    let mut peak = 0.0f32;
    let was = ARMED.load(Ordering::Relaxed); ARMED.store(false, Ordering::Relaxed);
    let mut bufs = vec![vec![0.0f32; block]; nc];
    ARMED.store(was, Ordering::Relaxed);
    let mut t = 0usize;
    for _ in 0..blocks {
        for ch in 0..nc { for i in 0..block {
            let x = ((t + i) as f32 * 2.0 * std::f32::consts::PI * 60.0 / 48000.0).sin() * 0.5
                + ((t + i) as f32 * 2.0 * std::f32::consts::PI * 1000.0 / 48000.0).sin() * 0.3
                + if (t + i) % 24000 < 200 { 0.6 } else { 0.0 };
            bufs[ch][i] = if ch == 1 { -x } else { x };
        } }
        { let was = ARMED.load(Ordering::Relaxed); ARMED.store(false, Ordering::Relaxed); let mut r: Vec<&mut [f32]> = bufs.iter_mut().map(|b| &mut b[..]).collect(); ARMED.store(was, Ordering::Relaxed); e.process(&mut r, block, s); ARMED.store(false, Ordering::Relaxed); drop(r); ARMED.store(was, Ordering::Relaxed); }
        for b in &bufs { for v in b { peak = peak.max(v.abs()); assert!(v.is_finite()); } }
        t += block;
    }
    peak
}

#[test]
fn no_allocations_in_process_and_param_changes_are_safe() {
    let block = 512;
    let mut e = Engine::new(2, 48000.0, block);
    let mut s = Settings::default();
    e.set_windows(s.long_order, s.short_order, s.oversample);
    drive(&mut e, &s, 2, 4, block); // warm up
    COUNT.store(0, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    drive(&mut e, &s, 2, 200, block);
    // change every parameter group mid-stream (no window change)
    s.dyn_.threshold_db = -40.0; s.dyn_.crossover_hz = 500.0; s.dyn_.mono_below_hz = 90.0;
    s.trans_boost_db = 6.0; s.drive_db = 9.0; s.matched_bypass = true;
    drive(&mut e, &s, 2, 200, block);
    s.matched_bypass = false; s.mix = 0.5;
    drive(&mut e, &s, 2, 50, block);
    ARMED.store(false, Ordering::Relaxed);
    let n = COUNT.load(Ordering::Relaxed);
    println!("allocations during process: {n}"); for i in 0..16 { let v = SIZES[i].load(Ordering::Relaxed); if v > 0 { println!("  alloc size {v}"); } }
    assert_eq!(n, 0);
    // window / oversample changes mid-stream must not panic; full wet, full transient clip → ceiling holds
    s.mix = 1.0; s.trans_clip = 1.0;
    for (lo, so, os) in [(9usize, 6usize, 1usize), (15, 12, 8), (11, 8, 2), (13, 10, 4)] {
        s.long_order = lo; s.short_order = so; s.oversample = os;
        let p = drive(&mut e, &s, 2, 120, block);
        println!("windows {lo}/{so} os {os}: latency {} peak {:.3}", e.latency(), p);
        assert!(p <= 1.0001, "peak {p}");
    }
}

