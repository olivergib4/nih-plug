# Clap Shaper

Shapes the clap itself after each hit. Put it last on the clap's bus, before the drum bus clip (after anything on that
bus that adds to the tail, or the tail dip is undone). It triggers from its own input, so no sidechain and no latency.

| Section | What it does | Defaults |
|---|---|---|
| Body | Lifts the body (about 150 Hz to 1.5 kHz) between *From* and *To* after the hit, so it stays dense for as long as the clap sounds. *Lift centre by* and *Lift sides by* are separate: equal amounts keep the width, more on the sides widens the body, sides at 0 narrows it. | 8 dB centre, 8 dB sides, 20 to 95 ms |
| Push | Raises the whole signal by *Push by* from *From* to *To* after the hit (5 ms up, 30 ms back), so the clap's body goes into the drum clip harder. The offline version that sounded best had 6 dB of this on top of the body lift; without it the clipped body came out 1 to 3 dB thinner at 150 to 600 Hz and above 5 kHz. | 0 dB (off), 23 to 63 ms |
| Lean | *Lean by* lifts the left channel's mids (600 Hz to 1.5 kHz) by that much from 15 to 60 ms and the right channel's highs (above 1.5 kHz) by 0.8 of it from 70 to 180 ms, so the clap starts a little left and ends a little right, as measured on the reference. Level only, no delay between the channels. | 0 dB (off); 2.5 dB was picked by ear in the car |
| Tail | Dips everything above *Above* by *Dip by* between *From* and *To*, so the clap ends cleanly before the growls return. | 7 dB, 150 to 250 ms, above 150 Hz |
| Tail to centre | From 85 ms to the tail's *To*, the sides of the tail (400 Hz to 7 kHz) come down by *To centre by* and centre and sides are then raised together by whatever keeps the stereo level where it was. The tail folds to mono louder at the same stereo loudness. Turning the sides down alone does not do that: it only makes the stereo quieter. | 4 dB: on the session it brought the tail's mono loss from 3.7 dB to 1.9 dB (the reference loses 2.0 dB) |

Set either amount to 0 dB to switch that section off. The trigger is the same as in Clap Space; on a bus that also
carries the kick it stays on the clap as long as the kick's top end is well below the clap's (true before the drum clip).

The DSP and its tests live in `plugins/clapdsp`.
