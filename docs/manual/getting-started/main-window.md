# The main window

Oxide has one main window. Along the top are the menu bar and the transport bar; along the
bottom, the status bar. Between them sit the panes: the Multitrack (your tracks and clips), the
inspector (the selected track's settings), and the mixer, the piano roll and the Waveform view
when you open them.

::: shot main-window
highlight menu "1" "The menu bar, with the switch between the Waveform and Multitrack views."
highlight transport "2" "The transport bar: play, record, the time, tempo, Snap, Click, Loop, Punch, the DSP load."
highlight dock.tab.multitrack "3" "The panes' tabs: drag one to rearrange the window."
highlight track.header "4" "A track's header: its name and buttons. Click it to select the track."
highlight arrange.ruler "5" "The ruler: bars and beats. Click it to move the playhead."
highlight pane.inspector "6" "The inspector: the selected track's or clip's settings."
highlight status "7" "The status bar: the audio device, latency, MCP and the version."
dim
The example project, with Guitar L selected.
:::

## The menu bar

| menu | what is in it |
|---|---|
| File | New, open, save and save as; templates and backups; importing audio and MIDI; opening a file in the Waveform view; exporting and mixdowns; Exit. |
| Edit | Undo and redo (each names what it undoes), cut, copy, paste at the playhead, duplicate, Quantize…, Add marker at playhead. |
| Track | Add a track of each kind; for the selected track: a send, Export MIDI…, Render track…, Disable or Enable track. |
| View | The panes (Mixer, Inspector, Diagnostics, Piano roll), Follow playhead, Layouts, zoom, Track height, Waveform gain. |
| Options | Settings…, AI control (MCP)…, Rescan plug-ins, Reconnect MIDI devices. |
| Help | Manual ([[F1]]) and About Oxide…. |

At the right of the menus, **Waveform** and **Multitrack** switch between the two views
([[Ctrl+Tab]] does too). The Multitrack is where you record and arrange a song; the Waveform
view edits one audio file at a time. The project's file and folder are shown at the far right.

## The transport bar

::: shot main-window
crop transport.home transport.time 6
highlight transport.home "1" "Go to start ([[Home]])."
highlight transport.play "2" "Play and stop ([[Space]])."
highlight transport.stop "3" "Stop."
highlight transport.record "4" "Record ([[R]]); right-click it for a count-in."
highlight transport.time "5" "The playhead's place: bar.beat.tick, and minutes:seconds under it."
The left end of the transport bar.
:::

::: shot main-window
crop transport.tempo transport.follow 6
highlight transport.tempo "6" "The tempo, in quarter notes a minute."
highlight transport.meter "7" "The time signature."
highlight transport.snap "8" "Snap to the grid."
highlight transport.click "9" "The metronome; ▾ beside it holds its options."
highlight transport.loop "10" "Loop playback ([[L]])."
highlight transport.punch "11" "Punch recording ([[P]])."
highlight transport.follow "12" "Scroll the view with the playhead."
The middle of the transport bar.
:::

::: shot main-window
crop transport.inspector transport.dsp 6
highlight transport.inspector "13" "Show or hide the inspector."
highlight transport.mixer "14" "Show or hide the mixer ([[M]])."
highlight transport.dsp "15" "The DSP load and its peak. Click it for the CPU panel."
The right end of the transport bar.
:::

- **Rewind, Play, Stop, Record.** Play turns into Pause while playing. Stop stops playback
  and recording.
- **The time display** shows where the playhead is, in bars, beats and ticks (960 a beat), with
  the time in minutes and seconds under it. During a count-in it counts the beats left.
- **Tempo and time signature** are the song's at the playhead. Drag the tempo, or click it and
  type. To change the tempo from a bar on, right-click the ruler at that bar
  ([[page:multitrack#the-grid-and-snap]]).
- **Snap, Click, Loop, Punch and Follow** are switches: lit is on. See
  [[page:multitrack#the-grid-and-snap|Snap]], [[page:recording#the-metronome|Click]],
  [[page:multitrack#the-loop-range|Loop]], [[page:recording#punch-recording|Punch]].
- **I** and **TP** are the master's loudness (integrated, in LUFS) and true peak, measured as
  you play; click them to see the loudness panel in the inspector.
- **Hide inspector / Inspector** and **Mixer / Hide mixer** show and hide those panes ([[M]]
  for the mixer).
- **DSP** and **peak** are how hard the computer works to keep up with the audio. They are
  explained on [[page:choppy-audio]].

## The Multitrack

The Multitrack is the main pane: a row per track, with its header on the left and its clips on
the timeline to the right. Above the tracks are the ruler (bars and beats) and the marker
strip. The orange line is the playhead. [[page:multitrack]] explains everything you do here.

- **+ Track**, top left, adds a track. **Master**, beside it, selects the master, the mix of
  every track.
- **−**, **+** and **Fit**, bottom left, zoom out, in and to the whole project. The bar beside
  them scrolls the timeline.

## The inspector {#the-inspector-pane}

The inspector shows the settings of whatever is selected: a track's colour, routing (input,
output, monitoring), volume, pan, sends, plug-ins and clips; a clip's stretch and pitch; or,
with the master selected, the master's output, loudness and plug-ins. It is described with the
Multitrack: [[page:multitrack#the-inspector]].

## The panes and the dock

Each pane has a tab. Drag a tab onto a side of another pane to put it there, or onto another
pane's tab bar to stack them as tabs. View > Layouts > Save layout as… keeps an arrangement
of panes under a name; View > Layouts > Default puts everything back.

The panes:

- **Multitrack**: always there.
- **Inspector** (View > Inspector).
- **Mixer** (View > Mixer, [[M]]): a strip per track, with faders and meters.
- **Piano roll** (View > Piano roll): a MIDI clip's notes ([[page:piano-roll]]).
- **Waveform**: one audio file to edit; double-clicking an audio clip opens its file there.
- **Diagnostics** (View > Diagnostics): clicks and clipping found in the Waveform view's file.

Keys act on the view you last clicked in: [[Space]] plays the project in the Multitrack, and
the file in the Waveform view.

## The status bar

::: shot main-window
crop status 4
highlight status.device "1" "The driver and the devices playing and recording."
highlight status.format "2" "Sample rate, buffer size, inputs and outputs, sample format."
highlight status.pdc "3" "PDC: the latency your plug-ins add, which Oxide compensates."
highlight status.threads "4" "Audio threads working in parallel."
highlight status.mcp "5" "The MCP server for AI agents. Click it for its settings."
highlight status.midi "6" "The MIDI inputs open."
highlight status.version "7" "Oxide's version. Click it for About."
The status bar.
:::

When they are not zero, the status bar also shows **input gaps** and **plugin skips**: see
[[page:choppy-audio#input-gaps-and-skips]]. Messages (a file saved, something refused) appear
for a few seconds at the bottom right.

## See also

- [[page:keyboard]]: every key and mouse gesture.
- [[page:glossary]]: the words used in this manual.
