# Track renders and mixdowns

Two ways to turn what plays into audio inside the project: **render a track** to a new audio
track (to free the CPU its plug-ins use, or to edit what an instrument played as audio), and
**mix down to a new file** (the whole mix, opened in the Waveform view for final touches).
Both run through the same engine as playback and export, so they sound exactly as you heard
them.

## Render a track

1. Right-click the track's header (or use the Track menu) and choose **Render track…**.
2. Set the **Tail**: time after the last clip for reverb tails and note releases (2 s at
   first).
3. Leave **Switch this track off** ticked to take the original out of the mix once the render
   is in (it then costs no CPU), or untick it to keep both playing.
4. Click **Render**.

::: shot render
crop render 8
The Render dialog for Guitar L.
:::

The track renders as you hear it: its clips (or its MIDI through its instrument), its
plug-ins, fader, pan and automation, and for a folder its tracks; not its sends. The file goes
into the project's `Rendered` folder, and a new audio track, *name* **(rendered)**, appears
right under the original, with copies of its sends, so its reverb still reaches the bus.

It renders in the background with a progress bar and **Cancel**; the window keeps working
meanwhile. A muted track must be unmuted first (the dialog says so); a solo elsewhere doesn't
matter.

## Switch a track off (freeze)

Rendering with **Switch this track off** is Oxide's freeze: the original track keeps its
clips, plug-ins and settings, dimmed, but is out of the audio graph and costs nothing. To
change it, switch it back on (the power button on its header, or right-click > **Enable
track**), edit, and render again; delete the old render.

Any track can be switched off without rendering: the power button, or right-click > **Disable
track**. Unlike a mute, its plug-ins stop.

::: shot render
crop pane.multitrack 0
The Multitrack with Guitar R switched off (dimmed) under its render.
:::

## Mix down to a new file

**File > Mixdown to new file > Whole session** (or **Time selection**) renders the mix as it
plays, every track, plug-in, automation and send and the master, into a new 32-bit float WAV
in the project's `Rendered` folder (*Project* mixdown *date time*.wav), and opens it in the
Waveform view. There you can fade, normalise, repair, tag and export it
([[page:waveform-view]]).

The master must be unmuted (unmute it if Oxide says so). The mix's tail rings out for two
seconds after the end.

::: shot mixdown
crop pane.waveform 0
The example project mixed down, open in the Waveform view.
:::

## Export a track's MIDI

On an instrument track, right-click the header and choose **Export MIDI…** to save the track's
MIDI clips as one MIDI file, with the project's tempo, for another program. It holds the notes
as programmed, nothing the plug-ins add.

## Over MCP

[[action:render_track]] renders (and returns at once; [[action:get_export_status]] gives the
progress and then the new `track`), [[action:set_track_enabled]] switches a track off or on,
[[action:mixdown_to_new_file]] mixes down, and [[action:export_midi_track]] writes a track's
MIDI.

## See also

- [[page:exporting]]: writing the mix to a file of any format.
- [[page:choppy-audio]]: when to render a heavy track.
