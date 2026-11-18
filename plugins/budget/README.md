# Budget

Shows how loud the growls and the sub may get before they take room from the clap. It only measures; the audio passes through untouched and there is no latency.

## Setup

1. Put one copy **last** on the drums bus, one on the growls and one on the sub.
2. Set **This is** on each copy (Drums, Growls, Sub).
3. Feed the clap into the sidechain of all three: the same signal Clap Space listens to. (The drums copy falls back to its own input when the sidechain is silent, but then the kick counts as a clap too.)
4. If something after a copy changes that bus's level (its fader, the drive of a later clip), enter it as **Level change after this**. If a clip at full scale follows after that (the bus clip the growls and the sub run into), switch on **Then clipped at full scale**: the copy then reads the level as it leaves that clip, not as it enters it.

The copies share what they measure, so every copy shows the whole table.

## What the table shows

Everything is relative to the clap: the level of the drums above 150 Hz from the hit to 95 ms after it.

| Row | What is measured | Default budget (stay under the clap by) |
|---|---|---|
| Growls: right after the clap | loudest 30 ms between 95 and 500 ms after the hit, above 150 Hz | 8 dB |
| Growls: rest of the bar | loudest 30 ms from 500 ms after the hit until 60 ms before the next clap | 3 dB |
| Growls: just before the clap | the last 40 ms before the hit | 20 dB |
| Sub: under the clap | from the hit to 95 ms after it, full band | 12 dB |
| Sub: rest of the bar | loudest 30 ms from 500 ms after the hit until 60 ms before the next clap | 0 dB |

Each row shows where it is **now** ("5.2 dB under"), its **budget** (a slider: how far under the clap it has to stay) and what is **left** ("2.8 dB left" or "OVER by 1.4 dB"). Every value is the loudest of the last four claps. Readings disappear two seconds after the claps stop.

The growl defaults are the balance that sounded right in the tough session (growls 2.5 dB down, softer return after the clap); the sub defaults follow the reference track. They are starting points: when a mix sounds right, set each budget to what the row reads.
