# How actions work

An *action* is one thing Claude can do in Oxide: add a track, split a clip, start an export.
Each has a name (`add_track`), the inputs it takes, the answer it gives back, and a few badges
that say what kind of action it is. This page explains what an input's numbers mean, what the
badges on [[page:mcp-reference]] and the pages after it stand for, and how to read their
examples.

## Ids

Everything in a project has a number: tracks, clips, inserts, sends, notes, automation
lanes and points, markers. Actions take these ids, never a position in a list ("the third
track"), and [[action:get_project]] is where to learn them. It is the first call to make in a
session. The answers of other calls hand over new ids as things are made (an
[[action:add_track]] answers with the new track's `track`), and a refusal names the id that
didn't match.

Ids come from one counter, so a number is one thing: in the example project the tracks are 1
to 6, the audio clips 7 to 9, the MIDI clip 10, its notes 11 to 16, the inserts 17 and 18, a
send 19 and a marker 20, and the next new thing gets 21. The slots of the Waveform view's effects rack have ids in a range of their own
([[action:get_audio_editor]] lists them), and files open in that view are
*documents*, with ids of their own (the first is 1; [[action:open_audio_editor]] gives it).
Where a plug-in has *parameters*, each has an id and a name; either picks it
([[action:set_plugin_parameter]]).

## Units: samples, ticks and bars

- **Positions and lengths on the timeline are in samples**, at the project's sample rate
  (`sample_rate` in [[action:get_project]]). At 48 kHz and 120 BPM in 4/4 a beat is 24,000
  samples, a bar 96,000. A file open in the Waveform view is measured in the file's own
  samples.
- **MIDI is in ticks**: 960 to a quarter note, so a bar of 4/4 is 3,840 ticks. A note's
  `tick` counts from the start of its clip. MIDI clips stay on their beats when the tempo
  changes; audio clips stay where they are in time.
- **Or in bars and beats.** Where an action takes `at`, it is a song position written
  `"bar:beat:tick"`, counting from 1: `"5:3:000"` (or `"5:3"`) is the third beat of bar 5.
  [[action:describe_midi_clip]] reads a clip back in the same form, and tempo and time
  signature changes (`bar`, `beat`) count the same way. Everything that turns bars into time
  goes through the tempo map, so a song with tempo changes still lines up.
- **Pitches** are 0 to 127 with 60 as middle C, or names such as `"C4"`, `"Bb3"`, `"F#2"`.
  Channels are 0 to 15.
- **Levels are in dB**: a track's `gain_db` runs from -60 to +12, a clip's from -60 to +24.
  Pan is -1 (left) to 1 (right). Loudness is in LUFS, true peak in dBTP.
- **Plug-in parameters are normalised**, 0 to 1, whatever they show. The answer of
  [[action:set_plugin_parameter]] gives the display text ("-6.0 dB") so the mapping can be
  checked.
- **Tempo is in quarter notes per minute.**
- **Times such as `tail_seconds` are in seconds**; `min_gap_ms` and `duration_ms` in
  milliseconds.

## Safety classes

Each action has a class, shown as its first badge:

| badge | the action |
|---|---|
| **Read** | reads; changes nothing. Safe to call as often as needed. |
| **Edit** | changes the project in a way you can see and reverse. |
| **Transport** | starts, stops or moves playback or recording. |
| **Destructive** | deletes or overwrites something: a track, a file, a saved project, a template. |

Destructive doesn't mean it can't be undone: removing a track is an undo step. It means a
call deserves a second look, and that overwriting a file on disk isn't undone by Edit > Undo.

## Undo

Every action that edits the project is one undo step, named after the action, as long as it
changed something. Edit > Undo takes it back, and so does [[action:undo]]; [[action:redo]] brings
it back, and [[action:get_history]] lists what each would do. A few details:

- Each action's badge says which of three it is:
  - *Undo step*: every call that succeeds is one step, because it always makes or removes
    something ([[action:add_track]], [[action:remove_insert]], [[action:split_clip]]).
  - *Undo step when it changes the project*: one step when the call changes something, and
    nothing when it doesn't. Setting a value to what it already is, or quantizing notes that
    are already on the grid, leaves no step behind.
  - *Not an undo step*: Edit > Undo never takes it back.
- *(repeats merge)* after the badge marks the `set_` actions: several calls on the same object
  in quick succession, such as a fader moved in ten small steps, are one undo step.
- A recording is one undo step.
- **Not undo steps:** plug-in parameter changes ([[action:set_plugin_parameter]]), a
  Convolution Reverb's impulse file ([[action:set_impulse_file]]) and a learnt noise print or
  scan ([[action:capture_noise_print]]): these live in the plug-in, so set them back the same
  way. Also view state such as a track's height, and the view itself ([[action:set_view]]);
  the clipboard ([[action:copy_clips]]); plug-in windows ([[action:set_plugin_editor]]); the
  plug-in list and MIDI ports ([[action:rescan_plugins]], [[action:reconnect_midi]]); the
  Waveform view's files, which each have a history of their own ([[action:undo_audio_edit]]);
  and everything that works on files, settings or the app rather than on the project: saving,
  backups, exports and mixdowns to files ([[action:export_mixdown]],
  [[action:mixdown_to_new_file]], [[action:export_audio]], [[action:export_midi_clip]],
  [[action:export_midi_track]]), templates, rack presets, the audio settings, the window
  layouts and the recent projects.
- Opening or starting a project, or changing the sample rate, starts the history afresh.
- Undo is refused while recording, while a project opens and while an export or render runs.

## With no project open, and while one opens

On the start screen no project is open. Every read is allowed, and so are the actions badged
*Runs without a project*: [[action:new_project]], [[action:open_project]],
[[action:save_project]], the audio settings ([[action:set_audio_settings]]) and a few more. An
action badged *Needs a project* is refused there. In the app a new project that has no
folder yet counts as no project, so give [[action:new_project]] a `name` and a `folder`.

While a project opens in the app (its audio files are read, then its plug-ins load a few at a
time behind the loading bar), every other call waits until it is in, so a call never sees a
project half loaded. Only three answer at once, badged *Answers while a project opens*:
[[action:get_busy]], which says how far the open has come, [[action:cancel_open]], which goes
back to the project before, and [[action:screenshot]]. The same holds while a project is
being saved.

An action that opens something ([[action:open_project]], [[action:open_backup]], a new project
from a template, [[action:undo]] or [[action:redo]] when plug-ins have to come back) answers
once it is done, which can take a few seconds with heavy plug-ins.

## Long jobs and status actions

Calls run in the app, one at a time. A long job doesn't hold the call: it starts the work and
answers at once, and a status action reports how it is going.

| job | started by | ask how it is going | stop it |
|---|---|---|---|
| an export of the mix | [[action:export_mixdown]] | [[action:get_export_status]] | [[action:cancel_export]] |
| a mixdown into the Waveform view | [[action:mixdown_to_new_file]] | [[action:get_export_status]] | [[action:cancel_export]] |
| a track's render | [[action:render_track]] | [[action:get_export_status]] | [[action:cancel_export]] |
| a file exported in another format | [[action:export_audio]] | [[action:get_export_status]] | [[action:cancel_export]] |
| the effects rack applied to a file | [[action:apply_audio_effects]] | [[action:get_audio_editor]] (`applying`) | |
| a project opening or saving | [[action:open_project]], [[action:save_project]] | [[action:get_busy]] | [[action:cancel_open]] (opens only) |

Poll the status action until `state` is `finished` (it then has the file's path and loudness) or
a failure (it says why). A call that takes more than five minutes is given up on.

## Errors

A refused call changes nothing and answers with a message that names the bad id or value, and
often where to find good ones: *no track with id 99 (see get_project)*. Calls that take several
things (notes, clips, points) are all or nothing: one bad item refuses the whole call and the
message says which. The examples on each action's page show the refusals worth knowing.

## Reading the examples

Each action's page ([[page:mcp-reference]] lists the areas) ends with examples: a call and
what it answers. They are not hand-typed: a test runs each against a headless copy of one small
example project, 48 kHz at 120 BPM in 4/4, and fails if Oxide's answer stops matching what the
page shows, so they can't go out of date. That is why every example uses the ids of the
project in the table:

| id | what |
|---|---|
| 1 | folder **Guitars**, holding Guitar L and Guitar R |
| 2 | track **Guitar L**: clip 7 (`guitar-l.wav`, one bar) and insert 17, a Compressor |
| 3 | track **Guitar R**: clip 8 (`guitar-r.wav`, one bar) |
| 4 | instrument track **Keys**: MIDI clip 10 "Chords" over bars 1-2, with notes 11 to 16 |
| 5 | track **Vocal**: clip 9 (`vocal.wav`, at bar 2) and send 19 to Reverb |
| 6 | bus **Reverb**, with insert 18, a Room Reverb |
| 20 | marker **Chorus** at bar 2 |

An example may start with *setup* calls, shown above it, that make what it needs. Each setup
call takes the next new id, 21 and up, so the example's own call gets the one after.

In the answers, `"*"` stands for a value that differs from run to run (a time, a file's
size), `"..."` for more of the same (or, as a key, more fields that aren't shown), and
`<project>` for the example project's folder. A *shown, not run* badge marks an example whose
answer was written by hand because it depends on the machine (its audio devices, its MIDI
ports), on the app window or on the app's own folders; it is never checked. Examples that end
in an error show a refusal.
