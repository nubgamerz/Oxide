# Keyboard and mouse reference

Every key and mouse gesture Oxide has, by where it works.

**Where keys go.** Keys act in the view you last clicked in, the Multitrack or the Waveform
view; some mean different things in each ([[Space]] plays the project in one and the file in
the other). The piano roll's keys work while the pointer is over the piano roll. While you type
in a field (a name, a value, a search box), keys go to the field, and while the plug-in browser
is open they go to its search box; [[F1]] works everywhere.

::: shot piano-roll
crop piano_roll.toolbar 4
highlight piano_roll.toolbar "1" "The piano roll's toolbar sums up its mouse gestures."
:::

## Everywhere

| key | does |
|---|---|
| [[F1]] | Opens this manual at the page for what the pointer is over (Help > Manual opens its first page). |
| [[Ctrl+Z]] | Undo: the project's last edit in the Multitrack, the file's in the Waveform view. |
| [[Ctrl+Shift+Z]] or [[Ctrl+Y]] | Redo. |
| [[Ctrl+Tab]] | Switches between the Multitrack and the Waveform view. |
| [[Space]] | Play or stop: the project in the Multitrack, the file in the Waveform view. |
| [[Ctrl+C]] / [[Ctrl+X]] / [[Ctrl+V]] | Copy, cut and paste: clips in the Multitrack, audio in the Waveform view. [[Ctrl+Insert]], [[Shift+Delete]] and [[Shift+Insert]] do the same. |

## The Multitrack

| key | does |
|---|---|
| [[R]] | Record, or stop recording. |
| [[Home]] | Playhead to the start. |
| [[M]] | Show or hide the mixer. |
| [[=]] or [[+]] / [[-]] | Zoom in / out around the playhead. |
| [[F]] | Fit the whole project in view. |
| [[C]] | Metronome on or off. |
| [[P]] | Punch on or off. |
| [[L]] | Loop on or off; with a time selection, loop the selection first. |
| [[S]] | Split the selected clips at the playhead; with a time selection, at its edges. |
| [[Delete]] or [[Backspace]] | Delete: the selected automation point, else what is in the time selection, else the selected clips. |
| [[Ctrl+D]] | Duplicate the selected clips right after themselves. |
| [[Ctrl+A]] | Select every clip. |
| [[Esc]] | Select no clips, and clear the time selection. |
| [[Q]] | Open Quantize for the selected audio clips. |
| [[Insert]] | Add a marker at the playhead. |
| [[1]]–[[9]] | Go to marker 1 to 9. |
| [[[]] / [[]]] | Go to the previous / next marker. |
| [[Ctrl+S]] | Save the project. |
| [[Ctrl+O]] | Open a project. |
| [[Ctrl+N]] | New project (the start screen). |
| [[Ctrl+T]] | Add an audio track. |

## The piano roll (pointer over it)

| key | does |
|---|---|
| [[Delete]] or [[Backspace]] | Erase the selected notes (never the clip). |
| [[Ctrl+A]] | Select every note of the clip. |
| [[Q]] | Quantize the selected notes (all, with none selected) to the grid. |
| [[Esc]] | Select no notes. |

## The Waveform view

| key | does |
|---|---|
| [[Home]] | The file's playhead to its start. |
| [[Esc]] | Clear the selection and the spectral box. |
| [[E]] / [[T]] | The box tool (drag a box on the spectrogram) / the time tool. |
| [[=]] or [[+]] / [[-]] | Zoom in / out around the file's playhead. |
| [[F]] | Fit the whole file. |
| [[Delete]] or [[Backspace]] | Delete the selection, or remove what is in the spectral box. |
| [[Ctrl+S]] | Save the file (asks first: it overwrites it). |
| [[Ctrl+Shift+S]] | Save the file as a new WAV. |

## Fields and dialogs

