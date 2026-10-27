# The Multitrack

The Multitrack is where a song is built: a row per track, each with its header on the left and
its clips along the timeline. Above the rows are the ruler and the marker strip; the orange
line is the playhead. This page covers tracks, moving around, clips, the grid, selections,
the loop range, markers and importing files.

::: shot main-window
crop 2 97 1136 475
highlight arrange.add_track "1" "**+ Track** adds a track."
highlight arrange.ruler "2" "The ruler: click to move the playhead, drag for the loop range."
highlight arrange.markers "3" "The marker strip."
highlight track.header "4" "A track's header."
highlight clip "5" "A clip."
The Multitrack. Under the tracks, at the bottom left, **−**, **+** and **Fit** zoom.
:::

## Tracks

### Adding a track

Click **+ Track** above the track list and choose **Audio track**, **Instrument track**,
**Folder (submix)** or **Bus (return)**. The Track menu and a right-click under the last track
offer the same; [[Ctrl+T]] and a double-click under the last track add an audio track. A new
track goes into the selected folder, or the selected track's folder.

To delete a track, right-click its header and choose **Delete track** (undo brings it back).

### A track's header

Each header holds the track's name and, from right to left: the **power** button (the track
switched on, green, or off: an off track makes no sound and uses no CPU), the **headphones**
button (input monitoring, [[page:recording#monitoring-while-playing-and-recording]]), **R**
(arm for recording), **S** (solo) and **M** (mute). Folders and buses have no R or monitoring.
Taller rows add a volume and a pan knob, the automation mode, and the track's effects rack.

::: shot header
crop track.header 4
highlight track.name "1" "The name: double-click to rename."
highlight track.volume "2" "Volume: drag up or down (Shift: finer); double-click for 0 dB."
highlight track.pan "3" "Pan."
highlight track.rack "4" "The effects rack: click an empty slot to add a plug-in, a plug-in's name to open its window."
highlight track.automation_mode "5" "Automation mode."
highlight track.power "6" "The track on or off."
A tall track header.
:::

- **Rename:** double-click the name, or right-click the header and choose **Rename**, or click
  the name at the top of the inspector. Type, then [[Enter]] (or click away) keeps it and
  [[Esc]] cancels.
- **Volume and pan:** drag the knobs, or click the number beside one and type a value (*-6*,
  *-6.5 dB*, *+3*, *-inf*; for pan *L30*, *R20*, *C*).
- **Colour:** click the **Colour** swatch in the inspector.

### Track heights and zoom

- Drag a header's bottom edge to resize that track; **Ctrl**+drag to make every track that
  height.
- Hold **Alt** and turn the mouse wheel to make every track taller or shorter together; the
  track under the pointer stays in place.
- View > Track height, or the header's right-click **Height** menu, sets Small, Medium or
  Large (and *All small*, *All medium*, *All large* for every track).

### Reordering tracks

Drag a track's header (its background, colour stripe or name) up or down: a line shows where
it will go. Dropped on the upper half of a track it goes above it, on the lower half below it,
and on the lower half of a folder it goes inside that folder. The header's right-click menu
also has **Move up**, **Move down**, **Move into folder** and **Move out of folder**. In the
mixer, drag a strip's name sideways.

### Folders, buses and the master {#folders-buses-and-the-master}

- A **folder** groups tracks: their sound is mixed into the folder, which has its own fader,
  plug-ins and mute, like a submix. Its arrow (▶/▼) shows or hides its tracks. Drag tracks
  into it, or right-click it for **Add audio track inside** and **Add instrument track
  inside**.
- A **bus** is a return track: other tracks reach it through **sends** (right-click a header >
  **Add send**), so several tracks can share one reverb.
- The **master** is the mix of every track, at the top of the list. Click **Master** beside
  + Track to select it: the inspector then shows its output, loudness and plug-ins.

A track's **Output** in the inspector chooses where it goes: its parent folder (or the master),
the master, a bus, or a pair of the device's outputs directly. Mixing has its own page,
[[page:mixer]].

### Switching a track off, rendering it

Right-click the header for **Render track…**, which renders the track through its plug-ins to a
new audio track, and **Disable track** / **Enable track**, which switches it off and on as
the power button does. Rendering is on [[page:rendering]], exporting on [[page:exporting]].

## Moving around: zoom, scroll and the playhead

| to | do |
|---|---|
| scroll the tracks up and down | turn the mouse wheel |
| scroll along the timeline | **Shift**+wheel, or drag the scroll bar at the bottom |
| zoom in or out around the pointer | **Ctrl**+wheel, or pinch on a touchpad |
| zoom in or out around the playhead | [[=]] and [[-]], or **+** and **−** at the bottom left |
| see the whole project | [[F]], or **Fit** |
| zoom to the selected clip | View > Zoom to selected clip, or a clip's right-click **Zoom to clip** |
| make waveforms look bigger | **Alt+Shift**+wheel (display only; **Wave ×N** at the bottom resets it) |

The **playhead** is where playback starts and where pastes and markers go:

- Click the ruler, an empty part of a lane, or a clip to put it there (on the grid with Snap on).
- Drag the orange line (or its head in the ruler) to move it. With Snap on it moves from grid
  line to grid line, otherwise sample by sample; dragged past the edge, the view scrolls with
  it.
- [[Home]] goes to the start.
- **Follow** in the transport bar keeps the playhead in view while playing.

## Clips

A clip is a piece of audio (or MIDI) on a track. It plays part of a file; editing a clip never
changes the file.

### Selecting clips

- **Click** a clip to select it (it is outlined in white) and its track. The playhead moves to
  the click.
- **Ctrl**+click or **Shift**+click adds a clip to the selection, or takes it out.
- [[Ctrl+A]] selects every clip; [[Esc]] selects none.
- **Drag** in an empty part of the lanes to draw a time selection: the clips inside it are
  selected too ([[page:#time-selections]]).

### Moving and copying clips

- **Drag** a clip to move it, with every other selected clip, along the timeline and onto
  other tracks. Audio clips go on audio tracks and MIDI clips on instrument tracks; where they
  can't go, a message says why.
- **Alt**+drag drags copies, leaving the originals in place.
- **The playhead magnet:** a clip whose start or end comes within a few points of the playhead
  jumps onto it, and the line lights up, even with Snap off. Hold **Ctrl** while dragging to
  turn the magnet off. Otherwise, with Snap on, the clip's start goes to the grid.

::: shot clip-magnet
crop arrange.ruler clip 8
highlight clip "1" "The dragged clip, its start held on the playhead by the magnet."
A clip dragged onto the playhead.
:::

### Trimming and stretching

- **Trim:** point at a clip's left or right edge (below its title strip) until the pointer
  changes, then drag. Dragging in hides audio; dragging back out brings it back, as far as the
  file goes. The edge snaps to the playhead and the grid as a moved clip does.
- **Stretch:** **Alt**+drag an edge to time-stretch the clip to that length, keeping its pitch
  (the left edge keeps the clip's end in place). A stretched clip's name shows its ratio
  (*×1.50*); while it renders it says *rendering…* and plays unstretched.

The inspector's **Clip** section, shown while a clip is selected, sets the same by number:

::: shot stretch
crop inspector.clip pane.inspector 0
highlight inspector.clip "1" "The selected clip's Stretch, Keep attacks, Pitch and Tempo."
dim
A stretched clip in the inspector.
:::

- **Stretch**: 2 plays twice as long at the same pitch.
- **Keep attacks**: picks and hits play unstretched and on time, and the audio between them
  is stretched instead, so they stay sharp.
- **Pitch**: transposes in semitones at the same length.
- **Tempo** with **Follow**: the clip follows the song's tempo, from the tempo it was recorded
  at (shown beside it). A following clip can be trimmed but not stretched by hand.

### Fades and clip gain

::: shot fades
crop clip 30
highlight clip.fade_in "1" "The fade-in handle."
highlight clip.fade_out "2" "The fade-out handle (here a crossfade into the next clip)."
highlight clip.gain "3" "The gain line."
A clip split in two, faded and crossfaded.
:::

- **Fades:** each clip has a small square handle at the top of its waveform, at each end. Drag
  it inwards for a fade-in or a fade-out; the curve is drawn over the waveform.
- **Crossfade:** where two clips overlap, the earlier one's fade-out and the later one's
  fade-in make a crossfade. Right-click the later clip and choose **Crossfade with previous
  clip** to make one at the join.
- **Clip gain:** drag the thin line across the clip up or down, from −60 to +24 dB.
  Double-click it for 0 dB.

### Split a clip

- [[S]] splits the selected clips at the playhead (the right-hand pieces stay selected). With
  a time selection, S cuts the clips at the selection's edges instead.
- A clip's right-click menu has **Split at playhead (S)**.

MIDI clips split the same way: a note across the cut ends there and starts again in the second
piece.

### Copy, cut, paste, duplicate and delete

| to | press | or |
|---|---|---|
| copy the selected clips | [[Ctrl+C]] | Edit > Copy |
| cut them | [[Ctrl+X]] | Edit > Cut |
| paste at the playhead, on the selected track | [[Ctrl+V]] | Edit > Paste at playhead |
| duplicate them right after themselves | [[Ctrl+D]] | Edit > Duplicate |
| delete them | [[Delete]] or [[Backspace]] | right-click > Delete (Del) |

Pasted clips keep their distances; with several tracks copied, the topmost goes on the selected
track. [[Ctrl+Insert]], [[Shift+Delete]] and [[Shift+Insert]] also copy, cut and paste. If
[[Ctrl+V]] ever does nothing (after copying files in Explorer), use Edit > Paste at playhead.

### Slicing at transients and quantizing audio

Right-click an audio clip and choose **Slice at transients…**: the window shows the attacks it
finds on the clip (**Sensitivity**: higher finds softer attacks; **Shortest gap** between two
cuts). **Slice** cuts the clip there, selects the slices and opens **Quantize** (also [[Q]],
Edit > Quantize…), which pulls the selected clips to the **Grid** by a **Strength**, with short
crossfades between them. Set the tempo to the performance's first.

::: shot quantize
crop clip 30
highlight clip "1" "The attacks found, drawn on the clip."
The slicer on a guitar clip.
:::

## The grid and Snap

The grid is the set of lines the playhead, clips, selections and markers snap to. **Snap** in
the transport bar switches snapping on and off; the grid's division belongs to the song.

To choose the division, right-click the ruler and open **Grid**:

- **Auto** picks the finest division that leaves room between lines at the current zoom, so it
  gets finer as you zoom in. The menu says which it uses now (*Auto (now 1/16)*).
- **1/4** to **1/64** are fixed divisions of a whole note (1/4 is a beat in 4/4).
- **Triplets** divides each into three instead (not with Auto).

::: shot grid-menu
crop arrange.ruler 8
The ruler's right-click menu with its Grid list open.
:::

The same right-click menu sets a **tempo** or **time signature** change from the bar you
clicked (tick **Tempo** or **Time signature** and set it; **Remove change** takes it away). The
song's opening tempo and meter are in the transport bar. In the piano roll, the **Snap** list
sets the same grid.

## Time selections

A time selection is a span of time across one or more tracks: drag in an empty part of the
lanes. **Shift**+drag extends the selection you have. Drag either edge of it in the ruler's
top band to adjust it.

::: shot selection
crop pane.multitrack 0
The duplicated clips selected, with a time selection over their first half.
:::

Right-click inside it for what it can do: **Loop selection (L)**, **Split at edges (S)**,
**Delete in selection (Del)**, **Export selection…** and **Clear selection (Esc)**. With a time
selection, [[S]] splits at its edges, [[Delete]] deletes what is inside it, and [[L]] sets the
loop range to it.

## The loop range

Drag along the ruler to draw the loop range; it shows as a band in the ruler, bright while Loop
is on. **Loop** in the transport bar (or [[L]]) switches looping on and off. With a time
selection, L first loops the selection. Switching Loop on with no range loops the four bars from
the playhead's bar.

::: shot loop-range
crop arrange.ruler 8
highlight arrange.loop_range "1" "The loop range, dim while Loop is off."
A loop range over bars 2 and 3.
:::

Recording with Loop on records a take per pass: [[page:recording#loop-recording-and-takes]].

## Markers

Markers name places in the song (*Verse*, *Chorus*) and take you there.

- **Add:** [[Insert]] adds one at the playhead; so does Edit > Add marker at playhead.
  Double-click the marker strip (under the ruler) to add one there.
- **Go to:** click a marker; [[1]]–[[9]] go to markers 1 to 9 (their numbers are on the
  flags); [[[]] and [[]]] step to the previous and next marker.
- **Move:** drag a marker along the strip.
- **Rename:** double-click it, or right-click > **Rename…**. **Delete marker** is in the same
  menu.

::: shot markers
crop arrange.markers 8
highlight arrange.markers "1" "The marker strip: numbered flags."
Four markers.
:::

## Importing audio and MIDI files

Drag WAV, FLAC, MP3, AIFF or MIDI files from Explorer onto the Multitrack. They land where you
drop them: on the track under the pointer (one track per file, then the ones under it) and at
the time under the pointer, snapped. Dropped below the last track, they make new tracks.

Where a file would land on a track that can take it, Oxide asks first:

::: shot drop-question
crop pane.multitrack 0
The question after dropping a file over Vocal.
:::

- **Into Vocal** (or **Into these tracks**) puts it on that track.
- **New track** (or **New tracks**) makes a track for it instead.

File > Import audio… and File > Import MIDI… import at the playhead on the selected track. A
MIDI file whose tempo or time signature differs from the project's asks **Use the MIDI file's
tempo?**: either way the notes keep their beats.

Oxide copies every imported file into the project's `Imported` folder and plays the copy, so
the original is never changed.

## The inspector

The inspector, on the right, shows the settings of what is selected. Show or hide it with the
**Inspector** button in the transport bar or View > Inspector.

For a **track** it shows, from the top:

- its name (click to rename), its kind and its **Colour**;
- **Routing**: **Input**, **Output** and **Monitor** ([[page:recording#choosing-the-input]]);
- **Mixer**: **Volume**, **Pan** and the **Automation** mode;
- **Sends**: each send's bus, level, **Pre** (before the fader) and **M** (mute), and **+ Add
  send**;
- **Inserts**: its plug-ins in order, each with its on/off box, **▲**/**▼** to move it and **✕**
  to remove it; click a name to open the plug-in's window; **+ Add plug-in…** adds one;
- **Clips**: its clips with their start times.

With a **clip** selected, a **Clip** section comes first ([[page:#trimming-and-stretching]]).
With the **master** selected, it shows the master's **Output**, **Loudness**, mixer and
inserts. With nothing selected, **Master bus** selects the master.

::: shot main-window
crop pane.inspector 0
highlight inspector.routing "1" "Routing."
highlight inspector.mixer "2" "Mixer."
highlight inspector.sends "3" "Sends."
highlight inspector.inserts "4" "Inserts: the track's plug-ins."
highlight inspector.clips "5" "Clips."
dim
The inspector with Guitar L selected.
:::

## Over MCP

Every edit here has an action: [[action:add_track]], [[action:set_track]],
[[action:move_track_to]], [[action:move_clips]], [[action:split_clip]],
[[action:copy_clips]], [[action:paste_clips]], [[action:duplicate_clips]],
[[action:delete_clips]], [[action:set_clip]], [[action:crossfade_clips]], [[action:set_grid]],
[[action:set_loop]], [[action:add_marker]], [[action:go_to_marker]], [[action:import_audio]],
[[action:set_view]] (zoom and track heights). [[action:get_project]] reads the whole
arrangement.

## See also

- [[page:piano-roll]]: editing MIDI clips.
- [[page:keyboard]]: every key and gesture in one place.
