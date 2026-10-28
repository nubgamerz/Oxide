# MCP tools

The full reference is the manual's AI control (MCP) chapter: every action's input and output fields, and examples the tests run against a headless session (`docs/manual/mcp/`, and the pages generated from the registry: `mcp-reference.html` and a page per area such as `mcp-tracks.html`, in `manual` beside the program, or in `out/manual` after `cargo run -p manual`). This file is the short list.

Generated from the action registry (`daw-core/src/api`); do not edit by hand. Regenerate with `$env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\UPDATE_DOCS` (PowerShell) or `UPDATE_DOCS=1 cargo test -p daw-core docs_are_current` (bash).

The DAW serves these over MCP at `http://127.0.0.1:7878/mcp` while it runs. Connect Claude Code and Claude Desktop from the app's Options > AI control (MCP): both then run the bridge `oxide-mcp.exe`, which lists these tools even while Oxide is closed (a call then answers that it isn't running). Or connect Claude Code by hand with `claude mcp add --transport http --scope user oxide http://127.0.0.1:7878/mcp`, which connects only while Oxide runs.

| tool | kind | summary |
|---|---|---|
| [`get_project`](#get_project) | read | Describe the open project: tracks (with ids, kind, folder parent, input, output, mixer settings, inserts, clips and automation lanes), the master bus, the opening tempo and time signature (changes: get_tempo_map) and sample rate |
| [`new_project`](#new_project) | destructive | Close the open project and start a new one |
| [`open_project`](#open_project) | destructive | Open a project file (.oxide, or an older .dawproj; absolute `path`), replacing the open project (unsaved changes are lost; the Waveform view's files close, unsaved edits in them lost too, and the files the project had open there reopen) |
| [`list_backups`](#list_backups) | read | The open project's backups in its Backups folder, newest first: `path`, `modified` (seconds since 1970), `age_seconds` and `size` in bytes |
| [`save_backup`](#save_backup) | edit | Back the open (saved) project up now into its Backups folder, as autosave does, keeping the newest backups; the project file is untouched |
| [`open_backup`](#open_backup) | destructive | Open one of the open project's backups (absolute `path`, from list_backups or open_project's `newer_backup`) in its place: it becomes the open project, unsaved until save_project writes it over the project file |
| [`get_busy`](#get_busy) | read | What Oxide is busy with, for an agent to wait on: `loading` (a project being opened in the app, in stages: `stage` is `closing` (the last project's plug-ins destroyed), `reading` (its audio files read on workers) or `loading` (its plug-ins loaded a few a frame); `done` of `total` over the whole open, `stage_done` of `stage_total` within the stage; `next`, the plug-in or file it is on, and `track`, the loading plug-in's track; cancel_open stops it; `kind` says what the plug-ins load for: `open` (a project), `rack` (only the Waveform view's effects rack, the last run's or a preset, which leaves the project open; `rack` is then true too), `undo` or `redo` (a step that brought plug-ins back; `label` is its name; the project plays as before the step until they are in) or `sample_rate` (every plug-in reloading at a new rate); only an open can be cancelled), `saving` (the project file a save is writing in the background), `save` (how far it has come: `done` of `total` plug-in states read, `next` and `track` the plug-in read next, `writing` once the file is being written; also while the app looks for unsaved changes before closing, with `saving` null) and `exporting` (an export or render running; get_export_status has its progress) |
| [`cancel_open`](#cancel_open) | destructive | Stop opening the project the app is opening (get_busy's `loading`) and go back to the project open before it, reopened from its file with its plug-ins (changes not saved there were given up when the open was asked for), or, with none before, to no project (the start screen) |
| [`get_unsaved_changes`](#get_unsaved_changes) | read | What closing the project now would lose, as the app's "Save the changes?" question sees it: `project` is true when the project differs from its file as last saved or opened (every plug-in's state read now; an edit undone again is no change, a plug-in turned in its editor is one; a backup opened in its place always is), and `files` lists the Waveform view's files with unsaved edits (`document` ids and paths) |
| [`save_project`](#save_project) | destructive | Save the project, with every plug-in's state |
| [`undo`](#undo) | edit | Undo the last edit: every action that changes the project (tracks, clips, takes, routing, mixer, inserts, tempo map, loop, punch, metronome) is one undo step, recorded only if it changed something; repeated set_ calls on the same object in quick succession are one step, and a recording is one step |
| [`redo`](#redo) | edit | Redo the last undone edit |
| [`get_history`](#get_history) | read | The undo history: what undo would undo (`undo`, most recent first) and what redo would redo (`redo`), with the history's estimated memory use |
| [`set_tempo`](#set_tempo) | edit | Set the opening tempo, in quarter notes per minute (20 to 300), which holds until the first tempo change (see get_tempo_map) |
| [`set_time_signature`](#set_time_signature) | edit | Set the opening time signature, e.g |
| [`set_grid`](#set_grid) | edit | Set the song's grid division (`grid`), saved with the project: "1/4", "1/8" (a new project's), "1/16", "1/32" or "1/64" of a whole note (1/4 is a beat in 4/4, 1/8 a beat in 6/8), each with a "t" for triplets ("1/8t"), or "auto": the division follows each view's zoom (the finest of 1/64 to 1/4 whose lines are at least 14 points apart, then the beat or every 1, 2, 4… bars; get_view's `grid_division` is the one the Multitrack shows now) |
| [`get_tempo_map`](#get_tempo_map) | read | The tempo map: the opening tempo (quarter notes per minute) and time signature, and every change after it with its bar and beat (from 1) and where it falls in samples (`position`) and ticks (`tick`, 960 per quarter note, as MIDI clips count them) |
| [`add_tempo_change`](#add_tempo_change) | edit | Change the tempo (`bpm`, quarter notes per minute) and/or time signature (`numerator`, `denominator`) from bar `bar` (from 2; bar 1 is the opening, see set_tempo) and beat `beat` (from 1, default 1) |
| [`remove_tempo_change`](#remove_tempo_change) | edit | Remove the tempo/time-signature change at `bar` and `beat` (from 1; beat defaults to 1) |
| [`add_track`](#add_track) | edit | Add a track and return its id |
| [`remove_track`](#remove_track) | destructive | Delete a track with its clips and plug-ins |
| [`set_track`](#set_track) | edit | Rename a track, recolour it (`color` as "#rrggbb"), collapse/expand a folder in the arrangement, set its row `height` in points (32 to 400; 0 for the default; taller rows draw clearer waveforms), or show or hide its automation lanes (`show_automation`) |
| [`set_track_input`](#set_track_input) | edit | Set what feeds a track |
| [`set_track_output`](#set_track_output) | edit | Route a track's main output: {"type":"parent"} (its folder, or the master), {"type":"master"}, {"type":"bus","track":id} (any folder or bus track that would not create a loop through outputs or sends), or {"type":"hardware","channel":0} (straight to the output pair starting at that 0-based channel, bypassing the master) |
| [`set_track_monitor`](#set_track_monitor) | edit | Input monitoring: auto (hear the input while the track is armed), on, or off |
| [`move_track`](#move_track) | edit | Move a track (with its sub-tracks) one place up or down among its siblings |
| [`set_track_parent`](#set_track_parent) | edit | Move a track (with its sub-tracks) into a folder track, or to the top level when `folder` is omitted |
| [`move_track_to`](#move_track_to) | edit | Move a track (with its sub-tracks) to a place in the track list, as dragging its header does: `before` or `after` another track (it joins that track's folder, or the top level), or into `folder` as its `position`th sub-track (0 = first; omitted or past the last: at the end), or with none of them to the end of the top level |
| [`set_track_mixer`](#set_track_mixer) | edit | Change a track's mixer settings |
| [`set_track_enabled`](#set_track_enabled) | edit | Switch a track off (`enabled` false) or on again |
| [`render_track`](#render_track) | edit | Render a track as it sounds (its clips, or its MIDI through its instrument; its plug-ins, fader, pan and automation; a folder's tracks; not its sends) from the song's start (sample 0) to the end of its last clip plus `tail_seconds` (default 2, for reverb and note releases) into the project's Rendered folder as a 32-bit float WAV, through the same engine and plug-in pre-roll as an export, so it sounds exactly like playback; the file is silent up to the track's first sound, so it lines up with the song wherever it is loaded |
| [`set_master`](#set_master) | edit | Change the master bus: `gain_db` (-60 to +12), `pan`, `mute`, and `output_channel` (first channel of the hardware output pair, 0-based and even); and its row at the top of the arrangement: `height` in points (32 to 400; 0 for the default) and `show_automation` (its lanes shown under it) |
| [`add_send`](#add_send) | edit | Send a copy of `track`'s signal into `target`, a folder or bus track (add a bus with add_track kind "bus" for a reverb return), alongside its main output |
| [`add_send_to_new_bus`](#add_send_to_new_bus) | edit | Add a new bus (a return track, at the end of the project, routed to the master) and send `track` into it: post-fader at 0 dB, or as `gain_db` and `pre_fader` say |
| [`set_send`](#set_send) | edit | Change a send by its id: `gain_db` (-60 to +12 dB), `pre_fader`, `mute`, or `target` (another folder or bus; refused if it would create a loop) |
| [`remove_send`](#remove_send) | destructive | Delete a send by its id |
| [`list_plugins`](#list_plugins) | read | List the built-in effects (vendor "Built in": Oxide's own effects, path native:<id>; an id from before an effect was renamed still names it) and the installed VST3 plug-ins (name, vendor folder, path) |
| [`rescan_plugins`](#rescan_plugins) | edit | Look for newly installed VST3 plug-ins |
| [`add_insert`](#add_insert) | edit | Load a VST3 plug-in or a built-in effect ("Amplify") into a track's insert chain (or the master's when `track` is omitted, or the Waveform view's effects rack with `editor` true) |
| [`remove_insert`](#remove_insert) | destructive | Remove a plug-in from its insert chain, discarding its settings |
| [`set_insert`](#set_insert) | edit | Bypass or enable a plug-in (`bypass`) |
| [`move_insert`](#move_insert) | edit | Move a plug-in in its insert chain, where the order is the signal's: the first processes first (a tuner before an amp hears the dry input) |
| [`copy_insert`](#copy_insert) | edit | Copy a plug-in: a new instance of it with the same settings (its state now) and bypass, without its automation, into `track`'s chain (or the master's with `master` true; without either, its own chain, right after it) at `position` (0 is first; default: last) |
| [`copy_plugin`](#copy_plugin) | edit | Copy a plug-in insert to the plug-in clipboard, as its menu's Copy does: which plug-in, its name, bypass and its settings as they are now |
| [`paste_plugin`](#paste_plugin) | edit | Paste the copied plug-in (copy_plugin): a new instance with the settings it had when copied, into `track`'s chain (or the master's with `master` true) at `position` (0 is first; default: last) |
| [`set_inserts_bypassed`](#set_inserts_bypassed) | edit | Bypass (`bypass` true) or enable every plug-in in a track's insert chain at once, or the master's when `track` is omitted (the Waveform view's rack with `editor` true), as the fx rack's power button does |
| [`set_plugin_editor`](#set_plugin_editor) | edit | Open or close a plug-in's own editor window (`open`) |
| [`list_plugin_parameters`](#list_plugin_parameters) | read | List a plug-in's parameters: id, name, normalized value (0-1), the value as the plug-in displays it, unit, default and step count (0 = continuous, 1 = on/off, n = n+1 choices) |
| [`set_plugin_parameter`](#set_plugin_parameter) | edit | Set a plug-in parameter to a normalized `value` from 0 to 1 |
| [`set_impulse_file`](#set_impulse_file) | edit | Give a Convolution Reverb insert (`insert`) its impulse response: the audio file at `path` (an absolute path; WAV, AIFF or FLAC, up to 30 s) |
| [`get_impulse_file`](#get_impulse_file) | read | The impulse response a Convolution Reverb or Space Reverb insert (`insert`) plays: `file` (as saved, relative to the project's folder; null when none is chosen, and always for the Space Reverb, which makes its impulse from its settings), `preparing` (true while it or a settings change is being prepared; exports wait for it), `error` (why it could not be read, if so), and once prepared `seconds` (its length as played: with Room Size and Pre-Delay; the Space Reverb's as trimmed), `file_channels`, `file_rate` (Hz) and `file_seconds` (the file as it is; for the Space Reverb its impulse channels, 2 or 4 in true stereo, the session's rate and 1.45 x Decay) |
| [`list_plugin_outputs`](#list_plugin_outputs) | read | A plug-in insert's audio outputs, as it names them: `bus` (0 is the main pair, which plays on the plug-in's own track; 1 and up are extra outputs, like a drum sampler's kick and snare or a sampler's outputs 2..32), `name`, `channels`, `active` (on in the plug-in: a track listens to it), and `track`, the track listening to it if any |
| [`create_multi_out_rack`](#create_multi_out_rack) | edit | Route a multi-out plug-in's extra outputs to tracks of their own (a multi-out rack in a submix): a folder right after the plug-in's track holding one audio track per output in `outputs` (bus numbers from list_plugin_outputs, 1 and up; default every extra output without a track yet), each named after its output and hearing it at unity (0 dB), with its own fader, pan, inserts and sends |
| [`remove_multi_out_rack`](#remove_multi_out_rack) | destructive | Remove the tracks listening to a plug-in insert's extra outputs (and the folder they leave empty); the plug-in turns those outputs off |
| [`split_range`](#split_range) | edit | Cut the clips on `tracks` (track ids; omit for every track that holds clips) at both edges of the time range `start`..`end` (samples), wherever an edge falls inside a clip: what a time selection's Split does |
| [`delete_range`](#delete_range) | destructive | Remove everything in the time range `start`..`end` (samples) on `tracks` (track ids; omit for every track that holds clips), as Delete does on a time selection: audio and MIDI clips are cut at the edges (as split_range cuts them) and the parts inside removed, the parts outside kept |
| [`import_audio`](#import_audio) | edit | Import an audio file (WAV, FLAC, MP3 or AIFF; absolute `path`) as a clip on an audio track at `position` (samples; default: the playhead) |
| [`move_clip`](#move_clip) | edit | Move a clip so it starts at `start` (samples), and/or onto another audio or instrument `track` |
| [`split_clip`](#split_clip) | edit | Cut an audio or MIDI clip in two at timeline `position` (samples, strictly inside the clip) |
| [`delete_clip`](#delete_clip) | destructive | Remove a clip from the timeline (the audio file on disk is kept) |
| [`set_clip`](#set_clip) | edit | Change a clip's `gain_db` (-60 to +24), `name`, `stretch` or `pitch` |
| [`copy_clips`](#copy_clips) | edit | Copy clips (audio or MIDI, by id) to the clipboard, replacing what it held |
| [`cut_clips`](#cut_clips) | destructive | Copy clips (audio or MIDI, by id) to the clipboard and remove them from the timeline (their files stay on disk) |
| [`delete_clips`](#delete_clips) | destructive | Remove several clips (audio or MIDI, by id) in one undo step; their files stay on disk |
| [`paste_clips`](#paste_clips) | edit | Paste the clipboard so its earliest clip starts at `position` (samples; default: the playhead), the others keeping their distances |
| [`duplicate_clips`](#duplicate_clips) | edit | Copy clips (audio or MIDI, by id) to right after the last of them ends, on the same tracks (the clipboard is left alone) |
| [`move_clips`](#move_clips) | edit | Move clips (audio or MIDI, by id) together: `delta` samples later (negative: earlier) and `track_delta` rows down (negative: up) |
| [`copy_clips_to`](#copy_clips_to) | edit | Copy clips (audio or MIDI, by id) and move the copies as move_clips would: `delta` samples later (negative: earlier) and `track_delta` rows down (negative: up); the originals stay (an Alt-drag in the arrangement) |
| [`crossfade_clips`](#crossfade_clips) | edit | Crossfade a clip with the clip before it on its track: the earlier one fades out and this one fades in over their overlap |
| [`set_active_take`](#set_active_take) | edit | Choose which take a clip plays (`take`, an index from 0) |
| [`delete_take`](#delete_take) | destructive | Remove one take from a clip (the audio file on disk is kept) |
| [`get_clip_waveform`](#get_clip_waveform) | read | Read the waveform of a clip's playing take as `points` (default 200, max 4000) evenly spaced segments, each the minimum and maximum sample value (-1 to 1) of the clip's channels in that segment, with the segment's timeline start in samples |
| [`insert_midi_track`](#insert_midi_track) | edit | Insert a MIDI track, as a time selection's "Insert MIDI track" does: a new instrument track (armed) right after track `after` (and its sub-tracks; at the end without it), holding an empty MIDI clip from `start` to `end` (samples; without `end`, two bars from the bar `start` is in), or from bar `bar` (from 1) for `bars` bars (default 2) |
| [`add_midi_clip`](#add_midi_clip) | edit | Add an empty MIDI clip to an instrument track and return it |
| [`get_midi_clip`](#get_midi_clip) | read | Read a MIDI clip: start and length in ticks (960 per quarter note) and where they fall in samples now, its notes (id, tick from the clip start, length in ticks, pitch 0-127 with 60 = middle C, velocity 1-127, channel 0-15) and controller events |
| [`describe_midi_clip`](#describe_midi_clip) | read | Read a MIDI clip as music, to write a part that fits it: where it starts and ends (`start`, `end` as "bar:beat:tick": bars and beats from 1 as the tempo map's time signatures count them, then ticks into the beat, 960 per quarter note), the bars it spans, the tempo (quarter notes per minute) and time signature where it plays and each change inside it (`meters`), its estimated `key` and `runner_up` (Krumhansl-Kessler profiles over the notes' lengths; `score` is the fit, -1 to 1, with the key signature and scale), its `range` (lowest and highest note), its chords bar by bar (`progression` like "Am | F | C G", and `chords` with each one's place, length in ticks, root, quality, bass and tones; one per bar, or per half bar or beat where it changes; `implied` when only some tones are heard, as in a bass line; "N" for none) and every note as get_midi_clip gives it plus `at` (its song position, "bar:beat:tick") and `name` (C4 = 60) |
| [`set_midi_clip`](#set_midi_clip) | edit | Move, resize or rename a MIDI clip: `start` and `length` in ticks (960 per quarter note) |
| [`remove_midi_clip`](#remove_midi_clip) | destructive | Delete a MIDI clip with its notes and controller events |
| [`add_notes`](#add_notes) | edit | Add notes to a MIDI clip |
| [`set_notes`](#set_notes) | edit | Change notes in a MIDI clip by id: `tick` (from the clip start, 960 per quarter note) or `at` (song position "bar:beat:tick"), `length` in ticks, `pitch` (0-127 or a name like "Bb3"), `velocity`, `channel` (only the fields given change) |
| [`resize_notes`](#resize_notes) | edit | Resize notes in a MIDI clip by one edge, as dragging a note's edge in the piano roll does: `edge` "start" or "end" moves by `by` ticks (960 per quarter note; negative is earlier) and the other edge stays |
| [`remove_notes`](#remove_notes) | destructive | Delete notes from a MIDI clip by id |
| [`copy_notes`](#copy_notes) | edit | Copy notes of a MIDI clip (`clip`, `notes` as note ids) to the note clipboard, as          Ctrl+C in the piano roll does, replacing what it held: their distances from the          earliest one (in ticks, 960 per quarter note), pitch, length, velocity and channel |
| [`cut_notes`](#cut_notes) | destructive | Copy notes of a MIDI clip (`clip`, `notes` as note ids) to the note clipboard, as          copy_notes does, and delete them from the clip, as Ctrl+X in the piano roll does |
| [`paste_notes`](#paste_notes) | edit | Paste the note clipboard (copy_notes, cut_notes, or Ctrl+C in the piano roll) into          MIDI clip `clip`, as Ctrl+V in the piano roll does: the earliest copied note starts at          `position` (samples, at the tick nearest it; default: the playhead) or `at` (song          position "bar:beat:tick", bars and beats from 1), not snapped, and every other          note at its same distance from it and its same pitch, length, velocity and channel |
| [`add_controller_events`](#add_controller_events) | edit | Add controller events to a MIDI clip |
| [`read_midi_file`](#read_midi_file) | read | Look inside a Standard MIDI file (absolute `path`) without importing it: its tracks with notes (name, `import_as`: the name import_midi_file gives its track, note count, length in ticks at 960 per quarter note), and the tempo map it asks for (opening tempo and time signature, later changes in bars and beats from the file's start), so you can decide on import_midi_file's `set_tempo` |
| [`import_midi_file`](#import_midi_file) | edit | Import a Standard MIDI file (absolute `path`, type 0 or 1) as MIDI clips, placed at `bar` (and `beat`, from 1) or `start` in ticks (960 per quarter note); bar 1 by default |
| [`export_midi_clip`](#export_midi_clip) | destructive | Save a MIDI clip as a Standard MIDI file (type 0, 960 ticks per quarter note) at absolute `path`: the notes that play (cropped at the clip's end) and its controller events, with the tempo and time signature at the clip's start |
| [`export_midi_track`](#export_midi_track) | destructive | Save every MIDI clip of instrument track `track` as one Standard MIDI file (type 0, 960 ticks per quarter note) at absolute `path`, each clip at its place in the song (tick 0 is the project's start), with the project's tempo and time signature changes: the notes as programmed (cropped at each clip's end, as they play) and controller events, nothing from plug-ins |
| [`quantize_notes`](#quantize_notes) | edit | Move a MIDI clip's notes toward the grid: each start goes `strength` (0-100 %, default 100) of the way to its nearest line of `division` ("1/16", "1/8t", "beat", "bar"…), exactly in ticks, following the time signature (lines restart each bar) |
| [`preview_note`](#preview_note) | transport | Play a note on an instrument track's plug-in now, whether or not the track is armed: `pitch` 0-127 (60 = middle C), `velocity` 1-127 (default 100), released after `duration_ms` (default 500, up to 10000) |
| [`set_midi_record_options`](#set_midi_record_options) | edit | How recording on armed instrument tracks lands: `replace` (true: first delete what the track has in the recorded range; false, the default: merge, overdubbing into a clip that holds the range, or a new clip) and `quantize` (a grid division like "1/16", "1/8t", "beat" to snap recorded note starts to, or "off") |
| [`set_chase_notes`](#set_chase_notes) | edit | Chase notes (on by default): when playback starts, loops back or jumps into the middle of a MIDI note, strike it there instead of waiting for the next note |
| [`remove_controller_events`](#remove_controller_events) | destructive | Delete controller events from a MIDI clip by id |
| [`set_automation_mode`](#set_automation_mode) | edit | Set what a track's (or, with `track` omitted, the master's) controls do to its automation lanes while playing: "read" (lanes play; the controls follow them), "touch" (holding a fader, pan, mute or plug-in knob in the app writes it; letting go glides back to the lane), "latch" (like touch, but keeps writing the last value until playback stops) or "write" (every playing lane is written from the start of playback) |
| [`add_automation_lane`](#add_automation_lane) | edit | Add an automation lane to a track (or the master when `track` is omitted) |
| [`get_automation_lane`](#get_automation_lane) | read | Read an automation lane by id: its owner (`track`, null for the master), target, units, whether it is enabled, and its points (id, tick, value, curve) in time order |
| [`set_automation_lane`](#set_automation_lane) | edit | Switch an automation lane on or off (`enabled`): an off lane keeps its points but moves nothing |
| [`remove_automation_lane`](#remove_automation_lane) | destructive | Delete an automation lane by id, with its points |
| [`add_automation_points`](#add_automation_points) | edit | Add points to an automation lane |
| [`set_automation_point`](#set_automation_point) | edit | Change an automation point by id: `tick` (or `bar` and `beat`), `value`, `curve` |
| [`remove_automation_points`](#remove_automation_points) | destructive | Delete points from an automation lane: those listed in `points` (ids), and/or those from `from_tick` up to (not including) `to_tick` |
| [`get_automation_value`](#get_automation_value) | read | The value an automation lane gives at `tick` (from the start of the song, 960 per quarter note; fractions allowed) or at `bar` (and `beat`, from 1), in the lane's units; null for a lane with no points |
| [`detect_transients`](#detect_transients) | read | Find the transients (note attacks, hits) in a clip's audio |
| [`split_clip_at`](#split_clip_at) | edit | Cut a clip at several timeline positions (samples) in one go, e.g |
| [`quantize_positions`](#quantize_positions) | read | Snap timeline positions (samples) to the grid |
| [`quantize_clips`](#quantize_clips) | edit | Move clips so they start on the grid, like audio quantize: after split_clip_at at the transients, each piece starts at its transient, so this lines the hits up with the beat |
| [`open_audio_editor`](#open_audio_editor) | edit | Open an audio file in the Waveform view's editor: `path` (absolute; WAV, FLAC, MP3 or AIFF) or `clip` (an audio clip id: its playing take's file) |
| [`get_audio_editor`](#get_audio_editor) | read | The audio files open in the Waveform view, which one is playing and where, the effects rack (its plug-ins, bypass and latency; `loaded` false for a slot whose plug-in is still loading or failed to, with `error` saying why it failed: such a slot keeps its settings and is skipped) and an Apply's progress |
| [`close_audio_editor`](#close_audio_editor) | edit | Close an audio file in the Waveform view (`document` id), stopping it if it plays |
| [`play_audio_editor`](#play_audio_editor) | transport | Play an open audio file on its own (`document`), from `start` to `end` (the file's own samples; the whole file by default), on the master's outputs with no track processing, as the Waveform view's Play does; `play` false stops it |
| [`get_spectrum`](#get_spectrum) | read | The spectrum of an open audio file (`document`) over `start`..`end` (the file's own samples; the whole file by default), of one `channel` (from 0) or all channels mixed: `bands` (default 32, up to 512) log-spaced bands from 20 Hz to the Nyquist frequency, each its low and high edge in Hz and its level in dB, where a full-scale sine reads 0 dB (Hann-windowed FFTs of `fft` points, default 4096, half overlapping, power averaged) |
| [`edit_audio`](#edit_audio) | edit | Edit an open audio file (`document`) as the Waveform view does, as a step in the file's own history (undo_audio_edit), not the project's; nothing is written until save_audio |
| [`apply_audio_effects`](#apply_audio_effects) | edit | Render `start`..`end` of an open audio file (`document`; the file's own samples, the whole file by default) through the Waveform view's effects rack (add_insert with `editor` true) and write the result into the file as one step of its own history |
| [`capture_noise_print`](#capture_noise_print) | edit | Have a built-in effect learn from `start`..`end` (the file's own samples) of an open audio file (`document`) what it learns from audio: a noise print for Noise Reduction or Hiss Reduction (select the noise alone), or the clip levels for Clip Repair (its Scan: select the clipped audio; each channel's highest and lowest sample become the levels its bands sit under, and the runs found are counted), or for the Click & Pop Repair one of its scans (`scan`: "Scan For Threshold Levels", the default, sets Max Level to the highest level measured, Avg Level to the lowest and Min Level to the mean; "Scan For All Levels" also sets the six detection and rejection thresholds from a trial detection; they are its parameters, to change after) |
| [`undo_audio_edit`](#undo_audio_edit) | edit | Take back the last edit to an open audio file (`document`), or with `redo` true bring one back: its own history, not the project's |
| [`copy_audio`](#copy_audio) | read | Copy `start`..`end` (the file's own samples) of an open audio file (`document`) for edit_audio's paste, into this file or another |
| [`save_audio`](#save_audio) | destructive | Write an open audio file's edits over the file itself, in its own WAV format (16- or 24-bit stays so) |
| [`save_audio_as`](#save_audio_as) | destructive | Write an open audio file (`document`) to `path` (absolute, .wav; replaced if it exists; its folder must exist) in the file's WAV format (32-bit float for a FLAC, MP3 or AIFF), and carry on editing that file |
| [`get_metadata`](#get_metadata) | read | An open audio file's tags (`document`): title, artist, album, album_artist, year, track ("3" or "3/12"), genre, composer, comment, and its cover art's type and size (null without one) |
| [`set_metadata`](#set_metadata) | edit | Change an open audio file's tags (`document`): only the fields given change, and an empty string clears one |
| [`export_audio`](#export_audio) | destructive | Export an open audio file (`document`) as it is now, unsaved edits included, to `path` (absolute; overwritten) in another format, leaving the document and its file as they are |
| [`find_audio_problems`](#find_audio_problems) | edit | Scan an open audio file (`document`) for problems, over `start`..`end` (the file's own samples; the whole file when both are left out) |
| [`get_audio_problems`](#get_audio_problems) | read | The problems the last find_audio_problems on an open audio file (`document`) found that are still to repair, as it returned them; `stale` is true (and the list empty) when there was no scan or the file's audio has changed since (an edit, undo or redo) |
| [`repair_audio_problems`](#repair_audio_problems) | edit | Repair problems find_audio_problems found in an open audio file (`document`): `ids` (from its list), or every one with `all` true |
| [`get_transport`](#get_transport) | read | Transport state: playing, recording, playhead position (samples; in beats of the time signature's lower number; as bar.beat from 1; and in quarter notes, which is what plug-ins are told), sample rate, the tempo (quarter notes per minute) and time signature at the playhead (see get_tempo_map for changes), the loop region (`loop`: start, end in samples, enabled), the punch range (`punch`, the same shape) and the metronome settings |
| [`play`](#play) | transport | Start playback from the playhead |
| [`stop`](#stop) | transport | Stop playback or recording |
| [`locate`](#locate) | transport | Move the playhead to `position` (samples) |
| [`set_loop`](#set_loop) | edit | Set the loop region and/or turn looping on or off |
| [`set_punch`](#set_punch) | edit | Set the punch range and/or turn punch recording on or off, like set_loop: `start` and `end` in samples go together, `enabled` turns it on or off (a range alone leaves it as it was) |
| [`set_metronome`](#set_metronome) | edit | Change the metronome: `enabled`, `volume_db` (-60 to +6), `only_while_recording`, `count_in_bars` (0 = off, 1 or 2: when recording starts from a stop, the transport starts that many bars early with the click, even with the metronome off, and the take starts at the playhead; in the app, the Record button's right-click menu and the metronome's menu set it too); fields left out stay as they are |
| [`record`](#record) | transport | Start recording every armed audio track that has an audio input, and play |
| [`export_mixdown`](#export_mixdown) | destructive | Render the master mix to a 32-bit float WAV at `path` (absolute), through the live engine and plug-ins (bit-faithful to playback) |
| [`mixdown_to_new_file`](#mixdown_to_new_file) | edit | Mixdown to a new file: render the mix as it plays (every track, level, plug-in, automation and send, and the master) into a new 32-bit float WAV in the project's Rendered folder, then open it in the Waveform view for final edits or an export |
| [`get_export_status`](#get_export_status) | read | Progress of the running export, render, mixdown or file export (`progress` 0 to 1 through rendering and writing the file; `kind`: export, render, mixdown or file; `what` it is making, as the app's progress dialog says), or how the last one ended: finished with its path, its `loudness` (as get_loudness reports it: integrated, range, true peak, maxima) and, if it was normalised, `normalized` (target, ceiling, `gain_db` applied, null when nothing was above the -70 LUFS gate, and `held_by_peak`), and for a track's render the `track` its file went on; or failed with the reason (a cancel: "export cancelled") |
| [`cancel_export`](#cancel_export) | transport | Stop the running export; no file is written |
| [`get_loudness`](#get_loudness) | read | Read the master's loudness meter (EBU R 128 / ITU-R BS.1770-4, 'EBU Mode') |
| [`set_loudness_target`](#set_loudness_target) | edit | Set the integrated loudness the mix aims at, in LUFS: a `preset` ("streaming" for -14, "broadcast" for -23, EBU R 128), or `lufs` for any value from -40 to -5 |
| [`reset_loudness`](#reset_loudness) | transport | Start a new loudness measurement on the master: clear the integrated loudness, loudness range, true peak and maxima (see get_loudness) |
| [`measure_loudness`](#measure_loudness) | read | Measure a mono or stereo audio file (WAV, FLAC, MP3 or AIFF; absolute `path`) with the same EBU R 128 meter as the master: integrated loudness (LUFS), loudness range (LU), true peak (dBTP), and the maximum momentary and short-term loudness; momentary and short-term are their values at the end of the file |
| [`get_markers`](#get_markers) | read | List the markers in time order: `number` (1, 2, 3… as the keys 1-9 jump to them), `id`, `name`, `tick` (960 per quarter note; markers keep their bar when the tempo changes), `position` (samples now) and `bar` / `beat` (from 1) |
| [`add_marker`](#add_marker) | edit | Add a marker at `position` (samples), or at `bar` (and `beat`, both from 1); the playhead by default |
| [`set_marker`](#set_marker) | edit | Rename a marker (`name`) and/or move it to `position` (samples) or `bar` / `beat` (from 1) |
| [`remove_marker`](#remove_marker) | destructive | Remove a marker by id |
| [`go_to_marker`](#go_to_marker) | transport | Move the playhead to a marker: by `marker` id, by `number` (1 is the first in time), or `direction` "next" / "previous" from the playhead (from a marker, the one after or before it) |
| [`save_template`](#save_template) | destructive | Save the open project's setup as a template called `name`, to start new projects from (new_project's `template`): every track, folder, bus, route, send, mixer setting and plug-in with its current settings (an amp sim's chosen amp too), automation lanes without their points, the tempo map, time signature, loudness target and markers |
| [`list_templates`](#list_templates) | read | The saved project templates by name: `name` (what new_project's `template` takes), `path`, `modified` (seconds since 1970) and `size` in bytes |
| [`rename_template`](#rename_template) | edit | Rename the template `name` to `new_name`; refused if one is called that already |
| [`delete_template`](#delete_template) | destructive | Delete the template `name` for good (projects made from it keep their setup) |
| [`list_rack_presets`](#list_rack_presets) | read | The saved presets of the Waveform view's effects rack, by name: `name` (what load_rack_preset takes), `path` and `modified` (seconds since 1970) |
| [`save_rack_preset`](#save_rack_preset) | destructive | Save the Waveform view's effects rack as it is now as the preset `name`: its slots in order, each plug-in (VST3 or built-in), its bypass and its current settings (a built-in effect's noise print too) |
| [`load_rack_preset`](#load_rack_preset) | destructive | Replace the Waveform view's effects rack with the preset `name` (from list_rack_presets): the plug-ins in the rack now are removed with their settings, and the preset's slots come in with theirs |
| [`set_rack_sync`](#set_rack_sync) | edit | Sync the Waveform view's effects rack with the track of the clip `document` was opened from (`sync` true), or stop (false) |
| [`delete_rack_preset`](#delete_rack_preset) | destructive | Delete the rack preset `name` for good (the rack as it is now is untouched) |
| [`get_audio_settings`](#get_audio_settings) | read | The audio configuration: chosen driver type (host), devices, sample rate, buffer size, processing block and audio threads, plus what the running device actually opened with (or why it failed) |
| [`list_audio_devices`](#list_audio_devices) | read | Driver types (hosts: WASAPI, ASIO) and, for `host` (default: the current one), its output and input devices plus the sample rates and buffer sizes the chosen output device supports |
| [`set_audio_settings`](#set_audio_settings) | destructive | Change the audio configuration and restart the device; only the fields given change |
| [`get_midi_inputs`](#get_midi_inputs) | read | Open MIDI input ports (keyboards, controllers) and ports that failed to open (on Windows a port can be open in one application at a time) |
| [`reconnect_midi`](#reconnect_midi) | edit | Close and reopen all MIDI input ports, e.g |
| [`get_stats`](#get_stats) | read | Engine health: DSP load (time spent processing over the time the audio driver allows: a buffer period, or a whole burst for a driver that calls back in bursts; over 1 is a dropout), delay-compensation latency in samples, the block size the engine processes and exports in (`block_size`), the device's buffer (`device_block`; smaller when buffers are gathered into blocks, set_audio_settings' `processing_block`), the driver's cadence in samples (the audio it asks for between waits: the device buffer for an even driver, more for a bursty one such as FlexASIO on shared WASAPI), worker threads, and counters of plug-in skips, engine skips and input gaps |
| [`get_cpu_usage`](#get_cpu_usage) | read | CPU use of each track (and the master, `track` null) and of each insert on it, averaged over the last half second of processing (`window`, seconds): `per_buffer_us` (microseconds per audio buffer), `share` (that time over the buffer's duration, its deadline: 1 is all of it) and `buffers` measured |
| [`screenshot`](#screenshot) | read | Capture the DAW window as a PNG image, to see what the user sees |
| [`get_meters`](#get_meters) | read | Current meter levels as the UI shows them: per track and for the master, the peak (falling at 24 dB/s) and peak-hold of the left and right channels in dBFS (-100 means silence), and the hardware input and output levels (`inputs_db`, `outputs_db`; the outputs include the metronome, which plays after the master) |
| [`get_view`](#get_view) | read | What the UI shows: the selected track, clip or master, the visible timeline range (samples) and zoom (`samples_per_pixel`), the waveform display gain (`waveform_scale`), which panels are open (`piano_roll`: the MIDI clip open in the editor, if any), whether snap and follow-playhead are on, the song's grid (`grid`: a division or "auto", see set_grid) and the division the Multitrack shows now (`grid_division`, Auto's at this zoom), the loudness meter's scale and units (`loudness_scale`, `loudness_relative`), and the Waveform view's time display (`editor_time`) with the grid division it shows in bars and beats (`editor_grid_division`) |
| [`set_view`](#set_view) | edit | Change what the UI shows: select a track, clip or the master; scroll and zoom the timeline to `start`..`end` (samples; as close as 32 points a sample, where single samples show as dots on a line, and as wide as 2 seconds a point), or `zoom` in, out, to fit the project or to a clip; set every track's row height (`track_height`, points 32 to 400); set the waveform display gain (`waveform_scale`, 1 to 16, display only); open a MIDI clip in the piano roll (`piano_roll`: its id, or 0 to close); show or hide the mixer and inspector; turn snap to the grid (its division: set_grid) and follow-playhead on or off; set the loudness meter's scale (`loudness_scale`: "ebu9", -18 to +9 LU around the target, or "ebu18", -36 to +18) and units (`loudness_relative`: LU from the target, or LUFS); set the Waveform view's time display (`editor_time`: `time`, `samples` or `bars_beats`) |
| [`get_layout`](#get_layout) | read | The window's layout: where the panes are (`multitrack`, `mixer`, `inspector`, `piano_roll`, `diagnostics`, and `waveform`: an open file's tab, with its `document`, or the place files open while none is), as nested containers (`horizontal` and `vertical` splits with each child's share, `tabs` with the active one) and whether each shows; and the saved layouts' names (`layouts`, "Default" first) |
| [`set_layout`](#set_layout) | edit | Arrange the window as the saved layout `name` (get_layout's `layouts`; "Default" is the built-in one): its panes where it has them, the mixer, inspector and Diagnostics shown or hidden as it has them; open files go where it keeps their place |
| [`save_layout`](#save_layout) | edit | Keep the window's layout as `name` (app-wide, in the settings), replacing a saved layout of that name: where each pane is and how big, and where files open (not the open files themselves) |
| [`delete_layout`](#delete_layout) | destructive | Delete the saved layout `name` |
| [`list_recent_projects`](#list_recent_projects) | read | The start screen's recent projects (app-wide, in the settings), most recently opened first: each one's `path` (absolute, for open_project), `name`, `folder`, `state` ("present", "missing" when the file is gone or its drive isn't connected, or "unknown" when its drive didn't answer in time), `modified` (when it was last saved, RFC 3339, local time) with `saved` as the screen says it ("today 14:32", "yesterday", "3 days ago", a date), the `template` it was started from when Oxide made it from one, and `open` for the project open now |
| [`get_app_info`](#get_app_info) | read | What Help > About shows: Oxide's `version` (major.minor.patch, the one its installer compares to refuse a downgrade), the `commit` it was built from (a short git hash, `-modified` if built from changed files; null without the app window) and the program's `folder` (absolute; its settings, templates and rack presets are kept there; null without the app window) |
| [`remove_recent_project`](#remove_recent_project) | destructive | Take a project off the start screen's recent projects (the list only: the project's files stay as they are) |
| [`search_manual`](#search_manual) | read | Search Oxide's manual (the HTML user guide installed beside the program) for a question or a few words, to answer "how do I…" from it: the best passages first, each a page (`page`: its id, for open_manual; `title`) or a section of one (`anchor` and `heading`), with a `snippet` around the first word matched and the passage's whole `text`, and its `score` (words of the question matched times 1000, plus where: a page's title counts most, then a heading, then the text) |
| [`open_manual`](#open_manual) | edit | Show the user a page of Oxide's manual in their default web browser (needs the app window) |

## get_project

Describe the open project: tracks (with ids, kind, folder parent, input, output, mixer settings, inserts, clips and automation lanes), the master bus, the opening tempo and time signature (changes: get_tempo_map) and sample rate. Positions and lengths are in samples at `sample_rate`. Call this first to learn the ids other actions take.

Kind: read. No arguments.

## new_project

Close the open project and start a new one. Unsaved changes are lost (save_project first). With `name` and `folder` (an absolute path), the project gets a folder of its own, `folder/<name>/`, holding `<name>.oxide` and the folders its files go in (Recorded, Imported, Rendered, Exported, Backups, Temp), and is saved there at once; refused if that folder exists and isn't empty. Without them it is an unsaved, empty project, which can't record or import files until it is saved into a folder; in the app it is no project at all (the start screen), and changes to it are refused until save_project gives it a path, so give `name` and `folder`. With `template` (a name from list_templates; needs `name` and `folder`) it starts as that template's setup: its tracks, buses, routing and plug-ins with their settings, no clips. Returns the project file's path.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `folder` | string |  | Absolute path of the folder the project's own folder is made in. |
| `name` | string |  | The project's name: its folder and file are named after it. |
| `template` | string |  | The template to start from, by name (list_templates). |

## open_project

Open a project file (.oxide, or an older .dawproj; absolute `path`), replacing the open project (unsaved changes are lost; the Waveform view's files close, unsaved edits in them lost too, and the files the project had open there reopen). Plug-ins reload with their saved state, which can take a few seconds: the call answers once the project is in (the app shows its loading bar meanwhile; get_busy reports the progress, and if the open is cancelled (cancel_open, or Cancel on the bar) or replaced by another, this call answers with an error). `newer_backup` names a backup written after the file was last saved (Oxide closed without saving, or crashed): open_backup opens it instead.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | Path of the .dawproj file. |

## list_backups

The open project's backups in its Backups folder, newest first: `path`, `modified` (seconds since 1970), `age_seconds` and `size` in bytes. Oxide backs a project up every few minutes while it has unsaved changes and the transport is stopped, keeping the newest (both set in the app's settings); the project file itself is only written by save_project.

Kind: read. No arguments.

## save_backup

Back the open (saved) project up now into its Backups folder, as autosave does, keeping the newest backups; the project file is untouched. Returns the backup's path.

Kind: edit. No arguments.

## open_backup

Open one of the open project's backups (absolute `path`, from list_backups or open_project's `newer_backup`) in its place: it becomes the open project, unsaved until save_project writes it over the project file. Unsaved changes are lost.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | Path of the .dawproj file. |

## get_busy

What Oxide is busy with, for an agent to wait on: `loading` (a project being opened in the app, in stages: `stage` is `closing` (the last project's plug-ins destroyed), `reading` (its audio files read on workers) or `loading` (its plug-ins loaded a few a frame); `done` of `total` over the whole open, `stage_done` of `stage_total` within the stage; `next`, the plug-in or file it is on, and `track`, the loading plug-in's track; cancel_open stops it; `kind` says what the plug-ins load for: `open` (a project), `rack` (only the Waveform view's effects rack, the last run's or a preset, which leaves the project open; `rack` is then true too), `undo` or `redo` (a step that brought plug-ins back; `label` is its name; the project plays as before the step until they are in) or `sample_rate` (every plug-in reloading at a new rate); only an open can be cancelled), `saving` (the project file a save is writing in the background), `save` (how far it has come: `done` of `total` plug-in states read, `next` and `track` the plug-in read next, `writing` once the file is being written; also while the app looks for unsaved changes before closing, with `saving` null) and `exporting` (an export or render running; get_export_status has its progress). All null/false when idle. open_project and the other actions wait for their own work. While the app is opening or saving a project, every other call waits until it is done (except get_busy, cancel_open and screenshot, which answer at once), so a call never sees a project half loaded or changes one being saved.

Kind: read. No arguments.

## cancel_open

Stop opening the project the app is opening (get_busy's `loading`) and go back to the project open before it, reopened from its file with its plug-ins (changes not saved there were given up when the open was asked for), or, with none before, to no project (the start screen). Returns `reopened`: that project's path, or null, once it is back whole; the open_project call it cancelled answers with an error. Refused when nothing is being opened. A plug-in already loading finishes first.

Kind: destructive. No arguments.

## get_unsaved_changes

What closing the project now would lose, as the app's "Save the changes?" question sees it: `project` is true when the project differs from its file as last saved or opened (every plug-in's state read now; an edit undone again is no change, a plug-in turned in its editor is one; a backup opened in its place always is), and `files` lists the Waveform view's files with unsaved edits (`document` ids and paths). False while a project's plug-ins are still loading.

Kind: read. No arguments.

## save_project

Save the project, with every plug-in's state. Without `path` it saves to the file it was opened from or last saved to; give an absolute path (.oxide) the first time or to save a copy elsewhere: saved into another folder, the project's files (recordings, imported copies, renders) are copied along to the same places inside it, so the new folder is complete. Overwrites that file. In the app the save runs behind its busy bar (each plug-in's state read in a frame of its own, the file written on a worker; get_busy reports it) and the call answers once the file is written; other calls wait meanwhile. Refused while another save is being written or plug-ins are loading.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `path` | string |  | Where to save (.dawproj); defaults to the project's current file. |

## undo

Undo the last edit: every action that changes the project (tracks, clips, takes, routing, mixer, inserts, tempo map, loop, punch, metronome) is one undo step, recorded only if it changed something; repeated set_ calls on the same object in quick succession are one step, and a recording is one step. Plug-in parameter changes are not undo steps. A step that brings plug-ins back (a removed insert, a replaced plug-in) loads them a plug-in a frame behind the app's busy bar (get_busy's `kind` is `undo`); the call answers once they are in. Returns what was undone and the history. Refused while recording, while a project opens and while an export or render runs.

Kind: edit. No arguments.

## redo

Redo the last undone edit. Any new edit after an undo clears what could be redone. Plug-ins it brings back load as undo's do, and the call answers once they are in. Returns what was redone and the history.

Kind: edit. No arguments.

## get_history

The undo history: what undo would undo (`undo`, most recent first) and what redo would redo (`redo`), with the history's estimated memory use. Opening or starting a project, or changing the sample rate, starts it afresh.

Kind: read. No arguments.

## set_tempo

Set the opening tempo, in quarter notes per minute (20 to 300), which holds until the first tempo change (see get_tempo_map). Moves the bar/beat grid and MIDI clips; audio clips stay where they are in time unless they follow the tempo (set_clip `source_tempo`), which move and stretch with it.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bpm` | number | yes |  |

## set_time_signature

Set the opening time signature, e.g. 4/4 or 7/8 (`numerator` 1 to 16; `denominator` 2, 4, 8 or 16), which holds until the first time-signature change.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `denominator` | integer | yes |  |
| `numerator` | integer | yes |  |

## set_grid

Set the song's grid division (`grid`), saved with the project: "1/4", "1/8" (a new project's), "1/16", "1/32" or "1/64" of a whole note (1/4 is a beat in 4/4, 1/8 a beat in 6/8), each with a "t" for triplets ("1/8t"), or "auto": the division follows each view's zoom (the finest of 1/64 to 1/4 whose lines are at least 14 points apart, then the beat or every 1, 2, 4… bars; get_view's `grid_division` is the one the Multitrack shows now). Lines run from each bar's start, so in a meter whose bar isn't a whole number of divisions (7/8 at 1/4) the bar's last one is shorter. The Multitrack draws its lines and, with snap on (set_view `snap`), snaps the playhead, clips, selections, markers and automation points to them; the piano roll and the Waveform view's bars and beats show the same grid. Returns the grid.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `grid` | string | yes | "auto", or "1/4", "1/8", "1/16", "1/32", "1/64" (a "t" after it for triplets). |

## get_tempo_map

The tempo map: the opening tempo (quarter notes per minute) and time signature, and every change after it with its bar and beat (from 1) and where it falls in samples (`position`) and ticks (`tick`, 960 per quarter note, as MIDI clips count them). Each change sets a tempo, a time signature, or both, from there on.

Kind: read. No arguments.

## add_tempo_change

Change the tempo (`bpm`, quarter notes per minute) and/or time signature (`numerator`, `denominator`) from bar `bar` (from 2; bar 1 is the opening, see set_tempo) and beat `beat` (from 1, default 1). A time signature changes only on beat 1. Changes sit in bars and beats, so they stay on their bar when an earlier tempo changes; changes are instant (no ramps). A change already at that place is updated. Audio clips stay where they are in time (unless they follow the tempo: set_clip `source_tempo`); MIDI clips stay on their beats.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer | yes | Bar, from 2. |
| `beat` | integer |  | Beat within the bar, from 1 (default 1). |
| `bpm` | number |  | Quarter notes per minute, 20 to 300. |
| `denominator` | integer |  |  |
| `numerator` | integer |  |  |

## remove_tempo_change

Remove the tempo/time-signature change at `bar` and `beat` (from 1; beat defaults to 1).

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer | yes |  |
| `beat` | integer |  |  |

## add_track

Add a track and return its id. `kind` is audio, instrument, folder (a submix bus for the tracks inside it) or bus (a return track for reverbs and other shared effects, fed by other tracks' sends or outputs; it holds no clips). `parent` puts it inside that folder track.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `kind` | Kind | yes |  |
| `name` | string |  | Display name; a default like "Audio 3" when omitted. |
| `parent` | integer |  | Folder track to put it in. |

## remove_track

Delete a track with its clips and plug-ins. A folder's sub-tracks move up a level.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `track` | integer | yes |  |

## set_track

Rename a track, recolour it (`color` as "#rrggbb"), collapse/expand a folder in the arrangement, set its row `height` in points (32 to 400; 0 for the default; taller rows draw clearer waveforms), or show or hide its automation lanes (`show_automation`). Only the fields given change. The height and the lanes shown are view state: saved with the project, but not undo steps.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `collapsed` | boolean |  | Folders only: hide (true) or show the sub-tracks. |
| `color` | string |  | Colour as "#rrggbb". |
| `height` | number |  | Row height in the arrangement, in points (32 to 400); 0 for the default. |
| `name` | string |  |  |
| `show_automation` | boolean |  | Show (true) or hide the track's automation lanes under it in the arrangement. |
| `track` | integer | yes |  |

## set_track_input

Set what feeds a track. Audio tracks: {"type":"audio","channel":0,"stereo":false} where channel is the 0-based hardware input (0 is "In 1"); stereo takes channel and channel+1. Instrument tracks: {"type":"midi","port":null,"channel":null} where port is a MIDI input name (null = all ports) and channel 0-15 filters one MIDI channel (null = all). {"type":"none"} disconnects it. A track fed by a plug-in's extra output (get_project shows its input as {"Plugin":{"insert":..,"output":..}}) belongs to a multi-out rack: create_multi_out_rack and remove_multi_out_rack set and clear those.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `input` | InputArg | yes |  |
| `track` | integer | yes |  |

## set_track_output

Route a track's main output: {"type":"parent"} (its folder, or the master), {"type":"master"}, {"type":"bus","track":id} (any folder or bus track that would not create a loop through outputs or sends), or {"type":"hardware","channel":0} (straight to the output pair starting at that 0-based channel, bypassing the master). Sends are separate: see add_send.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `output` | OutputArg | yes |  |
| `track` | integer | yes |  |

## set_track_monitor

Input monitoring: auto (hear the input while the track is armed), on, or off. Monitoring runs the input through the track's plug-ins.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `monitor` | MonitorArg | yes |  |
| `track` | integer | yes |  |

## move_track

Move a track (with its sub-tracks) one place up or down among its siblings.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `direction` | Direction | yes |  |
| `track` | integer | yes |  |

## set_track_parent

Move a track (with its sub-tracks) into a folder track, or to the top level when `folder` is omitted. A folder cannot go inside itself.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `folder` | integer |  | Folder to move into; omit for the top level. |
| `track` | integer | yes |  |

## move_track_to

Move a track (with its sub-tracks) to a place in the track list, as dragging its header does: `before` or `after` another track (it joins that track's folder, or the top level), or into `folder` as its `position`th sub-track (0 = first; omitted or past the last: at the end), or with none of them to the end of the top level. Give at most one of `before`, `after` and `folder`. Folder sums and routing follow. Refused for a folder inside itself, beside a track inside the one moved, or a move that would close a loop.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `after` | integer |  | Just below this track and its sub-tracks, in its folder (or the top level). |
| `before` | integer |  | Just above this track, in its folder (or the top level). |
| `folder` | integer |  | Into this folder. |
| `position` | integer |  | With `folder`: its place among the folder's sub-tracks (0 = first; default: last). |
| `track` | integer | yes |  |

## set_track_mixer

Change a track's mixer settings. Only the fields given change: `gain_db` (-60 to +12 dB), `pan` (-1 left to 1 right), `mute`, `solo`, `arm` (record arm; for instrument tracks, play from the MIDI keyboard). Returns the resulting settings.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `arm` | boolean |  | Record arm (for instrument tracks: play from the MIDI keyboard). |
| `gain_db` | number |  | Volume in dB, -60 to +12. |
| `mute` | boolean |  |  |
| `pan` | number |  | Balance, -1 (left) to 1 (right). |
| `solo` | boolean |  |  |
| `track` | integer | yes |  |

## set_track_enabled

Switch a track off (`enabled` false) or on again. Off, the track and whatever reaches the mix only through it (a folder's tracks) leave the audio graph: its plug-ins cost no CPU, unlike a mute, and it is silent; every setting is kept. An undo step.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `enabled` | boolean | yes |  |
| `track` | integer | yes |  |

## render_track

Render a track as it sounds (its clips, or its MIDI through its instrument; its plug-ins, fader, pan and automation; a folder's tracks; not its sends) from the song's start (sample 0) to the end of its last clip plus `tail_seconds` (default 2, for reverb and note releases) into the project's Rendered folder as a 32-bit float WAV, through the same engine and plug-in pre-roll as an export, so it sounds exactly like playback; the file is silent up to the track's first sound, so it lines up with the song wherever it is loaded. When it finishes, the file goes on a new audio track right under it at sample 0, routed where it goes, with its sends, at 0 dB and centre; with `disable` (default true) the original is switched off (set_track_enabled) in the same undo step, to save its CPU. Stops playback and runs in the background on export's thread, as export_mixdown does: it answers at once with the file's path; poll get_export_status (its `track` names the new track once finished), and cancel_export stops it (no new track, the original left on). Refused while the project's plug-ins are still loading.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `disable` | boolean |  | Switch the original off once the render is in (default true). |
| `tail_seconds` | number |  | Seconds rendered past the last clip's end (default 2). |
| `track` | integer | yes |  |

## set_master

Change the master bus: `gain_db` (-60 to +12), `pan`, `mute`, and `output_channel` (first channel of the hardware output pair, 0-based and even); and its row at the top of the arrangement: `height` in points (32 to 400; 0 for the default) and `show_automation` (its lanes shown under it). Only the fields given change. The height and the lanes shown are view state: saved with the project, but not undo steps.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `gain_db` | number |  |  |
| `height` | number |  | The master's row height in the arrangement, in points (32 to 400); 0 for the default. |
| `mute` | boolean |  |  |
| `output_channel` | integer |  | First channel (0-based, even) of the hardware output pair. |
| `pan` | number |  |  |
| `show_automation` | boolean |  | Show (true) or hide the master's automation lanes under its row. |

## add_send

Send a copy of `track`'s signal into `target`, a folder or bus track (add a bus with add_track kind "bus" for a reverb return), alongside its main output. `gain_db` is the send level (-60 to +12 dB, default 0). `pre_fader` takes the signal before the track's fader and pan, so the fader doesn't change it (default false: post-fader). Refused if it would create a loop. Returns the send with its id.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `gain_db` | number |  | Send level in dB, -60 to +12; 0 when omitted. |
| `pre_fader` | boolean |  | Take the signal before the fader and pan. |
| `target` | integer | yes | The folder or bus track to send into. |
| `track` | integer | yes |  |

## add_send_to_new_bus

Add a new bus (a return track, at the end of the project, routed to the master) and send `track` into it: post-fader at 0 dB, or as `gain_db` and `pre_fader` say. Put a reverb or delay on the bus with add_insert. Returns the send, whose `target` is the new bus's track id.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `gain_db` | number |  | Send level in dB, -60 to +12; 0 when omitted. |
| `pre_fader` | boolean |  | Take the signal before the fader and pan. |
| `track` | integer | yes |  |

## set_send

Change a send by its id: `gain_db` (-60 to +12 dB), `pre_fader`, `mute`, or `target` (another folder or bus; refused if it would create a loop). Only the fields given change. Muting the track also silences its sends, pre-fader ones included.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `gain_db` | number |  | Send level in dB, -60 to +12. |
| `mute` | boolean |  |  |
| `pre_fader` | boolean |  |  |
| `send` | integer | yes |  |
| `target` | integer |  | Another folder or bus track to send into. |

## remove_send

Delete a send by its id.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `send` | integer | yes |  |

## list_plugins

List the built-in effects (vendor "Built in": Oxide's own effects, path native:<id>; an id from before an effect was renamed still names it) and the installed VST3 plug-ins (name, vendor folder, path). `filter` matches name or vendor, case-insensitively.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `filter` | string |  | Case-insensitive text to match in the name or vendor. |

## rescan_plugins

Look for newly installed VST3 plug-ins. Returns how many are installed.

Kind: edit. No arguments.

## add_insert

Load a VST3 plug-in or a built-in effect ("Amplify") into a track's insert chain (or the master's when `track` is omitted, or the Waveform view's effects rack with `editor` true). `plugin` is a built-in effect's or an installed plug-in's name (exact, or a unique part of it; see list_plugins; a built-in effect's id or name from before it was renamed still finds it, and the insert gets today's) or a path. `position` puts it at that place in the chain (0 is first; past the end appends); without it instruments go first and effects last. Returns the insert id and its latency in samples. Loading takes up to a few seconds.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `editor` | boolean |  | The Waveform view's effects rack instead of a track or the master. |
| `plugin` | string | yes | A built-in effect's or installed plug-in's name (or a unique part of it), or a path. |
| `position` | integer |  | The place in the chain, from 0; omit for instruments first, effects last. |
| `track` | integer |  | Track id; omit for the master bus. |

## remove_insert

Remove a plug-in from its insert chain, discarding its settings.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |

## set_insert

Bypass or enable a plug-in (`bypass`). A bypassed plug-in passes audio through unchanged but keeps its latency, so timing stays aligned.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bypass` | boolean |  |  |
| `insert` | integer | yes |  |

## move_insert

Move a plug-in in its insert chain, where the order is the signal's: the first processes first (a tuner before an amp hears the dry input). One place up (earlier) or down (later) with `direction`, or to `position` (0 is first; past the end makes it last). With `track` (or `master` true) it moves to that track's (or the master's) chain instead, at `position` or last, as the same instance with its settings and its parameters' automation lanes. Give `direction` or a place, not both.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `direction` | any |  |  |
| `insert` | integer | yes |  |
| `master` | boolean |  | Move it to the master's chain. |
| `position` | integer |  | The place in the chain, from 0. |
| `track` | integer |  | Move it to this track's chain. |

## copy_insert

Copy a plug-in: a new instance of it with the same settings (its state now) and bypass, without its automation, into `track`'s chain (or the master's with `master` true; without either, its own chain, right after it) at `position` (0 is first; default: last). Returns the new insert's id.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |
| `master` | boolean |  | Copy it to the master's chain. |
| `position` | integer |  | The place in the chain, from 0. |
| `track` | integer |  | Copy it to this track's chain. |

## copy_plugin

Copy a plug-in insert to the plug-in clipboard, as its menu's Copy does: which plug-in, its name, bypass and its settings as they are now. paste_plugin makes new instances from it, also after the original is removed. Replaces what was copied before; not an edit of the project. Returns what is on the clipboard.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |

## paste_plugin

Paste the copied plug-in (copy_plugin): a new instance with the settings it had when copied, into `track`'s chain (or the master's with `master` true) at `position` (0 is first; default: last). Paste as often as wanted. Returns the new insert's id.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `master` | boolean |  | Paste into the master's chain. |
| `position` | integer |  | The place in the chain, from 0. |
| `track` | integer |  | Paste into this track's chain. |

## set_inserts_bypassed

Bypass (`bypass` true) or enable every plug-in in a track's insert chain at once, or the master's when `track` is omitted (the Waveform view's rack with `editor` true), as the fx rack's power button does. Bypassed plug-ins keep their latency. Returns how many changed.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bypass` | boolean | yes |  |
| `editor` | boolean |  | The Waveform view's effects rack instead. |
| `track` | integer |  | Track id; omit for the master bus. |

## set_plugin_editor

Open or close a plug-in's own editor window (`open`). The window is a separate native window; opening it briefly silences that track.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |
| `open` | boolean | yes |  |

## list_plugin_parameters

List a plug-in's parameters: id, name, normalized value (0-1), the value as the plug-in displays it, unit, default and step count (0 = continuous, 1 = on/off, n = n+1 choices). `filter` matches names case-insensitively; results are paged by `offset`/`limit` (default 100) because some plug-ins have thousands. A built-in effect that counts what it does adds `counters` (the Click & Pop Repair: clicks corrected, big pops corrected, clicks rejected, since playback last started). Reading never interrupts the plug-in's audio, so it is safe during playback.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `filter` | string |  | Case-insensitive text to match in parameter names. |
| `insert` | integer | yes |  |
| `limit` | integer |  | At most this many (default 100). |
| `offset` | integer |  |  |

## set_plugin_parameter

Set a plug-in parameter to a normalized `value` from 0 to 1. `parameter` is its id or its name (exact, or a unique part; see list_plugin_parameters). Returns how the plug-in displays that value, e.g. "-6.0 dB", so the mapping can be checked. list_plugin_parameters shows the new value at once; the audio takes it at the plug-in's next block (a few milliseconds while audio runs), without interrupting it.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |
| `parameter` | ParameterRef | yes |  |
| `value` | number | yes | Normalized value, 0 to 1. |

## set_impulse_file

Give a Convolution Reverb insert (`insert`) its impulse response: the audio file at `path` (an absolute path; WAV, AIFF or FLAC, up to 30 s). The file is checked, copied into the project's Imported folder (a file already in the project is used in place) and saved with the effect by its project-relative path. A stereo file plays left on left and right on right; any other channel count is averaged to one impulse on both sides; it is resampled to the session's rate, then shaped by Room Size, Width, Damping LF/HF and Pre-Delay and normalised (k = 0.5/mean Σ|h|). It is prepared off the audio thread (waited for here, up to 10 s; exports always wait for it) and the impulse before it plays until it is in. Without an impulse the effect passes its input unchanged. Returns the impulse as get_impulse_file does.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |
| `path` | string | yes | Absolute path of a WAV, AIFF or FLAC file. |

## get_impulse_file

The impulse response a Convolution Reverb or Space Reverb insert (`insert`) plays: `file` (as saved, relative to the project's folder; null when none is chosen, and always for the Space Reverb, which makes its impulse from its settings), `preparing` (true while it or a settings change is being prepared; exports wait for it), `error` (why it could not be read, if so), and once prepared `seconds` (its length as played: with Room Size and Pre-Delay; the Space Reverb's as trimmed), `file_channels`, `file_rate` (Hz) and `file_seconds` (the file as it is; for the Space Reverb its impulse channels, 2 or 4 in true stereo, the session's rate and 1.45 x Decay).

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |

## list_plugin_outputs

A plug-in insert's audio outputs, as it names them: `bus` (0 is the main pair, which plays on the plug-in's own track; 1 and up are extra outputs, like a drum sampler's kick and snare or a sampler's outputs 2..32), `name`, `channels`, `active` (on in the plug-in: a track listens to it), and `track`, the track listening to it if any. Native effects have none.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |

## create_multi_out_rack

Route a multi-out plug-in's extra outputs to tracks of their own (a multi-out rack in a submix): a folder right after the plug-in's track holding one audio track per output in `outputs` (bus numbers from list_plugin_outputs, 1 and up; default every extra output without a track yet), each named after its output and hearing it at unity (0 dB), with its own fader, pan, inserts and sends. The main pair stays on the plug-in's track; the plug-in turns the outputs on. `insert` must be on an audio or instrument track. Returns the folder and the new tracks.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |
| `outputs` | array |  | The extra outputs to give tracks, by bus number (1 and up). |

## remove_multi_out_rack

Remove the tracks listening to a plug-in insert's extra outputs (and the folder they leave empty); the plug-in turns those outputs off. Their clips and plug-ins go too. Returns the tracks removed.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `insert` | integer | yes |  |

## split_range

Cut the clips on `tracks` (track ids; omit for every track that holds clips) at both edges of the time range `start`..`end` (samples), wherever an edge falls inside a clip: what a time selection's Split does. MIDI clips are cut at the tick nearest each edge, as split_clip cuts them. Returns the new clips' ids.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `end` | integer | yes | Range end, samples (after `start`). |
| `start` | integer | yes | Range start, samples. |
| `tracks` | array |  | The tracks; omit for every track that holds clips. |

## delete_range

Remove everything in the time range `start`..`end` (samples) on `tracks` (track ids; omit for every track that holds clips), as Delete does on a time selection: audio and MIDI clips are cut at the edges (as split_range cuts them) and the parts inside removed, the parts outside kept. Later clips don't move. Returns how many clips (or parts) were removed.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `end` | integer | yes | Range end, samples (after `start`). |
| `start` | integer | yes | Range start, samples. |
| `tracks` | array |  | The tracks; omit for every track that holds clips. |

## import_audio

Import an audio file (WAV, FLAC, MP3 or AIFF; absolute `path`) as a clip on an audio track at `position` (samples; default: the playhead). Files at another sample rate are converted. Returns the clip id and length.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | Path to a WAV, FLAC, MP3 or AIFF file. |
| `position` | integer |  | Timeline position in samples; the playhead when omitted. |
| `track` | integer | yes |  |

## move_clip

Move a clip so it starts at `start` (samples), and/or onto another audio or instrument `track`. Only the fields given change.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `start` | integer |  | New start in samples. |
| `track` | integer |  | Track to move it to. |

## split_clip

Cut an audio or MIDI clip in two at timeline `position` (samples, strictly inside the clip). The original keeps the first part and its id; returns both parts (the second with a new id), each as get_clip or get_midi_clip shows it. A MIDI clip is cut at the tick nearest `position` (960 per quarter note, through the tempo map): each note goes to the part it starts in; a note across the cut ends there and a copy (new id, same pitch, velocity and channel) starts the second part with the rest of its length; controller events from the cut on move to the second part, which starts with each controller (CC, pitch bend, program, per channel) set to its value in force at the cut.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `position` | integer | yes | Timeline position in samples, strictly between the clip's start and end. |

## delete_clip

Remove a clip from the timeline (the audio file on disk is kept).

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |

## set_clip

Change a clip's `gain_db` (-60 to +24), `name`, `stretch` or `pitch`. Only the fields given change. `stretch` is a time-stretch ratio from 0.25 to 4 (2 = twice as long, same pitch; 1 = untouched): the start stays and the length scales with it. `pitch` transposes by semitones, -24 to +24, at the same length. A stretched or transposed clip renders in the background and plays unchanged until `rendering` is false; export_mixdown refuses while any clip is rendering. `source_tempo` (quarter notes per minute the audio was recorded at, 20 to 999) makes the clip follow the project tempo and its changes: it keeps its start and the audio it plays, and stretches to fit the tempo map, moving with the bars when the tempo changes (its `stretch` is then set by the tempo and can't be set). `source_tempo` 0 stops following: the clip stays where and as long as it is, at the ratio the tempo gave its start. `fade_in` and `fade_out` are fade lengths in samples from each end (0 = none; together at most the clip's length, the fade-in first), with `fade_in_shape` / `fade_out_shape` "linear" or "equal_power" (the default). `transients` (default true for new clips) keeps attacks sharp and on time when stretching: each plays unstretched and the audio between them takes up the stretch. `trim_start` / `trim_end` move the clip's start or end edge to a timeline sample while its audio stays where it is: moving an edge in hides the audio there, moving it back out restores it, as far as the audio reaches (errors say where). Works on stretched clips, split pieces and clips that follow the tempo.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `fade_in` | integer |  | Fade lengths in samples from each end (0 = none). |
| `fade_in_shape` | any |  |  |
| `fade_out` | integer |  |  |
| `fade_out_shape` | any |  |  |
| `gain_db` | number |  |  |
| `name` | string |  |  |
| `pitch` | number |  | Transposition in semitones, -24 to +24 (0 = untouched). |
| `source_tempo` | number |  | Follow the project tempo, recorded at this many quarter notes per minute (20 to 999);
0 stops following. |
| `stretch` | number |  | Time-stretch ratio, 0.25 to 4 (1 = untouched). |
| `transients` | boolean |  | Keep attacks sharp and on time when stretching. |
| `trim_end` | integer |  | Move the clip's end edge to this timeline sample likewise. |
| `trim_start` | integer |  | Move the clip's start edge to this timeline sample, its audio staying put: later hides
the beginning, earlier brings it back (as far as the audio starts). |

## copy_clips

Copy clips (audio or MIDI, by id) to the clipboard, replacing what it held. The clipboard belongs to the session: it isn't saved and isn't part of undo.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes | Clip ids, audio or MIDI. |

## cut_clips

Copy clips (audio or MIDI, by id) to the clipboard and remove them from the timeline (their files stay on disk). All or none.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes | Clip ids, audio or MIDI. |

## delete_clips

Remove several clips (audio or MIDI, by id) in one undo step; their files stay on disk. All or none.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes | Clip ids, audio or MIDI. |

## paste_clips

Paste the clipboard so its earliest clip starts at `position` (samples; default: the playhead), the others keeping their distances. On the tracks they were copied from, or with `track` moved together so the topmost lands on that track (the others keep their rows below it). MIDI clips go on instrument tracks only. Every pasted clip gets new ids; fades, stretch, pitch and tempo following come along. Returns the new ids.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `position` | integer |  | Samples; the playhead when omitted. |
| `track` | integer |  | The track the topmost clip lands on (default: where they came from). |

## duplicate_clips

Copy clips (audio or MIDI, by id) to right after the last of them ends, on the same tracks (the clipboard is left alone). Returns the copies' ids, in the order given.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes | Clip ids, audio or MIDI. |

## move_clips

Move clips (audio or MIDI, by id) together: `delta` samples later (negative: earlier) and `track_delta` rows down (negative: up). All or none: refused if any would start before 0 or land on a track that can't hold it.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes |  |
| `delta` | integer | yes | Samples later (negative: earlier). |
| `track_delta` | integer |  | Rows down (negative: up); default 0. |

## copy_clips_to

Copy clips (audio or MIDI, by id) and move the copies as move_clips would: `delta` samples later (negative: earlier) and `track_delta` rows down (negative: up); the originals stay (an Alt-drag in the arrangement). All or none: refused if any copy would start before 0 or land on a track that can't hold it (MIDI clips go on instrument tracks, audio clips not on folders or buses). Returns the copies' ids, in the order given.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes |  |
| `delta` | integer | yes | Samples later (negative: earlier). |
| `track_delta` | integer |  | Rows down (negative: up); default 0. |

## crossfade_clips

Crossfade a clip with the clip before it on its track: the earlier one fades out and this one fades in over their overlap. `length` (samples) defaults to the overlap, or 20 ms if they only touch; if they overlap less than `length`, both are extended into the audio beyond their ends (half each, as far as there is audio). `shape`: "equal_power" (default; different audio, such as two takes) or "linear" (two pieces of the same audio, which then sum back to it exactly). Refused if a gap separates them. Returns both clips.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes | The later clip; it crossfades with the one before it on its track. |
| `length` | integer |  | Samples (default: the overlap, or 20 ms if they only touch). |
| `shape` | any |  | "equal_power" (default) or "linear". |

## set_active_take

Choose which take a clip plays (`take`, an index from 0). Loop recording gives a clip one take per pass, newest last; get_project lists them. Split the clip first to pick different takes for different parts (comping).

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `take` | integer | yes | Take index, from 0 (see get_project's `takes`). |

## delete_take

Remove one take from a clip (the audio file on disk is kept). A clip's last take cannot be removed; delete the clip instead. If the playing take is removed, the one before it plays.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `take` | integer | yes | Take index, from 0 (see get_project's `takes`). |

## get_clip_waveform

Read the waveform of a clip's playing take as `points` (default 200, max 4000) evenly spaced segments, each the minimum and maximum sample value (-1 to 1) of the clip's channels in that segment, with the segment's timeline start in samples. Use it to see where the audio is loud, quiet or silent.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `points` | integer |  | Number of segments (default 200, max 4000). |

## insert_midi_track

Insert a MIDI track, as a time selection's "Insert MIDI track" does: a new instrument track (armed) right after track `after` (and its sub-tracks; at the end without it), holding an empty MIDI clip from `start` to `end` (samples; without `end`, two bars from the bar `start` is in), or from bar `bar` (from 1) for `bars` bars (default 2). `instrument` (an installed plug-in name or a unique part of it, or a path; see list_plugins) is loaded on it and names the track; if it fails to load, nothing is added. Returns the track, the insert and the clip.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `after` | integer |  | The track to insert after; omit for the end. |
| `bar` | integer |  | The bar the clip starts on, from 1, instead of `start`. |
| `bars` | integer |  | Its length in bars (with `bar`); 2 when omitted. |
| `end` | integer |  | Where it ends, samples (with `start`); omit for two bars. |
| `instrument` | string |  | An instrument to load: installed plug-in name (or a unique part of it), or a path. |
| `start` | integer |  | Where the clip starts, samples; or give `bar`. |

## add_midi_clip

Add an empty MIDI clip to an instrument track and return it. Place it with `bar` (and optionally `beat`, both from 1, following the tempo map's time signatures) or with `start` in ticks (960 per quarter note, from the start of the song). Size it with `bars` (with `bar`) or `length` in ticks; one bar by default. Clips may overlap. MIDI clips stay on their beats when the tempo changes.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer |  | Bar to start at, from 1. |
| `bars` | integer |  | Length in bars (with `bar`). |
| `beat` | integer |  | Beat of that bar, from 1 (default 1). |
| `length` | integer |  | Length in ticks, instead of `bars`. |
| `name` | string |  |  |
| `start` | integer |  | Start in ticks from the start of the song, instead of `bar`. |
| `track` | integer | yes |  |

## get_midi_clip

Read a MIDI clip: start and length in ticks (960 per quarter note) and where they fall in samples now, its notes (id, tick from the clip start, length in ticks, pitch 0-127 with 60 = middle C, velocity 1-127, channel 0-15) and controller events. Notes past the clip's end are kept but don't play. describe_midi_clip reads the same notes as music (bars and beats, names, key, chords).

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |

## describe_midi_clip

Read a MIDI clip as music, to write a part that fits it: where it starts and ends (`start`, `end` as "bar:beat:tick": bars and beats from 1 as the tempo map's time signatures count them, then ticks into the beat, 960 per quarter note), the bars it spans, the tempo (quarter notes per minute) and time signature where it plays and each change inside it (`meters`), its estimated `key` and `runner_up` (Krumhansl-Kessler profiles over the notes' lengths; `score` is the fit, -1 to 1, with the key signature and scale), its `range` (lowest and highest note), its chords bar by bar (`progression` like "Am | F | C G", and `chords` with each one's place, length in ticks, root, quality, bass and tones; one per bar, or per half bar or beat where it changes; `implied` when only some tones are heard, as in a bass line; "N" for none) and every note as get_midi_clip gives it plus `at` (its song position, "bar:beat:tick") and `name` (C4 = 60). Names use flats in a flat key unless `spelling` is "sharps" or "flats". add_notes and set_notes take `at` and names back. An estimate: check it by ear or against the other parts.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `spelling` | any |  | "auto" (the default: flats in a flat key), "sharps" or "flats". |

## set_midi_clip

Move, resize or rename a MIDI clip: `start` and `length` in ticks (960 per quarter note). Only the fields given change. Shortening hides the notes past the new end without deleting them.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `length` | integer |  | In ticks, at least 1. |
| `name` | string |  |  |
| `start` | integer |  | Ticks from the start of the song. |

## remove_midi_clip

Delete a MIDI clip with its notes and controller events.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |

## add_notes

Add notes to a MIDI clip. Each note: where it starts, as `tick` (from the clip start; 960 per quarter note) or `at` (its song position "bar:beat:tick", bars and beats from 1 as describe_midi_clip gives them, e.g. "5:3:000" or "5:3"; it must fall in or after the clip's start), `length` in ticks (960 a quarter note, 480 an eighth), `pitch` 0-127 (60 = middle C) or a name ("C4", "Bb3", "F#2"), `velocity` 1-127 (default 100), `channel` 0-15 (default 0). All or nothing; an error names the note (from 0) and the bad value. Returns the new notes' ids, in the order given.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `notes` | array | yes |  |

## set_notes

Change notes in a MIDI clip by id: `tick` (from the clip start, 960 per quarter note) or `at` (song position "bar:beat:tick"), `length` in ticks, `pitch` (0-127 or a name like "Bb3"), `velocity`, `channel` (only the fields given change). All or nothing. Returns the changed notes.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `notes` | array | yes |  |

## resize_notes

Resize notes in a MIDI clip by one edge, as dragging a note's edge in the piano roll does: `edge` "start" or "end" moves by `by` ticks (960 per quarter note; negative is earlier) and the other edge stays. A note keeps at least 1 tick and never starts before the clip's start. `notes` are note ids; all or nothing. Returns the changed notes.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `by` | integer | yes | Ticks to move it by; negative is earlier. |
| `clip` | integer | yes |  |
| `edge` | NoteEdge | yes | Which edge moves: "start" or "end". |
| `notes` | array | yes | Note ids. |

## remove_notes

Delete notes from a MIDI clip by id. All or nothing.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `notes` | array | yes |  |

## copy_notes

Copy notes of a MIDI clip (`clip`, `notes` as note ids) to the note clipboard, as          Ctrl+C in the piano roll does, replacing what it held: their distances from the          earliest one (in ticks, 960 per quarter note), pitch, length, velocity and channel.          The note clipboard is the session's, apart from the clip clipboard (copy_clips): it          isn't saved and isn't part of undo. All or nothing. Returns how many.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `notes` | array | yes |  |

## cut_notes

Copy notes of a MIDI clip (`clip`, `notes` as note ids) to the note clipboard, as          copy_notes does, and delete them from the clip, as Ctrl+X in the piano roll does.          All or nothing. Returns how many.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `notes` | array | yes |  |

## paste_notes

Paste the note clipboard (copy_notes, cut_notes, or Ctrl+C in the piano roll) into          MIDI clip `clip`, as Ctrl+V in the piano roll does: the earliest copied note starts at          `position` (samples, at the tick nearest it; default: the playhead) or `at` (song          position "bar:beat:tick", bars and beats from 1), not snapped, and every other          note at its same distance from it and its same pitch, length, velocity and channel.          Refused when that point is before the clip's start. Notes reaching past the clip's          end lengthen it to the bar line after them. Copied notes may be pasted into any MIDI          clip. Returns the new notes' ids, in time order.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `at` | string |  | Or the song position "bar:beat:tick" (from 1; describe_midi_clip's form), instead of
`position`. |
| `clip` | integer | yes |  |
| `position` | integer |  | Where the earliest copied note starts: a song position in samples (the tick nearest
it). Default: the playhead. |

## add_controller_events

Add controller events to a MIDI clip. Each: `tick` (from the clip start), `channel` 0-15 (default 0) and `kind`: {"type":"cc","number":1,"value":64} (control change 0-127, e.g. 1 mod wheel, 64 sustain pedal), {"type":"pitch_bend","value":0} (-8192..8191) or {"type":"program_change","program":0}. All or nothing. Returns the new events' ids.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `events` | array | yes |  |

## read_midi_file

Look inside a Standard MIDI file (absolute `path`) without importing it: its tracks with notes (name, `import_as`: the name import_midi_file gives its track, note count, length in ticks at 960 per quarter note), and the tempo map it asks for (opening tempo and time signature, later changes in bars and beats from the file's start), so you can decide on import_midi_file's `set_tempo`.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | Absolute path of the file. |

## import_midi_file

Import a Standard MIDI file (absolute `path`, type 0 or 1) as MIDI clips, placed at `bar` (and `beat`, from 1) or `start` in ticks (960 per quarter note); bar 1 by default. Its first track with notes goes on `track` if that's an instrument track; the others (and the first, otherwise) on new instrument tracks after it, named from the file (tracks that share a name, like a piano's two staves, become "Name (treble)" and "Name (bass)" by register, else "Name 1", "Name 2"). `same_instrument` (default true) gives each new track its own copy of `track`'s instrument (its first insert), with its settings. `set_tempo` (default false) first takes the file's tempo and time signatures as the project's (opening tempo from bar 1, later changes from the clip's bar on), keeping the clips on the bar they were placed at. Returns the tracks and clips made.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer |  | Bar to place it at, from 1. |
| `beat` | integer |  | Beat of that bar, from 1. |
| `path` | string | yes | Absolute path of the MIDI file. |
| `same_instrument` | boolean |  | Give each new track a copy of `track`'s instrument. |
| `set_tempo` | boolean |  | Take the file's tempo map as the project's. |
| `start` | integer |  | Ticks from the start of the song, instead of `bar`. |
| `track` | integer |  |  |

## export_midi_clip

Save a MIDI clip as a Standard MIDI file (type 0, 960 ticks per quarter note) at absolute `path`: the notes that play (cropped at the clip's end) and its controller events, with the tempo and time signature at the clip's start. Overwrites `path`.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `path` | string | yes | Absolute path to write (.mid); overwritten. |

## export_midi_track

Save every MIDI clip of instrument track `track` as one Standard MIDI file (type 0, 960 ticks per quarter note) at absolute `path`, each clip at its place in the song (tick 0 is the project's start), with the project's tempo and time signature changes: the notes as programmed (cropped at each clip's end, as they play) and controller events, nothing from plug-ins. For another program to import. Overwrites `path`.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | Absolute path to write (.mid); overwritten. |
| `track` | integer | yes |  |

## quantize_notes

Move a MIDI clip's notes toward the grid: each start goes `strength` (0-100 %, default 100) of the way to its nearest line of `division` ("1/16", "1/8t", "beat", "bar"…), exactly in ticks, following the time signature (lines restart each bar). Lengths are kept. `notes` picks some by id; all by default.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `division` | string | yes | "1/16", "1/8t", "beat", "bar"… |
| `notes` | array |  | Note ids; every note when omitted. |
| `strength` | number |  | 0-100 %; 100 when omitted. |

## preview_note

Play a note on an instrument track's plug-in now, whether or not the track is armed: `pitch` 0-127 (60 = middle C), `velocity` 1-127 (default 100), released after `duration_ms` (default 500, up to 10000). Nothing is recorded or changed.

Kind: transport.

| argument | type | required | description |
|---|---|---|---|
| `duration_ms` | integer |  | Milliseconds before the release; 500 when omitted. |
| `pitch` | integer | yes | 0-127; 60 is middle C. |
| `track` | integer | yes |  |
| `velocity` | integer |  | 1-127; 100 when omitted. |

## set_midi_record_options

How recording on armed instrument tracks lands: `replace` (true: first delete what the track has in the recorded range; false, the default: merge, overdubbing into a clip that holds the range, or a new clip) and `quantize` (a grid division like "1/16", "1/8t", "beat" to snap recorded note starts to, or "off"). Only the fields given change. Saved with the project.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `quantize` | string |  | A grid division ("1/16", "1/8t", "beat", "bar") or "off". |
| `replace` | boolean |  |  |

## set_chase_notes

Chase notes (on by default): when playback starts, loops back or jumps into the middle of a MIDI note, strike it there instead of waiting for the next note. Saved with the project.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `on` | boolean | yes |  |

## remove_controller_events

Delete controller events from a MIDI clip by id. All or nothing.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `events` | array | yes |  |

## set_automation_mode

Set what a track's (or, with `track` omitted, the master's) controls do to its automation lanes while playing: "read" (lanes play; the controls follow them), "touch" (holding a fader, pan, mute or plug-in knob in the app writes it; letting go glides back to the lane), "latch" (like touch, but keeps writing the last value until playback stops) or "write" (every playing lane is written from the start of playback). A pass becomes one undo step when playback stops. Writing needs a person at the controls; agents add points with add_automation_points instead.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `mode` | AutomationMode | yes |  |
| `track` | integer |  | Track id; omit for the master bus. |

## add_automation_lane

Add an automation lane to a track (or the master when `track` is omitted). `target` is {"type":"volume"} (dB, -60 to +12), {"type":"pan"} (-1 left to 1 right), {"type":"mute"} (0 plays, 1 muted; steps), {"type":"send_level","send":id} (dB, one of the track's own sends) or {"type":"parameter","insert":id,"parameter":id or name} (0 to 1, a plug-in on the same track; see list_plugin_parameters). One lane per target. `points` adds points at once (see add_automation_points); without `points` the lane starts with one point at tick 0 holding the current setting (the fader, pan, mute, send level or parameter value), so adding it changes nothing. The track's lanes are shown in the arrangement. Lanes play live and in exports alike, following the audio they move (after plug-in latency), with a 32-sample grid. A mute lane overrules the track's mute button; solo still wins. Returns the lane with its id.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `points` | array |  | The lane's points; without them it starts with one holding the current setting. |
| `target` | TargetArg | yes |  |
| `track` | integer |  | Track id; omit for the master bus. |

## get_automation_lane

Read an automation lane by id: its owner (`track`, null for the master), target, units, whether it is enabled, and its points (id, tick, value, curve) in time order.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `lane` | integer | yes |  |

## set_automation_lane

Switch an automation lane on or off (`enabled`): an off lane keeps its points but moves nothing.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `enabled` | boolean |  |  |
| `lane` | integer | yes |  |

## remove_automation_lane

Delete an automation lane by id, with its points.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `lane` | integer | yes |  |

## add_automation_points

Add points to an automation lane. Each point: `tick` (from the start of the song, 960 per quarter note) or `bar` (and `beat`, from 1), `value` in the lane's units (see add_automation_lane), and optionally `curve` (-1 to 1; 0 straight, above 0 starts slowly and ends fast, below 0 the opposite) shaping the segment to the next point. Values hold before the first point and after the last. Two points on one tick make a jump there: the later-added one holds from that tick on. All or nothing: one bad value refuses the call. Returns the new point ids in order.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `lane` | integer | yes |  |
| `points` | array | yes |  |

## set_automation_point

Change an automation point by id: `tick` (or `bar` and `beat`), `value`, `curve`. Only the fields given change.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer |  | Bar, from 1, instead of `tick`. |
| `beat` | integer |  | Beat of that bar, from 1 (default 1). |
| `curve` | number |  |  |
| `point` | integer | yes |  |
| `tick` | integer |  | Ticks from the start of the song (960 per quarter note). |
| `value` | number |  |  |

## remove_automation_points

Delete points from an automation lane: those listed in `points` (ids), and/or those from `from_tick` up to (not including) `to_tick`. Returns how many went.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `from_tick` | integer |  | Delete points from this tick on… |
| `lane` | integer | yes |  |
| `points` | array |  | Point ids to delete. |
| `to_tick` | integer |  | …up to (not including) this one. |

## get_automation_value

The value an automation lane gives at `tick` (from the start of the song, 960 per quarter note; fractions allowed) or at `bar` (and `beat`, from 1), in the lane's units; null for a lane with no points.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer |  | Bar, from 1, instead of `tick`. |
| `beat` | integer |  | Beat of that bar, from 1 (default 1). |
| `lane` | integer | yes |  |
| `tick` | number |  | Ticks from the start of the song (960 per quarter note); fractions allowed. |

## detect_transients

Find the transients (note attacks, hits) in a clip's audio. Returns their timeline positions in samples, where each attack starts, with a strength from 0 to 1. `sensitivity` 0 to 1 (default 0.5; higher finds softer attacks), `min_gap_ms` the shortest distance between two (default 50). Typical use: split_clip_at these positions, then quantize_clips the pieces.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `min_gap_ms` | number |  | Shortest distance between two transients, in milliseconds (default 50). |
| `sensitivity` | number |  | 0 to 1, default 0.5. |

## split_clip_at

Cut a clip at several timeline positions (samples) in one go, e.g. at every transient. Returns all the pieces in time order (the original clip id first) and any positions that were not inside the clip.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer | yes |  |
| `positions` | array | yes | Timeline positions in samples. |

## quantize_positions

Snap timeline positions (samples) to the grid. `division` is bar, beat, 1/4, 1/8, 1/16, 1/32, or a triplet like 1/8t (fractions of a whole note, following the tempo map: grid lines run from each bar's start, so they follow tempo and time-signature changes); `rounding` nearest (default), previous or next. Returns each position with its grid position.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `division` | string | yes | bar, beat, 1/4, 1/8, 1/16, 1/32, or a triplet like 1/8t. |
| `positions` | array | yes |  |
| `rounding` | RoundingArg |  |  |

## quantize_clips

Move clips so they start on the grid, like audio quantize: after split_clip_at at the transients, each piece starts at its transient, so this lines the hits up with the beat. Set the project tempo to the performance's tempo first (set_tempo, or add_tempo_change where it changes), or the hits are pulled to the wrong beats. `division` as for quantize_positions; `strength` 0 to 1 (default 1 = fully on the grid, 0.5 = halfway). A piece that would now overlap the next is shortened to meet it; with `fill_gaps` (default true) a piece with a gap before it starts a little earlier, playing the audio just before its hit, so no hit is heard twice and no silent gaps open. Neighbours are then joined by `crossfade_ms` (default 5, 0 = hard cuts) linear crossfades over the quiet audio just before each hit, so moved slices don't click; a quantized piece therefore starts that much before its hit (its hit stays on the grid). Only clips quantized together are joined, so include the piece before the first hit. Returns where each piece's hit moved from and to.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clips` | array | yes |  |
| `crossfade_ms` | number |  | Crossfade between neighbouring pieces, in milliseconds (default 5; 0 = hard cuts). |
| `division` | string | yes |  |
| `fill_gaps` | boolean |  | Lengthen pieces to close gaps (default true). |
| `strength` | number |  | 0 to 1, default 1. |

## open_audio_editor

Open an audio file in the Waveform view's editor: `path` (absolute; WAV, FLAC, MP3 or AIFF) or `clip` (an audio clip id: its playing take's file). A file already open gives its existing document. Opened from a clip, the document remembers the clip and its track (`source`), and the effects rack can then sync with that track (set_rack_sync). Returns the document (see get_audio_editor). To show it, set_view `view_mode` "waveform" with `document`. The project remembers its open files: saving it (and its backups) writes the list, which file shows and whether the Waveform view is in front, and opening the project reopens them (a file that is gone is skipped with a notice); opening another project, or a new one, closes them. Opening or closing a file is neither an undo step nor a change to save on its own.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clip` | integer |  | An audio clip: its playing take's file. |
| `path` | string |  | An audio file, absolute path. |

## get_audio_editor

The audio files open in the Waveform view, which one is playing and where, the effects rack (its plug-ins, bypass and latency; `loaded` false for a slot whose plug-in is still loading or failed to, with `error` saying why it failed: such a slot keeps its settings and is skipped) and an Apply's progress. The rack is kept between runs, and saved and loaded as presets (list_rack_presets). `sync` (null when off) says the track the rack is synced with (set_rack_sync), its document and clip, and each rack slot's `track_insert` is the track's insert it mirrors. Each document: id, path, sample rate, channels, length in the file's own samples, its WAV format (`null` for FLAC, MP3 or AIFF: those save only with save_audio_as), `dirty` (unsaved edits), its own history (step names, the showing step, the saved one), the clips that use the file (a save changes what they play) and `source` (the clip it was opened from and that clip's track, or null). The open files are saved with the project and reopen with it.

Kind: read. No arguments.

## close_audio_editor

Close an audio file in the Waveform view (`document` id), stopping it if it plays. A file with unsaved edits is refused unless `discard` is true.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `discard` | boolean |  | Close even with unsaved edits, losing them. |
| `document` | integer | yes |  |

## play_audio_editor

Play an open audio file on its own (`document`), from `start` to `end` (the file's own samples; the whole file by default), on the master's outputs with no track processing, as the Waveform view's Play does; `play` false stops it. Playing the project stops it, and it stops the project. It is never in an export. Returns the editor's state.

Kind: transport.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |
| `end` | integer |  | To this sample (default: the end). |
| `play` | boolean |  | Play (true, the default) or stop (false). |
| `start` | integer |  | From this sample of the file (default 0). |

## get_spectrum

The spectrum of an open audio file (`document`) over `start`..`end` (the file's own samples; the whole file by default), of one `channel` (from 0) or all channels mixed: `bands` (default 32, up to 512) log-spaced bands from 20 Hz to the Nyquist frequency, each its low and high edge in Hz and its level in dB, where a full-scale sine reads 0 dB (Hann-windowed FFTs of `fft` points, default 4096, half overlapping, power averaged). Returns the bands, the FFT size and how many frames were averaged.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `bands` | integer |  | How many bands (default 32). |
| `channel` | integer |  | One channel (from 0); all mixed when left out. |
| `document` | integer | yes |  |
| `end` | integer |  | To this sample (default: the end). |
| `fft` | integer |  | FFT size, a power of two from 256 to 32768 (default 4096). |
| `start` | integer |  | From this sample of the file (default 0). |

## edit_audio

Edit an open audio file (`document`) as the Waveform view does, as a step in the file's own history (undo_audio_edit), not the project's; nothing is written until save_audio. `op`: delete, trim (keep only the range), insert_silence (`at`, `length`), silence, amplify (`db`), normalize_peak (`peak_db` dBFS), normalize_loudness (`lufs`, integrated, EBU R 128; at least 400 ms), fade_in / fade_out (`shape`: linear or equal_power), reverse, invert, paste (copy_audio's audio at `at`, or over `start`..`end`; another rate is converted, a mono copy fills every channel), and the spectral view's repairs of a box from `start` to `end` and `low_hz` to `high_hz` (Hz): spectral_attenuate (`db`, negative to turn it down), spectral_remove, and spectral_heal (fills the box from the sound just before and after it, as spot healing does; the bins just below and above it when it spans the whole file). Audio outside the box is untouched beyond one analysis frame (about 43 ms at 48 kHz). The range is `start`..`end` (the file's own samples; the whole file when left out, except delete, trim, fades and the spectral ops, which need one). `channels` (from 0) limits silence, amplify, normalize, fades, reverse, invert and the spectral ops; ops that change the length apply to every channel. Returns the document.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `at` | integer |  | insert_silence / paste: where, the file's own samples. |
| `channels` | array |  | Only these channels (from 0); every channel when left out. |
| `db` | number |  | amplify, spectral_attenuate: the gain in dB. |
| `document` | integer | yes |  |
| `end` | integer |  | The range's end. |
| `high_hz` | number |  | spectral_*: the box's highest frequency, Hz (at most half the sample rate). |
| `length` | integer |  | insert_silence: how many samples. |
| `low_hz` | number |  | spectral_*: the box's lowest frequency, Hz. |
| `lufs` | number |  | normalize_loudness: the target, LUFS. |
| `op` | Op | yes |  |
| `peak_db` | number |  | normalize_peak: the loudest sample's level, dBFS. |
| `shape` | any |  | fade_in / fade_out: the curve (default equal_power). |
| `start` | integer |  | The range's start, the file's own samples. |

## apply_audio_effects

Render `start`..`end` of an open audio file (`document`; the file's own samples, the whole file by default) through the Waveform view's effects rack (add_insert with `editor` true) and write the result into the file as one step of its own history. It is exactly what play_audio_editor plays through the rack: the same plug-ins, settled the same way, in the same blocks, with their latency compensated. It renders in the background: get_audio_editor's `applying` shows its progress, and the document's history gains the step when it is done. Stops the file's playback first. The slots' bypass is taken as it is when it starts: bypassing or removing a slot while it renders changes the preview and the next Apply, not this one. While the rack is synced with this file's track (set_rack_sync) the track would play the printed effects again, so `turn_off_track` is required: true switches off the track's inserts that were applied once the Apply lands (an undo step of the project's own; the rack's copies follow), false applies anyway.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |
| `end` | integer |  | To this sample. |
| `start` | integer |  | From this sample of the file. |
| `turn_off_track` | boolean |  | While the rack is synced with this file's track: switch the applied effects off on
the track once the Apply lands (true), or apply anyway (false). Required then. |

## capture_noise_print

Have a built-in effect learn from `start`..`end` (the file's own samples) of an open audio file (`document`) what it learns from audio: a noise print for Noise Reduction or Hiss Reduction (select the noise alone), or the clip levels for Clip Repair (its Scan: select the clipped audio; each channel's highest and lowest sample become the levels its bands sit under, and the runs found are counted), or for the Click & Pop Repair one of its scans (`scan`: "Scan For Threshold Levels", the default, sets Max Level to the highest level measured, Avg Level to the lowest and Min Level to the mean; "Scan For All Levels" also sets the six detection and rejection thresholds from a trial detection; they are its parameters, to change after). It goes to `insert`, any such insert (a track's, the master's or the Waveform rack's), or without one to the Waveform view's effects rack: its first effect that learns (whichever kind), or a Noise Reduction added to it. Measured on the audio the rack plays (the engine's sample rate; a noise print at the effect's FFT Size at the time). The effect uses it from its next block, even while playing, and saves it with its settings. Returns the insert and the effect.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |
| `end` | integer | yes | Its end. |
| `insert` | integer |  | The effect to learn it (one that learns from audio); the Waveform rack's when left out. |
| `scan` | string |  | For the Click & Pop Repair: which scan, "Scan For Threshold Levels" (the default) or
"Scan For All Levels" (or a unique part of the name, "all levels"). |
| `start` | integer | yes | The first sample to learn from (the noise alone, or the clipped audio), the file's own
samples. |

## undo_audio_edit

Take back the last edit to an open audio file (`document`), or with `redo` true bring one back: its own history, not the project's. Returns the document; `done` names the step, or is null at the history's end.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |
| `redo` | boolean |  | Bring an undone edit back instead. |

## copy_audio

Copy `start`..`end` (the file's own samples) of an open audio file (`document`) for edit_audio's paste, into this file or another. Returns how many samples were copied.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |
| `end` | integer | yes |  |
| `start` | integer | yes |  |

## save_audio

Write an open audio file's edits over the file itself, in its own WAV format (16- or 24-bit stays so). It can't be undone on disk except by undoing the edits and saving again. Every clip on the file plays the new audio. FLAC, MP3 and AIFF files are refused: use save_audio_as. Returns the document.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |

## save_audio_as

Write an open audio file (`document`) to `path` (absolute, .wav; replaced if it exists; its folder must exist) in the file's WAV format (32-bit float for a FLAC, MP3 or AIFF), and carry on editing that file. Returns the document.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |
| `path` | string | yes | Where to write it: absolute path to a .wav. |

## get_metadata

An open audio file's tags (`document`): title, artist, album, album_artist, year, track ("3" or "3/12"), genre, composer, comment, and its cover art's type and size (null without one). Read from the file when it opened; set_metadata changes them.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |

## set_metadata

Change an open audio file's tags (`document`): only the fields given change, and an empty string clears one. `cover` is an absolute path to a JPEG or PNG image for the front cover, or "" to remove it. The change is a step of the file's own history (undo_audio_edit undoes it) and is written by save_audio and export_audio: ID3v2.3 in an MP3, Vorbis comments in FLAC and Ogg, LIST INFO and ID3 chunks in a WAV, an ID3 chunk in an AIFF. Returns the tags.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `album` | string |  |  |
| `album_artist` | string |  |  |
| `artist` | string |  |  |
| `comment` | string |  |  |
| `composer` | string |  |  |
| `cover` | string |  | Absolute path of a JPEG or PNG for the front cover; "" removes it. |
| `document` | integer | yes |  |
| `genre` | string |  |  |
| `title` | string |  |  |
| `track` | string |  | "3", or "3/12". |
| `year` | string |  |  |

## export_audio

Export an open audio file (`document`) as it is now, unsaved edits included, to `path` (absolute; overwritten) in another format, leaving the document and its file as they are. Runs in the background: poll get_export_status. `format` (default a 32-bit float WAV): {"type":"wav","bits":16|24|32}, {"type":"aiff","bits":16|24}, {"type":"flac","bits":16|24,"level":0-8}, {"type":"mp3","bitrate":128|160|192|224|256|320} or {"type":"mp3","vbr":0-9} (V0 best), {"type":"ogg","quality":0-10}; `sample_rate` (Hz) converts first (an MP3 must be 32000, 44100 or 48000); `dither` (default true) adds TPDF dither of one bit when the format holds integers. A path without an extension gets the format's; one with another extension is refused.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `dither` | boolean |  | TPDF dither on integer formats (default true). |
| `document` | integer | yes |  |
| `format` | any |  | The file's format (default a 32-bit float WAV). |
| `path` | string | yes | Absolute path of the file to write. |
| `sample_rate` | integer |  | Convert to this rate (Hz) before writing. |

## find_audio_problems

Scan an open audio file (`document`) for problems, over `start`..`end` (the file's own samples; the whole file when both are left out). `kind` "click-repair" finds clicks and pops with the Click Remover's detector: `threshold` (0..100 %, default 29.3; higher finds fewer) and `complexity` (0..100 %, default 15.2; higher takes longer clicks, up to 2 x (0.99 x complexity + 1.5) + 8 samples). `kind` "clip-repair" finds clipped runs with Clip Repair's scan: each channel's highest and lowest sample in the range are its clip levels, and a run is at least `min_clip_size` samples (3..20, default 3) within `tolerance` (0..50 %, default 10) of one; nothing under -24 dBFS counts. Each problem: `id`, `channel` (from 0), `start` and `length` (samples) and `level_db` (dBFS: a click's size, the largest change its repair makes; a clipped run's highest sample). The list stays on the file, replacing the last one, until the file's audio changes; ids are never reused while it is open. Changes no audio. Returns the list in order of time.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `complexity` | number |  | Click Repair's Complexity, 0..100 % (default 15.2). |
| `document` | integer | yes |  |
| `end` | integer |  | The range's end. |
| `kind` | ProblemKind | yes | "click-repair" (clicks) or "clip-repair" (clipped runs); the older "declicker" and
"declipper" are read as these. |
| `min_clip_size` | number |  | Clip Repair's Min. Clip Size, 3..20 samples (default 3). |
| `start` | integer |  | The range's start, the file's own samples. |
| `threshold` | number |  | Click Repair's Threshold, 0..100 % (default 29.3). |
| `tolerance` | number |  | Clip Repair's Tolerance, 0..50 % (default 10). |

## get_audio_problems

The problems the last find_audio_problems on an open audio file (`document`) found that are still to repair, as it returned them; `stale` is true (and the list empty) when there was no scan or the file's audio has changed since (an edit, undo or redo).

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes |  |

## repair_audio_problems

Repair problems find_audio_problems found in an open audio file (`document`): `ids` (from its list), or every one with `all` true. Each is rebuilt as the effect itself rebuilds it (the Click Remover's least-squares AR interpolation, the Clip Repair's cubic and AR fill), with the samples the scan made for it; nothing else changes. No gain is applied, so a rebuilt peak may rise past the old clip level. One step of the file's own history ("Repair 12 clicks"; undo_audio_edit takes it back); the repaired ones leave the list and the rest keep their ids. Stops the file's playback. Returns how many were repaired and the list left.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `all` | boolean |  | Repair every problem in the list. |
| `document` | integer | yes |  |
| `ids` | array |  | The problems to repair, by id. |

## get_transport

Transport state: playing, recording, playhead position (samples; in beats of the time signature's lower number; as bar.beat from 1; and in quarter notes, which is what plug-ins are told), sample rate, the tempo (quarter notes per minute) and time signature at the playhead (see get_tempo_map for changes), the loop region (`loop`: start, end in samples, enabled), the punch range (`punch`, the same shape) and the metronome settings. A plug-in behind latency or on a bus sees a position that much earlier. While recording, `recording_takes` lists each audio take so far: `track`, `start` and `length` in samples, where its clip will be when recording stops (a loop recording: the pass being recorded); left out when not recording and during a count-in. During a count-in (recording started from a stop with set_metronome's `count_in_bars` at 1 or 2), `count_in` says so: `bars` (its length), `beats_left` (beats of the time signature's lower number before the take starts, counting the one playing: 4, 3, 2, 1 through a bar of 4/4) and `record_point` (samples, where the take starts); left out otherwise. `position` stays at 0 while a count-in plays before the timeline's start.

Kind: read. No arguments.

## play

Start playback from the playhead.

Kind: transport. No arguments.

## stop

Stop playback or recording. The playhead stays where playback stopped. Recorded takes appear as clips shortly after.

Kind: transport. No arguments.

## locate

Move the playhead to `position` (samples). Refused while recording.

Kind: transport.

| argument | type | required | description |
|---|---|---|---|
| `position` | integer | yes | Playhead position in samples. |

## set_loop

Set the loop region and/or turn looping on or off. `start` and `end` are in samples and go together (end after start); `enabled` turns looping on or off. A region given without `enabled` leaves looping on or off as it was (as drawing one in the ruler does). Turning it on with no region set loops the 4 bars from the bar the playhead is in. While looping, playback wraps from `end` to `start` sample-accurately; playback that starts past `end` plays on. Recording and exports ignore the loop. The region is saved with the project.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `enabled` | boolean |  |  |
| `end` | integer |  | End in samples, after `start`. |
| `start` | integer |  | Start in samples; give `end` too. |

## set_punch

Set the punch range and/or turn punch recording on or off, like set_loop: `start` and `end` in samples go together, `enabled` turns it on or off (a range alone leaves it as it was). Turning it on with no range set punches the loop region if there is one, or else the 4 bars from the playhead's bar. While on, `record` records only inside the range, to the sample (playback runs on before and after), so takes are exactly the range or the part of it after the playhead. Recording fails if the playhead is already past the range's end. Saved with the project.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `enabled` | boolean |  |  |
| `end` | integer |  | End in samples, after `start`. |
| `start` | integer |  | Start in samples; give `end` too. |

## set_metronome

Change the metronome: `enabled`, `volume_db` (-60 to +6), `only_while_recording`, `count_in_bars` (0 = off, 1 or 2: when recording starts from a stop, the transport starts that many bars early with the click, even with the metronome off, and the take starts at the playhead; in the app, the Record button's right-click menu and the metronome's menu set it too); fields left out stay as they are. The click accents the first beat of each bar, follows the project tempo and time signature, and is aligned with what you hear (delay compensation and loop wraps included). It plays on the master's output pair after the master, is silenced when the master is muted, and is never in exports. Saved with the project.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `count_in_bars` | integer |  | Bars of click before recording starts: 0 (off), 1 or 2. |
| `enabled` | boolean |  |  |
| `only_while_recording` | boolean |  |  |
| `volume_db` | number |  | Click level in dB, -60 to +6. |

## record

Start recording every armed audio track that has an audio input, and play. Takes are written to the project's Audio folder and become clips when stop is called. From a stop with a count-in set (set_metronome count_in_bars), playback starts that many bars early with the click and the takes start at the playhead. With the loop on, this is loop recording: recording starts at the loop start (a playhead inside the loop moves there; one before it plays a run-up), and each pass becomes a take of one clip spanning the loop (trimmed to the punch range if punch is on). The last complete pass plays; set_active_take picks another. Recording the same loop again adds takes to that clip. While recording, the loop, punch range and playhead are fixed, and earlier passes are not heard until stop.

Kind: transport. No arguments.

## export_mixdown

Render the master mix to a 32-bit float WAV at `path` (absolute), through the live engine and plug-ins (bit-faithful to playback). Range is `start`..`end` in samples (default: the whole project) plus `tail_seconds` (default 2) for reverb tails. Runs in the background; poll get_export_status, which reports the file's loudness (EBU R 128) when it's done. `normalize` (default false) brings the file to the project's loudness target (see set_loudness_target) with one constant gain and no limiter, held back so the true peak stays at or under `ceiling_dbtp` (default -1): the file then differs from playback by exactly that gain, which get_export_status reports, with whether the ceiling held it. Overwrites `path`. `format` (default a 32-bit float WAV): {"type":"wav","bits":16|24|32}, {"type":"aiff","bits":16|24}, {"type":"flac","bits":16|24,"level":0-8}, {"type":"mp3","bitrate":128|160|192|224|256|320} or {"type":"mp3","vbr":0-9} (V0 best), {"type":"ogg","quality":0-10}; `sample_rate` (Hz) converts first (an MP3 must be 32000, 44100 or 48000); `dither` (default true) adds TPDF dither of one bit when the format holds integers. A path without an extension gets the format's; one with another extension is refused.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `ceiling_dbtp` | number |  | The true peak normalising may not pass, dBTP (default -1). |
| `dither` | boolean |  | TPDF dither on integer formats (default true). |
| `end` | integer |  |  |
| `format` | any |  | The file's format (default a 32-bit float WAV). |
| `normalize` | boolean |  | Normalise to the project's loudness target. |
| `path` | string | yes | Output .wav path. |
| `sample_rate` | integer |  | Convert to this rate (Hz) before writing. |
| `start` | integer |  |  |
| `tail_seconds` | number |  | Extra seconds after the end (default 2). |

## mixdown_to_new_file

Mixdown to a new file: render the mix as it plays (every track, level, plug-in, automation and send, and the master) into a new 32-bit float WAV in the project's Rendered folder, then open it in the Waveform view for final edits or an export. The same as export_mixdown without normalising, over `start`..`end` in samples (default: the whole project; give a time selection's range for just that) plus `tail_seconds` (default 2). Runs in the background: poll get_export_status; get_audio_editor then lists the file. Returns the file's path.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `end` | integer |  |  |
| `start` | integer |  |  |
| `tail_seconds` | number |  | Extra seconds after the end (default 2). |

## get_export_status

Progress of the running export, render, mixdown or file export (`progress` 0 to 1 through rendering and writing the file; `kind`: export, render, mixdown or file; `what` it is making, as the app's progress dialog says), or how the last one ended: finished with its path, its `loudness` (as get_loudness reports it: integrated, range, true peak, maxima) and, if it was normalised, `normalized` (target, ceiling, `gain_db` applied, null when nothing was above the -70 LUFS gate, and `held_by_peak`), and for a track's render the `track` its file went on; or failed with the reason (a cancel: "export cancelled").

Kind: read. No arguments.

## cancel_export

Stop the running export; no file is written.

Kind: transport. No arguments.

## get_loudness

Read the master's loudness meter (EBU R 128 / ITU-R BS.1770-4, 'EBU Mode'). It measures the master's hardware output pair after every plug-in and fader and before the metronome, so tracks routed straight to that pair count too. `momentary` (400 ms) and `short_term` (3 s) are in LUFS and always measured. `integrated` (gated, LUFS), `range` (LRA, LU; not yet stable in the first 60 s), `true_peak` (dBTP, 4x oversampled), `max_momentary` and `max_short_term` accumulate only while the timeline plays, until reset_loudness; `seconds` is how long they have run. -120 means silence; null means not measured yet. Also reports the project's loudness `target` (LUFS) and `offset`, the integrated reading minus the target (LU).

Kind: read. No arguments.

## set_loudness_target

Set the integrated loudness the mix aims at, in LUFS: a `preset` ("streaming" for -14, "broadcast" for -23, EBU R 128), or `lufs` for any value from -40 to -5. The meter shows the master against it, and get_loudness reports how far the integrated reading is from it. Saved with the project.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `lufs` | number |  | A target in LUFS, -40 to -5, instead of a preset. |
| `preset` | any |  |  |

## reset_loudness

Start a new loudness measurement on the master: clear the integrated loudness, loudness range, true peak and maxima (see get_loudness). Not an edit of the project.

Kind: transport. No arguments.

## measure_loudness

Measure a mono or stereo audio file (WAV, FLAC, MP3 or AIFF; absolute `path`) with the same EBU R 128 meter as the master: integrated loudness (LUFS), loudness range (LU), true peak (dBTP), and the maximum momentary and short-term loudness; momentary and short-term are their values at the end of the file. A mono file is measured as one channel, so it reads about 3 dB lower than the same audio panned centre in a stereo mix. Reads the whole file before returning (well under a second for a song).

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | Absolute path of a WAV file. |

## get_markers

List the markers in time order: `number` (1, 2, 3… as the keys 1-9 jump to them), `id`, `name`, `tick` (960 per quarter note; markers keep their bar when the tempo changes), `position` (samples now) and `bar` / `beat` (from 1).

Kind: read. No arguments.

## add_marker

Add a marker at `position` (samples), or at `bar` (and `beat`, both from 1); the playhead by default. Unnamed, it's called "Marker N". Returns the markers.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer |  | Bar, from 1. |
| `beat` | integer |  | Beat of that bar, from 1 (default 1). |
| `name` | string |  |  |
| `position` | integer |  | Samples; the playhead when neither this nor `bar` is given. |

## set_marker

Rename a marker (`name`) and/or move it to `position` (samples) or `bar` / `beat` (from 1). Only the fields given change.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `bar` | integer |  | Bar, from 1. |
| `beat` | integer |  | Beat of that bar, from 1 (default 1). |
| `marker` | integer | yes |  |
| `name` | string |  |  |
| `position` | integer |  | Samples. |

## remove_marker

Remove a marker by id.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `marker` | integer | yes |  |

## go_to_marker

Move the playhead to a marker: by `marker` id, by `number` (1 is the first in time), or `direction` "next" / "previous" from the playhead (from a marker, the one after or before it). Give exactly one. Refused while recording. Returns the position (samples) and the marker.

Kind: transport.

| argument | type | required | description |
|---|---|---|---|
| `direction` | any |  |  |
| `marker` | integer |  |  |
| `number` | integer |  | 1 is the first marker in time. |

## save_template

Save the open project's setup as a template called `name`, to start new projects from (new_project's `template`): every track, folder, bus, route, send, mixer setting and plug-in with its current settings (an amp sim's chosen amp too), automation lanes without their points, the tempo map, time signature, loudness target and markers. No clips, recordings or audio files. Kept in the app's Templates folder, for every project. Refused if a template of that name exists, unless `replace` (default false), which overwrites it. Returns its path.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes | The template's name: its file is named after it. |
| `replace` | boolean |  | Overwrite a template of that name. |

## list_templates

The saved project templates by name: `name` (what new_project's `template` takes), `path`, `modified` (seconds since 1970) and `size` in bytes.

Kind: read. No arguments.

## rename_template

Rename the template `name` to `new_name`; refused if one is called that already. Returns its new path.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes |  |
| `new_name` | string | yes |  |

## delete_template

Delete the template `name` for good (projects made from it keep their setup).

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes |  |

## list_rack_presets

The saved presets of the Waveform view's effects rack, by name: `name` (what load_rack_preset takes), `path` and `modified` (seconds since 1970).

Kind: read. No arguments.

## save_rack_preset

Save the Waveform view's effects rack as it is now as the preset `name`: its slots in order, each plug-in (VST3 or built-in), its bypass and its current settings (a built-in effect's noise print too). Kept in the app's RackPresets folder, for every project. The name is the file's: refused if it is empty or holds < > : " / \ | ? * (or ends with a dot), if the rack is empty, or if a preset of that name exists, unless `replace` (default false), which overwrites it. Returns its path.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes | The preset's name: its file is named after it. |
| `replace` | boolean |  | Overwrite a preset of that name. |

## load_rack_preset

Replace the Waveform view's effects rack with the preset `name` (from list_rack_presets): the plug-ins in the rack now are removed with their settings, and the preset's slots come in with theirs. Plug-ins load one at a time (the app shows its busy bar); the call answers once they are in, with the new slots' insert ids in order (get_audio_editor shows them; a plug-in that failed to load keeps its slot with `loaded` false and why). Refused while an Apply renders.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes |  |

## set_rack_sync

Sync the Waveform view's effects rack with the track of the clip `document` was opened from (`sync` true), or stop (false). On, the rack set aside (kept, and still what the app keeps between runs) and the rack becomes copies of the track's inserts with their settings now, so the preview sounds as the clip does in the Multitrack; from then on adding, removing, moving or bypassing an effect on either side does the same on the other (an edit of the rack mirrored onto the track is one undo step; undoing a track's edit brings the rack along), a parameter set on either side (set_plugin_parameter, an editor's knob) is set on the other, and a change no parameter shows (a preset loaded inside a plug-in, a noise print) copies the whole settings across. The track's automation lanes are not copied: the rack holds the settings set by hand. Off, the rack set aside comes back. Plug-ins load one at a time (the busy bar); the call answers once they are in, with the rack's slots and the sync (as get_audio_editor's `sync`). Refused for a file not opened from a clip (open_audio_editor with `clip`), while an Apply renders, and while other plug-ins load; closing the file or removing the track turns it off. load_rack_preset is refused while it is on.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `document` | integer | yes | An open audio file (get_audio_editor), opened from a clip. |
| `sync` | boolean | yes | Sync with the clip's track (true) or stop (false). |

## delete_rack_preset

Delete the rack preset `name` for good (the rack as it is now is untouched).

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes |  |

## get_audio_settings

The audio configuration: chosen driver type (host), devices, sample rate, buffer size, processing block and audio threads, plus what the running device actually opened with (or why it failed). `processing_block` (samples, null: off) gathers device buffers into larger blocks before processing; `stage_latency` is what that adds between input and output now (samples). Recording latency: `round_trip` is the output-to-input round trip measured from the device's timestamps (samples; null until known, or without an input), `record_offset` the manual correction on top, and `record_shift` the total that recordings are moved earlier by (including delay compensation and the stage latency).

Kind: read. No arguments.

## list_audio_devices

Driver types (hosts: WASAPI, ASIO) and, for `host` (default: the current one), its output and input devices plus the sample rates and buffer sizes the chosen output device supports. ASIO drivers only appear when they can start.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `host` | string |  | Driver type to list; the current one when omitted. |

## set_audio_settings

Change the audio configuration and restart the device; only the fields given change. `host` is a driver type from list_audio_devices; `output_device`/`input_device` are device names ("" = system default; ASIO uses the output driver for both); `sample_rate` in Hz and `buffer_size` in samples (0 = device default); `threads` is the number of worker threads or "auto"; `record_offset` in samples moves recordings earlier (negative: later) on top of the measured round trip, for what the driver does not report; `processing_block` in samples (0 = off, up to 2048) gathers the device's buffers into blocks of at least that many before processing (rounded up to whole buffers), so plug-ins that cost mostly per call (some amp-sim plug-ins at 16-sample buffers) run far cheaper, for (block - buffer) samples more latency; it helps drivers that call back in bursts (FlexASIO on shared WASAPI), and exports use the same block. Audio stops briefly (not for `record_offset` or `processing_block` alone). A sample rate change stops playback and reloads every plug-in and file at the new rate, a plug-in a frame behind the app's busy bar (get_busy's `kind` is `sample_rate`); the call answers once they are all in.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `buffer_size` | integer |  | Samples; 0 for the driver default. |
| `host` | string |  |  |
| `input_device` | string |  | Device name; "" for the system default. Ignored for ASIO. |
| `output_device` | string |  | Device name; "" for the system default. |
| `processing_block` | integer |  | Gather device buffers into blocks of at least this many samples; 0 for off. |
| `record_offset` | integer |  | Samples to move recordings earlier (negative: later), within ±1 second. |
| `sample_rate` | integer |  | Hz; 0 for the device default. |
| `threads` | any |  | Worker threads, or "auto". |

## get_midi_inputs

Open MIDI input ports (keyboards, controllers) and ports that failed to open (on Windows a port can be open in one application at a time).

Kind: read. No arguments.

## reconnect_midi

Close and reopen all MIDI input ports, e.g. after plugging in a keyboard.

Kind: edit. No arguments.

## get_stats

Engine health: DSP load (time spent processing over the time the audio driver allows: a buffer period, or a whole burst for a driver that calls back in bursts; over 1 is a dropout), delay-compensation latency in samples, the block size the engine processes and exports in (`block_size`), the device's buffer (`device_block`; smaller when buffers are gathered into blocks, set_audio_settings' `processing_block`), the driver's cadence in samples (the audio it asks for between waits: the device buffer for an even driver, more for a bursty one such as FlexASIO on shared WASAPI), worker threads, and counters of plug-in skips, engine skips and input gaps.

Kind: read. No arguments.

## get_cpu_usage

CPU use of each track (and the master, `track` null) and of each insert on it, averaged over the last half second of processing (`window`, seconds): `per_buffer_us` (microseconds per audio buffer), `share` (that time over the buffer's duration, its deadline: 1 is all of it) and `buffers` measured. A track's figure is all its work (clips, inserts, fader, sends); its inserts are listed most costly first, and the tracks too. Tracks run in parallel on the audio threads, so shares can add up past 1 without a dropout; get_stats has the overall DSP load. Every track and insert keeps running while stopped, so effects' tails and monitoring work: the numbers are not zero then. A bypassed insert (`bypass`) isn't called, so it costs (close to) nothing here. `process` is the whole app's CPU time over the window in cores (1 = one core all the time): every thread, including what plug-ins do on threads of their own, which the per-track figures can't see. Each insert's `misses` counts the blocks it has missed since it loaded (its track silent for that block) because its plug-in was held elsewhere when the audio thread came for it. The same numbers as the app's CPU panel.

Kind: read. No arguments.

## screenshot

Capture the DAW window as a PNG image, to see what the user sees. Plug-in editor windows are separate and not included. Fails while the window is minimised.

Kind: read. No arguments.

## get_meters

Current meter levels as the UI shows them: per track and for the master, the peak (falling at 24 dB/s) and peak-hold of the left and right channels in dBFS (-100 means silence), and the hardware input and output levels (`inputs_db`, `outputs_db`; the outputs include the metronome, which plays after the master).

Kind: read. No arguments.

## get_view

What the UI shows: the selected track, clip or master, the visible timeline range (samples) and zoom (`samples_per_pixel`), the waveform display gain (`waveform_scale`), which panels are open (`piano_roll`: the MIDI clip open in the editor, if any), whether snap and follow-playhead are on, the song's grid (`grid`: a division or "auto", see set_grid) and the division the Multitrack shows now (`grid_division`, Auto's at this zoom), the loudness meter's scale and units (`loudness_scale`, `loudness_relative`), and the Waveform view's time display (`editor_time`) with the grid division it shows in bars and beats (`editor_grid_division`). Track heights are in `get_project`.

Kind: read. No arguments.

## set_view

Change what the UI shows: select a track, clip or the master; scroll and zoom the timeline to `start`..`end` (samples; as close as 32 points a sample, where single samples show as dots on a line, and as wide as 2 seconds a point), or `zoom` in, out, to fit the project or to a clip; set every track's row height (`track_height`, points 32 to 400); set the waveform display gain (`waveform_scale`, 1 to 16, display only); open a MIDI clip in the piano roll (`piano_roll`: its id, or 0 to close); show or hide the mixer and inspector; turn snap to the grid (its division: set_grid) and follow-playhead on or off; set the loudness meter's scale (`loudness_scale`: "ebu9", -18 to +9 LU around the target, or "ebu18", -36 to +18) and units (`loudness_relative`: LU from the target, or LUFS); set the Waveform view's time display (`editor_time`: `time`, `samples` or `bars_beats`). `select_clip` takes audio and MIDI clips. Only the fields given change. View changes are never undo steps. Returns the new view.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `clear_editor_selection` | boolean |  | Clear the Waveform view's selection (and its spectral box). |
| `clear_time_selection` | boolean |  | Clear the time selection. |
| `document` | integer |  | Show this open audio file (open_audio_editor's `document`) in the Waveform view. |
| `editor_box` | any |  | A box on the Waveform view's spectrogram: a time range and a band, for edit_audio's
spectral ops. It replaces the time selection (and the selection replaces it). |
| `editor_end` | integer |  |  |
| `editor_playhead` | integer |  | The Waveform view's playhead, the file's own samples. |
| `editor_selection` | any |  | The Waveform view's selection (the file's own samples), on every channel. |
| `editor_start` | integer |  | The Waveform view's visible span, the file's own samples (both, to zoom; as close as 32
points a sample). |
| `editor_time` | any |  | The Waveform view's time display (its ruler and clock): `time` (minutes and seconds),
`samples`, or `bars_beats` (the song's tempo map from the file's start, with the
song's grid; set_grid). |
| `end` | integer |  | Last visible timeline sample (with `start`, sets the zoom: from 32 points a sample to 2
seconds a point). |
| `follow_playhead` | boolean |  | Scroll the timeline with the playhead. |
| `loudness_relative` | boolean |  | Show loudness relative to the target, in LU (true), or absolute, in LUFS (false). |
| `loudness_scale` | any |  | The loudness meter's scale (EBU Tech 3341): `ebu9` (-18 to +9 LU around the target)
or `ebu18` (-36 to +18 LU). |
| `piano_roll` | integer |  | Open this MIDI clip in the piano roll (the editor under the arrangement); 0 closes it. |
| `select_clip` | integer |  | Select this clip (and its track). |
| `select_clips` | array |  | Select exactly these clips (audio or MIDI; an empty list clears the selection). The
first is the one the inspector shows. |
| `select_master` | boolean |  | Select the master bus. |
| `select_track` | integer |  | Select this track (and show it in the inspector). |
| `show_diagnostics` | boolean |  | Show or hide the Diagnostics pane (the Waveform view's Click Repair and Clip Repair list:
find_audio_problems, repair_audio_problems). |
| `show_inspector` | boolean |  |  |
| `show_mixer` | boolean |  |  |
| `snap` | boolean |  | Snap the playhead, clip moves and edges, selections, markers and automation points to
the song's grid (set_grid); off, they go to the sample. |
| `spectral` | any |  | The Waveform view's display: `off` (waveform), `spectral` (spectrogram) or `both`. |
| `spectral_range_db` | number |  | The spectrogram's colour range: this many dB below full scale is black (20 to 200). |
| `start` | integer |  | First visible timeline sample. |
| `time_selection` | any |  | Select a time range across tracks, as dragging in empty lane space does: it also
selects the clips inside. `split_range` and `delete_range` act on such a range. |
| `track_height` | number |  | Set every track's row height, in points (32 to 400). `set_track` sets one track. |
| `view_mode` | any |  | Show the `multitrack` (the arrangement) or the `waveform` view (one audio file). |
| `waveform_scale` | number |  | Waveform display gain: 1 shows the audio as it is, up to 16 makes quiet audio
readable. Display only; the sound does not change. |
| `zoom` | any |  | Zoom the timeline: `in` / `out` (by a step, around the playhead), `fit` (the whole
project) or `clip` (the selected clip, or `select_clip` if given). |

## get_layout

The window's layout: where the panes are (`multitrack`, `mixer`, `inspector`, `piano_roll`, `diagnostics`, and `waveform`: an open file's tab, with its `document`, or the place files open while none is), as nested containers (`horizontal` and `vertical` splits with each child's share, `tabs` with the active one) and whether each shows; and the saved layouts' names (`layouts`, "Default" first). Users drag tabs to rearrange them.

Kind: read. No arguments.

## set_layout

Arrange the window as the saved layout `name` (get_layout's `layouts`; "Default" is the built-in one): its panes where it has them, the mixer, inspector and Diagnostics shown or hidden as it has them; open files go where it keeps their place. Returns the layout.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes | The layout's name. |

## save_layout

Keep the window's layout as `name` (app-wide, in the settings), replacing a saved layout of that name: where each pane is and how big, and where files open (not the open files themselves). Returns the saved names.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes | The layout's name. |

## delete_layout

Delete the saved layout `name`. Returns the saved names.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `name` | string | yes | The layout's name. |

## list_recent_projects

The start screen's recent projects (app-wide, in the settings), most recently opened first: each one's `path` (absolute, for open_project), `name`, `folder`, `state` ("present", "missing" when the file is gone or its drive isn't connected, or "unknown" when its drive didn't answer in time), `modified` (when it was last saved, RFC 3339, local time) with `saved` as the screen says it ("today 14:32", "yesterday", "3 days ago", a date), the `template` it was started from when Oxide made it from one, and `open` for the project open now.

Kind: read. No arguments.

## get_app_info

What Help > About shows: Oxide's `version` (major.minor.patch, the one its installer compares to refuse a downgrade), the `commit` it was built from (a short git hash, `-modified` if built from changed files; null without the app window) and the program's `folder` (absolute; its settings, templates and rack presets are kept there; null without the app window).

Kind: read. No arguments.

## remove_recent_project

Take a project off the start screen's recent projects (the list only: the project's files stay as they are). `path`: its absolute path as list_recent_projects gives it. Returns the list left.

Kind: destructive.

| argument | type | required | description |
|---|---|---|---|
| `path` | string | yes | The project file's absolute path, as list_recent_projects gives it. |

## search_manual

Search Oxide's manual (the HTML user guide installed beside the program) for a question or a few words, to answer "how do I…" from it: the best passages first, each a page (`page`: its id, for open_manual; `title`) or a section of one (`anchor` and `heading`), with a `snippet` around the first word matched and the passage's whole `text`, and its `score` (words of the question matched times 1000, plus where: a page's title counts most, then a heading, then the text). Common words ("how", "the") are left out; words match by their start ("proc" finds "processing"). `limit`: how many (default 5, at most 20). Fails when the manual isn't there (a build without it), saying where it was looked for.

Kind: read.

| argument | type | required | description |
|---|---|---|---|
| `limit` | integer |  | How many passages, best first (default 5, at most 20). |
| `question` | string | yes | What to look for: a question ("how do I stop choppy audio?") or a few words
("processing block"). |

## open_manual

Show the user a page of Oxide's manual in their default web browser (needs the app window). `page`: a page's id as search_manual gives it (`choppy-audio`; may carry `#anchor`), with `anchor` for a section on it; or `topic`: a question or a few words, opened at the best passage search_manual would give; neither: the manual's first page. Returns the `page`, `anchor`, `title` and `heading` shown, the `file` (absolute) and `opened` (false in the app's check runs, which only note the request). Changes nothing in the project; not an undo step. Errors name a page or section the manual doesn't have, and list those it does.

Kind: edit.

| argument | type | required | description |
|---|---|---|---|
| `anchor` | string |  | A section's anchor on `page` (search_manual's `anchor`). |
| `page` | string |  | A page's id (search_manual's `page`, e.g. `choppy-audio`), optionally with
`#anchor`. |
| `topic` | string |  | Words or a question: the manual opens at the best passage for it. |

# Prompts

Guides a client can offer (MCP `prompts/get`); each is one message.

## write_midi_part (Write a MIDI part)

How to write a new MIDI part (a melody, a lead, a counter-line, chords) that fits a clip already in the song: read it as music, make a track with an instrument, write the notes in bars and note names, and listen.

| argument | required | description |
|---|---|---|
| `clip` |  | The id of the MIDI clip to write against (a bass line, chords); see get_project |
| `part` |  | What to write: "a lead melody" (the default), "a counter-line", "pad chords"… |
