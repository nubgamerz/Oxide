# Reverbs

Three reverbs: an algorithmic one that is light and quick to set, a convolution
reverb that plays a real room's impulse response, and one that builds an impulse from a room
you describe. In the plug-in browser they are under **Built in**, in the Reverb group.

Put a reverb on a **bus** and send tracks to it, rather than on each track: one reverb gives
them one space, and costs once ([[page:mixer#sends-and-buses]]). On a bus, turn its dry
level down (the tracks' own sound already reaches the master directly) and leave only the
wet.

## Room Reverb {#room-reverb}

A classic algorithmic reverb (a Freeverb-style network): **Room Size** and **Decay** set the
space and how long it rings, **Damping** darkens the tail, **Diffusion** smears the echoes
together, **Early Reflections** and their delay give the first bounces, **Width** spreads it,
and **Low** and **High Frequency Cut** filter what goes in. **Dry** and **Wet** mix it.

Use it for everyday vocal and guitar space. It adds no delay and is cheap. Changing **Room
Size** empties the reverb, so it has no lane; its editor sketches the decay.

::: shot effect-room-reverb
crop effect.editor 6
highlight effect.curve "1" "The decay, with its times."
The Room Reverb on the Reverb bus.
:::

::: params room-reverb
:::

## Convolution Reverb {#convolution-reverb}

Plays the sound of a real space (or a hardware reverb, a speaker cabinet) from an **impulse
response**: a recording of that space's answer to a click. Load one with **Load impulse…** in
its editor: a WAV, AIFF or FLAC file up to 30 seconds. Oxide copies it into the project's
`Imported` folder, so the project keeps working if the original moves.

**Room Size** keeps part of the impulse (fading its end), **Damping LF** and **HF** reshape its
lows and highs as it decays, **Pre-Delay** adds a gap before it, **Width** spreads a stereo
impulse, and **Mix** and **Gain** set the output. A stereo impulse plays left on left and right
on right; any other is averaged to one.

Changing the impulse or its shaping prepares a new one in the background; the old one keeps
playing (and rings out) until it is ready, so these settings have no lanes. Without an impulse
the sound passes unchanged. Its delay is 256 samples at any buffer size.

::: shot effect-convolution-reverb
crop effect.editor 6
highlight effect.curve "1" "The impulse it plays, left above right, with its length and format."
The Convolution Reverb on the Reverb bus, a room impulse loaded.
:::

::: params convolution-reverb
:::

## Space Reverb {#space-reverb}

A convolution reverb whose impulse is made from the room you describe: early reflections from
a room of **Room Size** and **Dimension** (width over depth) with you at **Left/Right
Location**, a late reverb of **Decay Time**, **Diffusion** and **Perception**, a
**Pre-Delay**, and a three-band **Coloration** that darkens the tail as it decays. **Sum
Inputs** makes it mono in, stereo out (off: true stereo); **Include Direct** keeps the direct
sound in the early part. The three **Output Level**s mix the dry sound, the early reflections
and the reverb.

The same settings always make the same impulse, so a render sounds like playback. Making one
takes a moment off the audio thread (the editor shows it, and the impulse made); the old one
plays until then, so only the three output levels have lanes. The room's width, height and
depth are worked out from the size and dimension and shown, not set. Its delay is 256
samples.

Use it for realistic rooms and halls, when you want a space you can describe rather than
an impulse you have.

::: shot effect-space-reverb
crop effect.editor 6
highlight effect.curve "1" "The impulse made: the late reverb's envelope and the early reflections."
The Space Reverb's editor, with the impulse it made.
:::

::: params space-reverb
:::

## See also

- [[page:mixer#sends-and-buses]]: sharing a reverb.
- [[page:effects-delay]]: echoes and delays.
