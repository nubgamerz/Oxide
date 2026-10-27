# The piano roll

The piano roll edits the notes of a MIDI clip: a keyboard down the left, a row per note, time
along the top, and each note's velocity in a lane at the bottom. MIDI clips live on instrument
tracks, which play them through their instrument plug-in.

::: shot piano-roll
crop pane.piano_roll 0
highlight piano_roll.toolbar "1" "The clip and its track, the note count, Snap, Quantize and ✕ (close)."
highlight piano_roll.ruler "2" "The ruler: click or drag to move the playhead."
highlight piano_roll.keys "3" "The keyboard: press a key to hear it."
highlight piano_roll.grid "4" "The notes. Double-click to draw one."
highlight piano_roll.velocity "5" "Velocity: how hard each note is played."
highlight piano_roll.scroll "6" "Scroll along the clip."
The piano roll on Keys' MIDI clip.
:::

## Opening the piano roll

There is nothing to enable or install: the piano roll opens on a MIDI clip. To open a clip in
the piano roll, do any of these:

- **Double-click the MIDI clip** in the Multitrack.
- **Right-click the clip** and choose **Open in piano roll**.
- Tick **View > Piano roll**. It opens the selected MIDI clip, or the one it showed last, or
  the project's first; it is greyed while the project has no MIDI clips.

::: shot main-window
crop clip.midi 40
highlight clip.midi "1" "A MIDI clip: double-click it, or right-click it > Open in piano roll."
Keys' MIDI clip in the Multitrack.
:::

The piano roll opens as a pane under the Multitrack (drag its tab to put it elsewhere). The
clip it shows is outlined in the Multitrack. Close it with the **✕** at the right of its
toolbar, its tab, or by unticking View > Piano roll.

