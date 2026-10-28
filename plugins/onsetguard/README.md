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
* **Sub Split.** The band below Sub Split Freq is separated at the long resolution (sharp,
  zero-phase) and carried through the same onset protection as the full signal, so
  `full - sub` is an exact remainder. *Protect*: the sub bypasses both clippers and its peak
  envelope reserves headroom; the rest is clipped into what is left. Protect Amount blends this
  with the plain clip-everything output, so body level and sub purity trade proportionally and
  the ceiling holds at every setting. *Clip separately*: the sub is clipped on its own at Sub Ceiling, the rest into the
  remaining headroom; no intermodulation between sub and the rest (measured: intermod products
  drop from -24 dB to below -90 dB on a 50 Hz + 2 kHz test).
* **Perceived loudness in sones.** A Zwicker-style model (outer/middle-ear transfer, 24 Bark
  bands, level-dependent masking spread, threshold in quiet, Zwicker specific loudness) runs
  on the processed output and on the dry signal. It needs Reference Level: what 0 dBFS plays
  at on your system, in dB SPL. Validated against the textbook anchors (1 kHz at 40 dB SPL ≈
  1 sone, doubling per 10 dB, 60 Hz a third as loud as 1 kHz at equal SPL, broadband louder
  than a tone of equal power).
* **Matched Bypass** plays the dry signal at the processed signal's perceived loudness (a slow
  servo in sones), so you compare shape, not volume.
* **Presets** (Clap, Drum bus, Bass, Master) as starting points; they set every control except
  Output Gain, Mix, Matched Bypass and Reference Level.
* **Clip Stereo Link** (default 100%) gives both channels the same clip gain, driven by the
  louder one, so the clipper cannot move the stereo image.
* **Detector Weighting** (Flat / K-weighted / Equal loudness) shapes what the compressor hears:
  weighted, it controls the lows harder and leaves presence alone.
* **Analyzer** shows the detector spectrum, the threshold curve, the per-frequency cut/boost
  from both resolution stages, and a strip that lights while a hit is protected. Four meters
  show spectral-clip vs post-clip gain reduction and dry/processed K-weighted level.

## Latency

`long + short + short + 16` samples, plus a small lookahead when the windows are too short
for the protection pre-window. At defaults and 48 kHz: 10256 samples (214 ms). Hosts
compensate automatically; this is a bounce / mix tool, not a live one.

## Ringing, measured

2 ms noise burst into silence, dynamics only, hard settings (threshold -30, 6:1 / 3:1):

| | pre-ring (-40..-1 ms) | post-ring (30-170 ms) |
|---|---|---|
| protection off, no smoothing | -21.7 dB | -37.6 dB |
| protection on (25 ms pre), no smoothing | -34.6 dB | -37.6 dB |
| protection on, 1/6 oct smoothing | -36.7 dB | -43.3 dB |
| protection on, 1/6 oct, Post 120 ms | -36.7 dB | -55.5 dB |

Relative to the hit's own energy. Post-ringing sits 15-20 dB below pre-ringing to begin with
and is masked by the hit; Post and Frequency Smoothing push it further down.

## Verification

`cargo test -p onsetguard --release -- --nocapture` runs offline checks: identity null when
everything is off (error < 1e-6), zero correlation lag on a sine zap, pre-ring reduction with
protection on (-36 dB → -73 dB on a synthetic clap), transient null against dry (-61 dB),
clipper ceiling never exceeded at 1×/2×/4×/8× oversampling, zero allocations in the process
path, and stable behaviour across window/oversample changes.
