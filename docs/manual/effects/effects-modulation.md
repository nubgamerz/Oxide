# Modulation

Effects that move: a chorus of wandering voices, a flanger's sweeping comb, a phaser's moving
notches. In the plug-in browser they are under **Built in**, in the Modulation group.

## Chorus {#chorus}

Thickens a sound with copies of it (**Voices**, up to 16), each a little later than the dry
sound by about **Delay Time**, its delay and level wandering slowly (**Delay Rate**,
**Modulation Rate** and **Depth**), spread in time by **Spread** and across the stereo field by
**Stereo Field**. **Feedback** feeds the voices back, **Add Binaural Cues** gives headphone
listeners extra width, **Average Left & Right Channel Input** starts from mono, and **Highest
Quality** reads the voices more finely (dearer with many voices). **Dry** and **Wet Output
Level** mix it, up to 200 %.

Its wandering is random but repeatable: it starts the same way every time playback starts, so
an export sounds like playback. Only the two output levels have lanes. Its editor draws the
voices by delay and pan.

Use it to widen a clean guitar or a pad, or for a doubled vocal.

::: shot effect-chorus
crop effect.editor 6
highlight effect.curve "1" "The voices, by delay and pan."
The Chorus with four voices.
:::

::: params chorus
:::

## Flanger {#flanger}

Mixes the sound with a copy whose delay sweeps between **Initial** and **Final Delay Time**,
at **Modulation Rate**, making the jet-like swoosh of a flanger; **Feedback** sharpens it, and
**Stereo Phasing** puts the right channel's sweep ahead of the left's. **Sinusoidal Mode**
sweeps in a sine (off: a triangle). Normally two taps sweep either side of a fixed centre, so
the flange passes through zero where they meet; **Special Effects Mode** uses one tap
instead. **Inverted Mode** flips the copy's polarity. **Mix** sets how much you hear.

Its delay is 5082 samples (the centre, compensated), which is what lets it sweep through
zero. The editor draws the sweep for each side.

::: shot effect-flanger
crop effect.editor 6
highlight effect.curve "1" "The sweep, left and right."
The Flanger's editor.
:::

::: params flanger
:::

## Phaser {#phaser}

Sweeps notches through the sound: each of the **Stages** (1 to 6) makes one notch, **Intensity**
sharpens them, **Depth** sets how far they sweep under **Upper Freq**, at **Mod Rate**, in a
**Triangle** or **Sine** **Shape**, on a **Logarithmic** or **Linear** **Step**. **Feedback**
raises peaks between the notches, **Phase Diff** sets the right channel's sweep ahead of the
left's (**Sync Channels** keeps them together), and **Mix** and **Output Gain** set the
output.

Use it on guitars, keys and pads for movement. The editor draws the response at both ends of
the sweep. It is Oxide's own design, so unlike the other effects it isn't checked against a reference
render.

::: shot effect-phaser
crop effect.editor 6
highlight effect.curve "1" "The response at both ends of the sweep."
The Phaser with six stages.
:::

::: params phaser
:::

## See also

- [[page:effects-delay]]: echoes and delays.
- [[page:automation]]: moving a mix or rate over time.
