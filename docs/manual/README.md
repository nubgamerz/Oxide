# Writing Oxide's manual

The manual is Markdown in this folder, built to static HTML by `crates/manual`:

```sh
pwsh -NoProfile -File scripts/check.ps1 -Tier manual       # retakes every shot, builds, checks in Edge/Chrome
cargo run -p manual -- --strict                            # builds out/manual from the committed shots
cargo run -p manual -- --check                             # checks every reference, writes nothing
```

Options: `--out DIR`, `--src DIR` (this folder), `--shots DIR` (the screenshots and their marks;
`shots/` here by default, where they are committed), `--strict` (a missing screenshot is an error),
`--list-shots` (`shots.toml` as command lines, for the tier), `--pack-shots DIR|PNG` (re-encodes
screenshots without loss, smaller; `manual_shots.ps1` runs it), `--require-examples` (an MCP action without an example is an error). It prints `FAIL: file:line: ...`
for each error and `warning: ...` for each warning, and exits 1 on any error; a build ends with
each image's size and the whole manual's. `scripts/check.ps1`'s quick tier runs `--check` (on the
committed shots, so a highlight on a mark the shot hasn't fails there too), and `cargo test -p manual` builds this folder too,
so a broken link fails the checks like a failing test.

The output opens from disk (`file://`) in any browser: no server, no network. Light and dark
follow the system; it prints without the navigation.

## Pages and chapters

- `manual.toml` lists the chapters in order, each with its pages in order (paths from this
  folder). Every `.md` here except this README must be listed, and the first page is
  `index.md`.
- A page is built to `<file name>.html` and is linked by that name, so file names must differ
  across folders (`troubleshooting/choppy-audio.md` is `choppy-audio`).
