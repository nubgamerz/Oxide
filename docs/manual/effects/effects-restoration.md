# Noise reduction and restoration

Effects that take something out: steady noise and hiss, hum, clicks and crackle, and the
flattened peaks of clipped audio. In the plug-in browser they are under **Built in**, in the
Restoration group.

They are inserts like any effect, so they can run on a track while it plays, but they are
most at home in the Waveform view's effects rack, where you can teach them from a piece of
the file and then write the result into it ([[page:waveform-view#noise-prints-and-learning-from-audio]]).
For one-off clicks and clipped runs, the Waveform view's Diagnostics pane finds and repairs
them one by one ([[page:waveform-view#diagnostics]]).

## Noise Reduction {#noise-reduction}

Use it to remove a steady noise (amp hiss, a fan, room tone) that it has learned from a **noise
print**: a stretch of the noise alone. For each frequency it subtracts what the print says is
noise, and turns down what is left under it by **Reduce By**.

To use it, select half a second or more of the noise alone in the Waveform view and choose
Effects > **Capture noise print / clip levels**, then process the whole file. Without a print
it does nothing.

- **Noise Reduction** (%): how much of the noise to take out; lower it if the music starts to
  sound watery.
- **Reduce By**: how far the noise is turned down.
- **Spectral Decay Rate**, **Smoothing**, **Precision Factor**, **Transition Width**: how
  smoothly it works over time and frequency, against the warbling artefacts heavy reduction
  can leave.
- **FFT Size**: larger resolves frequency better (good for hum-like noise), smaller follows
  time better (good for drums); the delay is the FFT size.
- **Output Noise Only** plays what it takes away: you should hear only noise.

::: shot noise-reduction
crop effect.editor 6
highlight effect.curve "1" "The noise print it learned, by frequency."
A noise print from the example guitar recording's first half second.
:::

::: params noise-reduction
:::

## Hiss Reduction {#hiss-reduction}

Turns down quiet, high-frequency hiss (tape, an old recording, a noisy preamp) under a
threshold. Without a capture it uses a flat threshold set by **Noise Floor**; with one
(Effects > **Capture noise print / clip levels** on a stretch of hiss), the hiss's own level
at each frequency, raised by Noise Floor. What is under the threshold is turned down by
**Reduce By**; **Transition Width** makes the change gradual. **Output Hiss Only** plays what
it takes away.

::: shot effect-hiss-reduction
crop effect.editor 6
Hiss Reduction at its defaults, no noise floor captured.
:::

::: params hiss-reduction
:::

## Auto Noise Reduction {#auto-noise-reduction}

Reduces noise without a print: it learns the noise floor as the audio plays and follows it as
it changes. **Reduce Noise By** sets how far; **Noisiness** how much of the sound counts as
noise; **Noise Floor** and **Signal Threshold** where noise ends and music starts; **Spectral
Decay Rate** how fast the reduction lets go; **Broadband Preservation** keeps wideband sounds
(attacks) under that frequency width intact.

Use it on noise that changes (a room with traffic, a fan that speeds up) or when there is no
stretch of noise alone to learn from. Noise Reduction with a good print usually sounds
cleaner on steady noise.

::: shot effect-auto-noise-reduction
crop effect.editor 6
Auto Noise Reduction at its defaults.
:::

::: params auto-noise-reduction
:::

## Hum Remover {#hum-remover}

Removes mains hum (50 Hz in Europe, 60 Hz in America) and its harmonics with deep, narrow
notches: one at **Frequency** and one at each harmonic above it, up to **Number of
Harmonics**. **Q** is how narrow they are, **Gain** how deep the first is, and **Harmonic
Slope** makes the higher ones shallower. **Output Hum Only** plays just the hum.

Use it on a guitar recorded near a lamp dimmer or with a ground loop. Its curve marks each
harmonic. It adds no delay.

::: shot effect-hum-remover
crop effect.editor 6
highlight effect.curve "1" "The notches, each harmonic marked."
The Hum Remover set to 60 Hz and its harmonics.
:::

::: params hum-remover
:::

## Click Remover {#click-remover}

Finds clicks and crackle and rebuilds them from the audio on both sides. **Threshold**
(higher finds fewer clicks, only the louder ones) and **Complexity** (higher takes longer
clicks, with a longer model) set it; the editor also gives what they mean in the
engine's terms. Everything that isn't a click comes back exactly as it went in, 4096 samples
later (compensated).

Use it on vinyl transfers, crackly cables, mouth clicks. The Waveform view's Diagnostics
pane runs the same detector and repair as a list you can check click by click.

::: shot effect-click-remover
crop effect.editor 6
The Click Remover's editor.
:::

::: params click-remover
:::

## Click & Pop Repair {#click-pop-repair}

A thorough click and pop remover. It decides what is a click
against a detection threshold that follows the audio's level along a curve through three
points (**Max**, **Avg** and **Min Threshold** at **Max**, **Avg** and **Min Level**), checks
each one again against a rejection curve (**Second Level Verification**), can **Detect Big
Pops**, verify pulse trains, link the channels and run several passes, and repairs as the
Click Remover does. Its editor draws both curves and counts what it corrected and
rejected.

Set it from the audio rather than by hand: select the crackly audio in the Waveform view and
click **Scan For Threshold Levels** (the levels) or **Scan For All Levels** (the thresholds
too) in its editor, or choose Effects > **Capture noise print / clip levels**. Its delay is
22016 samples, compensated.

::: shot effect-click-pop-repair
crop effect.editor 6
highlight effect.curve "1" "The detection and rejection curves, with their points."
The Click & Pop Repair with the example recording's levels scanned.
:::

::: params click-pop-repair
:::

## Clip Repair {#clip-repair}

Rebuilds the tops of clipped peaks: runs of samples stuck near the highest level, as an
overdriven preamp or a too-hot recording leaves them. A scan learns each channel's clip
levels from the audio; then every run within **Tolerance** of them, at least **Min. Clip
Size** long (and never quieter than −24 dBFS), is redrawn from the audio on either side.
Rebuilt peaks rise above the old clip level, so the output is turned down by **Gain** (or
**Auto Gain**, from the scan) and held under −0.1 dBFS by a limiter.

To scan: select the clipped audio in the Waveform view and click **Scan the Waveform view's
selection** in the editor, or choose Effects > **Capture noise print / clip levels**. Not scanned, it takes clipping at full scale.
For a few runs, the Diagnostics pane's Clip Repair lets you check and repair them one by one.

::: shot effect-clip-repair
crop effect.editor 6
Clip Repair with the example recording scanned.
:::

::: params clip-repair
:::

## See also

- [[page:waveform-view#noise-prints-and-learning-from-audio]]: capturing a print.
- [[page:waveform-view#the-spectrogram-and-spectral-repair]]: removing one sound by hand.
