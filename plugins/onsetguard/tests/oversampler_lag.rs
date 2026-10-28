use onsetguard::dsp::clip::{Oversampler, LATENCY};

#[test]
fn oversampler_delay_matches_latency_constant() {
    for os in [1usize, 2, 4, 8] {
        let mut o = Oversampler::new(1, 8);
        o.set_os(os);
        let n = 400;
        let mut x = vec![0.0f32; n];
        x[100] = 0.5;
        let mut refs = vec![&mut x[..]];
        o.process(&mut refs, n, 1.0, 1.0, 1.0);
        let (mi, mv) = x.iter().enumerate().fold((0, 0.0f32), |(bi, bv), (i, v)| if v.abs() > bv { (i, v.abs()) } else { (bi, bv) });
        println!("os {os}: impulse peak at {mi} (expected {}), value {mv:.3}", 100 + LATENCY);
        assert_eq!(mi, 100 + LATENCY, "os {os}");
    }
}
