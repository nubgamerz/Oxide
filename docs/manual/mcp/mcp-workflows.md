# Workflows with Claude

Each workflow below is a real sequence of calls for a job, in the order Claude would make them,
with the inputs that matter. You don't type any of this: ask Claude for the job ("set up a
guitar session at 96 BPM with a reverb bus") and it chooses its own calls. The sequences show
what happens, what to check, and where Claude stops and you have to play or listen.

Ids come back from the calls that make things, or from [[action:get_project]]; the ones below
are examples. [[page:mcp-how-actions-work]] explains units and ids.

## Build a song's tracks

Start a project, then lay out the tracks you record into.

1. [[action:new_project]] `{"name": "My Song", "folder": "D:/Music"}`. The answer is the
   project file's path; the project gets its own folder with `Recorded`, `Imported`,
   `Rendered`, `Exported`, `Backups` and `Temp` in it. Always give a `name` and a `folder`: a
   project with no folder can't record or import yet.
2. [[action:set_tempo]] `{"bpm": 96}` (and [[action:set_time_signature]] if it isn't 4/4).
3. [[action:add_track]] `{"kind": "folder", "name": "Guitars"}` answers `{"track": 1}`. Add the
   tracks inside it with that id as `parent`: `{"kind": "audio", "name": "Guitar L", "parent": 1}`
   and `{"kind": "audio", "name": "Guitar R", "parent": 1}`.
4. Give them an input and a place in the stereo picture. [[action:set_track_input]]
   `{"track": 2, "input": {"type": "audio", "channel": 0, "stereo": false}}` records from the
   interface's first input; [[action:set_track_mixer]] `{"track": 2, "pan": -0.5}` and
   `{"track": 3, "pan": 0.5}` pan the pair.
5. Put the sound on them. [[action:list_plugins]] with a `filter` finds an amp sim or an effect
   by name; [[action:add_insert]] `{"track": 2, "plugin": "native:parametric-eq"}` adds a
   built-in one (a VST3 plug-in is added by its name from the list). Loading takes up to a
   few seconds, and the answer has the insert's id and its latency.
6. Add a reverb everything shares: [[action:add_track]] `{"kind": "bus", "name": "Reverb"}`, then
   [[action:add_insert]] `{"track": 6, "plugin": "native:room-reverb"}` on it, then
   [[action:add_send]] `{"track": 2, "target": 6, "gain_db": -12}` for each track that should
   feed it. (A single call, [[action:add_send_to_new_bus]], makes the bus and the send together.)
7. Add an instrument track for keys: [[action:add_track]] `{"kind": "instrument", "name": "Keys"}`
   and an instrument plug-in on it with [[action:add_insert]].
8. Mark the form with [[action:add_marker]] `{"bar": 5, "name": "Chorus"}`.
9. Read it all back with [[action:get_project]] and save with [[action:save_project]].

Each of these is an undo step, so a layout you don't like goes back with Edit > Undo or
[[action:undo]]. Saving isn't one: save when the layout is right.

## Write a MIDI part

The `write_midi_part` prompt walks Claude through writing a part that fits something already in
the song. Claude Code offers MCP prompts as slash commands and other clients in their own way;
it takes two optional arguments: the `clip` to write against (a bass line, the chords) and the
`part` to write ("a lead melody", "a counter-line", "pad chords"). You can also just ask for the part. [[page:mcp-prompts]] shows its whole text.

What Claude does:

1. **Read the song.** [[action:get_project]] gives the tracks, the MIDI clips (start and
   length in ticks) and the tempo; [[action:get_tempo_map]] gives every change. Then
   [[action:describe_midi_clip]] `{"clip": 10}` reads the clip as music: its key and the
   runner-up with their scores, the chords bar by bar (`"C | Am"`), its range and every note
   with its song position (`"2:1:000"`) and name (`"A3"`).
2. **Plan the part** in the same bars: the key's scale, chord tones on the strong beats, phrases
   of two or four bars, and a register that sits clear of the clip it fits.
3. **Make a track.** [[action:insert_midi_track]] `{"after": 4, "bar": 3, "bars": 2,
   "instrument": "<instrument>"}` adds an armed instrument track after Keys with the instrument
   loaded and an empty clip over those bars (the instrument name is one from
   [[action:list_plugins]], a synth you have installed;
   without one the track is silent). Or [[action:add_track]], [[action:add_insert]] and
   [[action:add_midi_clip]] by hand.
4. **Write the notes** with [[action:add_notes]]:
   `{"clip": 22, "notes": [{"pitch": "E5", "at": "3:1:0", "length": 960}, {"pitch": "D5",
   "at": "3:2:0", "length": 480}]}`. Positions are `bar:beat:tick` from 1, `length` is in ticks
   (960 a quarter note), pitches are names. One call is one undo step and is all or nothing: a
   bad note refuses the lot and names it.
5. **Check it.** [[action:describe_midi_clip]] on the new clip should agree with the clip it
   fits (same key, chords that make sense). Then [[action:set_loop]] over the bars,
   [[action:locate]] and [[action:play]], so *you* hear it. Claude can't hear: it relies on
   what you say.
6. **Change it.** [[action:set_notes]] edits notes by id, [[action:resize_notes]] moves a
   note's start or end, [[action:quantize_notes]] pulls loose notes toward the grid,
   [[action:remove_notes]] deletes, and [[action:undo]] takes the last call back.
   [[action:export_midi_clip]] saves the part as a MIDI file.

## A mix pass

Claude can't hear the mix, so a mix pass is you listening and Claude moving things, with
numbers to back it up: the meters, the loudness and a spectrum.

1. **See where things are.** [[action:get_project]] shows every track's gain, pan, mute and
   solo. [[action:get_loudness]] reads the master meter, and [[action:set_loudness_target]]
   `{"preset": "streaming"}` sets the loudness the mix aims at (-14 LUFS; `"broadcast"` is -23).
2. **Measure a pass.** [[action:reset_loudness]], then [[action:locate]] `{"position": 0}` and
   [[action:play]]. Integrated loudness, range and true peak accumulate only while the timeline
   plays, so let it run through the loudest part, then [[action:get_loudness]] and
   [[action:stop]]. `offset` says how far the integrated reading is from the target.
   [[action:get_meters]] shows each track's peak while it plays.
3. **Balance.** [[action:set_track_mixer]] `{"track": 5, "gain_db": -3}` for a level,
   `{"track": 3, "pan": 0.8}` for pan, `{"track": 4, "mute": true}` or `"solo": true` to hear
   one part alone. Repeated calls on one track in a row are one undo step.
4. **Shape the sound.** [[action:add_insert]] `{"track": 5, "plugin": "native:parametric-eq"}`,
   then [[action:list_plugin_parameters]] `{"insert": 21}` to see its parameters, and
   [[action:set_plugin_parameter]] `{"insert": 21, "parameter": "Band 1 Gain", "value": 0.55}`: values
   are 0 to 1, and the answer says what that is in dB or Hz. [[action:set_insert]]
   `{"insert": 21, "bypass": true}` compares with and without.
5. **Send to the reverb.** [[action:set_send]] `{"send": 19, "gain_db": -9}` changes how much of
   a track reaches a bus.
6. **Move it over time.** [[action:add_automation_lane]] `{"track": 5, "target": {"type":
   "volume"}, "points": [{"bar": 1, "value": -12}, {"bar": 3, "value": -3}]}` rides the vocal up
   into the chorus. Values are in the lane's units (dB for volume), and
   [[action:get_automation_value]] reads the value at any point.
7. **Look at the sound.** For a file in the Waveform view, [[action:get_spectrum]] gives the
   level in log-spaced bands, which shows a harsh or thin region in numbers.
8. **Measure again** as in step 2 and compare. [[action:undo]] backs out a level, pan, send,
   insert or lane that didn't help; [[action:get_history]] shows what each undo would take back.
   A plug-in parameter isn't an undo step: set it back with [[action:set_plugin_parameter]]
   (read its old value first with [[action:list_plugin_parameters]]).

## Export a mix at a target level

1. **Set the target.** [[action:set_loudness_target]] `{"preset": "streaming"}` (-14 LUFS) or
   `{"lufs": -16}`. It is saved with the project.
2. **Export.** [[action:export_mixdown]]
   `{"path": "D:/Music/My Song/Exported/my-song.flac", "format": {"type": "flac",
   "bits": 24, "level": 5}, "normalize": true, "ceiling_dbtp": -1}`. The mix is rendered
   through the same engine and plug-ins as playback, and `normalize` brings the file to the
   target with one constant gain and no limiter, held back so the true peak stays under the
   ceiling. Other formats: `wav`, `aiff`, `mp3` (a `bitrate` or a `vbr` quality) and `ogg`;
   `sample_rate` converts first, and `dither` (on by default) is added when the format holds
   integers. A path with the wrong extension for the format is refused.
3. **Wait for it.** The call answers at once; poll [[action:get_export_status]] until `state` is
   `finished`. It then gives the path, the file's loudness (integrated, range, true peak) and,
   because it was normalised, `normalized`: the target, the ceiling, the `gain_db` that was
   applied and `held_by_peak`, true when the peak ceiling, not the target, set the gain. A gain
   of null means nothing in the mix was loud enough to measure.
   [[action:cancel_export]] stops it and writes no file.
4. **Check the file.** [[action:measure_loudness]] `{"path": "..."}` measures it again with the
   same meter, from the file itself (WAV, FLAC, MP3 or AIFF).

Without `normalize` the file is bit for bit what plays, and `get_export_status` still reports
its loudness, so one export can be measured first and normalised in a second. Exports are
refused while the project's plug-ins are still loading, and a stretched clip must finish
rendering first ([[action:get_project]] shows `rendering`).

## Record takes

Recording needs the audio device, and someone to play, so Claude sets up and you play.

1. **Arm the tracks.** [[action:set_track_input]] gives an audio track its input,
   [[action:set_track_mixer]] `{"track": 2, "arm": true}` arms it, and
   [[action:set_track_monitor]] `{"track": 2, "monitor": "auto"}` lets you hear the input
   through the track's plug-ins while it is armed. For a MIDI track the input is the keyboard
   port, and [[action:set_midi_record_options]] chooses whether a take merges (overdubs) or
   replaces and whether it quantizes.
2. **Set the click.** [[action:set_metronome]] `{"enabled": true, "count_in_bars": 1}` plays a
   bar before the take starts, which is never in an export.
3. **Choose where.** [[action:locate]] puts the playhead at the start. For loop recording,
   [[action:set_loop]] `{"start": 96000, "end": 288000, "enabled": true}` makes each pass a take
   of one clip over the loop. For a fix, [[action:set_punch]] records only inside a range.
4. **Record.** [[action:record]] starts, and [[action:get_transport]] reports it: with a
   count-in, `count_in` says how many beats are left before the take; while recording,
   `recording_takes` lists the takes so far. The loop, punch range and playhead are fixed until
   you stop.
5. **Stop.** [[action:stop]]. The takes become clips shortly after, and are one undo step.
   [[action:get_project]] shows each clip's `takes`.
6. **Choose the best.** [[action:set_active_take]] `{"clip": 21, "take": 2}` plays a take. To
   comp, [[action:split_clip]] first, then pick different takes for different parts;
   [[action:crossfade_clips]] joins the pieces without a click; [[action:delete_take]] removes
   one for good (the file stays on disk).

If the take is early or late, the record offset in [[action:set_audio_settings]] corrects it
([[page:audio-device#record-offset]]). If recording can't start, check the input in
[[action:get_audio_settings]] and [[page:recording]].

## Use the Waveform view's effects rack

The Waveform view edits one audio file at a time, with its own rack of effects that plays on the
file and can be printed into it by Apply.

1. **Open the file.** [[action:open_audio_editor]] `{"clip": 7}` opens a clip's audio (or
   `{"path": "D:/Music/take.wav"}` any file) and answers with its `document` id.
   [[action:set_view]] `{"view_mode": "waveform", "document": 1}` shows it.
2. **Fill the rack.** [[action:add_insert]] `{"plugin": "native:noise-reduction", "editor":
   true}` puts an effect in the rack rather than on a track. [[action:get_audio_editor]] lists
   the slots, with `loaded` false for one whose plug-in is still loading or failed.
3. **Teach it.** For a noise print, [[action:capture_noise_print]] `{"document": 1, "start": 0,
   "end": 24000}` with a stretch of the file that holds the noise alone. The same action gives
   Clip Repair its clip levels and Click & Pop Repair its scan.
4. **Set it and listen.** [[action:set_plugin_parameter]] with the rack insert's id, then
   [[action:play_audio_editor]] `{"document": 1}` plays the file through the rack on the
   master outputs, and `{"document": 1, "play": false}` stops it. Playing the project stops it.
5. **Print it.** [[action:apply_audio_effects]] `{"document": 1}` renders the range through the
   rack exactly as it played and writes the result into the file as one step of the file's own
   history. It runs in the background: [[action:get_audio_editor]]'s `applying` shows its
   progress. Take a step back with [[action:undo_audio_edit]].
6. **Save.** [[action:save_audio]] writes the edits over the file (every clip that uses it
   plays the new audio), and [[action:save_audio_as]] writes a new WAV and carries on with it.
   Neither is an undo step of the project, and both overwrite. FLAC, MP3 and AIFF files save
   only with `save_audio_as`; [[action:export_audio]] writes another format and leaves the
   file as it is.

Other jobs on the same file: [[action:find_audio_problems]] finds clicks or clipped runs and
[[action:repair_audio_problems]] rebuilds them (`{"document": 1, "all": true}`);
[[action:edit_audio]] does the editing (`amplify`, `normalize_loudness`, `fade_out`, the spectral
repairs and more); [[action:set_metadata]] writes the tags.

To hear a clip with its own track's plug-ins, [[action:set_rack_sync]] `{"document": 1,
"sync": true}` makes the rack copies of the track's inserts and mirrors changes both ways;
turn it off before editing the rack on its own. [[action:save_rack_preset]] keeps a rack for
every project, and [[action:load_rack_preset]] brings it back.
