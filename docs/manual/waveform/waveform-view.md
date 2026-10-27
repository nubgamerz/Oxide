# The Waveform view

The Waveform view edits one audio file on its own, sample by sample: cut, fade, normalise,
repair clicks and clipping, take out noise, run effects into the file, tag it and export it.
It is where a mixdown gets its final touches before it goes out.

Switch between it and the Multitrack with **Waveform** and **Multitrack** in the menu bar, or
[[Ctrl+Tab]].

::: shot waveform
crop pane.waveform 0
highlight waveform.file "1" "The toolbar (below)."
highlight waveform.selection "2" "A selection."
highlight waveform.rack "3" "The effects rack."
The Waveform view on a guitar recording, part of it selected.
:::

::: shot waveform
crop waveform.file waveform.zoom 10
highlight waveform.file "1" "The open file: pick another open file here; **✕** closes it."
highlight waveform.menu.edit "2" "**Edit**, **Effects** and **History**."
highlight waveform.menu.effects
highlight waveform.menu.history
highlight waveform.info "3" "Its rate, channels and length; the selection's times beside it."
highlight waveform.display "4" "Waveform, Spectral or Both."
highlight waveform.time "5" "The time display: Time, Samples, or Bars and beats."
highlight waveform.zoom "6" "Zoom: **−**, **+**, **Fit**."
The toolbar.
:::

## Opening a file

- **A clip's file:** double-click an audio clip in the Multitrack. Its whole file opens, and
  edits you save play in every clip that uses it.
