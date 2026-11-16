# Clap Space

Makes room for the clap on another track. Put it on the growls or on the sub and route the clap track into its
sidechain; pick the job:

| Job | What it does on each clap | Defaults |
|---|---|---|
| Growls: duck | Everything above *Above* (150 Hz) goes down by *Duck by* from the clap until *Back at*, then returns over 30 ms. Below the split nothing changes. | 24 dB, 250 ms |
| Sub: dip | The whole signal drops 32 dB at the hit, stays down until *Down until* and is at full level at *Full at*. | 55 ms, 130 ms |

The defaults were found by ear against the first hit of the reference's second drop: 18 dB or 200 ms already let the
clap sink back, more than 24 dB / 250 ms made no difference.

It triggers when the sidechain's level above 2 kHz comes within *Trigger within* dB of the loudest recent hit, at most
once per 150 ms, and not again until the level has fallen a further 12 dB. Feed it the clap alone: on a clipped drum bus
the kick triggers it too. 5 ms of latency (reported to the host) so the gain is already down when the clap lands.
The line at the bottom shows how many claps it has reacted to and the gain right now.

The DSP and its tests live in `plugins/clapdsp`.
