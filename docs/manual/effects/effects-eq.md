# Filters and EQ

Equalisers change the balance of low, middle and high frequencies. Both of Oxide's draw their
frequency response above their settings, so you see what each change does. In the plug-in browser they are under **Built in**, in the
EQ group.

## Parametric EQ {#parametric-eq}

A full channel EQ with fixed bands, run in this order: a **high-pass** and a **low-pass**
filter (6 to 48 dB per octave), a **low shelf** and a **high shelf** (first order, or
steeper with **Second Order**), five peaking **bands**, then **Master Gain**. Each part has
its **Enable** switch, and each band its **Frequency**, **Gain** and **Q** (or **Width**).

**Constant Q** (on by default) keeps each band's Q as you move its frequency, so it stays the
same musical width; off, its **Width** in Hz is kept instead.

Use it for everything from a high-pass under a guitar (cut the rumble below 80 Hz) to a
narrow cut on a ringing frequency (a high Q) or a broad lift of a vocal's air (the high
shelf). A setting moved while playing glides over about 30 ms, so sweeping a band doesn't
click.
It adds no delay.

::: shot effect-eq
crop effect.editor 6
highlight effect.curve "1" "The response: a high-pass at 80 Hz, a low shelf, a cut, a boost and a high shelf cut."
The Parametric EQ set, on the Guitars folder.
:::

::: params parametric-eq
:::

## Graphic EQ {#graphic-eq}

A slider per band at fixed frequencies, octaves apart (10 bands), half octaves (20) or third
octaves (30), each from −70 to +70 dB within **Range**, plus **Master Gain**. Pick the 10,
20 or 30-band one in the plug-in browser: they are three effects.

It is a linear-phase EQ: it changes the level of each frequency without shifting its timing,
so a boost or cut doesn't smear transients. That costs 7193 samples of delay (0.15 s at
48 kHz), which Oxide compensates on playback and export, but which you hear when monitoring
through it while recording: keep it off tracks you record through. A change takes effect from
its next processing block, a fraction of a second later.

Use it for broad tonal shaping by eye and ear, on a master or a bus.

::: shot effect-graphic-eq
crop effect.editor 6
highlight effect.curve "1" "The response of the bands as set."
The 10-band Graphic EQ with four bands moved.
:::

### 10 bands {#graphic-eq-10}

::: params graphic-eq-10
:::

### 20 bands {#graphic-eq-20}

::: params graphic-eq-20
:::

### 30 bands {#graphic-eq-30}

::: params graphic-eq-30
:::

## See also

- [[page:effects]]: adding effects, the editor, automation.
- [[page:effects-restoration#hum-remover]]: deep, narrow notches for hum.
