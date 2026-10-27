# Developing Oxide

Building, releasing and checking Oxide; the full list of what works.

Paths in code spans, such as `crates/daw-core`, are relative to the repository root. This file lives in `docs/`.

## Workspace

| crate | what |
|---|---|
| `crates/daw-core` | The engine: project model, routing graph with plug-in delay compensation, VST3 hosting, audio devices (WASAPI and ASIO), MIDI input, recording, export. Has no UI dependency. |
| `crates/daw-app` | The desktop UI (`daw.exe`), built with egui/eframe, and `oxide-mcp.exe`, the bridge Claude Desktop and Claude Code run to reach the app's MCP server. |
| `crates/mcp-link` | Connecting Claude Code and Claude Desktop: their configs (found, read, changed one entry at a time) and the bridge's stdio-to-HTTP relay. Only std and serde_json. |
| `crates/manual` | Builds the HTML manual from `docs/manual/` (Markdown) and checks its references. A build step only: nothing of it ships inside the app (the installer ships what it builds). |
| `crates/manual-search` | The manual's search index and ranking (serde only), shared by the generator and the app (`search_manual`, `open_manual`). |
| `crates/dawpoc` | The proof-of-concept CLI: offline render, null tests and threading benchmarks. Its [README](../crates/dawpoc/README.md) has the measurements behind the engine's design. |

## Building

```sh
winget install LLVM.LLVM          # once: bindgen needs libclang for the ASIO backend
cargo run --release               # builds and starts the app
```

- `.cargo/config.toml` points `LIBCLANG_PATH` at `C:\Program Files\LLVM\bin`.
- **The ASIO SDK.** cpal's ASIO backend (`asio-sys`) compiles Steinberg's ASIO SDK from
  `CPAL_ASIO_DIR`, else from a copy cached in `%TEMP%sio_sdk`, else downloads the current SDK
  there on the first build. Oxide uses the SDK under its GPLv3 option, which SDK 2.3.4 and later
  offer, and `asio-sys` would also compile the SDK's driver-side sources (Microsoft's
  `combase.cpp` and `dllentry.cpp`, with no GPL option), which a host never calls. So builds use
  a checked copy: `pwsh -NoProfile -File scripts/asio_sdk.ps1 -From <SDK folder or zip>` (or
  `-Download`) puts SDK 2.3.4+ with the host files only in `out/asio-sdk`, and refuses an older
  one; set `CPAL_ASIO_DIR` to that folder once (a user environment variable, so every build uses
  the same SDK). `scripts/check.ps1` and `scripts/installer.ps1` use `out/asio-sdk` when
  `CPAL_ASIO_DIR` isn't set and it exists. `asio-sys` keeps old objects in its library when the
  SDK changes: after switching, `cargo clean -p asio-sys` (and `--release`) once.
- To build without ASIO: `cargo run --release --no-default-features`.

## Making a release

Releases are built on the maintainer's own PC and published on GitHub's Releases page; the
repository has GitHub Actions switched off (a public repository's Actions logs can't be hidden),
so nothing is built there.

1. **Release notes.** Every release has `docs/releases/<version>.md`: what's new, what's fixed,
   anything a user must do after updating, written for musicians. A change that will ship adds
   its lines to the next version's notes as it lands; the file becomes the release's text. The
   personal-names guard (`crates/manual/tests/personal_names.rs`) reads the notes and the README
   too.
2. **Bump the version** in the root `Cargo.toml` (`[workspace.package] version`; every crate
   shares it), in the same change as the notes. An installer refuses to replace a newer
   version, so each release needs a higher one.
3. **The checks:** the quick tier always, the maintainer's further tiers after engine, plug-in,
   export, project or MCP changes ("Checks" below).
4. **Commit, and push to `master`.** The release is built from that commit, with the checked
   ASIO SDK ("Building").
5. **The installer**, built by `scripts/installer.ps1` (below) from a clean tree, is published
   with its `.sha256` as release `v<version>`, the notes as its text.

**Building an installer by hand** still works: `pwsh -NoProfile -File scripts/installer.ps1`.
It builds release, stages the files in `out/dist/stage`, writes `THIRD-PARTY.txt` (every
library linked and its licence texts, from Cargo's metadata), builds the manual into
`stage\manual` from `docs/manual/shots` and compiles `installer/oxide.iss` with Inno Setup
(`ISCC.exe` in `G:\Tools\InnoSetup` or on the `PATH`). The installer is
`out/dist/Oxide-Setup-<version>.exe`, with its SHA-256 beside it (`.sha256`, as `sha256sum`
writes it; `sha256sum -c` checks a download). Commit first, so the build names a clean commit.
`-RetakeShots` takes the manual's shots again from the release just built (into
`docs/manual/shots`; `-Audio <wav>` picks the demo's audio, else a generated guitar riff,
`dawpoc gen --guitar`).

**The manual's shots** are committed in `docs/manual/shots` (PNG and marks; about 13 MB for 73
shots, packed without loss). Retake them only when a change alters what a figure shows, in the
same change, and only those shots:
`pwsh scripts/manual_shots.ps1 -Exe target/release/daw.exe -ManualExe target/release/manual.exe -Only name,name`
(after `cargo build --release -p daw-app -p manual`), then review what `git status` shows. The
`manual` tier retakes them all (for a UI change that moves many).

The installer isn't code-signed: on the first run on a machine, Windows SmartScreen says
"Windows protected your PC" (More info > Run anyway).

**Licence.** Oxide is GPLv3 (`LICENSE`, `GPL-3.0-only` in `[workspace.package]`), with one added
term under section 7(e) in `NOTICE`: the name "Oxide" and its logo aren't licensed. The
installer ships both as `LICENSE.txt` and `NOTICE.txt`. What it compiles in fits the GPLv3:
the Steinberg ASIO SDK under its GPLv3 option (dual-licensed since 2.3.4; asio-sys fetches the
current SDK from `www.steinberg.net/asiosdk`; using the ASIO name or logo follows Steinberg's
trademark rules, and `THIRD-PARTY.txt` credits it), and LAME under the LGPL, satisfied by the
complete source being public. `scripts/third_party.ps1` writes `THIRD-PARTY.txt` with every
library and its licence texts.

The installer check (a maintainer tier) installs, updates, downgrades and uninstalls check
builds under an AppId of their own in `out/check/install`, never touching a real installation.

## The repository on GitHub

`nubgamerz/Oxide` is public. Anyone may open issues, start discussions and open pull requests;
only the owner pushes to `master` and merges, and every outside pull request is reviewed by the
owner before it is merged (`CONTRIBUTING.md`).

- **Settings:** GitHub Actions off (there are no workflows in the tree), Issues and Discussions
  on, the wiki and projects off.
- **A ruleset** on the default branch: a pull request (one approving review, a code owner's,
  approvals dismissed by a new push, every conversation resolved), no force pushes, no
  deletion; the repository-admin role bypasses it. There are no required status checks: with
  Actions off nothing would report them.
- **Code owners:** `.github/CODEOWNERS` (`* @nubgamerz`).

**Questions** go to Discussions: the issue form's "How do I...?" link points there (with a
second link to the manual's sources), and `.github/DISCUSSION_TEMPLATE/q-a.yml` is the Q&A
category's form. Bugs and feature requests are issues (`.github/ISSUE_TEMPLATE/`).

**Pull requests aren't checked automatically.** Contributors run the quick tier locally
(`CONTRIBUTING.md`); the maintainer reads a pull request first and then runs the quick tier on
it when reviewing. `scripts/check.ps1` still streams each step's output in a group when
`GITHUB_ACTIONS` is set, which nothing uses now.

## Installing

- Run `Oxide-Setup-<version>.exe`. It installs for your account only (no administrator), by
  default into `G:\Oxide` (`%LOCALAPPDATA%\Programs\Oxide` without a G: drive); the folder
  must be yours to write, since Oxide keeps its settings there. Options: a desktop shortcut, and
  opening `.oxide` projects with Oxide (the same as Settings > Projects > File type). There is
  always a Start menu entry.