- **Any file:** File > **Open audio file…** (WAV, FLAC, MP3 or AIFF), or **Open audio file…**
  in the empty view. It isn't added to the project: Oxide copies it into the project's
  `Imported` folder ([[page:getting-started#the-project-folder]]) and opens the copy, so the
  original is never changed.

Each open file has a tab of its own beside the Multitrack's, and the list at the left of the
toolbar switches between them. **✕** beside the list closes the file; with unsaved edits it
asks first (**Close without saving** or **Cancel**). The file shows at its own sample rate,
each channel in a lane of its own.

## Moving around and playing

| to | do |
|---|---|
| place the playhead | click the waveform |
| go to the start | [[Home]] |
| zoom in or out | [[=]] / [[-]], **+** / **−**, or **Ctrl**+wheel around the pointer |
| see the whole file | [[F]] or **Fit** |
| scroll | the mouse wheel |
| play, stop | [[Space]] or the transport's Play and Stop |

Zoomed right in, each sample is a dot on a line, up to 32 points a sample. The time display
counts **Time** (seconds), **Samples**, or **Bars and beats**: the song's tempo and meter
from the file's start, with the song's grid over the waveform (selections and the playhead
still go to the sample).

Play plays the selection, or from the playhead, on its own through the master's outputs and
the view's effects rack. It is never part of the project: it stops when the project plays,
it is silent while the master is muted, and no export contains it.

## Selections

- **Drag** across the waveform to select a range; the toolbar shows it in seconds (the
  sample numbers on hover).
- **Alt**+drag selects in one channel only: edits and effects then touch that channel alone.
- Click, or [[Esc]], to clear it.

Edits act on the selection, or on the whole file when nothing is selected.

## Editing

The toolbar's **Edit** menu:

| item | does |
|---|---|
| **Cut** ([[Ctrl+X]]), **Copy** ([[Ctrl+C]]) | the selection, to paste into any open file |
| **Paste** ([[Ctrl+V]]) | over the selection, or in at the playhead; audio at another rate is converted |
| **Delete** ([[Delete]]) | removes the selection, closing the gap |
| **Trim to selection** | keeps only the selection |
| **Insert silence…** | that many seconds of silence at the playhead |
| **Silence** | the selection (or the whole file) becomes silence |
| **Save** ([[Ctrl+S]]), **Save As…** ([[Ctrl+Shift+S]]) | [[page:#saving-and-undo]] |

The **Effects** menu:

| item | does |
|---|---|
| **Amplify…** | a gain, −24 to +24 dB |
| **Normalize…** | to a **Peak level** (dBFS, −1 at first) or a **Loudness (integrated, EBU R 128)** (LUFS, −16 at first) |
| **Fade in**, **Fade out** (linear or equal power) | over the selection |
| **Capture noise print / clip levels** | [[page:#noise-prints-and-learning-from-audio]] |
| **Reverse**, **Invert** | plays the selection backwards; flips its polarity |
| **Attenuate box…**, **Remove box**, **Heal box** | [[page:#the-spectrogram-and-spectral-repair]] |

Equal-power fades keep the level steadier through the fade; linear ones are straight lines.

## Saving and undo

Each edit is a step in the file's own **History** (the toolbar's History menu lists them,
marking the one saved): [[Ctrl+Z]] and [[Ctrl+Y]] (or [[Ctrl+Shift+Z]]) undo and redo it,
and the project's own undo is untouched. An asterisk after the file's name means unsaved
edits.

**Save** ([[Ctrl+S]]) **overwrites the file**. It asks first, naming the
clips in the project that will play the new audio:

::: shot waveform-edit
crop waveform.dialog 8
The question before saving over a file.
:::

The file keeps its format (a 24-bit WAV stays 24-bit). It is written beside the old one and
then renamed over it, so a failed save never leaves half a file, and every clip on it plays
the new audio at once. To put the file back, undo and save again.

FLAC, MP3 and AIFF files can't be saved over: **Save As…** ([[Ctrl+Shift+S]]) writes the
audio to a new WAV (*name* edit.wav). To write another format, export it
([[page:#exporting-and-tags]]).

## The spectrogram and spectral repair

The display menu shows the **Waveform**, a **Spectral** frequency display, or **Both**
(the waveform above). The spectrogram runs from 20 Hz at the bottom on a log scale, its
frequencies labelled on the right, coloured from black (quiet) to pale yellow (loud). The
number beside the tools is its range: how far below full scale is black (100 dB at first;
a full-scale sine is 0 dB).

With the spectrogram showing, the **Box** tool ([[E]]) drags a box in time *and* frequency:
a squeak, a string noise, a click, a cough under a held chord (**Alt**: one channel only).
**Time** ([[T]]) goes back to selecting time ranges. Then the Effects menu:

- **Attenuate box…** turns what is inside down by up to 60 dB (−12 at first);
- **Remove box** ([[Delete]]) takes it out;
- **Heal box** fills it from the sound just before and after it: a spot heal for clicks,
  squeaks and string noise.

Only the box changes: audio more than a frame (about 43 ms) away from it in time is untouched,
and other frequencies at the same time move by less than 0.1 dB.

::: shot spectral
crop pane.waveform 0
highlight waveform.tool "1" "**Box** and **Time**."
highlight waveform.range "2" "The spectrogram's range."
highlight waveform.box "3" "A box, 1 to 4 kHz."
A guitar recording with its spectrogram under the waveform.
:::

## The effects rack

The **Effects Rack** at the left of the view holds plug-ins as a track's rack does: the
built-in effects and any VST3. The file plays through them, so you hear the result before
writing it; **Apply** then writes them into the file.

1. Click an empty slot and choose a plug-in (as on a track, [[page:mixer#inserts-adding-plug-ins]]).
   Click a plug-in's name for its window; its power button bypasses it; drag its number to
   reorder.
2. Play, and set the plug-ins as you like.
3. Select the part to process, or nothing for the whole file. The rack says which
   (*Process: the selection* or *Process: the entire file*) and the delay its plug-ins add
   (*Latency 810 samples, compensated*).
4. Click **Apply**. It renders in the background, with a bar and **Cancel**.

Apply writes exactly what you heard (the same plug-ins, settings and block size, the delay
compensated), as one step of the file's history; [[Ctrl+S]] saves it. A plug-in bypassed or
removed while Apply runs changes the next Apply, not this one.

The rack belongs to the view, not to a file or a project: it is kept when Oxide closes,
settings and all, and comes back the next time (its plug-ins loading one at a time behind the
busy bar). A plug-in that no longer loads keeps its slot and its settings, greyed, saying why.

### Rack presets

**Presets ▾**, above the rack, keeps racks by name for every project:

- **Save rack as…** saves this rack (its plug-ins and their settings) under a name;
- a preset's name loads it, replacing the rack;
- **Delete** removes one.

Presets live in `RackPresets` beside the program. They can't be loaded while the rack is synced
with a track.

### Sync the rack with a track

A file opened from a clip (double-click the clip) has **Sync with** *track* above the rack.
Tick it, and your own rack is set aside: the rack becomes copies of that track's effects with
their settings, so the file plays as the track plays its clip in the Multitrack. From then on
they are one rack: adding, removing, moving or bypassing an effect in either view does the
same in the other, and turning a knob in either plug-in window turns its twin.

::: shot rack-sync
crop waveform.rack waveform.rack.sync waveform.rack.presets waveform.rack.apply 6
highlight waveform.rack.sync "1" "**Sync with Guitar L**: ticked."
highlight waveform.rack.presets "2" "**Presets ▾**."
highlight waveform.rack.apply "3" "**Apply**."
The rack synced with Guitar L's effects.
:::

Untick it, close the file, or remove the track, and your own rack comes back. The track's
fader, pan, sends and automation aren't part of the rack: it plays the effects as set by hand.

**Apply** while synced asks first: **Apply and turn off on** *track* writes the effects into
the file and then switches those effects off on the track, so the clip isn't processed twice;
**Apply anyway** leaves the track's effects on.

## Noise prints and learning from audio

Some effects learn from a piece of audio: [[effect:noise-reduction]] and
[[effect:hiss-reduction]] a noise print, the [[effect:clip-repair]] the levels it clips at, the
[[effect:click-pop-repair]] its threshold levels. To teach one:

1. Put the effect in the rack (Noise Reduction is added if the rack has none of them).
2. Select a stretch of the noise alone: the hiss before the guitar comes in, the hum between
   phrases (or, for Clip Repair, the clipped audio).
3. Choose Effects > **Capture noise print / clip levels**.

The rack's first effect that learns takes it, and its window shows what it learned (a noise
print's level by frequency). Then select the whole file, play to check, and **Apply**.

::: shot noise-reduction
crop effect.editor waveform.rack 6
highlight effect.curve "1" "The noise print, by frequency."
A noise print taken from the recording's first half second into Noise Reduction.
:::

What an effect learned is saved with its settings (in the rack, kept between runs). The [[effect:clip-repair]]'s and [[effect:click-pop-repair]]'s windows also have scan
buttons of their own, which read the Waveform view's selection.

## Diagnostics: find and repair clicks and clipping {#diagnostics}

The **Diagnostics** pane (View > **Diagnostics**) finds clicks or clipped runs in the
Waveform view's file, lists them, and repairs the ones you pick.

1. Pick **Click Repair** (clicks, pops) or **Clip Repair** (clipped runs).
2. Set it: Click Repair's **Threshold** (higher finds fewer clicks, only the louder) and
   **Complexity** (higher takes longer clicks); Clip Repair's **Tolerance** and **Min. clip
   size** (the shortest run of samples taken for clipping).
3. Click **Scan selection** or **Scan file**.
4. Click a row to go there: the view zooms to it and selects it (**Ctrl** and **Shift** pick
   more rows). Listen.
5. **Repair selected**, or **Repair all**.

::: shot diagnostics
crop pane.diagnostics 0
highlight diagnostics.kind "1" "Click Repair or Clip Repair."
highlight diagnostics.settings "2" "Its settings."
highlight diagnostics.scan "3" "**Scan selection**, **Scan file**."
highlight diagnostics.list "4" "What was found: click a row to go there."
highlight diagnostics.repair "5" "**Repair selected**, **Repair all**."
Clicks found in a copy of a guitar recording.
:::

The list gives each problem's **Time**, channel (**Ch**), length in **Samples** and
**Level** (a click's size, a clipped run's highest sample); click a column's header to sort by
it. A repair is one step of the file's history, and the repaired rows leave the list; any
other edit, undo or redo of the file clears the list.

Repairing all is exactly what the [[effect:click-remover]] or the [[effect:clip-repair]]
plays on the same audio, but without any gain: rebuilt peaks can rise above the old clip
level, so normalise afterwards if they do.

## Exporting and tags

**File > Export file…** writes the open file, unsaved edits included, to a new file in any
format Oxide exports (WAV, AIFF, FLAC, MP3, Ogg Vorbis), leaving the file you are editing as
it is. The choices are the export dialog's ([[page:exporting#formats]]).

::: shot waveform-export
crop export 36
highlight export.format "1" "Format and its settings, sample rate."
File > Export file… set to an MP3.
:::

**View > Metadata…** edits the file's tags: **Title**, **Artist**, **Album**, **Album
artist**, **Year**, **Track**, **Genre**, **Composer**, **Comment** and a cover image (**Choose
image…**: a JPEG or PNG; **Remove**). **Apply** makes it a step of the file's history, and
Save or Export file… writes it: as ID3v2.3 in MP3s (the version Windows Explorer reads),
Vorbis comments in FLAC and Ogg, INFO and ID3 chunks in WAV, ID3 in AIFF. Tags are read when a
file opens.

::: shot metadata
crop metadata 6
View > Metadata… on an example file.
:::

Exports from the Multitrack carry no tags: mix down to a new file first
([[page:rendering#mix-down-to-a-new-file]]), then tag and export it here.

## Over MCP

An agent opens and edits files with [[action:open_audio_editor]], [[action:get_audio_editor]],
[[action:edit_audio]] (every Edit and Effects item, the spectral ones included),
[[action:undo_audio_edit]], [[action:copy_audio]], [[action:save_audio]],
[[action:save_audio_as]], [[action:play_audio_editor]] and [[action:close_audio_editor]];
[[action:get_spectrum]] reads a range's spectrum. The rack: [[action:add_insert]],
[[action:apply_audio_effects]], [[action:set_rack_sync]], [[action:list_rack_presets]],
[[action:save_rack_preset]], [[action:load_rack_preset]], [[action:delete_rack_preset]],
[[action:capture_noise_print]]. Diagnostics: [[action:find_audio_problems]],
[[action:get_audio_problems]], [[action:repair_audio_problems]]. Tags:
[[action:get_metadata]], [[action:set_metadata]]; exporting: [[action:export_audio]].

## See also

- [[page:effects]]: the built-in effects for the rack.
- [[page:exporting]]: formats, dither and loudness.
- [[page:keyboard#the-waveform-view]]: the view's keys.
