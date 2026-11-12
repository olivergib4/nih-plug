//! The simple view: six gauges that each answer one question against the loaded reference
//! (drums, growl low / mid / high, sub, loudness). Pure functions of two reports, so the editor only draws.

use crate::analysis::{room_left, Report};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    /// Nothing to show yet (no reference, or nothing measured).
    #[default]
    Idle,
    Ok,
    Warn,
    Bad,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Gauge {
    pub name: &'static str,
    /// Marker position 0..1 along the bar (NaN = no marker).
    pub pos: f32,
    /// Target zone 0..1 along the bar.
    pub zone: (f32, f32),
    pub status: Status,
    pub text: String,
}

pub const NGAUGES: usize = 6;
pub const NAMES: [&str; NGAUGES] = ["DRUMS", "GROWL LOW", "GROWL MID", "GROWL HIGH", "SUB", "LOUDNESS"];
/// Seconds of audio a pass needs before the gauges are trusted.
pub const MIN_MEASURED_S: f32 = 3.0;

fn place(x: f32, lo: f32, hi: f32) -> f32 {
    ((x - lo) / (hi - lo)).clamp(0.0, 1.0)
}

fn gauge(name: &'static str, x: f32, lo: f32, hi: f32, zone: (f32, f32), status: Status, text: String) -> Gauge {
    Gauge { name, pos: place(x, lo, hi), zone: (place(zone.0, lo, hi), place(zone.1, lo, hi)), status, text }
}

fn idle(name: &'static str, text: &str) -> Gauge {
    Gauge { name, pos: f32::NAN, zone: (0.0, 0.0), status: Status::Idle, text: text.into() }
}

/// Growl band gauge: `over` = how many dB the bed in this band sits closer to the drums than in the reference.
fn growl(name: &'static str, over: f32) -> Gauge {
    let (status, text) = if over > 2.0 {
        (Status::Bad, format!("{:.0} dB over", over))
    } else if over > 0.5 {
        (Status::Warn, format!("{:.0} dB over", over.max(1.0)))
    } else if over >= -3.0 {
        (Status::Ok, "on target".to_string())
    } else {
        (Status::Ok, format!("ok, {:.0} dB room", -over))
    };
    gauge(name, over, -9.0, 9.0, (-3.0, 0.5), status, text)
}