- **The manual** is installed in `manual\` beside `Oxide.exe` (Help > Manual, F1). An update
  replaces it whole, so pages a newer version dropped don't linger; uninstalling removes it.
- **Updating:** run a newer installer. It finds the installed folder without asking, replaces
  the program files only, and never touches what Oxide keeps beside itself: `settings.json`,
  `rack.json`, `Templates\`, `RackPresets\`, `mcp-bridge\`. If Oxide is running it asks you to
  close it first (save your work). An older installer refuses to replace a newer Oxide; run it
  with `/ALLOWDOWNGRADE` to go back. Help > About (or the version at the status bar's right)
  shows the version and commit.
- **Uninstalling:** Settings > Apps > Installed apps > Oxide, or `unins000.exe` in its folder.
  It asks whether to keep your settings, effects rack, templates and rack presets (default:
  keep). Silently (`/VERYSILENT`) it keeps them unless given `/DELETEUSERFILES`. Projects are
  never in the program's folder unless you put them there, and are never removed.

## The manual

The user's manual is Markdown in `docs/manual/` (`manual.toml` lists its chapters and pages;
`docs/manual/README.md` says how to write them), built to static HTML that opens from disk in any
browser, searchable, light or dark with the system:

```sh
pwsh -NoProfile -File scripts/check.ps1 -Tier manual   # retakes the shots, builds out/manual, checks it in Edge and Chrome
cargo run -p manual -- --check       # checks every link, heading, screenshot, highlight and action
cargo run -p manual -- --strict      # builds out/manual from the committed shots (docs/manual/shots)
```

Pages link to each other and to MCP actions with checked references (`[[page:choppy-audio]]`,
`[[action:add_track]]`), draw keys with `[[Ctrl+C]]`, and show screenshots with numbered
highlights (`::: shot`); the MCP reference page is generated from the actions' catalog (written
from the registry). The quick tier runs the check, so a broken link fails like a failing test.

**Screenshots come from the app.** `docs/manual/shots.toml` lists each shot with the app
arguments that make it (a `--show` scenario, `--mixer`, ...). The `manual` tier runs the release
app once per shot (`scripts/manual_shots.ps1`: `--neutral`, MCP on port 7896) with `--screenshot` and `--marks`: the app writes the
PNG and, from the same frame, the rects of the named parts it drew (`transport.dsp`,
`settings.audio.processing_block`, ...). A figure crops to marks (`crop status.device
status.pdc 6`) and outlines marks with numbered callouts drawn in the HTML; the generator cuts
and compresses each image (a palette PNG when it stays within 45 dB, about a third of the app's
PNG) and fails on a highlight naming a mark the app didn't draw.

- **Add a shot:** a `[shots.<name>]` table in `shots.toml` (`about`, `args`, optional `size`),
  then the `manual` tier.
- **Add a mark:** one line where the part is drawn, `crate::marks::put(ui, "area.part",
  response.rect)` (`daw-app/src/marks.rs`; a no-op unless a manual run asked), and its name in
  `docs/manual/README.md`'s list.

**In the app** (27.3): Help > Manual opens `manual\index.html` beside the program in the default
browser, and **F1** opens the page for the pane or window under the pointer (the Multitrack, the
piano roll, the mixer, the Waveform view, a settings section, an effect's editor, the CPU
window, ...; `daw-app/src/help.rs`'s table, checked against the pages by a test). A build
without `manual\` beside it opens the repository's `out\manual` when it was built, else says
where the manual would be and how to build it. Agents use the same manual over MCP:
`search_manual` returns the passages that answer a question (page, section, snippet and text,
ranked as the pages' own search box ranks them, from `search-index.json`), and `open_manual`
shows the user a page, a section or the best page for a topic. Check runs never start a
browser: they note the request.

**Neutral screenshots.** Every shot runs with `--neutral` (`daw-app/src/neutral.rs`): no audio
device is opened (a virtual one runs the engine), and what is drawn names no machine's devices,
MIDI ports or folders ("WASAPI · Speakers / Microphone", `Documents\Oxide Projects\Demo`, MCP
at port 7878).

**The README's images** (28.3) are cut from those same committed shots, never taken for the
README: `docs/images/images.toml` lists each image (its file name, the shot, a crop by marks
with a pad or by a rect, an optional `max_width`), and
`pwsh -NoProfile -File scripts/readme_images.ps1` (`manual --readme-images`) cuts and compresses
them with the manual's encoder into `docs/images/*.png` (committed; the same shots give the same
bytes, an unchanged file is left alone, over about 1.5 MB in all fails). After a UI change that
alters what one of them shows, retake that shot (`scripts/manual_shots.ps1 -Only <shot>`), then
run the script and commit both. A test checks the list against the shots' marks; the
personal-names guard reads the list's descriptions, not the pixels.

Still to come (dev plan phase 27): the full user guide (pages marked "being written" are stubs
F1 already opens) and the MCP manual.

## What works

- **The window:** the Multitrack view, the mixer, the inspector, the piano roll and each open
  audio file are tabs. Drag a tab onto any side of another pane to split it there, onto
  another tab bar to join it, and drag the gaps to resize; the tab under the pointer lights
  up, so it's clear which one a click picks. Closing a tab hides the pane (the
  View menu lists every pane, Multitrack, Waveform, Mixer (M), Inspector and Piano roll,
  and brings a closed one back where you left it); closing a file's tab closes
  the file (asking first if it has edits). View > Layouts saves the arrangement by name and
  brings it back; Default is the original one, and the last arrangement returns on the next
  start. A layout keeps where files open, not which files. Agents use `get_layout`,
  `set_layout`, `save_layout` and `delete_layout`.
- **Tracks:** audio, instrument and folder tracks. A folder is a submix bus for its
  sub-tracks, and folders can be nested. Tracks can be moved, nested, renamed, coloured and
  deleted.
  - **Renaming:** double-click a track's name in its header (or right-click > Rename), or click
    the name at the top of the inspector, and type. Enter or clicking away keeps the new name,
    Escape the old one; an empty name is refused (the field stays, outlined red, until a name
    is typed or Escape). Folders and buses alike; one undo step, "Rename track".
- **Routing:** each track's output goes to its parent folder, the master, any folder bus (loops
  are prevented) or a hardware output pair directly. The master's output pair is selectable.
- **Inputs:** a mono or stereo hardware input per audio track. Instrument tracks take a MIDI
  port (or all ports) and a channel filter.
- **Monitoring:** Auto (when armed), On or Off. The input runs through the track's insert
  chain, so an amp sim is heard live.
- **VST3 inserts per track and on the master:**
  - Plug-in browser grouped by vendor folder, with search.
  - Bypass, reorder and remove.
  - Native editor windows.
  - Instruments are put first in the chain so MIDI reaches them.
- **Loop:** drag on the ruler to draw a loop region (it stays on or off as it was), and toggle
  looping with the Loop button or `L`.
  - The wrap is sample-accurate, even inside a device buffer, and seamless through plug-ins
    and delay compensation.
  - Playback that starts past the loop end plays on.
  - Recording and exports ignore the loop.
  - The region is saved with the project.
- **MIDI clips:** instrument tracks hold MIDI clips with notes and controller events.
  - They're stored in musical time (960 ticks per quarter note), so they stay on their
    beats when the tempo changes.
  - They play into the track's instrument on the exact sample, in playback and exports
    alike.
  - Loops, stops and jumps release every note and the sustain pedal.
  - Notes already sounding at the start point are re-struck ("chase"; `set_chase_notes`
    turns it off).
  - They show in the arrangement and save with the project.
  - **Recording:** armed instrument tracks record their MIDI input (port and channel
    filters respected), alone or alongside audio.
    - Notes land where you heard them: delay compensation and the device's output latency
      are taken off.
    - Count-in, punch and loop recording apply. Loop passes overdub into one clip.
    - Merge (the default) or replace, and optional quantize on input
      (`set_midi_record_options`).
    - Notes still held at stop end there.
  - **Live MIDI timing:** a keyboard's notes reach the instrument on the sample they came
    in, one device buffer later: steady, not jittery (keep the buffer small for low
    latency). Recordings place them where you heard them.
  - **Piano roll:** double-click a MIDI clip to open it under the arrangement.
    - Double-click draws a note; drag moves it, either edge resizes it (all the selected
      notes by as much); right-click or Delete erases it; drag on empty space to box-select.
    - A click in the note grid, on empty space or on a note, moves the playhead there as a
      click on its ruler does (the editor's Snap on: the grid's nearest line; Off or Alt held:
      the tick under the pointer), as a click on a clip does in the Multitrack; it still
      selects the note, or clears the selection on empty space. A double-click's first click
      places it too, and the double-click still draws. Dragging notes, a box selection, the
      keyboard and the velocity lane leave it where it is. Over a note the playhead's line
      lets the note have the press, so a note clicked and then dragged from the same spot
      moves the note; drag the line from the ruler, empty grid or the velocity lane.
    - A note you drag sounds on the track's instrument while you hold it: once as you take
      it, again only when it moves to another pitch (moving it along in time keeps it
      ringing), and it stops when you let go. A drawn or clicked note plays briefly.
    - The velocity lane sets velocity; the keyboard plays notes on the track's
      instrument. The key and row under the pointer are lit, and notes show their names
      when they are wide enough.
    - Snap follows the song's grid (the same setting as the Multitrack ruler's Grid: Auto,
      1/4 to 1/64, triplets; picking one here sets it there) and is exact in any meter; Off
      turns it off in the piano roll only, and Alt places, moves or resizes without it.
      Quantize (or Q) snaps note starts. The ruler shows bars, beats and the grid's
      division.
    - The wheel works as in the arrangement, and a scroll bar under the notes (above the
      velocity lane) shows and drags the part of the clip and the song in view.
  - MIDI clips can be selected, dragged and deleted in the arrangement.
- **Drag and drop from Explorer:** audio files (WAV) and MIDI files (`.mid`) land on the
  track under the pointer and the ones after it, at the time under the pointer.
  - New tracks are made where the kind doesn't fit.
  - A MIDI file with a different tempo or time signature asks whether to take it. The
    clip stays on the bar you dropped it on either way.
  - A MIDI file with several tracks puts each on its own track. Tracks that share a name
    are told apart: a piano's two staves from Guitar Pro (both "Acoustic Piano") become
    "Acoustic Piano (treble)" and "(bass)", others "Name 1", "Name 2". Dropped on a track
    with an instrument, every new track gets its own copy of it, settings included. File >
    Import MIDI… does the same at the playhead, on the selected track.
  - A track header or mixer strip too narrow for a name keeps its ending: "Aco…(bass)". Hover for the whole
    name.
  - Export a MIDI clip from its menu ("Export as MIDI file…"), or a whole instrument track
    from the track menu ("Export MIDI…"): every clip at its place in the song, with the
    project's tempo and time signature changes, the notes as programmed (no plug-ins).
  - Agents use `read_midi_file`, `import_midi_file`, `export_midi_clip` and
    `export_midi_track`.
  - Agents create and edit them (`add_midi_clip`, `add_notes`, `set_notes`,
    `resize_notes`, `quantize_notes`, `preview_note`, …).
- **Multi-out instruments:** right-click a plug-in with extra outputs (a sampler's
  Out 2..32, a drum sampler's kick and snare, a multi-out synth's scenes) in a track's rack, the mixer or
  the inspector > "Create multi-out rack in a submix…", tick the outputs, and each gets an
  audio track in a folder under the instrument, with its own fader, pan, plug-ins and sends.
  Route the sounds to those outputs inside the plug-in; its main pair stays on its own
  track, and muting that track leaves the outputs playing. Delay compensation counts only
  the plug-ins before the instrument for its outputs, and a render of the folder includes
  the instrument. "Remove multi-out rack" undoes it and the plug-in turns the outputs off.
  Agents use `list_plugin_outputs`, `create_multi_out_rack` and `remove_multi_out_rack`.
- **Sends and return buses:** a "Bus" track is a return for shared effects such as a reverb.
  - Any track can send to buses and folders, pre- or post-fader, with its own level and
    mute. Each send is delay-compensated on its own.
  - Loops are refused, whether made by an output, a send or moving a track into a folder.
  - Muting a track silences its sends too.
  - Solo is in place:
    - Soloing a track keeps the buses it sends to.
    - Soloing a bus plays it with exactly what feeds it: the guitar's reverb send without
      the dry guitar.
  - **In the app:**
    - Add a send from the track's right-click menu ("Add send"), the inspector's SENDS
      section, the "+ send" button on a mixer strip, or Track → "Add send from selected
      track". "New bus" makes a return track and sends to it in one step.
    - On a mixer strip, each send is a level bar: drag it, double-click for 0 dB, and
      right-click for pre-fader, mute or remove. The inspector has the same controls plus
      the target.
    - Add a bus on its own from + Track → "Bus (return)".
  - **Agents** use `add_send`, `add_send_to_new_bus`, `set_send` and `remove_send`.
- **Automation:** tracks and the master hold lanes for volume, pan, mute, send levels and
  plug-in parameters, with points in musical time (straight or curved segments, jumps
  where two points share a tick). They save and undo, and go when the send or plug-in they
  move does.
  - They play live and in exports alike. A move lands on the audio it was drawn over,
    whatever the plug-in latency, and is the same at any buffer size.
  - Plug-in parameters move sample-accurately through the plug-in's own parameter queue.
  - A mute lane takes over from the mute button; solo still wins.
  - **In the app:** right-click a track → "Add automation lane" (volume, pan, mute, its sends, or
    any plug-in parameter; type to filter). A new lane starts at the current setting. Lanes open
    under the track; the chart button in its header shows or hides them.
    - Double-click a lane to add a point, drag a point to move it (snapped to the grid), Delete or
    right-click to remove it; right-click for a straight, eased-in or eased-out segment.
    - While a lane plays, the fader, pan and mute show its value, greyed and outlined.
  - **Write modes:** each track has a mode menu ("Read ▾") in its header, and each track and
    the master a mode button (R/T/L/W) in its mixer strip and a menu in the inspector.
    - **Read** plays the lanes.
    - **Touch:** while playing, holding a fader, pan or mute, or turning a knob in an open
      plug-in editor, writes it; letting go glides back to the lane. The editor's knob gestures
      are read without taking the plug-in from the audio thread, so writing never drops a
      block.
    - **Latch:** like Touch, but keeps writing the last value until you stop.
    - **Write:** writes every playing lane of the track from the start of playback.
    - What you hear while writing is the control. The pass becomes one undo step ("Write
      automation") when playback stops, and a control with no lane gets one.
    - Agents set the mode with `set_automation_mode`; writing needs someone at the
      controls.
  - The master's lanes open under its row at the top of the arrangement, as a track's do.
  - Agents use `add_automation_lane`, `add_automation_points`, `get_automation_value` and
    the other `*_automation_*` actions.
- **Loudness meter (EBU R 128):** the master is metered live: momentary, short-term,
  integrated (gated), loudness range and true peak, with their maxima. It reads the master's
  output pair before the metronome, so the click never counts. Integrated loudness, LRA and
  peaks build up while playing, until reset.
  - Checked against the EBU's Tech 3341/3342 test cases (synthesised from their definitions)
    and against ffmpeg's `ebur128` on real audio.
  - **The panel** is in the master's inspector (click the master, or the loudness readout in the
    transport bar). It has M and S bars on the EBU +9 or +18 scale with your target marked;
    integrated, loudness range (marked settling for the first minute), true peak (red above
    −1 dBTP) and the maxima; Reset; LU or LUFS; and the target: Streaming −14, Broadcast −23,
    or any value. The master strip shows the integrated reading.
  - **Exports are measured:** after each export a report shows the file's integrated loudness
    against the target, loudness range, true peak and maxima. File → Export can normalise the
    file to the target: one gain for the whole file, no limiter, held back so the true peak stays
    at or under −1 dBTP (the report says if that stopped it short). Off, the export is exactly
    what playback plays.
  - Agents use `get_loudness` (with the target and how far off it you are),
    `set_loudness_target`, `reset_loudness`, and `measure_loudness` for a WAV file.
- **Time-stretch and transpose clips:** a clip can be made longer or shorter without moving
  its pitch (0.25× to 4×), or transposed without changing its length (±24 semitones), with
  Signalsmith Stretch (MIT). Measured on a guitar recording: pitch within a few cents, level within
  0.2 dB, attacks in place to about 10 ms, 64× realtime or faster, and the same result every
  time.
  - The stretched audio renders in the background; the clip plays unchanged until it's ready,
    and export waits for it (it refuses until then). The export is exactly the render, and a
    split stretched clip joins without a seam.
  - **Following the tempo:** give a clip the tempo it was recorded at and it follows the
    project's tempo and its changes. It moves with the bars and stretches to fit, even across
    a tempo change inside it, in one seamless render. At its own tempo it plays the file as
    it is. Undo puts it back exactly.
  - **In the app:** select a clip and the inspector shows Stretch, Pitch and Follow tempo
    (with the tempo it was recorded at). Alt-drag either edge of a clip to stretch it there.
    A stretched clip's name says how (×1.50, +2.0 st, follows 120 BPM) and "rendering…"
    until its render is in; Export waits for it. Agents use `set_clip` with `stretch`, `pitch`
    and `source_tempo`, and see `rendering` in `get_project`.
  - **Keep attacks** (on for new clips, in the inspector): each pick or hit plays unstretched
    and exactly on time, and the audio between them takes up the stretch. On a guitar recording at 2×,
    10 of 11 pick attacks stay as sharp as the original's (4 without). Listen to
    `out/stretch/di_x2_kept.wav` against `di_x2_+0st.wav` after the full tier.
- **Fades and crossfades:** drag the small square at a clip's top corner inward for a fade
  in or out (the curve is drawn over the clip), and drag the yellow line up or down for the
  clip's gain (double-click: 0 dB). Right-click a clip that touches or overlaps the one
  before it → "Crossfade with previous clip" (equal power). A split clip crossfaded
  linearly plays back exactly as the unsplit clip. Agents use `set_clip` (`fade_in`,
  `fade_out`, shapes) and `crossfade_clips`.
- **The grid:** right-click the Multitrack's ruler → Grid: 1/4, 1/8 (a new project's),
  1/16, 1/32 or 1/64 of a whole note (1/4 is a beat in 4/4, 1/8 a beat in 6/8), Triplets,
  or Auto, which follows the zoom: the finest division whose lines are at least 14 points
  apart, from 1/64 down to 1/4, then the beat (2/2) and every 1, 2, 4… bars, so zooming in
  adds lines and zooming out takes them away (each view by its own zoom; the menu says what
  Auto is on now). Lines run from each bar's start along the tempo map (in 7/8 at 1/4 the
  bar's last quarter is an eighth), drawn fainter than beats and bars, and thinned where
  they would be under 6 points apart. With Snap on, the ruler's click, the playhead's drag,
  clips (moves and edges), time selections, markers and automation points go to the
  nearest line (Auto's as shown); off, to the sample. A dragged clip's edge near the
  playhead goes onto the playhead instead (below), Snap on or off. The setting is saved with the song
  and shared by the piano roll and the Waveform view's bars and beats. Agents use
  `set_grid` ("auto", "1/16", "1/8t"…); `get_project` has `grid`, `get_view` also
  `grid_division`, the division shown now.
- **Time selection:** drag in an empty part of the lanes to select a time
  range across the tracks you drag over (snapped when Snap is on). It shades those lanes and
  the top of the ruler, and selects the clips inside it; Shift-drag extends it, and the
  ruler's band has edges you can drag. While it's there, Delete removes what's inside it
  (clips, audio and MIDI, are cut at its edges), S cuts the clips at its edges, and L loops
  it; the export dialog can export just the selection. Right-click it for the same, or Escape
  (or a click in empty space) to clear it. Agents use `set_view` `time_selection`,
  `split_range` and `delete_range`.
- **Splitting MIDI clips:** S (or the clip's menu) splits a MIDI clip at the playhead like an
  audio one, at the nearest tick: the first piece keeps the clip, a note across the cut ends
  there and carries on from the start of the second piece (struck again there), and the
  second piece starts with each controller (pedal, mod wheel, bend, program) at the value it
  had at the cut, so either piece plays right on its own. A piano roll open on the clip stays
  on the first piece. Agents use `split_clip` with a MIDI clip's id.
- **Insert MIDI track:** right-click a time selection (or an empty part of a lane) → Insert
  MIDI track: a new MIDI track under the one you clicked, armed, with an empty MIDI clip over
  the selection (whole bars when Snap is on; two bars at the click without a selection), open
  in the piano roll. The submenu offers instruments to load on it: those you've picked before,
  those the project already plays, and installed ones that say they're instruments; "Choose
  instrument…" opens the plug-in browser for it. On a MIDI track the menu also has Insert MIDI
  clip. Agents use `insert_midi_track`.
- **Waveform view:** the Waveform / Multitrack buttons in the menu bar (or
  Ctrl+Tab) switch between the arrangement and one audio file on its own. Double-click an audio
  clip to open its file there, or use File > Open audio file (WAV, FLAC, MP3, AIFF; it isn't
  added to the project). The file shows at its own sample rate, channel by channel, from the
  whole file (F) down to single samples, 32 points each, drawn as the Multitrack draws clips
  (Ctrl+wheel or = / -; the wheel scrolls); the ruler
  and the clock count time, samples, or bars and beats (the toolbar's time display: the song's tempo and meter from the file's start, with the song's grid lines
  over the waveform; selections and the playhead still go to the sample). Drag to select (Alt: one channel), click to place the playhead.
  Space or Play plays the selection, or from the playhead, on its own through the master's
  outputs (silent while the master is muted), and never in an export; playing the project
  stops it. Agents use `open_audio_editor`, `get_audio_editor`, `play_audio_editor`,
  `close_audio_editor` and `set_view` (`view_mode`, `document`, `editor_selection`, …).
  - **Spectral display:** the toolbar's display menu shows the waveform, a spectrogram, or
    both (waveform above): a log frequency scale from 20 Hz with its labels on
    the right, coloured from black to pale yellow over an adjustable range (100 dB below full
    scale by default; a full-scale sine is 0 dB). It renders in the background, so long files
    scroll smoothly; the previous picture moves with the view until the new one lands.
    Agents read a range's spectrum in bands with `get_spectrum`, and set the display with
    `set_view` `spectral`.
  - **Spectral repair:** with the Box tool (the toolbar's Box, or E; T goes back to time),
    drag a box in time and frequency on the spectrogram (Alt: one channel). Effects then
    attenuates it, removes it (Del) or heals it from the sound just before and after it, a spot
    heal for clicks, squeaks and string noise. Only the box changes:
    audio more than a frame (about 43 ms) from it in time is untouched, and other frequencies
    at the same time move by less than 0.1 dB. Agents use `set_view` `editor_box` and
    `edit_audio`'s `spectral_attenuate`, `spectral_remove` and `spectral_heal`.
  - **Effects rack:** the panel left of the file holds plug-ins, as a track header's rack does
    (power buttons, click a name for its editor, drag a number to move it). The file plays
    through them, and Apply writes them into the selection (or the whole file) as one step of
    its history: exactly what was heard, latency compensated, rendered in the background with
    a progress bar and Cancel. Bypassing or removing a slot while Apply renders changes what
    you hear and the next Apply, not the one running. The rack is the view's, not a file's or
    a project's: it is kept beside the program (`rack.json`, plug-in settings and all) and comes
    back in the next run, its plug-ins loading one at a time behind the busy bar; one that no
    longer loads keeps its slot and settings, greyed, saying why. The rack's **Presets** menu
    loads a saved rack (replacing this one), saves this one by name (Save rack as…) or deletes
    one; presets live in `RackPresets` beside the program, for every project. Agents use
    `add_insert` with `editor` true, `apply_audio_effects`, `get_audio_editor`'s `rack` and
    `applying`, and `list_rack_presets`, `save_rack_preset`, `load_rack_preset` and
    `delete_rack_preset`.
  - **Sync with the track** (21.2): a file opened from a clip (double-click it in the
    Multitrack) has a **Sync with <track>** checkbox above the rack (off by default; greyed for
    a file opened on its own). On, your own rack is set aside (still the one kept between runs)
    and the rack becomes copies of the track's effects with their settings, loading one at a
    time behind the busy bar, so the file plays as the track's effects play its clip in the
    Multitrack (bit for bit: the same effects, settings, block size and pre-roll; the track's
    fader, pan, mute and sends, the clip's gain and fades, and what follows the track in the
    mix are not in the rack). Synced from a second file of the same track, the rack stays as
    it is. From then on the two are one
    rack: adding, removing, moving or bypassing an effect in either view does the same in the
    other, and in the Waveform view it is one undo step of the project's, which undoes both;
    turning a knob in either editor (or a built-in effect's slider) turns its twin; a change no
    knob shows, such as a preset loaded inside a plug-in or a noise print, copies the whole
    settings across (when the plug-in says its values changed, or when its editor closes).
    The track's automation lanes aren't copied: the rack plays the settings as set by hand.
    Off, your own rack comes back; closing the file or removing the track turns it off, and
    rack presets can't be loaded while it is on. **Apply** while synced asks first: "Apply and
    turn off on <track>" writes the effects into the file and, once it lands, switches off the
    track's effects it printed (an undo step of the project's; the Apply is the file's), so the
    clip isn't processed twice; "Apply anyway" doesn't. Agents use `set_rack_sync`,
    `get_audio_editor`'s `sync`, each document's `source` and each slot's `track_insert`, and
    `apply_audio_effects`' `turn_off_track` (required while synced).
  - **Diagnostics** (View > Diagnostics, a pane beside the inspector):
    pick **Click Repair** (the Click Remover's detector and repair; Threshold and
    Complexity) or **Clip Repair** (Clip Repair's scan and repair, its clip levels learned
    from what is scanned; Tolerance and Min. Clip Size), then Scan selection or Scan file. The
    list gives each problem's time, channel, length in samples and level (a click's size, a
    clipped run's highest sample); click a header to sort. A row takes the Waveform view there,
    zoomed in and selected (Ctrl and Shift pick more); Repair selected or Repair all rebuilds
    them as one step of the file's history, and the repaired ones leave the list. Repairing
    all is what the effect itself plays (bit for bit; Clip Repair at Gain 0 dB), but with
    no gain: rebuilt peaks can rise over the old clip level. An edit, undo or redo of the file
    clears the list. Agents use `find_audio_problems`, `get_audio_problems`,
    `repair_audio_problems` and `set_view` `show_diagnostics`.
  - **Editing:** the toolbar's Edit menu cuts, copies, pastes (into any open file; another rate
    is converted), deletes, trims to the selection, inserts silence and silences; Effects
    amplifies, normalises (peak dBFS, or integrated LUFS), fades in or out (linear or equal
    power), reverses and inverts. They act on the selection (one channel with Alt), or the whole
    file. Each is a step in the file's own History (Ctrl+Z / Ctrl+Y there; the project's undo
    is untouched), and an asterisk marks unsaved edits.
  - **Saving (Ctrl+S) overwrites the file** after a prompt naming the clips
    that use it; it keeps the file's format (a 24-bit WAV stays 24-bit) and is written beside
    it, then renamed over it, so a failed save never leaves half a file. Every clip on it plays
    the new audio at once. Undo and save again puts the file back. FLAC, MP3 and AIFF save only
    with Save As (Ctrl+Shift+S), which writes a WAV. Agents use `edit_audio`,
    `undo_audio_edit`, `copy_audio`, `save_audio` and `save_audio_as`.
- **Several clips at once:** Ctrl- or Shift-click to add clips (audio or MIDI) to the
  selection, or drag over them in an empty part of the lanes. Delete, split (S), cut, copy, paste,
  duplicate and dragging all act on the whole selection; drag up or down to move clips to
  other tracks. Paste lands at the playhead, on the selected track: click a clip, Ctrl+C, click
  another track's name (or its lane, which also moves the playhead there), put the playhead
  where it goes (click the ruler), Ctrl+V. The copy is the clip exactly (its trims, split
  piece, fades, gain, stretch, pitch and tempo following) and becomes the selection. Copying
  in Oxide also puts a line ("1 clip copied in Oxide") on the system clipboard, so Ctrl+V
  always reaches Oxide; whatever text is on the clipboard, Ctrl+V pastes Oxide's clips (in the
  Waveform view, its copied audio). In a text field (a name, a typed volume) the keys copy
  and paste text as usual. Agents use
  `copy_clips`, `cut_clips`, `paste_clips`, `duplicate_clips`, `move_clips`, `delete_clips`
  and `set_view` `select_clips`.
- **Markers:** the strip under the ruler holds named markers, numbered in time order. Click
  one to go there, drag it to move it, double-click to rename it, right-click to rename or
  delete it; double-click the strip (or press Insert, or Edit → Add marker at playhead) to
  add one. 1-9 jump to the first nine, [ and ] to the previous and next. Markers stay on
  their bars when the tempo changes. Agents use `get_markers`, `add_marker`, `set_marker`,
  `remove_marker` and `go_to_marker`.
- **Audio quantize:** right-click a clip → "Slice at transients…": the attacks it finds are
  drawn on the clip as you set the sensitivity and shortest gap. Slice cuts there and opens
  Quantize for the slices: a grid (bar to 1/32, triplets), a strength (100% on the grid, 50%
  halfway), fill gaps (no silence opens up) and a short crossfade where slices meet (5 ms, over the quiet just before each hit, so moved slices don't click). Q quantizes the selected clips any time. Set
  the tempo to the performance's first.
- **Track headers:** the name, then **M**, **S**, **R** (record arm)
  and a headphones button for input monitoring (blue: Auto while armed; lit: On; struck
  through: Off; click for the next). Taller tracks add volume and pan knobs with their values
  (drag up or down, Shift for fine, double-click to reset; automated knobs grey out and show
  the lane, as the mixer does), then the automation mode at the bottom. Folders and returns
  have no R or monitor.
  - **Typed values:** click the number beside the volume or pan knob (the master's too, the
    inspector's Volume and Pan, and the pan and level numbers on a mixer strip) and type.
    Volume in dB: "-6", "-6.5", "-6.5 dB", "+3", "0", "-inf" (the fader's bottom, -60 dB);
    out of range is clamped to -60..+12. Pan as the readouts show it: "C", "L30", "R20", or
    a number (-30 is L30, 30 is R30). Enter or clicking away applies it, Escape keeps the old
    value, text that doesn't read is refused (outlined red, the reason on hover). Each typed
    value is one undo step. A control a lane is playing (or a write pass) can't be typed into.
  - **fx rack:** a tall track's header has a rack between the knobs and the
    automation menu: numbered slots, each with a power button (bypass), the plug-in's name
    (click to open its editor) and a ▸ menu (open, bypass, replace, remove). Click an empty
    slot to add a plug-in there; drag a slot's number onto another slot to move it. The power
    button by "fx" bypasses or restores the whole chain. Shorter tracks show "FX n" instead.
  - **Plug-in order:** the signal runs top to bottom ("fx ↓"): slot 1 hears the input first,
    so a tuner above an amp hears the dry guitar. Drag a slot's number onto another track's
    (or the master's) rack to move the plug-in there with its settings and automation, or
    Ctrl+drag to copy it with its settings. Agents use `move_insert` (with `track` or
    `master`) and `copy_insert`.
  - **Copy and paste plug-ins:** right-click a plug-in (in a track's rack, the mixer or the
    inspector) > Copy, then right-click an empty slot (or "+ Add plug-in") > Paste, or a
    plug-in > Paste after: a new instance with the settings it had when copied, on any
    track or the master, as often as you like, even after the original is gone. Agents use
    `copy_plugin` and `paste_plugin`.
  - **Busy work shows:** opening a project shows its loading bar at once, over an empty
    window: the last project's plug-ins close, its audio files are read on worker threads
    ("Reading Guitar.wav (3 of 12)"), then its plug-ins load one after another ("Loading
    <plug-in> on Bass (2 of 6)"), one bar over all of it, and the window keeps
    painting between them. The Multitrack appears once everything is in; Cancel goes back to
    the project that was open (reopened from its file) or to the start screen. Saving (Ctrl+S,
    File > Save, Save As, the "Save the changes?" question's Save) shows "Saving Song" at once:
    each plug-in's state is read in a frame of its own ("Reading <plug-in> on Piano (1 of
    5)"), then the file is written in the background. A plug-in that is slow to give its state
    (a sampler: about 1.6 s) still holds the window that long, under the bar. The question's
    look for changes reads them the same way ("Looking for changes to Song"), and autosave
    reads them silently and writes its backup in the background. Adding, pasting or copying a
    plug-in shows what is loading first. The window takes no clicks or keys meanwhile. Agents
    ask `get_busy` (and can `cancel_open`); `open_project` returns once everything is in,
    `save_project` once the file is written.
  - **Undo, redo and a new sample rate don't freeze the window either:** an undo or redo that
    brings plug-ins back (a removed insert or track, a replaced plug-in) is the project at
    once, and its plug-ins load one after another behind the same bar ("Undoing Remove insert",
    "Loading <plug-in> on Guitar L (1 of 3)"), over the window as it is; playback goes on
    with the mix as it was until they are all in, then switches once. Plug-ins it drops are
    closed a few at a time afterwards. Changing the sample rate (or a device that opens at
    another rate) stops playback and reloads every plug-in and file the same way. Neither can
    be cancelled (undo or redo again, or change the rate back, once it is in), and a second
    Ctrl+Z pressed while the bar is up is ignored. Undo, redo and a device restart are refused
    while an export or a track's render runs (cancel it first). Agents' `undo`, `redo` and
    `set_audio_settings` return once everything is in (`get_busy`'s `kind`).
  - **Bypassed plug-ins cost nothing:** a bypassed plug-in isn't run at all (its latency is
    still compensated), so the DSP load drops by what the CPU panel showed it costing; the
    panel lists it as bypassed, and its top line shows the whole app's CPU, plug-ins' own
    threads included (`get_cpu_usage`'s `process`).
  - **The view follows a dragged playhead:** drag the playhead toward either edge of the
    arrangement and the view scrolls with it, faster the further past the edge, stopping at
    the start. However fast you throw it, and even out of the window, the drag holds until
    you let go and the playhead stays on screen at the edge (with Snap on, on the grid line
    just inside it; zoomed in past the grid, at the edge itself). The piano roll's playhead
    does likewise, and drag-selecting in the Waveform view scrolls it the same way.
  - **The piano roll's playhead:** the song's playhead runs through the piano roll too.
    Click or drag its ruler (or drag the line) to move it, snapped to the piano roll's grid
    (Alt: not snapped); the arrangement follows, and moving it there moves it here. Space
    plays from it.
  - **Drop files onto tracks:** drag a WAV (or a MIDI file) from Explorer over a track: a
    ghost of the clip follows the pointer, on the track it would land on (or a "New track"
    row below the tracks when none fits). Let go over a track that can take it and Oxide
    asks: into that track, as it is set up (a template's track, with its plug-ins), or onto
    a new track. Where nothing fits it makes the tracks without asking, one per file track
    (a piano's treble and bass get their own). File > Import audio and Import MIDI ask the
    same at the playhead on the selected track.
  - **Reorder tracks:** drag a track by its colour stripe, its name or its header's
    background (in the mixer, a strip's name): the others make room as it goes, so the drop
    looks as it will. A folder's lower half takes it inside; below the last track it goes to
    the end; Escape cancels. Agents use `move_track_to`.
  - **Clips between tracks:** drag a clip (or the selected clips) up or down onto another
    track as well as along it; over a track that can't take it (a folder or bus, MIDI onto
    a track that isn't an instrument) it stays on the last one that could, and that lane
    says why. Hold Alt as you start to drag copies instead. Agents use `move_clips` and
    `copy_clips_to`.
  - **Clips snap to the playhead:** dragging a clip (audio or MIDI), its start or end within
    8 points of the playhead goes exactly onto it, a magnet, whether Snap is on or off (it
    wins over the grid within its reach), and the playhead's line lights while it holds.
    Trimming a clip's edge does the same. Hold Ctrl while dragging (at any moment of the drag)
    to turn the magnet off; Ctrl-click still adds a clip to the selection. Agents place clips
    by the exact sample with `move_clip` / `move_clips`.
  - **The edge you mean is the edge you get:** a clip's edge can be grabbed within 4 points
    either side of it, below the title bar. Where two edges are in reach (clips side by side),
    the selected clip's wins, then the nearer, then the clip on top. An edge under another
    clip (overlapping clips) can still be grabbed: the pointer on it lights it as a line across
    the clip covering it, and away from every edge the covering clip moves by its body as
    before. Pressing an edge, a fade handle or a clip changes nothing until the mouse moves:
    the edge then follows the mouse from where it was grabbed, snapped (or onto the playhead)
    as it goes, and a press and release without a move is no edit at all.
  - **A click on a clip places the playhead:** clicking a clip's body (audio or MIDI) selects
    it and its track, as before, and also moves the playhead to the click, exactly as a click
    in an empty part of a lane or in the ruler does: with Snap on to the grid's nearest line,
    off to the sample under the pointer. Ctrl- and Shift-clicks that add a clip to the
    selection move it too. The click clears a time selection, as a lane click does, so S and
    Delete then act on the clip. Dragging a clip moves the clip and leaves the playhead where
    it is; a double-click opens the clip (the Waveform view or the piano roll) with the
    playhead at the click, a right-click its menu, and a click on a fade handle, the gain line
    or an edge doesn't move the playhead. (Until 26.2 a double-click did nothing: the first
    click put the playhead's drag strip under the pointer and it took the second press. For
    the 0.3 s of a double-click after a click, at the same spot, the strip takes no press;
    drag the playhead from there a moment later.)
  - **Save before closing:** closing the window, File > Exit, opening another project or a
    backup, or starting a new one asks "Save the changes?" (Save, Don't save, Cancel) when the
    project differs from its file, or a file in the Waveform view has unsaved edits; an edit
    undone again is no change. Agents ask `get_unsaved_changes`.
  - **The master** has a row at the top of the arrangement with the same header (name, mute,
    knobs, fx rack, automation mode and lanes) and a lane that holds no clips: files dropped
    on it are refused. Click it to see the master in the inspector; right-click its lane to
    add automation. Agents set its row with `set_master` (`height`, `show_automation`).
- **Built-in effects (written natively):** the plug-in browser lists them first, under "Built
  in", in the groups Dynamics, EQ, Restoration, Reverb, Delay and Modulation. Each is original
  code, checked against reference renders. They are inserts like any
  plug-in: on tracks, the master and the Waveform view's rack; they automate
  sample-accurately, save with the project and open a small editor (sliders; double-click for
  the default). Agents use `add_insert` with the effect's name and the parameter actions. So
  far:
  - **Amplify** (a gain per channel, −96 to +48 dB, Link Sliders) and **Channel Mixer** (each
    output a mix of the inputs, −100 to +100 %).
  - **EQ:** the **Parametric EQ** (high- and low-pass up to 48 dB/oct, two shelves, five
    bands, constant Q or width) and the **Graphic EQ** with 10, 20 or 30 bands (linear phase,
    7193 samples of latency, compensated).
  - **Dynamics:** the **Limiter** (ceiling, input boost, look-ahead, release, linked
    channels), **Compressor** (threshold, ratio, attack, release, auto make-up),
    **De-esser** (Broadband or Multiband, centre and width, Output Sibilance Only),
    **Dynamics** (a curve of up to 7 points, Peak or RMS, look-ahead, Envelope Only) and the
    **Multiband Compressor** (three crossovers, four bands with Solo and Bypass, a Soft or
    Brickwall output limiter; its delay follows the crossovers and is compensated).
  - **Noise reduction:** **Noise Reduction** (learns a noise print, then subtracts it bin by
    bin: Noise Reduction %, Reduce By, Spectral Decay Rate, Smoothing, Precision Factor,
    Transition Width, FFT Size, Output Noise Only), **Hiss Reduction** (a spectral expander
    under a flat curve or a captured noise floor) and **Auto Noise Reduction** (learns the
    noise as it plays, no print).
  - **Restoration:**
    - the **Hum Remover**: a notch at a hum's fundamental and each of up to 7 harmonics above
      it, 20 Hz to 20 kHz, Q 30 to 3000, the cut sloping from Gain at the fundamental toward
      the top harmonic, Output Hum Only;
    - the **Click Remover** (Threshold and Complexity): finds clicks by an AR model's
      prediction error and rebuilds them from the audio on both sides, up to 40 samples long
      at the default; everything else comes back bit for bit, 4096 samples later, which the
      graph compensates;
    - **Clip Repair**, the Diagnostics' clip repair as an insert: a scan learns each
      channel's clip levels, then runs in bands within Tolerance of them, at least Min. Clip
      Size long and never under -24 dBFS, are rebuilt from 320 samples each side by a cubic
      refined by an AR model, the first 200 samples of a run at most; then the Gain, -12 to
      0 dB or Auto from the scan, and a limiter at -0.1 dBFS; not scanned, it takes clipping
      at full scale; everything else comes back as it went in times the Gain, 1480 samples
      later at 48 kHz;
    - **Click & Pop Repair**, a thorough click and pop remover as an insert, with 23
      settings: a click is a burst of the pre-emphasised signal's third difference or
      low-passed level against its own recent average, judged from both directions, over a
      detection threshold that follows the audio's level along a curve through three
      points, which "Scan For Threshold Levels" sets from a selection and "Scan For All
      Levels" with the thresholds too; then a second-level spectral check against a rejection
      curve, Detect Big Pops on a jump in the level, Pulse Train Verify, Link Channels and up
      to 32 passes; the repair is the Click Remover's; it counts what it corrected and
      rejected; everything else comes back bit for bit, 22016 samples later at any rate.
  - **Reverb:**
    - the **Room Reverb**: a Freeverb-style network on each channel: the input through its Low
      and High Frequency Cut, eight damped combs whose lengths follow Room Size and whose
      feedback follows Decay, four series diffusers set by Early Reflections, the right side
      0.53 ms longer; Width, Diffusion, Dry and Wet; no latency; changing Room Size empties
      it; its editor sketches the decay with the RT60s;
    - the **Convolution Reverb**: your own impulse response, WAV, AIFF or FLAC up to 30 s,
      chosen with its editor's "Load impulse…" or `set_impulse_file` and copied into the
      project's `Imported` folder; Room Size keeps part of it with a half-cosine fade, Width
      cross-mixes the pair, Damping LF/HF reshape its lows and highs over time, Pre-Delay,
      then normalised; a stereo file plays left on left and right on right, anything else is
      averaged to one; Mix and Gain. It runs on a shared partitioned FFT convolver with 256
      samples of latency at any block size, about 0.3 % of a core for a 6 s stereo impulse at
      48 kHz; the impulse is prepared off the audio thread, the old one playing (and ringing
      out) until the new one is in, and an export waits for it. Without an impulse the input
      passes unchanged;
    - the **Space Reverb**: a convolution reverb whose impulse is made from its settings:
      early reflections from image sources in a shoebox room (Room Size, Dimension,
      Left/Right Location, a high-pass), a late reverb of random pulses under the decay
      (Decay, Diffusion, Perception, Pre-Delay as a fade-in) from a seeded generator, so the
      same settings always make the same impulse, and a three-band Coloration re-applied every
      Coloration Decay ms so the tail darkens as it decays; Sum Inputs or true stereo (two
      speakers, four paths), Include Direct; 29 parameters; the impulse is made off the audio
      thread in about 75 ms at the defaults and plays on the same convolver with 256 samples
      of latency; the three output levels have lanes; the editor groups the controls and
      draws the impulse made.
  - **Delay:**
    - the **Delay**: one copy per channel, -500 to +500 ms in whole samples, Mix and Invert,
      no feedback; a negative time puts the copy ahead of the dry sound and costs only that
      much latency, which the graph compensates; a change crossfades forwards over 100 ms; the
      editor draws the taps on a time axis and gives each delay in ms, samples and beats at
      the song's tempo;
    - the **Echo**: a feedback delay per channel up to 2 s, the dry sound at unity, Echo Level
      into the loop and Feedback around it, clipped at full scale, every repeat through the
      Successive Echo Equalization (eight octave bands made of cascaded running averages,
      drawn as a curve), Echo Bounce for ping-pong and Lock in the editor; the editor draws
      the repeats on a time axis;
    - the **Vintage Delay**: up to 8 s with a cubic read swaying ±0.1 ms (the wow), a shaper
      and a low-pass inside the loop by Mode (Analog, Tape/Tube, Tape) and Trash, Feedback to
      200 % with a soft limiter above 100 %, Spread on the wet mid/side, delay changes
      gliding at up to a sample a sample like tape, and Tempo Sync to a note value at the
      song's tempo, gliding through tempo changes; the editor has Mode's buttons and draws
      the shaper and the low-pass.

    All three cost under 0.6 % of a core at 48 kHz.
  - **Modulation:**
    - the **Chorus**: up to 16 voices whose delays and levels wander under filtered random
      noise, spread in time and across the stereo field by a pan law, with binaural cues,
      negative feedback, Dry and Wet to 200 % and a 32-tap sinc at Highest Quality; its
      random generator reseeds from the settings at every start (warmed up on the main thread
      when they change, so a start only copies it), so a render is what played; only Dry and
      Wet have lanes; the editor draws the voices on a delay axis by pan; 0.3 % of a core at
      the defaults, 3.5 % at 16 voices at Highest Quality;
    - the **Flanger**: two taps either side of a centre 5082 samples back, so it flanges
      through zero where they meet, or one tap; a sine or triangle sweep, the right channel
      ahead by Stereo Phasing, feedback through the centre; its 5082 samples of latency are
      compensated; the editor draws the sweep per side; 0.7 % of a core;
    - the **Phaser**: our own design, so unlike the other effects it isn't checked against a
      reference render: one second-order allpass a stage, each a notch, Intensity their
      sharpness, swept by a triangle or sine LFO on a logarithmic or linear scale, feedback
      that raises peaks between exact notches, the right channel's LFO ahead by Phase Diff;
      the editor draws the response at both ends of the sweep; 0.2 to 0.4 % of a core.
  - **Learning from audio:** in the Waveform view, select noise alone (or the clipped or
    crackly audio) and choose Effects > Capture noise print / clip levels: the rack's first
    effect that learns from audio takes it, Noise Reduction or Hiss Reduction a noise print,
    Clip Repair the clip levels, Click & Pop Repair its threshold levels (a Noise Reduction
    is added when the rack has none), and its editor shows what it learned; Clip Repair's and
    Click & Pop Repair's editors can also scan the Waveform view's selection themselves (the
    latter's draws its detection and rejection curves with their points). Agents use
    `capture_noise_print` (with `scan` for Click & Pop Repair's second scan), which can also
    target a track's insert. What an effect learned is saved with its settings, and reaches
    it while playing without a gap.
  - The editors draw what the settings do: the equalisers' and the Hum Remover's frequency
    response (the Hum Remover's with its harmonics marked), the dynamics' transfer curve.
  - The effects are compared with reference renders by a maintainer's tool kept
    outside the repository (built with daw-core's `reference` feature where present; the
    steps skip without it). None costs more when the
    music stops: their decaying state is flushed before it reaches the denormals (which had
    made the Limiter and Clip Repair 30 to 80 times dearer on silence, the Multiband
    Compressor, Dynamics and the De-esser up to twice), and `denormal_check` times every
    effect's silent tail against its cost on noise, block by block, in the full tier.
- **Zoom and track height:** the wheel bindings are in the shortcuts table below.
  - −/+/Fit buttons and a scroll bar along the bottom; "Zoom to clip" in the clip menu.
  - Both the Multitrack and the Waveform view zoom from 2 seconds a point right down to 32
    points a sample (the Multitrack used to stop at 4 samples a point). Zoomed out, a
    waveform is drawn from min/max peaks made once when the file loads (bins of 16 to 65,536
    samples): each point is its own samples' min and max below 16 samples a point, and
    within a bin of them further out (16 samples up to 256 a point, 256 beyond, as before);
    past one sample a point it is the samples
    themselves, a line, with a dot on each once they are 4 points apart. Clip gain, fades,
    the display gain and stretched clips' renders show at every zoom. A take shows blocks
    zoomed in below 256 samples a point while it records (only its 256-sample peaks come
    from the audio thread) and its full detail once it stops.
  - Track heights: drag a header's bottom edge, or the presets in the track and View menus.
    They're saved with the project but aren't undo steps.
  - Waveform display gain (up to 16×) makes quiet takes readable without changing the
    sound.
  - Agents use `set_view` (`zoom`, `start`/`end` down to 32 points a sample, `track_height`,
    `waveform_scale`; `editor_start`/`editor_end` in the Waveform view) and `set_track`
    (`height`).
- **Undo and redo:** Ctrl+Z, and Ctrl+Shift+Z or Ctrl+Y.
  - **The Edit menu** names the next undo and redo ("Undo Move clip").
  - **What's a step:** every edit, in the app or by an agent over MCP, is an undo step,
    named by what it did. A fader drag or a clip drag is one step.
  - **Recording:** a recording is one step.
  - **Plug-ins** come back with the track or insert they belong to.
  - **Not undo steps:** plug-in parameter changes.
  - **Agents** use `undo`, `redo` and `get_history`.
  - **Restored plug-ins:** a plug-in brought back by undo (of a removed insert or track)
    comes back with its knobs where they were, saved or not.
- **Tempo map:** tempo and time-signature changes along the song.
  - **Units:** tempo counts quarter notes, as in other DAWs; projects saved before this
    convert on load at the same speed.
  - **Adding and editing:** right-click the ruler to add or edit a change at a bar. The
    ruler marks each change ("3/4 · 90").
  - **The transport fields** edit the tempo and meter in force at the playhead.
  - **What follows the map:** the grid, snapping, quantize, the metronome, count-in and the
    tempo plug-ins see. Changes land on their exact sample, in playback and in exports.
  - **Limit:** changes are instant, with no ramps yet.
- **Metronome:** the Click toggle (or `C`) turns it on; the ▾ menu next to it has the
  volume and "only while recording".
  - The first beat of each bar is accented.
  - It is lined up with what you hear, through plug-in delay compensation and loop wraps.
  - It plays on the master's output pair, after the master, so no plug-in touches it.
  - Muting the master mutes it, and it is never in an export.
- **Count-in:** right-click the Record button and pick Off, 1 bar or 2 bars (the Click ▾
  menu has the same choice; `set_metronome`'s `count_in_bars` over MCP).
  - With one set, the Record button shows its bars in a small yellow tag, and its hover
    says so.
  - Recording from a stop starts that many bars early with the click, even with the
    metronome off.
  - While it plays, the time display counts down the beats left in red ("Count-in · 3",
    over "1 bar count-in"), then shows the position from the take's start;
    `get_transport` reports it as `count_in` (`bars`, `beats_left`, `record_point`).
  - The take starts exactly at the playhead, even at the very start of the song.
- **Loop recording:** record with the loop on, and each pass becomes a take of one clip.
  - **Where it starts:** recording starts at the loop start. A playhead before the loop
    plays a run-up; pressing Record while playing waits for the next pass.
  - **Which take plays:** the last complete pass. The clip shows "take n/m", and its
    right-click menu picks or deletes takes.
  - **While recording:** earlier passes aren't heard until you stop, and the record light
    stays on for the whole run.
  - **Comping:** split the clip, then pick a take per piece.
  - **Another round:** recording the same loop again adds takes to the same clip.
  - **Punch:** with punch on as well, each take is trimmed to the punch range.
- **Punch in/out:** Shift+drag on the ruler to set the punch range (a red strip; drawing it
  doesn't switch punch on), and toggle it with the Punch button or `P`.
  - While it is on, recording happens only inside the range, to the sample, and playback
    runs on before and after it.
  - With no range set, turning it on punches the loop region.
- **Plug-in transport:** every plug-in gets the playhead each block: playing or stopped,
  position, tempo and time signature. Tempo-synced delays and LFOs follow tempo changes,
  in playback and in exports alike. A plug-in behind latency, or on a bus, is told the
  position of the audio it is actually hearing.
- **Delay compensation:** solved per bus, so parallel paths with different plug-in latency stay
  sample-aligned at every folder bus and at the outputs. Runtime latency changes are picked up.
- **Seamless edits:** editing during playback (moving clips, adding tracks, rerouting)
  rebuilds the graph without a dip. Running state carries over to the new graph.
- **Dev plan:** what's next, and the technical debt, are tracked in the maintainer's dev plan (`devplan.md`, kept out of the repository).
- **AI control over MCP:** while the app runs it serves MCP at `http://127.0.0.1:7878/mcp`
  (the port is a setting), shown in the status bar.
  - **Starting:** the server starts in the background when the app starts, before any
    project, and stays up on the start screen, while a project opens and through new
    projects. The status bar says "MCP starting…", then the address, or why it is off: "MCP
    off: port 7878 is in use" in red with Retry (also "reserved by Windows" or a port outside
    1024–65535). Clicking it retries, or opens the settings.
  - **Settings:** Options > AI control (MCP)… (the settings window on its AI control
    section): Enabled, the port (applied on Enter, 1024 to 65535) and the token apply at
    once, restarting the server in the background, and are saved. Calls in flight get an
    error rather than hanging; clients reconnect. The `claude mcp add` line is greyed, with
    why, while the server isn't running.
  - **Connecting Claude:** the settings' "Connect Claude" rows show, for Claude Code and for
    Claude Desktop, whether Oxide is in their config: "Configured (port 7878)", "Configured
    for another port (7899)" (after the port changed), "with an old token", "Configured over
    HTTP (port 7878): Reconfigure to use the bridge" (Claude Code set up by Oxide 0.2.3 or earlier), "Configured (port 7878), but its bridge is out of date" (Oxide updated since: Reconfigure),
    "Not configured", "Not installed", or "Couldn't read the config: …". Check again reads
    them once more.
    - **Configure** (active only when Oxide isn't there) adds it under the name `oxide`,
      in both clients as the bridge (below), so Oxide stays listed in Claude while it is
      closed, like any server whose program runs, and its tools work as soon as it starts,
      with nothing to reconnect. Claude Code through `claude mcp add --transport stdio
      --scope user oxide -- <app>\mcp-bridge\oxide-mcp.exe --port <port>` (with `--token`
      when one is required); new Claude Code sessions have the tools, and `claude mcp list`
      shows it connected whether or not Oxide runs. Claude Desktop gets an `oxide` entry in
      its `claude_desktop_config.json` that runs the same bridge; every other server and
      setting in the file is left exactly as it was, and a copy of the file before the
      change is kept beside it (`claude_desktop_config.json.oxide-backup`). **Restart
      Claude Desktop** (quit it from the tray, not just the window) to use it.
    - **Reconfigure** (active once configured) removes Oxide's entries and adds them again
      for the current port and token: use it after changing either, and after installing a
      new Oxide (it refreshes the bridge both clients run). An entry under the old name `daw`
      (the command this page used to give) pointing at Oxide is replaced too, and Claude
      Code's HTTP entry from Oxide 0.2.3 or earlier is moved to the bridge.
    - **Remove** takes Oxide's entries out.
    - Desktop's config is the installer's (`%APPDATA%\Claude`) or, for the Microsoft Store
      build, the one inside its package (`%LOCALAPPDATA%\Packages\Claude_…\LocalCache\
      Roaming\Claude`), which is the file that build reads; where both apply, both are
      written. A config that isn't valid JSON is never touched: fix it first.
    - These are the only writes Oxide makes to Claude's settings, and only on your click:
      no MCP action can do it, so an agent can't rewrite its own client's config, and the
      token never leaves the app.
    - By hand, Claude Code: `claude mcp add --transport http --scope user oxide
      http://127.0.0.1:7878/mcp`; the settings' Copy gives the line for the port and token in
      use. Over HTTP, Claude Code connects only if Oxide is running when the session starts
      (otherwise it shows the server failed until `/mcp` reconnects it); Connect Claude's
      bridge has no such limit.
  - **The bridge** (`oxide-mcp.exe`, built beside `daw.exe`; keep the two together when
    copying the app): Claude Desktop only runs local programs that speak MCP on
    stdin/stdout, and Claude Code runs it too, so Oxide stays listed while it is closed.
    `oxide-mcp --port N [--token T]` forwards each message to the app's HTTP endpoint; no
    Node or `mcp-remote` is needed, and it writes nothing to disk.
    - It answers the handshake, the tool list and the prompt list itself, from the tools of
      the Oxide it was built with (`crates/mcp-link/src/catalog.json`, regenerated with
      `docs/mcp-tools.md`), at once and whether or not Oxide runs; its instructions tell the
      agent Oxide must be running for the tools to act. So Claude never marks Oxide as failed
      because it was closed (Desktop did with Oxide 0.2.3 and earlier, and dropped it from its Code tab too).
    - A tool call while Oxide is closed is answered "Oxide isn't running: start it, then try
      again" as the tool's own error (after about 2 s, Windows' wait on a closed port); once
      Oxide runs, the next call goes through, with nothing restarted. If the app restarts, the
      bridge opens a new session by itself.
    - Once connected it compares the app's tool list with its own: when they differ (Oxide
      updated but the bridge not, or the other way round) it tells Claude the tools changed
      and lists the app's, and a call to a tool only the bridge knows says which versions
      differ and to Reconfigure.
    - Both clients run one copy in `mcp-bridge\` beside the app (refreshed by Configure and
      Reconfigure; a running copy is moved aside), so rebuilding while Claude is open never
      fails on a locked `oxide-mcp.exe`.
  - **Token:** only programs on this computer can connect. To let in only the clients you
    choose, tick "Require a token" in the settings, then Reconfigure Claude Code and Claude
    Desktop; the Copy button gives the `claude mcp add` command with the token's
    `--header "Authorization: Bearer …"`. Agents can't read or change the token.
  - **While a project opens** every call waits until it is open, except get_busy (the
    progress), cancel_open and screenshot. An agent's own open_project runs behind the same
    loading bar as File > Open (the window keeps painting) and answers once the project is
    in, or with an error if the open is cancelled. With no project open (the start screen) an agent
    can read, open or make a project, and change the app's devices, templates and layouts;
    changes to a project are refused with a message saying so.
  - **While a file dialog is up** (Open, Save As, Import, Export, the impulse, the cover…)
    agents' calls are answered as usual and the window keeps painting: the dialog runs on a
    thread of its own, in front of the window, which waits for it (no clicks or keys) as for
    any modal dialog. If an agent opens another project meanwhile, the file chosen in the
    dialog is left alone (a message says so) rather than saved to or imported into it.
  - **What an agent can do:** 171 tools (listed in `docs/mcp-tools.md`) cover everything the app can do today:
    - projects: new, open, save, tempo and the tempo map;
    - tracks: routing, folders, mixer, master;
    - plug-ins: load, bypass, reorder, editors, and reading or setting parameters by name
      (during playback too: parameters are read from a cache and set through a queue, never
      by taking the plug-in from the audio thread, so the track never drops a block);
    - clips: import, split, move, gain, takes, and reading the waveform;
    - audio quantize: detect the transients in a clip, cut at all of them in one call, and
      quantize the slices to the grid (bar, beat, 1/4 to 1/32, triplets) with a strength
      setting;
    - transport, the loop, the metronome, count-in and punch, recording and export;
    - audio and MIDI devices, and engine stats;
    - the view: meters, selection, zoom and panels, and screenshots.
  - **Writing a part:** an agent can read a MIDI clip as music and write a new one against
    it, in bars and note names. `describe_midi_clip` gives the clip's place as bar:beat:tick,
    the tempo and time signature where it plays, its estimated key with the runner-up, its
    chords bar by bar, its range and every note's position and name; `add_notes` and
    `set_notes` take the same forms back (ticks and note numbers still work). The server's
    `write_midi_part` prompt walks through it. For the demo's Keys clip:

    ```
    describe_midi_clip {"clip": 5}
      -> start "2:1:000", end "4:1:000", 4/4 at 120, key C major (0.951; runner-up A minor,
         0.831), progression "C Am | F G", range F3 to G4, first note C4 at "2:1:000"
    insert_midi_track {"bar": 2, "bars": 2, "after": 4, "instrument": "<synth>"}
      -> a track with the synth and an empty clip over bars 2 and 3
    add_notes {"clip": <that clip>, "notes": [{"at": "2:1:000", "pitch": "E5", "length": 900},
                                             {"at": "2:2:000", "pitch": "C5", "length": 900}]}
    ```

    Then play it, or `render_track` it (the maintainer's checks do all of this). The key and chords are estimates (key profiles and
    chord templates over the notes' lengths): a part using only a scale's notes fits its
    major and relative minor almost alike, so the runner-up is given with its score.
  - **Coverage:** every feature is being built as an action in one registry, so coverage
    grows with the app. The full tool reference is
    [docs/mcp-tools.md](mcp-tools.md), generated from the code.
  - **Access:** localhost only, and browser pages are refused.
- **Multithreaded mixing:** independent tracks are processed in parallel.
  - **How it works:** the graph is split into levels by each node's longest path to the
    outputs, so a node always runs before everything it feeds (its bus and any send
    targets). Each level's tracks run on a real-time worker pool, with the heaviest tracks
    started first.
  - **Sends:** a node can have pre- and post-fader sends, each with its own delay
    compensation.
  - **Workers:** they run at MMCSS "Pro Audio" priority. During playback they stay awake from
    block to block, spinning for about two buffer periods, then park.
  - **Light levels stay serial:** a level with under 20 µs of work runs on the audio thread
    alone, so a light session doesn't keep cores busy.
  - **Same result as serial:** tracks only write their own buffers in parallel, and are summed
    into buses afterwards in a fixed order. The mix is therefore bit-identical to
    single-threaded processing.
  - **Thread count:** set in Options → Settings → Audio. Auto is physical cores − 1.
  - **DSP readout:** the transport shows the mean and the worst recent buffer (hover for what
    they mean). Click it for the CPU panel: each track's and plug-in's time per buffer and
    share of the deadline, most costly first; agents use `get_cpu_usage`, whose inserts also
    count the blocks each has missed since it loaded (`misses`: its plug-in was held when the
    audio thread came for it). Tracks and plug-ins keep running while stopped, so there is load
    then too.
  - **Not pinned to cores, on purpose:** pinning the workers caused rare stalls of up to six
    buffer periods at 64 samples, because the driver's audio thread landed on a worker's core.
  - **Live playback:** 8 tracks of an amp sim, master muted, Ryzen 7700X (8 cores / 16
    threads). The worst block is the one that decides whether you hear a dropout.

    | device, buffer | audio thread alone: mean / worst | 7 workers: mean / worst | 100 ms intervals with a dropout, of 80 |
    |---|---|---|---|
    | WASAPI, 480 | 73% / 80% | 12% / 16% | 0 → 0 |
    | ASIO4ALL, 128 | 72% / 103% | 13% / 31% | 1 → 0 |
    | ASIO4ALL, 64 (two runs) | 78% / 115–139% | 14% / 34–152% | 17–19 → 0–1 |

  - **Export:** 24 s of audio. The amp sim went from 18.1 s to 2.9 s (6.3×); a channel-strip compressor,
    which is far lighter, gained 3.8×. With the compressor the serial and parallel exports are
    bit-identical. With the amp sim any two exports differ by about −55 dB, serial or parallel,
    because the amp sim itself is not deterministic.
- **Mixer:** fader, pan (balance), mute, solo (solo keeps parent folders and sub-tracks audible),
  peak meters with hold, and DSP load.
- **Clips:**
  - Import WAV, FLAC, MP3 or AIFF by menu or by dropping files (any sample rate; converted to the project's). WAV is read exactly as before; FLAC and AIFF decode bit for bit, and MP3s come back exactly as long as they were encoded.
  - Move with snap to the grid (and onto the playhead near it), split at the playhead (S; MIDI clips too) and delete (Del).
  - Waveform display.
  - Files at another sample rate are resampled on import.
- **Recording:** armed audio tracks record their dry input to 32-bit float WAV in the
  project's `Audio` folder, and the takes become clips when you stop.
  - **Seen as you play:** each take draws in the Multitrack as it records, from where
    recording began (or the punch-in) to the playhead, its waveform a few milliseconds behind,
    marked in red. A loop recording shows the pass being recorded. On stop the clip takes its
    place with the same waveform. `get_transport` reports each take's length so far.
  - **Placement:** takes line up with what you heard while playing them. They are moved
    earlier by the plug-in delay compensation plus the device's output-to-input round trip,
    measured from the driver's own timestamps.
  - **Measured examples:** a wireless USB headset over WASAPI measured 61 ms; ASIO4ALL at
    512-sample buffers measured 25 ms.
  - **Correcting it:** if a loopback test shows takes still off, set a record offset in
    Options → Settings → Audio.
- **Audio devices:**
  - WASAPI, and ASIO (FlexASIO, ASIO4ALL, interface drivers).
  - Sample rate and fixed buffer size are configurable.
  - **Processing block** (Settings > Audio, off by default): the device's buffers are gathered
    into one larger block before processing, for the block less one buffer of extra latency
    (shown there). Some amp sims cost mostly per call (one about 300 µs whatever
    the call's length), so at FlexASIO's 16-sample buffers one track needs a whole core or
    more; at 64 it needs about a third. It helps drivers that call back in bursts (FlexASIO on
    shared WASAPI); exports run in the same block, and takes move earlier by the added latency.
  - Changing the sample rate reloads plug-ins with their state (behind the loading bar, a
    plug-in at a time) and converts clip positions.
- **MIDI:** every input port is opened (keyboards, controllers) and routed to armed instrument
  tracks.
- **The start screen** (on launch with no project, and File > New project) has New project
  on the left (a name, a template to start from, the folder it goes in, Create) and Open on
  the right: Open project… and the recent projects, the latest opened first. Each is a row
  with its name, the folder it is in (shortened in the middle, the full path on hover), when
  it was last saved ("today 14:32", "yesterday", "3 days ago", then the date) and the
  template it was made from. A click opens it; a right-click offers Open, Show in Explorer
  and Remove from the list. One that is gone (moved, deleted, or on a drive that isn't
  connected) is greyed with "Missing" and a Remove button. The files are looked at on
  background threads, so a slow or missing drive never holds the window up. Agents use
  `list_recent_projects` and `remove_recent_project`.
- **The manual:** Help > Manual opens it in the web browser, and F1 opens the page for the
  pane or window under the pointer (see "The manual" above). Agents use `search_manual` (the
  passages that answer a question) and `open_manual` (show the user a page or a topic).
- **Settings** (Options > Settings…) are in sections listed down the side: Audio (the
  driver, devices, rate, buffer, threads and record offset; Apply restarts the device and is
  greyed until something changed, and the list marks Audio while a change waits), MIDI
  (inputs, Reconnect), Projects (autosave, backups kept, the file type, where the settings
  file is) and AI control (MCP). Every setting has a line saying what it does; everything but
  Audio applies at once.
- **Projects are folders.** The start screen makes `<folder>/<Name>/` holding `<Name>.oxide` (JSON with plug-in state embedded)
  and the folders the project's files go in:
  - `Recorded/`: takes from recording (older projects' `Audio/` still plays).
  - `Imported/`: a copy of every audio file dropped, imported or opened in the Waveform view
    (MIDI files become clips, so they need none), so
    edits and Save never touch the original; a file already there with the same bytes is
    reused, and one with the same name but other bytes is numbered.
  - `Rendered/`, `Exported/` (the export dialogs start there), `Backups/` and `Temp/`.
  - Nothing is written outside the project folder; with no project open, recording and
    importing are refused. Save As into another folder copies the project's files along.
    Copy or move the folder and it opens anywhere. `.dawproj` projects still open.
  - `daw <project.oxide>` opens a project; Options > Settings > Projects > "Open .oxide
    files with Oxide" registers the file type for your account (Explorer then opens them
    with Oxide and shows its icon).
  - Settings (audio device, recent projects, MCP) live in `settings.json` beside the program,
    not in `%APPDATA%`: a first run reads the old file there once.
  - Agents use `new_project` with a `name` and `folder`; `get_project` reports the `folder`.
- **Export formats:** the export dialogs (Export mixdown, and File > Export file… in the
  Waveform view) write WAV (16 or 24-bit, or 32-bit float), AIFF, FLAC (levels 0-8), MP3
  (constant 128-320 kbps or variable V0-V9) or Ogg Vorbis (quality 0-10), at the audio's rate
  or converted (44.1, 48, 88.2 or 96 kHz), with TPDF dither on integer depths (an option).
  Everything is encoded inside the app: LAME (LGPL) and libvorbis (BSD) are compiled in, FLAC
  is written by `flacenc` (Apache). MP3s carry their encoder delay, so they decode at exactly
  their length. The format is remembered; exports go to `Exported/` by default. Ogg Vorbis
  files import too. Agents give `export_mixdown` and `export_audio` a `format`.
- **Metadata:** View > Metadata… in the Waveform view edits the file's title, artist,
  album, album artist, year, track, genre, composer, comment and cover art (a JPEG or PNG);
  Apply is one step of the file's history. A file's tags are read when it opens and written
  by Save and File > Export file…: ID3v2.3 in MP3s (the version Windows Explorer reads),
  Vorbis comments in FLAC and Ogg, INFO and ID3 chunks in WAV, ID3 in AIFF. Multitrack exports
  carry none: mix down to a new file to tag it. Agents use `get_metadata` and
  `set_metadata`.
- **Progress:** every export, track render, mixdown and file export shows a progress bar that
  says what it is making and runs through rendering and then writing the file; Cancel stops
  it at any point. Agents read the same from `get_export_status` (`kind`, `what`).
- **Mixdown to a new file** (File menu): the whole session or the time
  selection, as it plays (every track, level, plug-in, automation and send, and the master),
  rendered like an export into `Rendered/` and opened in the Waveform view for final edits or
  an export. The master has to be unmuted. Agents use `mixdown_to_new_file`.
- **Rendering a track:** right-click a track (or Track menu) > "Render track…" renders it as
  you hear it (its clips or MIDI through its instrument, plug-ins, fader, pan and automation,
  a folder's tracks; not its sends) through the same engine as an export into `Rendered/`, and
  puts it on a new audio track right under it, with copies of its sends. By default the
  original is then switched off. A muted track has to be unmuted first; a solo elsewhere
  doesn't matter. It renders on a thread of its own with a progress bar and Cancel (the
  window keeps painting; the file is read back on that thread too), and the new track appears
  when it is done. Agents use `render_track`, which returns at once like `export_mixdown`;
  `get_export_status` gives the progress and then the new `track`.
- **Switching a track off:** the power button in its header (or "Disable track") takes the
  track, and whatever reaches the mix through it, out of the audio graph: unlike a mute its
  plug-ins cost no CPU. Its header and clips are dimmed, and every setting is kept. Agents use
  `set_track_enabled`.
- **Autosave and backups:** every 5 minutes (Options > Settings > Projects; 0 turns it
  off) while the project has unsaved changes, it is copied to
  `Backups/<Name> <date time>.oxide`, keeping the newest 20; the project file itself is only
  written by Save. Backups wait while playing or recording (reading a plug-in's state can
  skip a block of its audio) and run once the transport stops. File > Open backup… lists
  them; opening a project with a backup newer than its file (a crash, or a close without
  saving) asks whether to open the backup. Agents use `list_backups`, `save_backup` and
  `open_backup`.
- **Templates:** File > Save as template… keeps the project's setup under a name: its
  tracks, folders, buses, routing, sends, mixer and plug-ins with their settings (an amp sim's
  state too), its automation lanes without their moves, the tempo, time signature,
  loudness target and markers, but no clips or recordings. New project on the start screen
  then offers it as the Template to start from; File > Templates… renames and deletes them.
  They live in `Templates/` beside the program, for every project. Agents use
  `save_template`, `list_templates`, `rename_template`, `delete_template` and
  `new_project`'s `template`.
- **The icon** is `crates/daw-app/assets/oxide.svg`, rendered to the `.ico` in the executable
  and the window icon (see `icon.rs` to re-render it after editing).
- **Export:**
  - Mixdown to 32-bit float WAV through the live engine, at the live block size.
  - Plug-ins are pre-rolled on silence first, so exports are repeatable.
  - In tests a re-export matched to -87 dB with an amp sim, and bit-identically with
    a mastering suite's plug-ins; an export after save and reopen matched to -141 dB.

### Shortcuts

| key | action |
|---|---|
| F1 | the manual's page for the pane or window under the pointer (Help > Manual: its first page) |
| Space | play / stop (in the Waveform view: the file) |
| E / T | Waveform view: the Box tool (drag a box on the spectrogram) / back to time selection |
| Ctrl+Tab | switch the Waveform and Multitrack views |
| R | record |
| Home | go to start |
| S | split selected clip at playhead (with a time selection: cut the clips at its edges) |
| Del | delete selected clip (with a time selection: what's inside it) |
| M | show/hide mixer |
| Ctrl+Z / Ctrl+Shift+Z (Ctrl+Y) | undo / redo |
| L | loop on/off (with a time selection: loop it) |
| C | metronome on/off |
| P | punch on/off |
| Ctrl+S / Ctrl+O / Ctrl+N | save / open / new |
| Ctrl+T | add audio track |
| wheel | scroll the tracks up and down |
| Ctrl+wheel (or pinch) | zoom the timeline around the pointer |
| Shift+wheel | scroll the timeline sideways |
| Alt+wheel | every track taller / shorter, around the pointer: the track under it (and the spot on it) stays put, the rest of a notch too after Alt is let go |
| Alt+Shift+wheel | waveform display gain |
| = / - | zoom in / out around the playhead |
| F | fit the whole project |
| drag a header's bottom edge | resize that track (Ctrl+drag: all tracks) |
| drag the playhead (the orange line) | move it: with Snap on, to the grid's nearest line; off, to the exact sample under the pointer |
| right-click the ruler | Grid (Auto, 1/4 to 1/64, Triplets) and the tempo or meter at that bar |
| click a clip | select it and its track, and put the playhead at the click (snapped as a lane click) |
| drag a clip | move it (and the other selected clips); its start or end within 8 points of the playhead goes onto it, the line lit |
| Ctrl held while dragging a clip or its edge | no magnet to the playhead (the grid or the sample as usual) |
| drag a clip's edge | trim it there: in hides the audio, back out restores it (as far as the audio goes); onto the playhead within 8 points. The selected clip's edge wins over a neighbour's; an edge under another clip is lit when the pointer is on it and can be dragged; nothing changes until the mouse moves |
| Alt+drag a clip's edge | stretch the clip there (the left edge keeps its end) |
| drag a clip's corner square | fade in / fade out |
| drag a clip's gain line | clip gain (double-click: 0 dB) |
| double-click a track's name (or click it in the inspector) | rename it: Enter or click away keeps it, Escape doesn't |
| click a volume or pan number | type a value ("-6.5 dB", "-inf", "L30", "C"): Enter or click away applies it, Escape doesn't |
| drag in an empty part of the lanes | time selection (and the clips in it); Shift extends it |
| Ctrl/Shift+click a clip | add a clip to the selection (the playhead to the click too) |
| Ctrl+A / Escape | select every clip / none (and clear the time selection) |
| Ctrl+C / Ctrl+X / Ctrl+V (also Ctrl+Insert / Shift+Delete / Shift+Insert) | copy / cut / paste at the playhead, onto the selected track (in the Waveform view: the selected audio) |
| Ctrl+D | duplicate the selected clips after themselves |
| Insert | add a marker at the playhead |
| 1 … 9 | go to marker 1 … 9 |
| [ / ] | go to the previous / next marker |
| Q | quantize the selected audio clips |

## Known limits

- **Time-stretch:** new recordings don't follow the tempo unless set to, and MIDI clips can't be trimmed by their edges yet. Keeping attacks is on or off (no élastique-style modes).
- **Parallelism is per level:** a folder bus waits for the whole level below it, not just its
  own sub-tracks. A plug-in chain on a single track is serial by nature and cannot be split.
- **Rare spikes at small buffers:** at 64 samples with ASIO4ALL, about one 100 ms interval in
  170 still peaks above a full buffer, in bursts (most minutes none). A trace shows Windows
  taking one or more of the audio threads off the CPU for 1.5–2 ms, longer than a 64-sample
  period; the engine's own work stays well inside it. 128 samples absorbs it (0 overruns,
  worst block 84% of the period). `DAW_TRACE=1` on `live_bench` prints each slow block.
- **Spinning workers use CPU:** while heavy plug-ins run, the workers keep their cores busy
  between blocks; that is the price of low wake-up latency. Set "Audio thread only" to turn it
  off.
- **FlexASIO with a USB guitar interface:** opens at 48 kHz and runs 8 instances of an amp sim at about 24% load with the pool
  and no overruns. Its shared-mode WASAPI calls back in bursts of thirty 16-sample blocks every
  10 ms; the DSP meter measures each burst against those 10 ms (the settings show "in bursts of
  480"). FlexASIO reports a 44 ms round trip, which places recordings; it isn't yet checked
  against a real take (TD10). One track of any of three heavy amp sims
  overloads it at 16-sample calls (106-191%, a true overload); with a processing block of 64
  (+1 ms) they play at 39-57% with nothing over the deadline for a minute
  (`crates/dawpoc/README.md`).
- **Plug-in skips:** the audio thread never waits for a plug-in; if the UI holds one (saving
  its state), that block of the track goes silent, not dry, and the skip count is shown in the
  status bar. Opening and closing an editor no longer does this: the editor's view is made and
  attached without holding the plug-in (measured on an amp sim: no skipped block, where
  it used to silence the track for about a quarter of a second). Export never skips: it waits
  for the plug-in.
- **Monitoring latency:** a high-latency plug-in on any track adds its delay to monitoring on
  every track, because all paths are delay-compensated to the slowest. For example, a mastering
  suite's maximizer adds 810 samples.
- **No sandboxing:** plug-ins run in-process, so a crashing plug-in takes the app down.
- **Recording latency unverified on hardware:** the round trip comes from the driver's
  timestamps, and a simulated loopback lands a click on the exact sample. A real loopback
  test needs a cable from an output to an input:
  `loopback_check` (see "Checks"). That plays clicks out loud.
- **Long WASAPI takes may drift:** with WASAPI, input and output run on separate clocks. A
  long take can drift slightly against the playback; ASIO doesn't.
- **Plug-in transport:** plug-ins get no loop range or bar position yet, because the VST3
  host library cannot send them. A tempo-synced arpeggiator has not been tried; nothing
  installed has one that runs without an instrument library.
- **Hardware not tested here:** MIDI routing is unit-tested, but no MIDI port was connected
  during development. FlexASIO was untested because its device wasn't connected; ASIO4ALL
  was tested.
- **FlexASIO:** it only shows up when the device named in `%USERPROFILE%\FlexASIO.toml` is
  connected, because FlexASIO fails to start otherwise.

## Checks

`scripts/check.ps1` runs the checks in tiers and writes `out/check/report.md` (logs in
`out/check/logs/`). It exits 1 on any failure. Two tiers can be run by anyone with a checkout:

| tier | what it runs |
|---|---|
| `quick` | fmt, build, the manual's references (`manual --check`), all tests, clippy `-D warnings` |
| `manual` | the manual's screenshots taken from the release app (`scripts/manual_shots.ps1`, `--neutral`: no audio device opened) with their marks, the manual built from them with `--strict`, then checked in headless Edge and Chrome (search, highlights and badges, previous/next, dark, narrow, print) |

Run `quick` on every change, and `manual` when the manual's pages, figures or marks change, or
a UI change alters what a figure shows. The `manual` tier takes its audio from `-Audio <wav>`;
without it the demo uses a generated guitar riff (`dawpoc gen --guitar`).

```sh
pwsh -NoProfile -File scripts/check.ps1 -Tier quick
```

The maintainer also runs further tiers (`full`, `live` and `ui`). They need commercial
plug-ins, audio devices and the release app, and their scripts are kept outside the repository.
Asking `check.ps1` for them without those files prints that and exits non-zero.

The individual checks, which take a plug-in (a VST3 name or path) and an audio file of your own:

```sh
cargo test --workspace
cargo run --release -p daw-core --example smoke -- in.wav "<plugin>" "<plugin>"
cargo run --release -p daw-core --example open_check -- in.wav "<plugin>"
cargo run --release -p daw-core --example parallel_bench -- in.wav "<plugin>" 8
cargo run --release -p daw-core --example live_bench -- in.wav "<plugin>" 8 8 ASIO "ASIO4ALL v2" 64
cargo run --release -p daw-core --example editor_check -- "<plugin>"
cargo run --release -p daw-core --example rate_change -- in.wav "<plugin>"
cargo run --release -p daw-core --example insert_check -- in.wav "<plugin>"
cargo run --release -p daw-core --example device_check -- ASIO "ASIO4ALL v2"
cargo run --release -p daw-core --example loopback_check -- ASIO "<device>" 0
cargo run --release -p daw-core --example stretch_check -- in.wav
cargo run --release -p daw-core --example export_check -- in.wav
cargo run --release -p daw-core --example import_check -- in.wav
cargo run --release -p daw-core --example loudness_check -- in.wav
cargo run --release -p daw-core --example denormal_check
cargo run --release -p daw-core --example wave_draw_cost
target/release/daw --demo in.wav --plugin "<plugin>" --mixer --play --mute --screenshot shot.png
```

- `smoke` builds a project with a folder bus and real plug-ins, exports it, then re-exports
  after a save and reopen.
- `open_check` opens a project of nine audio files and real plug-ins as the app does and
  times each update: `open` must return within a frame, the files read on workers, each
  plug-in load gets an update of its own, and a cancel partway goes back to the project
  before. `daw --demo in.wav --show open-plugins` (or `open-files`) times the app's own
  frames while it opens the demo and exits 1 on a failure; `--show undo-reload` (an undo
  bringing the demo's plug-ins back), `--show render-progress` (a track's render) and
  `--show save` (Ctrl+S on the demo: the bar first, a plug-in's state a frame) do the same for
  those.
- `parallel_bench` exports the same session single-threaded (twice) and with the worker pool.
  It reports the speedup and how much the exports differ.
- `live_bench` plays the session on a real device with the master muted. It reports the mean
  and worst-block DSP load, single-threaded and with the pool.
- `editor_check` opens, resizes and closes a plug-in's editor without silencing its track
  (it shows the plug-in's window for about a second).
- `rate_change` changes the sample rate staged, as the app (and an undo) does: the call
  returns before the plug-in is back, which loads in an update of its own.
- `insert_check` copies a plug-in to another track, and copies and pastes it onto a third:
  the copy has to null against the original, and a moved plug-in has to play the same.
- `device_check` opens a device silently and reports what it ran with.
- `loopback_check` plays clicks out loud and records the input, to measure the recording
  round trip; it needs a cable from an output to an input (only run it when you mean to).
- `stretch_check` time-stretches and transposes a recording and checks length, level, onsets,
  pitch in cents and speed.
- `export_check` decodes every export format Oxide writes with ffmpeg (WAV, AIFF and FLAC bit
  for bit, MP3 and Ogg at exact length and level) and reads each format's tags with ffprobe.
  It is skipped without ffmpeg.
- `import_check` makes FLAC, AIFF and MP3 files with ffmpeg and decodes them back bit for bit
  (MP3 at exact length, no offset). It is skipped without ffmpeg.
- `loudness_check` compares the loudness meter with ffmpeg's `ebur128` and any EBU test files in
  `out/ebu`. It is skipped without ffmpeg.
- `denormal_check` times every built-in effect's silent tail against its noise at 64 and 512
  samples: the tail must be no dearer.
- `wave_draw_cost` lays out 40 lanes of a 10-minute stereo file at every zoom and fails over a
  quarter of a frame.
- `--screenshot` renders the UI, saves a PNG and exits. `--show browser|settings` opens a
  dialog first.
- Each example prints `RESULT: PASS` or `FAIL:` lines, and exits non-zero on failure.