- A page starts with its title, one `# Heading`. Its sections are `##` and `###` (they make the
  page's contents on the right and its search results); `####` is allowed but not listed.
- A heading's anchor is its text in lower case, letters and digits kept, spaces as dashes:
  `## Raise the processing block` is `#raise-the-processing-block`. Two headings on a page
  can't share one; give either an anchor of its own with `## Heading {#my-anchor}`.
- Entries starting with `@` in `manual.toml` are generated from the action registry
  (`crates/manual/src/mcp.json`): `@mcp-reference` (every action by area), `@mcp-actions` (a
  page per area, each action a section anchored by its name) and `@mcp-prompts`. See
  "Writing the MCP manual" below.

Standard Markdown works: emphasis, lists, tables, code, block quotes, links. Typographic quotes
and dashes are made from straight ones.

## The extensions

There are six, kept few on purpose.

### Keys: `[[Ctrl+C]]`

Draws each key as a key cap: `[[Ctrl+Shift+Z]]`, `[[F1]]`, `[[Space]]`, `[[/]]`. For the plus
key itself write `[[Ctrl++]]`.

### Page links: `[[page:recording#monitoring]]`

A link to another page, or to a heading on it, checked when the manual is built:

| you write | it links to | its text |
|---|---|---|
| `[[page:choppy-audio]]` | the page | the page's title |
| `[[page:choppy-audio#raise-the-processing-block]]` | the heading | the heading's text |
| `[[page:#over-mcp]]` | a heading on this page | the heading's text |
| `[[page:choppy-audio\|crackles]]` | the page | "crackles" |

Ordinary Markdown links to pages are checked the same way: `[the CPU
panel](choppy-audio.md#find-what-is-heavy-the-cpu-panel)` becomes a link to
`choppy-audio.html#...`. Links to websites are left alone.

### Action links: `[[action:add_track]]`

An MCP action, shown as its name in code and linked to its entry on its area's page of the MCP
reference (`mcp-tracks.html#add_track`). The name must be a real action. `[[action:add_track|adds a track]]` gives it other text.

### Effect links: `[[effect:limiter]]`

A built-in effect, by its id (`native::Kind::id`, as `effects.json` lists them), linked to the
heading its parameter table sits under, its name as the text (*Limiter*);
`[[effect:graphic-eq-10|10]]` gives it other text. An id that isn't an effect, or an effect
with no table, is an error.

### Effect tables: `::: params`

```text
## Limiter {#limiter}

What it does...

::: params limiter
:::
```

The effect's parameter table: each parameter's range (or its choices), its default, and
notes (*No automation lane*, *Remakes the impulse*, *Set by the effect: shown, not set*, a
linked slider), with its delay at 48 kHz above it. It is made when the manual is built from
`crates/manual/src/effects.json`, which daw-core's `effect_docs_are_current` writes from
`native::KINDS` (`UPDATE_DOCS`, the same command as `docs_are_current`) and fails the quick
tier while it is stale, so the tables always match the app. The block holds nothing. The
effect is documented under the `##`/`###` heading the block sits under: `[[effect:]]` links
there, and F1 in the effect's editor opens it, so give it the effect's id as its anchor
(`{#limiter}`; `help.rs` maps an effect to its browser group's page and that anchor).
Once any page holds a table, every built-in effect must have exactly one: a new effect fails
the build until it is documented.

### Figures: `::: shot`

A screenshot of the app, with parts of it outlined:

```text
::: shot settings-audio
crop settings.audio 8
highlight settings.audio.processing_block "1" "Processing block: buffers gathered into a bigger block."
highlight settings.audio.apply "2"
highlight settings.audio.buffer_size
dim
Options > Settings… > Audio.
:::
```

- `::: shot <name>` shows the screenshot `<name>`, one of `shots.toml`'s (below). Shots come
  from the app itself, never edited by hand.
- `highlight <mark> "<label>" "<text>"` outlines a named part of the window. The label (usually
  a number) is drawn as a badge beside the outline, never on it: to its left when there is
  room, else above, below or to its right, clear of every other outline and badge; a strip
  too thin for any gets a margin round the picture for it. The text is listed under the
  picture beside the same label. Both are optional, and the text may hold keys and links. The part is found by
  its **mark**: the app writes beside each shot the rects of the parts it drew (`<name>.json`),
  so outlines stay on target when the layout changes. A mark the shot doesn't have, or one
  outside the figure's crop, is an error; one the crop cuts into is a warning.
- `crop <mark> [<mark>...] [padding]` cuts the picture to the rect around those marks, grown by
  the padding (image pixels); `crop <x> <y> <w> <h>` to a rect. Crop to what the text is
  about: a whole window shrinks to half its size in the page's column, too small to read,
  while a crop narrower than the column is shown pixel for pixel.
- `dim` darkens everything but the outlined parts.
- Every other line is the caption (it may hold keys and links).
- A figure sits on its own, outside lists, and its block ends with a line holding only `:::`.

The image is made when the manual is built: cut to the crop (whole pixels), scaled down if wider
than 1520 pixels (twice the column, for 200 % screens), and written as a PNG with a palette of
256 colours when that keeps it within 45 dB of the shot (the app's flat UI measures about 49-67 dB:
text keeps its edges), else in full colour. Two figures cropping a shot alike share a file. A
whole-window shot comes to about 60-90 KB, a pane or a dialog 15-40 KB, a bar under 10 KB; the
whole manual must stay under about 10 MB.

The marks file the app writes:

```json
{
  "image": [1440, 900],
  "marks": {
    "settings.audio.apply": [1037, 638, 45, 20],
    "settings.audio.processing_block": [514, 431, 549, 65]
  }
}
```

`image` is the PNG's size and each mark is `[x, y, width, height]` in its pixels. Outlines are
placed in percentages of the picture, so they follow it as the page is resized.

A missing picture is drawn as a placeholder and reported as a warning; with `--strict` (the
`manual` tier) it is an error.

## Screenshots: `shots.toml`

Each shot is a table: what it shows, and the app's arguments after `--demo <wav> --mute` (a shot
whose arguments hold `--silent` runs without `--mute`, so the master runs and is measured: the
loudness panel, an export's report, a mixdown; zeros are sent either way):

```toml
[shots.settings-audio]
about = "Options > Settings… on its Audio section."
args = ["--show", "settings-audio", "--claude-profile", "out/check/claude/settings"]
size = "1280x800"   # optional: the window's size in points (1440x900 otherwise)
```

`args` are the app's check flags (parsed in `crates/daw-app/src/main.rs`): a `--show` scenario (one of the
ui tier's, or a new one in the app when none shows what a page needs; `effect:<id>` opens any
built-in effect's editor at its defaults), `--mixer`, `--play`. While writing pages, take only the
new shots with `scripts/manual_shots.ps1 ... -Only name,name` (the others are kept), then rebuild
with `cargo run -p manual -- --strict`. The `manual` tier runs the release app once per shot
(`scripts/manual_shots.ps1`) and writes `shots/<name>.png` and `.json` here; a figure's shot must
be listed here.

**The shots are committed** (`shots/`, dev plan 28.1). Builds use them as they are:
`scripts/installer.ps1` (and so every release) builds the manual from them
(strictly) and never starts the app. Retake a shot only when a change alters what it shows, **in the same change as the
feature**, and only the shots it affects (`-Only`): a full retake also moves pictures whose
content changes run to run (the DSP readout, the CPU panel's times), which are noise in a
review. `git status` shows which pictures a retake moved; look at them before committing. Taken
pictures are packed without loss (`manual --pack-shots`), so their marks still fit pixel for
pixel. `installer.ps1 -RetakeShots` retakes them all from the release it just built. A full retake (the
tier, `-RetakeShots`, `manual_shots.ps1` without `-Only`) first deletes every shot here: if it
fails part way, `git checkout -- docs/manual/shots` brings the committed ones back. Never put a
recording in `shots/`: the demo's copy of the audio and each shot's log stay in
`out/manual-shots-work`. Every shot runs with `--neutral`, so it shows no
machine's details: no audio device is opened (a virtual one runs the engine) and the status bar
and Settings > Audio name "WASAPI · Speakers / Microphone" at 256 samples, the menu bar shows the
demo at `Documents\Oxide Projects\Demo\Demo.oxide`, MIDI shows one "MIDI Keyboard", MCP its
default port 7878, and there are 4 audio threads (`crates/daw-app/src/neutral.rs`). The start
screen's recent projects are all shown in `Documents\Oxide Projects`. The MCP settings section shows port 7878 in its port field, status, command and Connect Claude
rows (27.5); give it a `--claude-profile` of the shot's own so Connect Claude reads no real
config. What the neutral run doesn't cover yet (the plug-ins installed, Help > About's folder)
is TD194 in the maintainer's dev plan: keep those out of crops.

## F1 and the pages it opens

F1 in the app opens the page for the pane or window under the pointer (Help > Manual opens
`index`). Its table is `Place::page` in `crates/daw-app/src/help.rs`, a page id and a heading's
anchor per place; a test (`every_place_has_its_page_in_the_manual`) builds this folder and fails
while the table names a page or heading that isn't here, so **renaming a page or a heading it
names means changing the table in the same change**. Today it names:

| where | page |
|---|---|
| the start screen | `getting-started` |
| the Multitrack, its inspector | `multitrack`, `multitrack#the-inspector` |
| the piano roll | `piano-roll` |
| the mixer | `mixer` |
| the Waveform view, its Diagnostics | `waveform-view`, `waveform-view#diagnostics` |
| Settings' sections | `settings#audio`, `#midi`, `#projects`, `#ai-control-mcp` |
| a built-in effect's editor | the effect's section: `effects-dynamics#limiter`, `effects-eq#parametric-eq`, … (its browser group's page, its id as the anchor) |
| the CPU window | `choppy-audio#find-what-is-heavy-the-cpu-panel` |
| the export dialog | `exporting` |

The app
also reads `search-index.json`, which the generator writes beside `search-index.js`, for the
`search_manual` and `open_manual` actions.

## Mark names

Marks are named by area, then part, in dotted lower case. The app puts one with a line where the
part is drawn, `crate::marks::put(ui, "transport.record", response.rect)`
(`crates/daw-app/src/marks.rs`); a part the manual needs and doesn't have gets that one line and
a row here. Parts drawn once per track are the **selected** track's (the demo selects Guitar L).
Some show only in some states (noted).

| area | marks |
|---|---|
| menu bar | `menu` (the bar), `menu.file`, `menu.edit`, `menu.track`, `menu.view`, `menu.options`, `menu.help`, `menu.help.manual` (Help open), `menu.waveform` and `menu.multitrack` (the view switch) |
| transport bar | `transport` (the bar), `transport.home`, `transport.play`, `transport.stop`, `transport.record`, `transport.time` (the time display), `transport.tempo` (label and field), `transport.meter` (time signature; Multitrack only), `transport.snap`, `transport.click`, `transport.click_menu` (the metronome's ▾), `transport.loop`, `transport.punch`, `transport.follow`, `transport.dsp` (readout and bar), `transport.mixer`, `transport.inspector`, `transport.record_menu` (the Record button's right-click menu, open) |
| status bar | `status` (the bar), `status.device` (driver and devices), `status.format` (rate, buffer, processing block, channels, format), `status.pdc`, `status.threads`, `status.mcp`, `status.mcp_retry` (when MCP failed), `status.midi`, `status.skips` and `status.input_gaps` (only when not zero), `status.version` |
| docked panes | `pane.multitrack`, `pane.mixer`, `pane.inspector`, `pane.piano_roll`, `pane.diagnostics`, `pane.waveform` (each pane's area); `dock.tab.<same>` (its tab) |
| track header | `track.header`, `track.name`, `track.power`, `track.monitor`, `track.arm`, `track.solo`, `track.mute`, `track.volume` and `track.pan` (knobs; rows tall enough), `track.automation_mode`, `track.rack`, `track.rack.power`, `track.rack.slot.N` (tall rows); the master's row: `master.mute`, `master.rack…` |
| inspector | `inspector.<section>`: the heading of each section shown (`clip`, `routing`, `mixer`, `sends`, `inserts`, `clips`, `output`, `loudness`); `inspector.input`, `inspector.output` and `inspector.monitor` (the Routing rows: label and list; Input and Monitor on audio and instrument tracks); `inspector.master.mute` (the master selected) |
| Multitrack | `arrange.add_track` (+ Track), `arrange.ruler`, `arrange.loop_range` (the loop's band in the ruler, when there is one), `arrange.markers` (the marker strip), `arrange.zoom` (−, + and Fit, bottom left) |
| clips | `clip` (the first audio clip drawn), `clip.fade_in`, `clip.fade_out` (its handles; the first drawn of each), `clip.gain` (the first gain line drawn), `clip.midi` (the first MIDI clip drawn) |
| start screen | `start.new` (the New project column), `start.template`, `start.folder` (their rows), `start.create`, `start.open` (Open project…), `start.recent` (the recent projects) |
| mixer | `mixer.strip` (the selected track's strip) with `.name`, `.inserts`, `.sends`, `.pan`, `.fader`, `.meter`, `.mute`, `.solo`, `.arm`, `.automation_mode`, `.output`; `mixer.master` with `.name`, `.inserts`, `.pan`, `.fader`, `.meter`, `.mute`, `.automation_mode` |
| settings window | `settings` (the window), `settings.nav.audio`/`midi`/`projects`/`mcp` (the list), `settings.<section>` (the section shown); Audio: `settings.audio.driver`, `.output` (or the ASIO driver), `.input` (not with ASIO), `.sample_rate`, `.buffer_size`, `.processing_block`, `.threads`, `.record_offset`, `.apply`; MIDI: `settings.midi.inputs`, `.reconnect`; Projects: `settings.projects.autosave`, `.backups`, `.file_type`, `.settings_file`; AI control (MCP): `settings.mcp.server`, `.port`, `.token`, `.status`, `.command`, `.connect` (Connect Claude's rows) |
| piano roll | `piano_roll.toolbar`, `piano_roll.snap`, `piano_roll.quantize`, `piano_roll.ruler`, `piano_roll.keys`, `piano_roll.grid`, `piano_roll.scroll`, `piano_roll.velocity` |
| Waveform view | `waveform.rack` (title and slots), `waveform.rack.power`, `waveform.rack.slot.N`, `waveform.rack.presets`, `waveform.rack.sync` (a file from a clip), `waveform.rack.apply`; the toolbar: `waveform.file` (the file list and ✕), `waveform.menu.edit`, `.menu.effects`, `.menu.history`, `waveform.info` (rate, channels, length), `waveform.display`, `waveform.time`, `waveform.zoom`, `waveform.tool` and `waveform.range` (with the spectrogram shown); the lanes: `waveform.wave`, `waveform.spectrogram` (the first channel's), `waveform.selection`, `waveform.box` (a spectral box); `waveform.dialog` (Amplify, Normalize, the Save question…) |
| Diagnostics | `diagnostics.kind` (Click Repair / Clip Repair), `.settings`, `.scan`, `.list`, `.repair` |
| automation | `automation.lane` and `automation.lane.header` (the first lane drawn), `automation.point` (the selected point), `automation.toggle` (a header's lanes button) |
| plug-in browser | `browser` (the window), `browser.search`, `browser.rescan` |
| effect editor | `effect.editor` (the window), `effect.curve` (its first picture: a response, a transfer curve, a sketch), `effect.params` (the settings) |
| export | `export` (Export mixdown…, or the Waveform view's Export file…), `export.format` (format, its settings, rate, dither), `export.normalise`, `export.tail`, `export.report` |
| loudness | `loudness` (the master's panel), `loudness.integrated`, `.meters`, `.readings`, `.buttons`, `.target`; `transport.loudness` (the transport's I/TP readout) |
| windows | `render` (Render track…), `metadata` (View > Metadata…), `backups` (File > Open backup…), `templates` (File > Templates…) |
| CPU window | `cpu` (the window), `cpu.whole_app`, `cpu.list` |

A mark is clipped to what its pane shows (scrolled away: no mark), not to what covers it: a
window over a part still leaves its mark, so don't outline parts under a dialog.

## What the checker refuses

Every run fails, naming the file and line, on:

- a link to a page that doesn't exist, or to a heading that isn't on it;
- an action name that isn't in the registry;
- an effect id that isn't a built-in effect, an effect with two `::: params` tables, or (once
  any page has one) a built-in effect with none;
- a highlight or crop naming a mark the shot's JSON doesn't have, a highlight outside its
  figure's crop, or a crop outside the image;
- a figure whose shot isn't in `shots.toml`;
- two headings with the same anchor on one page, a page without a `#` title or with two;
- a page that isn't in `manual.toml`, or an entry there with no file;
- an unknown `[[kind:...]]` or `::: block`, or a `:::` block that never ends;
- an MCP examples file that can't be read, isn't named after an area, or holds another area's
  action; an example naming an action (or a setup call) that doesn't exist; and, with
  `--require-examples`, an action without an example.

A missing screenshot (or its marks) is a warning, and an error with `--strict` (the `manual`
tier).

## Search

`search-index.js` is built with the pages: every page's title, each `##`/`###` section's
heading and text, and the figures' captions. The search box ranks pages whose title matches
first, then sections whose heading matches, then text; entries matching more of the question's
words always come first, and common words ("how", "the") are dropped from it; among those, a
section whose heading holds every word of a question of two or more comes before a page whose
title holds only some ("connect Claude" opens Settings' Connect Claude). Words match by their
start, so "proc" finds "processing", but not the other way round ("clicks" doesn't find
"click"). Opening a result highlights the words on the page. Write headings with the words
people will search for, in the form they ask them ("Add a send", "Export an MP3"), and
spell both ways where the app differs (normalise / Normalize…).

## The settings page

`docs/manual/settings/settings.md` names every row Options > Settings… draws, in bold under its
section's `##` heading (`**Buffer size**`): daw-app's `the_settings_page_names_every_row` draws
each section headlessly (Audio with WASAPI and with ASIO), collects the labels its
`theme::setting_row` rows were given, and fails while one is missing from the page.

## Writing the MCP manual

The chapter **AI control (MCP)** (dev plan 27.6) has two halves: prose pages written by hand,
and reference pages generated from the action registry with examples written by hand and run
by a test, so they can't rot.

### The chapter's pages

In `manual.toml`, in this order:

| entry | page id | what |
|---|---|---|
| `mcp/ai-control.md` | `ai-control` | What Claude can do in Oxide, what it can't, where to go next. Hand-written (a stub now). |
| `mcp/mcp-connecting.md` | `mcp-connecting` | Connect Claude for Claude Code and Desktop, by hand, the token, ports, the bridge while Oxide is closed, what "Restart Desktop" means. Hand-written (a stub now). |
| `mcp/mcp-how-actions-work.md` | `mcp-how-actions-work` | Ids, units (samples, ticks, `bar:beat:tick`), safety classes, undo, calls with no project open and while one opens, long jobs and their status actions, how to read the examples (the wildcards). Hand-written (a stub now). The generated pages link here for their badges: keep its file name. |
| `mcp/mcp-workflows.md` | `mcp-workflows` | Workflow guides, a `##` each: build a song's tracks, write a MIDI part (the `write_midi_part` prompt), a mix pass, an export with loudness, recording takes, the Waveform view's rack. Hand-written (a stub now); split it into several pages if it grows, listing each in `manual.toml`. |
| `@mcp-prompts` | `mcp-prompts` | Generated: each prompt, its arguments and its message. |
| `@mcp-reference` | `mcp-reference` | Generated: every action by area, a line each, and the badges' legend. |
| `@mcp-actions` | `mcp-<area>` | Generated: a page per area (table below), each action a `##` anchored by its name, with its badges, description, an **Input** and a **Result** table (nested fields as `a.b[].c` rows), and its examples. |

Pages under "being written" are stubs: replace their text and headings freely (nothing links
to their headings yet), but keep their file names. Nothing in F1's table (`help.rs`) points into
this chapter. Link actions with `[[action:name]]` (it goes to the action's own section) and the
generated pages with `[[page:mcp-tracks]]`, `[[page:mcp-reference#tracks]]` (an area's section
of the overview: its id with `-` for `_`) or `[[page:mcp-prompts#write_midi_part]]`.

The generated pages come from `crates/manual/src/mcp.json`, which daw-core writes from the
registry (`api::describe`): an action's description, safety class (`read`, `edit`,
`transport`, `destructive`), what a call does to the undo history (`always` a step,
`when_changed`: a step only when the call changes the project, or `never`; daw-core's
`Registry::undo`, from `NOT_UNDO_STEPS`, `NO_PROJECT_CHANGE` and `ALWAYS_CHANGES` in
`api/mod.rs`), whether it runs with no project open or while one opens, and its input and output JSON Schemas. To change what a generated page says
about an action, change the action (its description, or the doc comments on its argument and
result types, which become the tables' "What" column) and regenerate:

```powershell
$env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\UPDATE_DOCS
```

(bash: `UPDATE_DOCS=1 cargo test -p daw-core docs_are_current`). The quick tier fails while
`mcp.json`, `docs/mcp-tools.md` or the bridge's `catalog.json` is stale.

### The areas and their examples files

Examples live in `docs/manual/mcp/examples/<area>.toml`, one file per area, holding only that
area's actions (the checker refuses an action in the wrong file). An area is a module of
daw-core's `api/actions/`.

| file | page | actions |
|---|---|---|
| `project.toml` | `mcp-project` | get_project, new_project, open_project, list_backups, save_backup, open_backup, get_busy, cancel_open, get_unsaved_changes, save_project, undo, redo, get_history, set_tempo, set_time_signature, set_grid, get_tempo_map, add_tempo_change, remove_tempo_change |
| `tracks.toml` | `mcp-tracks` | add_track, remove_track, set_track, set_track_input, set_track_output, set_track_monitor, move_track, set_track_parent, move_track_to, set_track_mixer, set_track_enabled, render_track, set_master |
| `sends.toml` | `mcp-sends` | add_send, add_send_to_new_bus, set_send, remove_send |
| `plugins.toml` | `mcp-plugins` | list_plugins, rescan_plugins, add_insert, remove_insert, set_insert, move_insert, copy_insert, copy_plugin, paste_plugin, set_inserts_bypassed, set_plugin_editor, list_plugin_parameters, set_plugin_parameter, set_impulse_file, get_impulse_file, list_plugin_outputs, create_multi_out_rack, remove_multi_out_rack |
| `clips.toml` | `mcp-clips` | split_range, delete_range, import_audio, move_clip, split_clip, delete_clip, set_clip, copy_clips, cut_clips, delete_clips, paste_clips, duplicate_clips, move_clips, copy_clips_to, crossfade_clips, set_active_take, delete_take, get_clip_waveform |
| `midi_clips.toml` | `mcp-midi-clips` | insert_midi_track, add_midi_clip, get_midi_clip, describe_midi_clip, set_midi_clip, remove_midi_clip, add_notes, set_notes, resize_notes, remove_notes, add_controller_events, read_midi_file, import_midi_file, export_midi_clip, export_midi_track, quantize_notes, preview_note, set_midi_record_options, set_chase_notes, remove_controller_events |
| `automation.toml` | `mcp-automation` | set_automation_mode, add_automation_lane, get_automation_lane, set_automation_lane, remove_automation_lane, add_automation_points, set_automation_point, remove_automation_points, get_automation_value |
| `edit.toml` | `mcp-edit` | detect_transients, split_clip_at, quantize_positions, quantize_clips |
| `editor.toml` | `mcp-editor` | open_audio_editor, get_audio_editor, close_audio_editor, play_audio_editor, get_spectrum, edit_audio, apply_audio_effects, capture_noise_print, undo_audio_edit, copy_audio, save_audio, save_audio_as, get_metadata, set_metadata, export_audio |
| `diagnostics.toml` | `mcp-diagnostics` | find_audio_problems, get_audio_problems, repair_audio_problems |
| `transport.toml` | `mcp-transport` | get_transport, play, stop, locate, set_loop, set_punch, set_metronome, record, export_mixdown, mixdown_to_new_file, get_export_status, cancel_export |
| `loudness.toml` | `mcp-loudness` | get_loudness, set_loudness_target, reset_loudness, measure_loudness |
| `markers.toml` | `mcp-markers` | get_markers, add_marker, set_marker, remove_marker, go_to_marker |
| `templates.toml` | `mcp-templates` | save_template, list_templates, rename_template, delete_template |
| `rack.toml` | `mcp-rack` | list_rack_presets, save_rack_preset, load_rack_preset, set_rack_sync, delete_rack_preset |
| `devices.toml` | `mcp-devices` | get_audio_settings, list_audio_devices, set_audio_settings, get_midi_inputs, reconnect_midi, get_stats, get_cpu_usage |
| `ui.toml` | `mcp-ui` | screenshot, get_meters, get_view, set_view, get_layout, set_layout, save_layout, delete_layout, list_recent_projects, get_app_info, remove_recent_project |
| `manual.toml` | `mcp-manual` | search_manual, open_manual |

`project.toml`, `tracks.toml`, `plugins.toml`, `clips.toml`, `midi_clips.toml`,
`manual.toml` and `ui.toml` already hold examples to copy from. An action may have several
examples; they show in file order.

### An example

```toml
[[example]]
action = "split_clip"
title = "Split Guitar L's clip on beat 3"
notes = "Optional Markdown under the title: why, units, what to notice. `code` that names an action links to it."
setup = [
    { action = "set_tempo", input = '{"bpm": 90}' },
]
input = '''
{"clip": 7, "position": 48000}
'''
result = '''
{"first": {...}, "second": {...}}
'''
```

| field | |
|---|---|
| `action` | Required. The action called. |
| `title` | Required. A short phrase saying what the example does ("Rename the vocal"). |
| `notes` | Optional Markdown shown under the title. Use `"""` for several lines. |
| `setup` | Optional calls made first, in order, each `{ action = "...", input = '...' }`; every one must succeed. Use them to make what the example needs (a selection, an undo step, a second clip). They are shown above the call. |
| `input` | The call's arguments: a JSON object in a TOML literal string (`'...'` on one line, `'''` over several; nothing is escaped in them, so the JSON is written as it is). Leave it out for an action with no arguments. |
| `result` | What the call answers, as JSON, with the wildcards below. Leave it out and let the runner write it (below). |
| `error` | For an example of a refusal, instead of `result`: the message. Write `error = ''` and the runner fills it in. |
| `shown_only` | The reason an example can't run headless ("it needs the app window"): it is shown, never run, so its `result` (or `error`) is written by hand. |

JSON inside the TOML: backslashes are JSON's own, so a Windows path in an input is
`"C:\\Music\\take.wav"`; prefer `<project>` paths with forward slashes, `"<project>/Exported/mix.wav"`.
The answers show the app's own backslashes (`"<project>\\Imported\\vocal.wav"`).

### The example project

Every example runs on a fresh copy of the same small project (made by daw-core's
`api/actions/examples.rs`, `build_fixture`), opened in a headless session, so no example sees
another's changes and the ids below hold in every one. 48 kHz, 120 BPM, 4/4: a beat is 24,000
samples, a bar 96,000 samples, and 960 ticks a quarter note (a bar is 3,840 ticks).

| id | what |
|---|---|
| 1 | track **Guitars**, a folder holding Guitar L and Guitar R |
| 2 | track **Guitar L** (audio, in Guitars, pan -0.5): audio clip **7** (`guitar-l.wav`, at 0, one bar) and insert **17**, the built-in Compressor (47 samples of latency) |
| 3 | track **Guitar R** (audio, in Guitars, pan 0.5): audio clip **8** (`guitar-r.wav`, at 0, one bar) |
| 4 | track **Keys** (instrument, armed, no plug-in, so silent): MIDI clip **10** "Chords", bars 1-2 (ticks 0-7680), with notes **11-13** (C4, E4, G4 in bar 1) and **14-16** (A3, C4, E4 in bar 2), whole notes at velocity 90 |
| 5 | track **Vocal** (audio, -3 dB): audio clip **9** (`vocal.wav`, at bar 2 = sample 96,000, one bar) and send **19** to Reverb at -6 dB, post-fader |
| 6 | track **Reverb** (a bus): insert **18**, the built-in Room Reverb |
| 20 | marker **Chorus** at bar 2 |

The master has no inserts. The files are in the project's `Imported` folder; the project file
is `<project>\Example.oxide`. The first id a new object gets is **21**, then 22 and so on, in
the order the calls make them (setup calls first). `get_project`'s example in `project.toml`
shows the whole project. `the_example_project_is_as_documented` checks these ids.

What the headless session can and can't do:

- No audio device: transport actions work (`play` answers `"playing": true`), exports start and
  answer `"state": "running"` (the runner waits for them before the session ends), `get_stats`
  is all zeros.
- Use only built-in effects in examples (`"plugin": "native:parametric-eq"`, ids as
  `list_plugins` shows them with path `native:<id>`): the session scans the machine's VST3
  plug-ins, which differ from machine to machine.
- `search_manual` and `open_manual` read a manual built from this folder at the start of the
  run (`open_manual` still needs the app window to show a page: a refusal headless).
- Templates and rack presets live beside the program; the session has no folder for them. If
  an action of theirs is refused for that, show its example (`shown_only`) with a hand-written
  answer.

Mark an example `shown_only` when it needs the app window (`screenshot`, `get_meters`,
`get_view`, `set_view`, the layouts, the recent projects; headless they answer "... need the app
window"), a real device or MIDI port, or when its answer is about this machine
(`list_plugins`, `rescan_plugins`, `list_audio_devices`, `get_midi_inputs`): write its answer
by hand, trimmed with `"..."`, with names that could be anyone's. Better still, when an action
headless gives a clear refusal, an example of that refusal (`error`) is checked like any other.

### Wildcards

In a `result` (and an `error`):

| write | matches |
|---|---|
| `"*"` | any value at all at that place: a number, a string, an object, an array. Use it for what differs run to run or release to release (a time, a version, a byte count, a score). |
| `"<project>"` inside a string | the example project's folder, wherever the copy is (`"<project>\\Imported\\vocal.wav"`). It works in inputs too. |
| `"...": "*"` as a key of an object | the object may have keys not shown. Show the ones that matter. |
| `"..."` as the last item of an array | more items may follow the ones shown (none is fine too). |
| `error = '*'` | any refusal. |

Numbers match within a millionth. Everything else must match exactly, and an object without
`"..."` must have exactly the keys shown. Keep results readable: trim long ones (a project, a
waveform) with `"..."`, but show what the example is about.

### Running the examples

```powershell
cargo test -p daw-core the_manuals_mcp_examples -- --nocapture
```

runs every example (on up to 8 threads; each makes its own session) and prints `MCP examples:
N ran, M shown only (not run); K of 173 actions have no example yet`. A failure names the file,
line, action and title, and the first difference (`` `tracks[0].name` is "A", the example says
"B" ``).

It also checks each example against its action's undo badge, from the undo history before
and after the call: a `never` action adds no step; a `when_changed` or `always` one adds
exactly one, named after the action, when the project changed (or merges into the step before,
for a `set_` action repeated on the same object), and none when it didn't (an example that sets
a value to what it was is fine); an `always` one must change the project; a refusal adds no
step. And every `when_changed` or `always` action needs at least one example that changes the
project, or it fails as "documented as an undo step, but none of its examples changed the
project" (`CANT_CHANGE_HERE` in the runner lists the ones the example project can't show,
with why). The run prints the undo-step actions not seen changing anything.

To write answers, run it with `UPDATE_DOCS` set:

```powershell
$env:UPDATE_DOCS='1'; cargo test -p daw-core the_manuals_mcp_examples; Remove-Item Env:\UPDATE_DOCS
```

(bash: `UPDATE_DOCS=1 cargo test -p daw-core the_manuals_mcp_examples`). Every run example's
`result` (or `error`) is rewritten with what came back, keeping your wildcards and `"..."` where
they still fit, and every other byte of the file (comments, order, other fields); a missing
`result` is added after the example's last field. Short answers go on one line, longer ones
over several. It never turns a result into an error or back: an example that was refused but
has no `error` (or answered but has one) still fails, so a mistake isn't written down as the
truth. Then **read what it wrote**: the answer is now what the manual promises. Add `"*"`
where a value isn't the point and would change (versions, times, sizes), and run again without
`UPDATE_DOCS` to see it pass. The test
(`the_manuals_mcp_examples_give_the_answers_they_show`) runs in the quick tier with every other
test. `UPDATE_DOCS` with `docs_are_current` doesn't run it (its name doesn't contain that), so
regenerating `mcp.json` never rewrites your examples.

### Checking the chapter

```powershell
cargo run -p manual -- --check                      # every reference; examples files read and checked
cargo run -p manual -- --check --require-examples   # and every action has an example
cargo run -p manual                                 # builds out/manual: open out/manual/mcp-tracks.html
pwsh -NoProfile -File scripts/check.ps1 -Tier quick # all of it, with the examples run
```

Without `--require-examples`, actions without an example are one warning listing them. Once
every action has one, turn the rule on for good: add `'--require-examples'` to the quick tier's
`manual` step in `scripts/check.ps1` (`Invoke-Step 'manual' $cargo @('run', '-q', '-p', 'manual',
'--', '--check', '--require-examples')`) and to the `manual-build` step of the manual tier.

The search index holds each action's name, description, field names and example titles and
notes. `crates/manual/tests/questions.rs` checks that plain questions open the guide's pages
first ("Claude can't connect" opens Claude can't reach Oxide, "connect Claude" the settings);
a heading in this chapter that holds every word of such a question takes it over, so word
headings for what the page answers and run `cargo test -p manual` after writing.
