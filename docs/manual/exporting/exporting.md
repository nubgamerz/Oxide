# Exporting

Exporting writes the mix to a file: a WAV to master elsewhere, an MP3 or Ogg to share, a FLAC
to keep. An export runs through the same engine, plug-ins, block size and settings as
playback, so it sounds exactly like what you heard; nothing is added unless you ask for it
(normalising, dither, a sample-rate change).

## Export a mixdown

1. Choose File > **Export mixdown…**.
2. Pick the **Format** and its settings, the **Sample rate**, **Dither**, and whether to
   **Normalise** ([[page:#formats]], [[page:#normalise-to-a-loudness-target]]).
3. With a time selection in the Multitrack, tick **Only the time selection** to export just
   that; untick it for the whole project.
4. **Tail after the end** adds time after the last clip (2 s at first, up to 60) so reverbs and
   delays ring out.
5. Click **Export…**, choose the file's name and folder (the project's `Exported` folder at
   first), and **Save**.

A bar shows the export as it renders and then writes the file; **Cancel** stops it. When it
is done, the export's report opens ([[page:#the-export-report]]).

::: shot export
crop export 36
highlight export.format "1" "Format, its settings, sample rate and dither."
highlight export.normalise "2" "Normalise to the project's loudness target."
highlight export.tail "3" "Time after the end, for tails."
File > Export mixdown…, set to a 24-bit FLAC.
:::

The dialog remembers its choices for next time. **Export…** is greyed while a stretched clip
is still being rendered (the dialog says how many); a project with no clips has nothing to
export. Exports from the Multitrack carry no tags: to tag a song, mix it down to a new file and
export that from the Waveform view ([[page:rendering#mix-down-to-a-new-file]],
[[page:waveform-view#exporting-and-tags]]).

## Formats

| format | settings | for |
|---|---|---|
| **WAV** | **Samples**: 16-bit, 24-bit or 32-bit float | mastering elsewhere, archiving; 32-bit float (the default) is exactly what Oxide played |
| **AIFF** | **Samples**: 16-bit or 24-bit | Mac programs |
| **FLAC** | **Samples**: 16-bit or 24-bit; **Compression** 0 (fastest) to 8 (smallest) | lossless and about half the size; every level is lossless |
| **MP3** | **Bitrate**: **Constant** (128 to 320 kbps) or **Variable** (**Quality** V0, the best at about 245 kbps, to V9, the smallest) | sharing, players, the web |
| **Ogg Vorbis** | **Quality** 0 (smallest) to 10 (best sound) | games, the web |

Everything is encoded inside Oxide: no other program is needed. Each format is picked with the
settings it starts from (24-bit, FLAC level 5, MP3 at 320 kbps, Ogg at 6). An MP3 decodes at
exactly its length (its encoder delay is written in it).

### Export an MP3

1. File > **Export mixdown…**, **Format** *MP3*.
2. **Bitrate** *Constant* and *320 kbps* for the best quality at a fixed size, or *Variable*
   and *V0* to *V2* for a smaller file that sounds as good.
3. Keep the **Sample rate** at 44 100 or 48 000 Hz: an MP3 can only be 32 000, 44 100 or
   48 000 Hz, and the dialog says so if the rate won't do.
4. Tick **Normalise** to bring it to your loudness target, then **Export…**.

### Sample rate

**Sample rate** is *As it is* (the project's rate, the default) or 44 100, 48 000, 88 200 or
96 000 Hz, converted after the mix is rendered. Export a CD
master or an MP3 at 44 100 Hz; most video and streaming takes 48 000 Hz.

### Dither

**Dither** (offered for 16- and 24-bit files: on at first) adds a tiny, even noise (TPDF, of
one bit) before the samples are rounded to whole numbers, so quiet passages and fades fade
into noise rather than into distortion. Leave it on for 16-bit; it matters little at 24-bit.
32-bit float, MP3 and Ogg need none.

## Normalise to a loudness target

**Normalise to the** *n* **LUFS target, peaks at most −1.0 dBTP** brings the file to the
project's loudness target (the loudness panel's, [[page:#the-loudness-meter]]): it normalizes
the loudness, not the peaks. It is one gain
for the whole file, with no limiter: the sound doesn't change, only its level. If reaching the
target would push the true peak over −1 dBTP, the gain stops there, and the report says the
target wasn't reached.

To normalise to −14 LUFS for streaming:

1. Select the master and, in its **Loudness** section, set **Target** to *Streaming (-14
   LUFS)* (or type a value).
2. File > **Export mixdown…**, tick **Normalise**, then **Export…**.

To be louder than the ceiling allows, a limiter is needed before the export: a mastering
limiter plug-in, or the built-in [[effect:limiter]] on the master, with the mix pushed into it. Unticked, the
file is exactly what playback plays.

## The export report

After each export, **Exported** shows where the file went and its loudness, measured on the
file itself: **Integrated** loudness against the **Target** and how far **From the target**
it is (*on target*, *+1.2 LU (louder)*), **Loudness range**, **True peak** (red over −1
dBTP), **Max short-term** and **Max momentary**, and whether it was normalised and by how
much.

::: shot export-report
crop export.report 8
The report after a normalised export.
:::

## The loudness meter

Oxide meters the master's loudness as it plays, by the EBU R 128 standard that streaming
services and broadcasters use:

- the transport bar's **I** (integrated loudness) and **TP** (true peak); click it to show the
  panel;
- the master's mixer strip shows **Integrated**;
- the panel, in the inspector with the master selected (**Loudness**), has the rest.

::: shot loudness
crop loudness 8
highlight loudness.integrated "1" "Integrated loudness: the whole programme's, since Reset."
highlight loudness.meters "2" "M (momentary) and S (short-term), the target marked."
highlight loudness.readings "3" "The other readings."
highlight loudness.buttons "4" "**Reset**, LU or LUFS, the scale."
highlight loudness.target "5" "The target."
The master's loudness panel while the example project plays.
:::

| reading | is |
|---|---|
| **Integrated** | the whole programme's loudness since the last reset, gated so silences don't pull it down: the number streaming services compare with their target |
| **Short-term**, **Momentary** | the loudness of the last 3 s and 400 ms |
| **Max short-term**, **Max momentary** | their highest since the reset |
| **Loudness range** | how much the loudness varies (LRA, in LU); *settling* for the first minute |
| **True peak** | the highest level between samples (4× oversampled), in dBTP; red above −1 dBTP |
| **Measured** | the time measured since the reset |

The meter only measures while playing, and keeps adding up until you press **Reset**: reset
it, then play the whole song (or the part you care about) for the integrated reading. **Show
LU** gives the readings as LU from the target (+1.0 LU is 1 dB too loud), **Show LUFS** as
loudness. The scale menu sets the bars' **EBU +9** or **EBU +18** range. **Target** is
*Streaming (-14 LUFS)*, *Broadcast (EBU R 128) (-23 LUFS)*, or any value from −40 to −5 LUFS
typed beside it; it is saved with the project.

The meter listens to the master's output before the metronome, so the click never counts.

## Over MCP

[[action:export_mixdown]] exports (with a `format`, normalising, a range) and returns at once;
[[action:get_export_status]] gives its progress and then the report, and
[[action:cancel_export]] stops it. [[action:get_loudness]] reads the meter (and how far from
the target), [[action:set_loudness_target]] and [[action:reset_loudness]] set it, and
[[action:measure_loudness]] measures a WAV file.

## See also

- [[page:rendering]]: track renders and mixdowns to a new file.
- [[page:waveform-view#exporting-and-tags]]: exporting one file, with tags and a cover.
- [[page:glossary]]: LUFS, true peak, dither.
