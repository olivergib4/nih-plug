# Clap Shaper

Shapes the clap itself after each hit. Put it last on the clap's bus, before the drum bus clip (after anything on that
bus that adds to the tail, or the tail dip is undone). It triggers from its own input, so no sidechain and no latency.

| Section | What it does | Defaults |
|---|---|---|
| Body | Lifts the body (about 150 Hz to 1.5 kHz) between *From* and *To* after the hit, so it stays dense for as long as the clap sounds. *Lift centre by* and *Lift sides by* are separate: equal amounts keep the width, more on the sides widens the body, sides at 0 narrows it. | 8 dB centre, 8 dB sides, 20 to 95 ms |
| Tail | Dips everything above *Above* by *Dip by* between *From* and *To*, so the clap ends cleanly before the growls return. | 7 dB, 150 to 250 ms, above 150 Hz |

Set either amount to 0 dB to switch that section off. The trigger is the same as in Clap Space; on a bus that also
carries the kick it stays on the clap as long as the kick's top end is well below the clap's (true before the drum clip).

The DSP and its tests live in `plugins/clapdsp`.