pub fn gauges(live: &Report, reference: Option<&Report>) -> [Gauge; NGAUGES] {
    let Some(r) = reference else {
        return NAMES.map(|n| idle(n, "load a reference"));
    };
    if live.measured_s < MIN_MEASURED_S || live.claps == 0 {
        return NAMES.map(|n| idle(n, "play the drop"));
    }
    // drums: how often the output is pinned during hits, against the reference
    let d = live.pinned_hits_pct - r.pinned_hits_pct;
    let drums = {
        let (status, text) = if d > 15.0 {
            (Status::Bad, "too hot into the master".to_string())
        } else if d > 6.0 {
            (Status::Warn, "a little hot into the master".to_string())
        } else if d >= -6.0 {
            (Status::Ok, "on target".to_string())
        } else if d >= -15.0 {
            (Status::Warn, "a little soft".to_string())
        } else {
            (Status::Bad, "too soft: not reaching the ceiling".to_string())
        };
        gauge(NAMES[0], d, -30.0, 30.0, (-6.0, 6.0), status, text)
    };
    let low = growl(NAMES[1], -room_left(live, r, 2));
    let mid = growl(NAMES[2], -room_left(live, r, 3));
    let high = growl(NAMES[3], -room_left(live, r, 4));
    let sub = if !r.guard || !(r.sub_crest > 0.0) {
        idle(NAMES[4], "reload the reference")
    } else if !(live.sub_crest > 0.0) {
        idle(NAMES[4], "no sub measured")
    } else {
        let over = 20.0 * (live.sub_crest / r.sub_crest).log10();
        // Once the sub is clipped its measured crest stops rising, so how often the low end is held flat at
        // the ceiling (against the reference) is the second sign of it being pushed too far.
        let flattened = live.sub_flat_per_s > 2.0 * r.sub_flat_per_s + 2.0;
        let crushed = live.sub_flat_per_s > 6.0 * r.sub_flat_per_s + 10.0;
        let (status, text) = if over > 1.5 || (over > 0.5 && flattened) {
            (Status::Bad, format!("{:.0} dB over: being flattened", over.max(1.0)))
        } else if crushed {
            (Status::Bad, "low end flattened far more than the reference".to_string())
        } else if flattened {
            (Status::Warn, "low end flattened more than the reference".to_string())
        } else if over > 0.5 {
            (Status::Warn, format!("{:.0} dB over", over.max(1.0)))
        } else if over >= -1.5 {
            (Status::Ok, "at the reference level".to_string())
        } else {
            (Status::Ok, format!("ok, {:.0} dB room", -over))
        };
        gauge(NAMES[4], over, -6.0, 6.0, (-1.5, 0.5), status, text)
    };
    let dl = live.phon - r.phon;
    let loud = {
        let (status, text) = if dl > 1.5 {
            (Status::Bad, format!("{:.1} phon louder", dl))
        } else if dl > 0.5 {
            (Status::Warn, format!("{:.1} phon louder", dl))
        } else if dl >= -0.5 {
            (Status::Ok, "on target".to_string())
        } else {
            (Status::Ok, format!("{:.1} phon quieter", -dl))
        };
        gauge(NAMES[5], dl, -4.0, 4.0, (-0.5, 0.5), status, text)
    };
    [drums, low, mid, high, sub, loud]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rep(hits_full: f32, rest: [f32; 3], pinned_hits: f32, crest: f32, phon: f32) -> Report {
        let mut r = Report { claps: 8, measured_s: 12.0, guard: true, pinned_hits_pct: pinned_hits, sub_crest: crest, sub_flat_per_s: 2.0, phon, ..Default::default() };
        r.band_hits_db[0] = hits_full;
        r.band_rest_db[2] = rest[0];
        r.band_rest_db[3] = rest[1];
        r.band_rest_db[4] = rest[2];
        r
    }

    #[test]
    fn idle_without_reference_or_audio() {
        let live = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 0.95, 109.3);
        assert!(gauges(&live, None).iter().all(|g| g.status == Status::Idle && g.text == "load a reference"));
        let silent = Report::default();
        assert!(gauges(&silent, Some(&live)).iter().all(|g| g.text == "play the drop"));
    }

    #[test]
    fn same_as_reference_is_all_green() {
        let r = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 0.95, 109.3);
        let g = gauges(&r, Some(&r));
        assert!(g.iter().all(|x| x.status == Status::Ok), "{g:?}");
        assert_eq!(g[0].text, "on target");
        assert_eq!(g[4].text, "at the reference level");
        // markers sit inside their zones
        assert!(g.iter().all(|x| x.pos >= x.zone.0 && x.pos <= x.zone.1), "{g:?}");
    }

    #[test]
    fn hot_low_mids_and_hot_master_are_flagged() {
        // the measured case: bed 250-1k 3.2 dB hotter, highs 6 dB hotter, drums pinned 65 % against 35 %
        let r = rep(-3.0, [-14.9, -17.2, -24.0], 34.6, 0.95, 109.3);
        let live = rep(-3.0, [-11.7, -15.5, -17.9], 65.4, 1.02, 110.5);
        let g = gauges(&live, Some(&r));
        assert_eq!(g[0].status, Status::Bad);
        assert_eq!((g[1].status, g[1].text.as_str()), (Status::Bad, "3 dB over"));
        assert_eq!(g[2].status, Status::Warn);
        assert_eq!((g[3].status, g[3].text.as_str()), (Status::Bad, "6 dB over"));
        assert_eq!(g[4].status, Status::Warn);
        assert_eq!(g[5].status, Status::Warn);
    }

    #[test]
    fn a_darker_quieter_bed_shows_room() {
        let r = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 0.95, 109.3);
        let live = rep(-3.0, [-15.5, -22.0, -31.0], 33.0, 0.75, 108.0);
        let g = gauges(&live, Some(&r));
        assert_eq!(g[1].text, "on target");
        assert_eq!(g[2].text, "ok, 5 dB room");
        assert_eq!(g[3].text, "ok, 7 dB room");
        assert_eq!(g[4].text, "ok, 2 dB room");
        assert_eq!(g[5].text, "1.3 phon quieter");
        assert!(g.iter().all(|x| x.status == Status::Ok));
    }

    #[test]
    fn a_flattened_low_end_is_flagged_even_when_the_crest_matches() {
        // measured: same crest as the reference (1.09 against 1.10) but held flat 73 times a second against 8.5
        let mut r = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 1.09, 109.3);
        r.sub_flat_per_s = 8.5;
        let mut live = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 1.10, 109.3);
        live.sub_flat_per_s = 73.4;
        assert_eq!(gauges(&live, Some(&r))[4].status, Status::Bad);
        live.sub_flat_per_s = 25.0;
        assert_eq!(gauges(&live, Some(&r))[4].status, Status::Warn);
        live.sub_flat_per_s = 12.0;
        assert_eq!(gauges(&live, Some(&r))[4].status, Status::Ok);
    }

    #[test]
    fn an_old_reference_asks_for_a_reload_on_the_sub_only() {
        let mut r = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 0.0, 109.3);
        r.guard = false;
        let live = rep(-3.0, [-15.0, -17.0, -24.0], 35.0, 0.95, 109.3);
        let g = gauges(&live, Some(&r));
        assert_eq!(g[4].text, "reload the reference");
        assert_eq!(g[1].status, Status::Ok);
    }
}
