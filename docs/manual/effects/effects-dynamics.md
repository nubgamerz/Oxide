# Amplitude and compression

Effects that change the level: a plain gain, a channel matrix, a limiter, compressors, a
de-esser and a curve you draw. In the plug-in browser they are under **Built in**, in the
Dynamics group.

## Amplify {#amplify}

A gain for each channel, from −96 to +48 dB. With **Link Sliders** on (the default) both
sliders move together.

Use it to bring a quiet direct-input guitar up before an amp sim, to trim a plug-in's output, or, with the
sliders unlinked, to balance a stereo pair. It adds nothing but gain; a change glides over
10 ms, so moving it while playing doesn't click.

::: shot effect-amplify
crop effect.editor 6
Amplify on the Guitars folder, its left channel at +3 dB.
:::

::: params amplify
:::

## Channel Mixer {#channel-mixer}

Builds each output channel from both inputs: **Left from Left** and **Left from Right** make
the new left, **Right from Left** and **Right from Right** the new right, each from −100 % to
+100 % (a negative amount inverts that input's polarity).

Use it to swap left and right (Left from Right and Right from Left at 100 %, the others 0),
to fold a stereo track to mono (all four at 50 %), or to flip one side's polarity.

::: shot effect-channel-mixer
crop effect.editor 6
The Channel Mixer at its defaults: each side from itself.
:::

::: params channel-mixer
:::

## Limiter {#limiter}

Keeps every sample at or under **Maximum Amplitude**: peaks above it are turned down,
looking ahead so the gain is already down when a peak arrives, and coming back up over the
**Release Time**. **Input Boost** pushes the level into it first, to make a mix louder.
**Link Channels** turns both channels down together, so the stereo image doesn't shift.

Use it last on the master to stop peaks, or to raise a mix's loudness a few dB. It limits
sample peaks: the peaks between samples (true peak) can still pass a little over, so for a
master that must stay under −1 dBTP set the ceiling to −1.5 dB or lower, or let the export's
normalising hold the true peak ([[page:exporting#normalise-to-a-loudness-target]]).

::: shot effect-limiter
crop effect.editor 6
highlight effect.curve "1" "The output level for each input level: flat at the ceiling."
The Limiter at its defaults.
:::

::: params limiter
:::

## Compressor {#compressor}

A compressor: above the **Threshold**, the level rises only 1 dB for every **Ratio** dB in.
**Attack** is how fast it turns down when the level rises, **Release** how fast it lets go.
**Output Gain** makes up the level lost, or **Auto Makeup Gain** does it for you, adding back
the average reduction of the last two seconds. It looks ahead 1 ms.

Use it to even out a vocal or a bass, or to glue a bus. A short attack catches pick
transients; a longer one lets them through and controls the sustain.

::: shot effect-compressor
crop effect.editor 6
highlight effect.curve "1" "The curve: input level across, output up; it bends at the threshold."
The Compressor at −24 dB, 4:1.
:::

::: params compressor
:::

## De-esser {#de-esser}

Turns down sibilance (*s*, *sh*, *t*) when it crosses the **Threshold**. **Center Frequency**
and **Bandwidth** say where the sibilance is. In **Broadband** mode the whole signal dips while
an *s* sounds; in **Multiband** only that band does, leaving the voice's body alone. **Output
Sibilance Only** plays just the band it listens to, so you can tune the centre and width by
ear.

Use it on vocals, after EQ and compression have made the esses stand out.

::: shot effect-de-esser
crop effect.editor 6
The De-esser at its defaults.
:::

::: params de-esser
:::

## Dynamics {#dynamics}

A dynamics curve you draw: up to seven points (**Points**, and each point's **X**, the input
level, and **Y**, the output level, in dB). A line straight through is no change; bend it
down above a level to compress or limit, down below a level to expand or gate. The
**Detector** reads **Peak** or **RMS** level; the level detector's and the gain processor's
attack and release set how it moves, and **Lookahead Time** lets it act before the sound
arrives. **Link Channels** follows the louder channel. **Envelope Only** plays the gain it
would apply instead of the audio, to see what it does.

Use it for shapes a compressor can't make: a gentle expander under a noisy amp, a hard gate,
or upward and downward compression together.

::: shot effect-dynamics
crop effect.editor 6
highlight effect.curve "1" "The curve its points make."
The Dynamics effect at its defaults (three points, a straight line).
:::

::: params dynamics
:::

## Multiband Compressor {#multiband-compressor}

Splits the sound into four bands at three **Crossover** frequencies and compresses each on
its own (each band's **Threshold**, **Ratio**, **Attack**, **Release** and **Gain**;
**Bypass** leaves a band alone, **Solo** plays it alone), then sums them through an output
limiter (**Limiter Threshold**, **Margin**, **Attack**, **Release**; **Brickwall Limiter** for
a hard ceiling, else a soft one).

Use it on a master or a bus to control the lows without touching the highs: a boomy low end,
harsh upper mids. Its delay follows the crossovers and is compensated.

::: shot effect-multiband
crop effect.editor 6
The Multiband Compressor with band 1 and the limiter set.
:::

::: params multiband-compressor
:::

## See also

- [[page:effects]]: adding effects, the editor, automation.
- [[page:exporting#normalise-to-a-loudness-target]]: loudness without a limiter.
