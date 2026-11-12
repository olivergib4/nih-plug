# Bloater 1.0 — bus clipper with hit analysis

Crate `onsetguard` (plugin IDs unchanged from the earlier spectral Bloater, so old sets still
find the plugin; their settings do not carry over). Built on NIH-plug (GPL-3.0).

## What it is

A fixed **1x hard clip at 0 dBFS** with no output trim, plus a pre-stage that shapes what goes
into the clip, plus an analyser that measures the clipped output the way reference tracks were
measured and shows a loaded reference as deltas. Zero latency.

```
in ─ [Attack EQ] ─ Drive ─ Tilt / Bell EQ ─ Band leveler ─ [Soft stage] ─ [Sub split] ─ hard clip 0 dBFS ─ out
                                                              └─ onset detector + hit analyser → editor
```

Why no ceiling and no output gain: turning a clipped bus down just lowers a squared waveform;
loudness comes from what is driven into the clip, and the master clipper after it sets the
final pinned share. Put one Bloater on each bus (Bus mode, drive each to taste) and one last on
the master (Master mode, drive set last to the pinned share you want).

## Controls

| Section | Control | What it does |
|---|---|---|
| Clip | Mode | Bus / Master: same processing, labels the readouts (the reference is a master measurement). |
| | Clip | Bypass the clip stage for A/B (pre-stage stays). |
| | Drive | Gain into the clip, −12…+24 dB. |
| Attack EQ | Attack EQ, Trigger within, Start after hit, Length, Bell freq/gain, Shelf freq/gain | Off by default. A bell (Q 1) and a high shelf that act only in a window after each hit (default 8.8 ms after the trigger for 52 ms, 1 ms fade in, 15 ms fade out), so the first milliseconds (the zap) and the body of the clap are untouched. It triggers when the level above 2 kHz comes within *Trigger within* dB (default 12) of the loudest recent hit, at most once per 150 ms; a kick's top end is far lower, so it does not trigger. Zero latency. Meant for the drum bus that carries the clap, with Clip off and Drive 0: −4 dB at 2 kHz and +4 dB shelf at 5 kHz is the "brighter attack" setting. |
| Soft stage | Soft stage, Knee, Push into clip | Off by default. Linear below the knee (0.50 = half of full scale), rounding smoothly into the ceiling above it; Push is the gain from the soft stage into the hard clip and sets how much still gets pinned (fine steps: 0.20–0.25 dB covers roughly 16–22 % pinned on a dense mix). Meant for the master instance: Drive +3, knee 0.50, push 0.22 reproduced the reference fingerprint. |
| EQ | Tilt | ±6 dB tilt around 1 kHz (low shelf down / high shelf up or the reverse), before the clip. |
| | Bell | One bell, freq and gain (e.g. +3 dB at 2 kHz for bite). |
| Band leveler | Leveler, Low, High, Threshold, Ratio, Attack, Release | A compressor on one band only (default 250–1 k, 1:3, 3/80 ms): flattens low-mid growl peaks so they do not eat the clip. |
| Sub split | Sub split, Crossover, Sub ceiling | Clips the band below the crossover on its own (to its own ceiling) and the rest at 0, then sums and clips again at 0. Off by default. |
| Hit detection | Sensitivity | Onset detector threshold for the analyser. |
| | Profile | Loud HF hits (claps/snares: hits within 6 dB of the loudest recent 2–8 kHz level) or all hits. |

## Simple view (default)

Six gauges, each a bar with a green target zone taken from the loaded reference and a marker for
the current pass. Play the drop once; the words next to each bar say what to do.

| Gauge | What it compares with the reference | Reads |
|---|---|---|
| DRUMS | How often the output is pinned during hits | on target / hot into the master / soft |
| GROWL LOW, MID, HIGH | How close the bed between hits (250-1k, 1-4k, 4-10k) sits to the drums | on target / N dB over / N dB room |
| SUB | Sub crest between hits (level below 120 Hz against the ceiling) | at the reference level / over: being flattened / room |
| LOUDNESS | Mean loudness in phon | on target / louder / quieter |

"Over" on a growl gauge means that band of the bed is closer to the drums than in the reference by
that many dB, so the drums stand out less; "room" means it can come up by that much first. The
gauges need a reference and at least 3 s of a pass. **Details** shows the full table below, which
also has the room per band, the sub crest and flattening rate, and stereo width (side minus mid)
per band at hits and between hits.

## Readouts

All measured on the clipped output, mid channel unless noted, silence excluded.

* **Spectrum**: live mid-channel spectrum (1/6 octave, +4.5 dB/oct display tilt), its average
  over the current pass, and the reference's average over its loudest 24 s.
* **Measure per playback** (on by default): the statistics restart when the transport starts
  and hold when it stops, so playing a section once gives one stable set of numbers; the
  header shows how many seconds they cover.
* **Hit profile graph**: clap level (RMS dBFS) in 0–3 / 3–10 / 10–25 / 25–60 / 60–120 /
  120–200 ms after the hit, and rail share (% of samples with either channel ≥ 0.999) in
  0–10 … 120–150 ms. Live median filled, last hit as a line, reference as an orange marker.
* **pinned**: total, inside the 150 ms after hits, between hits.
* **loudness**: mean phon (Zwicker-style sone model), at claps (30 ms after), between claps.
* **drums over rest** per band (250–1 k, 1–4 k, 4–10 k): mean power inside the 150 ms after
  every hit minus the rest of the time. **rest level**: the absolute level of the rest.
* **500–6 k spread**: p90 − p10 of 50 ms window levels, stereo. Lower = flatter ("bricked").
* **clap 5–10 k at 25–60 ms**: the clap's top-end tail.
* Header line: pre-clip peak (how hard the clip is hit), pinned share of the last block,
  leveler gain reduction.

Which hits are claps: the analyser finds the half-bar grid (period 1.2–2.4 s) that collects the
most bright hits in the last 20 s and, among the grid phases, takes the brightest one (the
clap/snare, not the kick on the other beat); until it locks (4 hits) the loudest-HF rule is
used. The opposite-beat hit is the loudest other hit between two claps. The drums-vs-rest
statistics run ~2 s behind the input so those decisions are known.

**Load reference…** decodes a WAV/MP3/FLAC/M4A, takes its loudest 24 s, runs exactly the live
analysis on it (same detector, same grid) and stores it with the project; the table shows
live − reference.

The analysis matches the offline Python measurements the method was developed with to within
0.1 dB (see `examples/report.rs` and `examples/refcheck.rs`).

## Building

Windows VST3 / CLAP from the rolling GitHub release (`bloater-latest-windows.zip`), or
`cargo xtask bundle onsetguard --release`.
