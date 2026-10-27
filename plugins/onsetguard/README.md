# OnsetGuard

Onset-protected, zero-phase spectral dynamics with spectral clipping. Built on the
NIH-plug framework and derived from Robbert van der Helm's Spectral Compressor (GPL-3.0).

## What it does

```
input ─┬─ long STFT  (mono-below + low-band dynamics)  ─ short STFT (high-band dynamics) ─┐
       ├─ long STFT  (mono-below only, reference)      ─ delay ────────────────────────────┤
       ├─ onset detector ─ protection / transient-boost envelopes ─────────────────────── blend
       │                                                                                   │
       │   drive ─ short STFT (per-bin ceiling) ─ oversampled soft clip ─ transient blend ──┤
       └─ dry delay ─────────────────────────────────────────── mix / loudness-matched bypass
```

* **Zero-phase everywhere.** Every stage is sample-aligned with the dry signal, so parallel
  blending and the Mix control never comb-filter.
* **Two resolutions.** A long window (default 8192) handles the lows, a short one (default
  1024) the highs, crossfaded over one octave around the Crossover. Lots of bands in the lows,
  tight timing in the highs.
* **Onset protection.** A spectral-flux detector finds hits; the processed signal fades to the
  untouched reference just before each hit (Pre), holds through it (Hold) and fades back
  (Post). This is what removes pre-ring smear while keeping the transient's exact phase.
* **Transient boost.** Optional upward expansion on detected hits only.
* **Frequency smoothing** of the gain curve removes the watery per-bin swirl.
* **Spectral clip.** A per-bin ceiling (detail-preserving style) feeds an oversampled soft
  clipper with a hard output ceiling. Transient Clip Amount decides how much of each hit the
  clipper is allowed to touch.
* **Mono below** a set frequency, done in the long STFT (zero-phase).
* **Matched Bypass** plays the dry signal at the processed signal's K-weighted level so you
  compare shape, not volume.

## Latency

`long + short + short + 16` samples, plus a small lookahead when the windows are too short
for the protection pre-window. At defaults and 48 kHz: 10256 samples (214 ms). Hosts
compensate automatically; this is a bounce / mix tool, not a live one.

## Verification

`cargo test -p onsetguard --release -- --nocapture` runs offline checks: identity null when
everything is off (error < 1e-6), zero correlation lag on a sine zap, pre-ring reduction with
protection on (-36 dB → -73 dB on a synthetic clap), transient null against dry (-61 dB),
clipper ceiling never exceeded at 1×/2×/4×/8× oversampling, zero allocations in the process
path, and stable behaviour across window/oversample changes.