There is no MIDI clip yet? Make one with **Insert MIDI track** (below), or right-click an
instrument track's lane and choose **Insert MIDI clip**, or record one from a keyboard
([[page:recording#instrument-tracks-and-midi]]).

## Instrument tracks and their plug-ins

An instrument track plays its MIDI through the instrument in its **first insert**: a synth, a
sampler or a drum plug-in. Without one it makes no
sound.

- Add the instrument with **+ Add plug-in…** in the inspector, or click an empty slot in the
  track header's rack, and pick it in the browser (type to search).
- A new instrument track is armed, so your MIDI keyboard plays it at once (its **Input** and
  **MIDI channel** in the inspector choose which keyboard).

### Inserting a MIDI track

The quickest way to start a part: right-click a track's lane where the part should go and
choose **Insert MIDI track**, then an instrument:

- under *With an instrument*, one you used lately or one already in the project;
- **Choose instrument…** to pick one in the browser;
- **No instrument** to add it later.

Oxide adds an instrument track right after the one you clicked, armed and named after its
instrument, with an empty MIDI clip over the time selection (whole bars with Snap on), or two
bars from the bar you clicked. The clip opens in the piano roll, ready to draw.

::: shot insert-midi
crop pane.piano_roll 0
highlight piano_roll.grid "1" "The new, empty clip, ready to draw in."
A MIDI track inserted over a time selection.
:::

## Drawing and editing notes

| to | do |
|---|---|
| draw a note | double-click an empty spot: a note one grid step long, at velocity 100 |
| select a note | click it |
| add to or take from the selection | **Shift**+click a note |
| select several | drag a box around them on empty space (**Shift** adds to the selection) |
| select all, or none | [[Ctrl+A]], or [[Esc]] |
| move notes | drag a selected note: up and down changes pitch, sideways changes time |
| make notes longer or shorter | drag a note's left or right end |
| erase notes | right-click a note (all the selected ones, if it is selected), or [[Delete]] |
| copy, cut and paste notes | [[Ctrl+C]], [[Ctrl+X]], then [[Ctrl+V]] pastes at the playhead (below) |
| place without snapping | hold **Alt** while you draw, drag or click |

You hear the notes as you work: a note sounds when you draw it or click it, and while you drag
it, at each new pitch, through the track's instrument. Press a key of the keyboard on the left
to hear that pitch, and drag along the keys to play more.

The piano roll's keys work while the pointer is over it, or, with the pointer over no other
pane (on the menu bar or the transport, say), when the piano roll was the last pane you clicked:
[[Delete]] erases notes there (never the clip), [[Ctrl+A]] selects the clip's notes, [[Q]]
quantizes, and [[Ctrl+C]], [[Ctrl+X]] and [[Ctrl+V]] copy, cut and paste notes. With the
pointer over the Multitrack the same keys act on clips. The toolbar's grey line
sums up the mouse: *Double-click: draw · Drag: move · Edges: resize · Alt: no snap ·
Right-click/Del: erase · Ctrl+wheel: zoom*.

## Copying and pasting notes

1. Select the notes to copy (click, **Shift**+click or drag a box).
2. Press [[Ctrl+C]] to copy them, or [[Ctrl+X]] to cut them (copy and erase).
3. Put the playhead where the first of them should start: click the piano roll's ruler or an
   empty spot of its grid.
4. With the pointer over the piano roll, press [[Ctrl+V]]. (Over the Multitrack, [[Ctrl+V]]
   pastes clips: if you placed the playhead there, move the pointer back first.) The earliest
   copied note starts exactly at the playhead (not snapped) and the others keep their places
   around it: the same distances, pitches, lengths and velocities. The pasted notes are
   selected, so you can drag them on or paste again.

Notes paste into the clip open in the piano roll, so you can copy notes from one clip, open
another and paste them there. Notes reaching past the clip's end make it longer, to the bar line
after them, as drawing a note there does. With the playhead before the clip's start nothing is
pasted and Oxide says why. Each cut or paste is one step for **Edit > Undo**.

The Edit menu's **Cut**, **Copy** and **Paste at playhead** do the same while the piano roll
was the last pane you clicked. Copying notes doesn't replace copied clips: [[Ctrl+V]] over the
Multitrack still pastes those.

## Velocity

The lane at the bottom shows each note's velocity (1 to 127) as a bar. Drag a bar up or down to
change it; with the note selected, every selected note follows. Higher is played harder: most
instruments play louder and brighter.

## Snap, the grid and the playhead

**Snap** in the toolbar is the piano roll's own switch, over the song's grid:

- **Off** places notes freely (**Alt** does the same for one drag).
- **Auto (…)**, **1/4** to **1/64** and **Triplets** pick the grid, the same setting as the
  Multitrack's ruler menu ([[page:multitrack#the-grid-and-snap]]): choosing one here changes it
  there too.

Click the ruler or an empty spot of the grid to put the playhead there (snapped, unless Alt is
held); drag the orange line or the ruler to move it.

## Quantize

**Quantize** in the toolbar (or [[Q]] with the pointer over the piano roll) moves the starts of
the selected notes, or of all notes when none is selected, onto the grid. It is greyed while
Snap is off. Undo takes it back.

## Moving around

| to | do |
|---|---|
| scroll up and down the notes | mouse wheel |
| scroll along the clip | **Shift**+wheel, or drag the scroll bar under the grid |
| zoom in and out around the pointer | **Ctrl**+wheel, or pinch |

## Splitting MIDI clips

Split a MIDI clip in the Multitrack as an audio clip: put the playhead where to cut and press
[[S]] with the clip selected, or right-click it and choose **Split at playhead (S)**. A note
that crosses the cut ends there and a copy starts the second piece; the controllers in force
(sustain, for example) carry on. MIDI clips can't yet be trimmed by dragging their edges.

::: shot midi-split
crop pane.multitrack pane.piano_roll 0
highlight clip.midi "1" "The first piece, open in the piano roll."
dim
Keys' clip split through a chord.
:::

## MIDI files

- **Import:** File > Import MIDI… puts a MIDI file at the playhead on the selected track; or
  drag `.mid` files from Explorer onto the Multitrack. A file with several tracks (a piano's
  two staves, say) needs an instrument track for each, the one under the pointer and those
  under it. If its tempo or time signature differs from the
  project's, Oxide asks **Use the MIDI file's tempo?**: **Use the file's tempo** or **Keep the
  project's tempo** (the notes keep their beats either way).
- **Export a clip:** right-click it > **Export as MIDI file…**.
- **Export a track's MIDI:** right-click its header (or the Track menu) > **Export MIDI…**.

## Over MCP

An agent reads a clip with [[action:get_midi_clip]] and [[action:describe_midi_clip]] (key,
chords, note names), writes notes with [[action:add_notes]], [[action:set_notes]],
[[action:remove_notes]] and [[action:resize_notes]], copies and pastes them with
[[action:copy_notes]], [[action:cut_notes]] and [[action:paste_notes]], quantizes with [[action:quantize_notes]],
adds tracks and clips with [[action:insert_midi_track]] and [[action:add_midi_clip]], plays a
note with [[action:preview_note]], and moves MIDI files with [[action:import_midi_file]],
[[action:export_midi_clip]] and [[action:export_midi_track]]. The `write_midi_part` prompt
guides an agent through writing a part.

## See also

- [[page:recording#instrument-tracks-and-midi]]: playing and recording a keyboard.
- [[page:multitrack#clips]]: moving, copying and splitting clips.
