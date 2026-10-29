# OnsetGuard

Onset-protected, zero-phase spectral dynamics with spectral clipping. Built on the
NIH-plug framework and derived from Robbert van der Helm's Spectral Compressor (GPL-3.0).

## What it does

```
input ─ pre-EQ (min-phase) ─┬─ long STFT (stereo, balance, mono-below, low dynamics) ─ short STFT (high dynamics) ─┐
                            ├─ long STFT (stereo, balance, mono-below; reference)      ─ delay ───────────────────┤
                            ├─ onset detector ─ protection / transient-boost envelopes ───────────────────────── blend
                            │                                                                                      │
                            │   drive ─ [sub / highs split] ─ oversampled clip ─ transient blend ──────────────────┤
                            └─ dry delay ──────────────────────────────────────────────── mix / loudness-matched bypass
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
* **Clip.** Clip Drive into an oversampled clipper with a hard output ceiling. Transient Clip
  decides how much of each hit goes through the oversampled clipper; the ceiling holds at every
  setting. At 1x the clip stage is bit-identical to a plain sample-level hard clip; at 2-8x it
  matches one within 0.1 dB in every band except the alias energy it removes. (A per-bin
  "spectral ceiling" was measured and removed: it gave no loudness.)
* **Pre-EQ.** Four minimum-phase nodes drawn on the analyzer (low shelf, two one-octave bells,
  high shelf), applied at the input before every path. No pre-ringing by construction; the
  compressor sees the shaped signal.
* **Stereo.** Zero-phase, in the long STFT: Width scales the Side above a frequency (the mono
  sum is the Mid, untouched, so it is mono-compatible by construction); the Anti-phase Guard
  pulls down Side where left and right are in anti-phase per bin, which vanishes on a mono sum
  and wastes clipper headroom. Measured: mono-sum error below -119 dB with Width at +6 dB; an
  anti-phase tone's Side removed by 77 dB while panned in-phase material is untouched.
* **Loudness Balance.** A slow, zero-phase per-band gain toward equal specific loudness across
  the Bark bands (Zwicker). For a fixed energy, that balance reads loudest (the loudness sum is
  concave). Energy-preserving, limited to Balance Range per band, measured before the pre-EQ.
  Measured on a dull noise at equal energy: 35.8 -> 40.0 sones.
* **Translation readouts.** Sones of dry vs processed through full range, laptop (120 Hz-15 kHz)
  and phone (300 Hz-8 kHz) bands.
* **Mono below** a set frequency, done in the long STFT (zero-phase).
* **Sub Split.** The band below Sub Split Freq is separated at the long resolution (sharp,
  zero-phase) and carried through the same onset protection as the full signal, so
  `full - sub` is an exact remainder. *Protect*: the sub passes clean (clipped only if it alone
  exceeds the ceiling); *Clip separately*: the sub is clipped on its own at Sub Ceiling first.
  Either way the rest is clipped, sample by sample, into the room the sub leaves at that
  instant, so the sum never exceeds the ceiling and the sub keeps its shape (harmonics -62 dB
  vs -34 dB for a plain clip on a 50 Hz + 2 kHz test, same overall rms within 0.2 dB). Protect
  Amount blends this with the plain clip-everything output.
* **Preserve Highs.** The band above Preserve Above bypasses the clipper and is summed back;
  Highs Amount sets how much of it. Off by default: on sounds whose highs are the hit (zaps)
  there is nothing to protect.
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

`long + short + short + 128` samples, plus a small lookahead when the windows are too short
for the protection pre-window. At defaults and 48 kHz: 10368 samples (216 ms). Hosts
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
