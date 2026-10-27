# Delays and echoes

Three delays: a plain one with no feedback, a feedback echo whose repeats are filtered as they
fade, and a tape-and-analogue style delay that can follow the song's tempo. In the plug-in
browser they are under **Built in**, in the Delay group.

## Delay {#delay}

One copy of each channel, **Left** and **Right Channel Delay Time** later, from −500 to +500
ms, mixed in by its **Mix** (**Invert** flips its polarity). There is no feedback: one echo.

A negative time puts the copy *ahead* of the dry sound (Oxide delays the track that much and
compensates it). Use short times (10 to 40 ms) with different left and right values to widen a
mono guitar, or one longer slap-back echo. **Delay Time Units** only changes how the editor
shows the times (milliseconds, beats at the song's tempo, or samples). A time change
crossfades over 100 ms; the times have no lanes, since they set the effect's delay.

::: shot effect-delay
crop effect.editor 6
highlight effect.curve "1" "The taps on a time axis: here the left one early."
The Delay with the left copy ahead of the dry sound.
:::

::: params delay
:::

## Echo {#echo}

A feedback echo for each channel, up to 2 seconds: **Echo Level** sets the first repeat,
**Feedback** how much each repeat feeds the next, and every repeat goes through **Successive
Echo Equalization**, eight bands (**EQ Band 1** to **8**, low to high), so the echoes darken
or thin as they fade. **Echo Bounce** sends each echo to the other side (ping-pong), and
**Lock Left & Right Channels** sets both sides together in the editor. The dry sound always
passes at full level.

Use it for dub echoes, a darkening vocal throw, or rhythmic repeats. Its editor draws the EQ
curve and the repeats.

::: shot effect-echo
crop effect.editor 6
highlight effect.curve "1" "The repeats' EQ, its eight bands marked."
The Echo, its repeats' EQ drawn as a curve.
:::

::: params echo
:::

## Vintage Delay {#vintage-delay}

A delay that behaves like a tape or bucket-brigade unit, up to 8 seconds: a little wow in
the delay time, and a shaper and low-pass inside the feedback loop set by **Mode** (*Analog*,
*Tape/Tube*, *Tape*) and **Trash** (how much it dirties the repeats). **Feedback** goes to
200 %: past 100 % the repeats grow until a soft limiter holds them, for runaway self-oscillation.
**Spread** widens the wet sound. **Dry Out** and **Wet Out** mix it.

With **Tempo Sync** on, the delay is a **Sync Note** (*1/4*, *1/8 dotted*, …) at the song's
tempo, gliding through tempo changes; off, it is **Delay** in milliseconds. Changing the delay
glides like tape, rather than jumping.

Use it for guitar pedal style echoes and synced repeats. Its editor has the Mode buttons and
draws the shaper and low-pass.

::: shot effect-vintage-delay
crop effect.editor 6
The Vintage Delay's editor.
:::

::: params vintage-delay
:::

## See also

- [[page:effects-reverb]]: space rather than repeats.
- [[page:mixer#sends-and-buses]]: putting a delay on a bus.
