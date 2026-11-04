//! Starting points. Values are plain parameter values keyed by parameter id; anything not
//! listed is reset to its default. Applied from the editor through the host's parameter
//! gestures, so they are undoable and automation-safe.

pub struct Preset {
    pub name: &'static str,
    pub values: &'static [(&'static str, f32)],
}

// Enum parameters take their variant index as the plain value.
// weighting: 0 Flat, 1 K, 2 Equal loudness · sub_split: 0 Off, 1 Protect, 2 Separate
// oversample: 0 1x, 1 2x, 2 4x, 3 8x
// Stereo, Loudness Balance and Pre EQ are deliberately left at their defaults by every
// preset (off): they are mix decisions, not starting points.

pub const PRESETS: &[Preset] = &[
    // Bus: make the drums dense and hand them hot to the master clipper (Clip off, Meter Thru
    // Clip on so the readouts and Autopilot see what the master will do).
    Preset {
        name: "Drum bus",
        values: &[
            ("weighting", 2.0),
            ("threshold", -24.0),
            ("slope", -4.5),
            ("down_ratio", 2.5),
            ("up_ratio", 3.0),
            ("up_range", 8.0),
            ("up_depth", 24.0),
            ("body", 150.0),
            ("attack", 10.0),
            ("release", 120.0),
            ("smooth", 0.17),
            ("long_window", 13.0),
            ("short_window", 10.0),
            ("crossover", 300.0),
            ("mono_below", 120.0),
            ("protect", 1.0),
            ("sens", 0.6),
            ("pre", 25.0),
            ("hold", 4.0),
            ("post", 30.0),
            ("trans_boost", 0.0),
            ("clip", 0.0),
            ("drive", 6.0),
            ("hit_drive", 3.0),
            ("ceiling", 0.0),
            ("oversample", 0.0),
            ("hf_preserve", 0.0),
            ("hf_hits", 0.0),
            ("trans_clip", 1.0),
            ("sub_split", 0.0),
            ("meter_thru_clip", 1.0),
        ],
    },
    // Master: the last clipper in the chain. 1x per-channel = a standard hard clipper, plus
    // Transient Clip and Sub Protect that a plain clipper cannot do.
    Preset {
        name: "Master",
        values: &[
            ("weighting", 2.0),
            ("threshold", -30.0),
            ("slope", -4.5),
            ("down_ratio", 1.8),
            ("up_ratio", 1.5),
            ("up_range", 6.0),
            ("up_depth", 24.0),
            ("attack", 25.0),
            ("release", 250.0),
            ("smooth", 0.33),
            ("long_window", 12.0),
            ("short_window", 10.0),
            ("crossover", 200.0),
            ("mono_below", 100.0),
            ("protect", 1.0),
            ("sens", 0.5),
            ("pre", 15.0),
            ("hold", 2.0),
            ("post", 30.0),
            ("trans_boost", 0.0),
            ("clip", 1.0),
            ("drive", 3.0),
            ("hit_drive", 2.0),
            ("ceiling", 0.0),
            ("oversample", 0.0),
            ("clip_link", 0.0),
            ("hardness", 1.0),
            ("trans_clip", 1.0),
            ("hf_preserve", 0.0),
            ("sub_split", 1.0),
            ("sub_hz", 80.0),
            ("band_protect", 0.6),
            ("meter_thru_clip", 1.0),
        ],
    },
];
