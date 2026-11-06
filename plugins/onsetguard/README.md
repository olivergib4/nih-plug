# Bloater 1.0 — bus clipper with hit analysis

Crate `onsetguard` (plugin IDs unchanged from the earlier spectral Bloater, so old sets still
find the plugin; their settings do not carry over). Built on NIH-plug (GPL-3.0).

## What it is

A fixed **1x hard clip at 0 dBFS** with no output trim, plus a pre-stage that shapes what goes
into the clip, plus an analyser that measures the clipped output the way reference tracks were
measured and shows a loaded reference as deltas. Zero latency.

```
in ─ Drive ─ Tilt / Bell EQ ─ Band leveler ─ [Sub split] ─ hard clip 0 dBFS ─ out
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
| EQ | Tilt | ±6 dB tilt around 1 kHz (low shelf down / high shelf up or the reverse), before the clip. |
| | Bell | One bell, freq and gain (e.g. +3 dB at 2 kHz for bite). |
| Band leveler | Leveler, Low, High, Threshold, Ratio, Attack, Release | A compressor on one band only (default 250–1 k, 1:3, 3/80 ms): flattens low-mid growl peaks so they do not eat the clip. |
| Sub split | Sub split, Crossover, Sub ceiling | Clips the band below the crossover on its own (to its own ceiling) and the rest at 0, then sums and clips again at 0. Off by default. |
| Hit detection | Sensitivity | Onset detector threshold for the analyser. |
| | Profile | Loud HF hits (claps/snares: hits within 6 dB of the loudest recent 2–8 kHz level) or all hits. |

## Readouts

All measured on the clipped output, mid channel unless noted, silence excluded.

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

**Load reference…** decodes a WAV/MP3/FLAC/M4A, takes its loudest 24 s, finds the backbeat
claps (2–8 kHz flux hits on a regular half-bar grid), runs the same analysis and stores it with
the project; the table shows live − reference.

The analysis matches the offline Python measurements the method was developed with to within
0.1 dB (see `examples/report.rs` and `examples/refcheck.rs`).

## Building

Windows VST3 / CLAP from the rolling GitHub release (`bloater-latest-windows.zip`), or
`cargo xtask bundle onsetguard --release`.
