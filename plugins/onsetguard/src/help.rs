//! Hover help: what each control does, and what it is for. Keyed by parameter id.

pub fn for_param(id: &str) -> Option<&'static str> {
    Some(match id {
        // --- Global ---
        "output" => "Output Gain. Final level after everything else, before Mix. Use it to level-match against your old chain; it is not part of the sound.",
        "mix" => "Mix. Blends dry and processed. Everything is zero-phase and sample-aligned, so any blend is clean (no comb filtering). Parallel density: 50-70% keeps the dry transient and adds the processed body.",
        "matched_bypass" => "Matched Bypass. Plays the DRY signal, gained until it reads the same perceived loudness (sones) as the processed signal. Flip it on and off: if processed still wins at equal loudness the processing is earning its keep; if they sound alike, you were hearing volume.",
        "ref_level" => "Reference Level. Tells the loudness model how loud 0 dBFS plays on your system, in dB SPL (club ~100-105, studio ~85). The sone numbers only mean something once this is roughly right. It changes no audio.",

        // --- Resolution ---
        "long_window" => "Long Window. FFT size for the lows (below Crossover). Bigger = more bands and finer control in the lows, but longer smear and latency (8192 = 171 ms at 48k). Bass-heavy material 8192-16384; on a master 4096 keeps tails short.",
        "short_window" => "Short Window. FFT size for the highs (above Crossover). Smaller = tighter timing on transients at the cost of resolution. 1024 is a good default; 512 for very snappy material.",
        "crossover" => "Crossover. Where the long and short windows hand over (one-octave crossfade). Lower it if the mids feel smeared; raise it if the low-mids need finer bands.",
        "mono_below" => "Mono Below. Sums the sides to mono below this frequency, inside the long FFT (zero-phase). Stops out-of-phase sub from cancelling on club systems and phones. 0 turns it off.",

        // --- Threshold ---
        "weighting" => "Detector Weighting. Shapes what the compressor HEARS, not the audio. Flat = raw magnitudes; K = broadcast weighting; Equal loudness = the ear's curve. Weighted, lows are controlled harder (they eat headroom but add little loudness) and presence is left alone. Try Equal loudness on a bus.",
        "threshold" => "Threshold. Level at the Threshold Center where compression starts; the curve extends from here using Slope and Curve. The main 'amount' control. Lower = more bins compressed.",
        "center" => "Threshold Center. The frequency the Threshold value refers to. Slope and Curve pivot around it.",
        "slope" => "Threshold Slope. Tilt of the threshold curve in dB/octave. -3 follows pink noise, -4.5 a typical EDM balance. Steeper = the lows are allowed more level before compression than the highs.",
        "curve" => "Threshold Curve. Bends the threshold line: positive makes a V around the center, negative a dome. Use small values to let the center frequency breathe or to clamp it.",

        // --- Dynamics ---
        "down_ratio" => "Downward Ratio. How hard bins above the threshold are pulled down. 1:1 = off. 2-4 for control, 6+ for the dense OTT-style sound.",
        "up_ratio" => "Upward Ratio. How hard bins BELOW the threshold are pushed up (upward compression). 1:1 = off. This is where density and perceived loudness come from without touching the peaks.",
        "up_range" => "Upward Range. Maximum boost the upward compressor may apply, in dB. Caps how far quiet bins (and noise) are lifted; 6-12 is musical, more gets noisy.",
        "attack" => "Attack. How fast each bin's level detector follows a rise. Faster = tighter control but more pumping and ringing per frame. 5-20 ms is a good range; the onset guard handles the hits themselves.",
        "release" => "Release. How fast each bin's detector lets go after a drop. Longer = smoother, more sustain; shorter = more movement.",
        "smooth" => "Frequency Smoothing. Averages the gain curve across neighbouring bins (octaves). A smoother curve is a shorter filter: less watery per-bin swirl and less ringing before and after hits. 1/6 oct default; raise toward 1/3 if things sound phasey.",
        "stereo_link" => "Stereo Link. How much the two channels share one level detector. 100% keeps the image where it was; 0% lets each side move on its own. Keep it at 100% on drums and anything centered.",

        // --- Onset Protection ---
        "protect" => "Onset Protection. Detects hits and hands the output to the untouched signal just before, through, and just after each one. This removes the pre-ring smear and keeps the transient's exact phase. The core idea of the plugin.",
        "sens" => "Onset Sensitivity. How easily a rise counts as a hit. Higher catches ghost notes and hats; lower only the big hits. Watch the yellow strip in the analyzer: it should light on every hit you care about and nothing else.",
        "pre" => "Protect Pre. How long BEFORE each hit the output fades to the dry signal. Pre-ringing lives here. 20-30 ms covers most of it; longer if lows still smear into the hit.",
        "hold" => "Protect Hold. How long AFTER the hit the output stays fully dry. A few ms keeps the attack itself untouched.",
        "post" => "Protect Post. How long after the hold the output fades back to processed. Longer hides post-ringing and keeps the early decay natural; shorter lets the compressor grab sooner.",
        "trans_boost" => "Transient Boost. Adds a short gain bump on each detected hit (upward expansion on transients only). Restores punch that compression takes away, without raising the body.",
        "trans_len" => "Transient Boost Length. Decay time of the boost. Short = click, longer = whole attack.",

        // --- Clip ---
        "clip" => "Clip. Turns the clip stage on: per-bin spectral ceiling, then an oversampled soft clipper with a hard output ceiling. Off, the stage is a plain delay so latency stays constant.",
        "drive" => "Clip Drive. Gain into the clippers: the loudness knob. More drive = more clipping = louder. Watch the two GR meters to see which stage is doing the work.",
        "bin_ceil" => "Spectral Ceiling. Per-bin magnitude limit before the clipper; 0 dB = off (default). Bites on tonal, concentrated energy (sines, bass, zaps), barely on broadband hits. Lower it to tame resonant tones before the clipper; it dulls tonal material fast.",
        "ceiling" => "Output Ceiling. Hard maximum of the clipped signal. 0 dBFS for maximum level; leave a few tenths if your encoder complains.",
        "hardness" => "Clip Hardness. 100% = hard clip (default: loudest, most harmonics). Lower starts a soft knee earlier for a rounder, quieter clip.",
        "trans_clip" => "Transient Clip. How much of each detected hit the clipper may touch. 0% = hits pass unclipped (max punch; peaks may exceed the ceiling), 100% = hits clipped like everything else (max loudness). The punch-vs-loudness dial.",
        "clip_link" => "Clip Stereo Link. 100% gives both channels the same clip gain, driven by the louder side, so clipping cannot move the stereo image or soften the center. Lower only for deliberate width.",
        "oversample" => "Oversample. Runs the clipper at 2-8x the sample rate to keep clipping harmonics from folding back as aliasing (harsh fake highs). 4x is a good default; 8x on bright material.",
        "hf_preserve" => "Preserve Highs. The band above Preserve Above bypasses the clipper and is added back after it, so hard clipping cannot flatten air and hat detail riding on a clipped body. Leave it OFF for sounds whose highs are the hit (zaps, tonal sweeps): there is nothing to protect and the tops come out lumpy. Blend in with Protect Amount.",
        "hf_hz" => "Preserve Above. Where the preserved highs start (one-octave crossfade). 6-10 kHz keeps air and hat detail; lower keeps more but reserves more headroom.",
        "sub_split" => "Sub Split. Separates the band below Sub Split Freq before clipping (zero-phase). Protect: the sub passes clean and reserves headroom, the rest is clipped into what is left (clean sub, costs loudness). Clip separately: sub clipped on its own at Sub Ceiling, the rest into the remaining headroom; no intermod between them.",
        "band_protect" => "Protect Amount. Blend between clipping everything (0%) and the protected version (100%) when Sub Split or Preserve Highs is on. At 100% the protected bands are untouched and the body is clipped harder to make room; lower settings give back body level in proportion. The ceiling holds at every setting.",
        "sub_hz" => "Sub Split Freq. Where the sub band ends (one-octave crossfade). 80-120 Hz typical.",
        "sub_ceil" => "Sub Ceiling. In Clip-separately mode, the sub's own clip ceiling relative to the output ceiling. -6 dB leaves the rest room to breathe; closer to 0 gives a hotter sub and less room above it.",
        _ => return None,
    })
}

pub const PRESET_HELP: &str = "Starting points. Sets every control except Output Gain, Mix, Matched Bypass and Reference Level. The host sees ordinary parameter changes, so undo works.";
pub const SONE_HELP: &str = "Perceived loudness (Zwicker-style sone model). Dry at unity vs processed. Bars are in phon (40 = 1 sone, +10 = twice as loud). Needs Reference Level to be roughly right.";
pub const GR_SPEC_HELP: &str = "How much the per-bin Spectral Ceiling is reducing. Strong on tonal material, weak on broadband hits.";
pub const GR_POST_HELP: &str = "How much the final soft clipper is reducing. If this is doing most of the work on tonal material, lower Spectral Ceiling.";
pub const ANALYZER_HELP: &str = "Analyzer. Grey: what the detector hears (weighted). Orange: the threshold curve. Blue bars: cut, green bars: boost, per frequency, from both resolution stages. Yellow strip along the top: a hit is being protected right now.";
pub const K_HELP: &str = "K-weighted level (LUFS-style, 3 s window). Kept for comparison with other meters; the sone readout is the better loudness number.";