| key | does |
|---|---|
| [[Enter]] | Keeps what you typed in a name or value field (a track's name, a volume); in a name dialog (a layout, a template, a rack preset), saves it; in the plug-in browser, loads the only plug-in listed. |
| [[Tab]] | Keeps what you typed and leaves the field. |
| [[Esc]] | Cancels the field you are typing in; closes the plug-in browser. |

Clicking away from a field also keeps what was typed. A value that can't be read keeps the
field open, outlined in red.

## Mouse: the Multitrack

| gesture | where | does |
|---|---|---|
| wheel | anywhere | Scroll the tracks up and down. |
| **Shift**+wheel | anywhere | Scroll along the timeline. |
| **Ctrl**+wheel, or pinch | anywhere | Zoom around the pointer. |
| **Alt**+wheel | anywhere | Every track taller or shorter; the one under the pointer stays put. |
| **Alt+Shift**+wheel | anywhere | Waveforms bigger or smaller on screen (not the sound). |
| click | ruler, empty lane, clip | Move the playhead there (snapped with Snap on). |
| drag | ruler | Set the loop range. |
| **Shift**+drag | ruler | Set the punch range. |
| right-click | ruler | Grid, and a tempo or time signature change at that bar. |
| drag | the playhead's line or head | Move the playhead; past an edge the view scrolls. |
| drag | empty lane | Draw a time selection (selecting the clips in it); **Shift** extends it. |
| drag | a time selection's edge, in the ruler's top band | Move that edge. |
| right-click | empty lane | The selection's menu, **Insert MIDI track**, **Insert MIDI clip** (instrument tracks). |
| double-click | under the last track | Add an audio track. |
| right-click | under the last track | Add a track of any kind. |
| click | clip | Select it alone. |
| **Ctrl**+click or **Shift**+click | clip | Add it to the selection, or take it out. |
| double-click | audio clip | Open its file in the Waveform view. |
| double-click | MIDI clip | Open it in the piano roll. |
| drag | clip | Move the selected clips (onto other tracks too); they jump onto the playhead when close. |
| **Alt**+drag | clip | Copy the selected clips there. |
| **Ctrl** held while dragging | clip or edge | No jumping onto the playhead. |
| drag | clip's left or right edge | Trim it. |
| **Alt**+drag | clip's edge | Stretch it. |
| drag | the square at a clip's top corner | Fade in or out. |
| drag up or down | the line across a clip | Clip gain; double-click for 0 dB. |
| right-click | clip | Takes, split, crossfade, slice, quantize, zoom to clip, delete (MIDI: open in piano roll, export). |
| click | track header | Select the track. |
| drag | track header | Reorder tracks, or move one into a folder. |
| double-click | track name | Rename it. |
| drag up or down | volume or pan knob | Change it (**Shift**: finer); double-click to reset. |
| click | the number by a knob | Type a value. |
| drag | a header's bottom edge | Resize the track (**Ctrl**: every track). |
| right-click | track header | Rename, sends, automation lanes, height, move, render, delete. |
| click | marker | Go to it. |
| drag | marker | Move it. |
| double-click | marker | Rename it. |
| double-click | empty marker strip | Add a marker there. |
| right-click | marker or marker strip | Rename, delete, add a marker. |
| drop files | from Explorer | Import them where they are dropped. |

## Mouse: the effects racks and plug-ins

| gesture | where | does |
|---|---|---|
| click | a plug-in's name (header rack, inspector, mixer) | Open or close its window. |
| click | an empty rack slot or **+ Add plug-in…** | Choose a plug-in in the browser. |
| drag | a slot's number | Move the plug-in to another slot or track (**Ctrl** at the drop: copy it). |
| right-click | a plug-in | Open, bypass, replace, copy, paste after it, remove. |
| right-click | an empty slot or **+** | Paste a copied plug-in. |

## Mouse: the piano roll

| gesture | where | does |
|---|---|---|
| double-click | empty grid | Draw a note. |
| click | note | Select it and hear it; **Shift**+click adds or removes it. |
| click | empty grid or ruler | Move the playhead there. |
| drag | note | Move the selected notes in time and pitch. |
| drag | a note's end | Resize the selected notes. |
| drag | empty grid | Select the notes in a box (**Shift** adds). |
| right-click | note | Erase it (and the other selected notes). |
| **Alt** held | while drawing, dragging or clicking | No snapping. |
| press | a key on the keyboard | Hear that pitch; drag along the keys to play more. |
| drag | velocity lane | Set the velocity of a note (and the other selected notes). |
| wheel / **Shift**+wheel / **Ctrl**+wheel | grid | Scroll pitch / scroll time / zoom. |

## Mouse: the mixer and automation

| gesture | where | does |
|---|---|---|
| double-click | fader | 0 dB. |
| double-click | pan slider | Centre. |
| click | the level or pan number | Type a value. |
| drag sideways | send bar | Send level; double-click for 0 dB; right-click for Pre-fader, Mute, Remove. |
| drag | a strip's name | Reorder tracks. |
| click | the automation mode button (R, T, L, W) | Step through Read, Touch, Latch, Write. |
| double-click | automation lane | Add a point. |
| drag | automation point | Move it; [[Delete]] deletes the selected one. |
| right-click | automation lane or point | Add a point, switch the lane off, remove it; a point's curve to the next. |

## Mouse: the Waveform view

| gesture | where | does |
|---|---|---|
| click | waveform | Move the file's playhead; clear the selection. |
| drag | waveform | Select a range; **Alt**: only the channel under the pointer. |
| drag | spectrogram, with the box tool ([[E]]) | Select a box of time and frequency. |
| wheel / **Ctrl**+wheel | waveform | Scroll / zoom. |

## Mouse: the window

| gesture | where | does |
|---|---|---|
| drag | a pane's tab | Put the pane beside another, or stack it as a tab. |
| click | DSP readout | Show or hide the CPU panel. |
| click | MCP in the status bar | Open its settings (or **Retry** after a failure). |
| click | the version in the status bar | About Oxide. |
| right-click | Record | Count-in. |

## See also

- [[page:main-window]]: the parts of the window.
- [[page:multitrack]] and [[page:piano-roll]]: what the gestures do, in context.
