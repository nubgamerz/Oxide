//! MIDI clips on instrument tracks: notes and controller events in musical time. Positions are
//! MIDI ticks, 960 to a quarter note: a clip's from the start of the song, its events' from the
//! start of the clip. (Playback comes with dev plan 4.2: until then MIDI clips are silent.)

use super::{check_track, error};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::midi_analysis::{self, Spelling};
use crate::midi_clip::{Controller, ControllerEvent, EventId, MidiClip, Note, NoteEdge, NoteId};
use crate::project::{ClipId, TrackId};
use crate::session::Session;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "insert_midi_track",
        "Insert a MIDI track, as a time selection's \"Insert MIDI track\" does: a new \
         instrument track (armed) right after track `after` (and its sub-tracks; at the end \
         without it), holding an empty MIDI clip from `start` to `end` (samples; without `end`, \
         two bars from the bar `start` is in), or from bar `bar` (from 1) for `bars` bars \
         (default 2). `instrument` (an installed plug-in name or a \
         unique part of it, or a path; see list_plugins) is loaded on it and names the track; \
         if it fails to load, nothing is added. Returns the track, the insert and the clip.",
        Safety::Edit,
        insert_midi_track,
    );
    r.add(
        "add_midi_clip",
        "Add an empty MIDI clip to an instrument track and return it. Place it with `bar` (and \
         optionally `beat`, both from 1, following the tempo map's time signatures) or with \
         `start` in ticks (960 per quarter note, from the start of the song). Size it with \
         `bars` (with `bar`) or `length` in ticks; one bar by default. Clips may overlap. \
         MIDI clips stay on their beats when the tempo changes.",
        Safety::Edit,
        add_midi_clip,
    );
    r.add(
        "get_midi_clip",
        "Read a MIDI clip: start and length in ticks (960 per quarter note) and where they \
         fall in samples now, its notes (id, tick from the clip start, length in ticks, pitch \
         0-127 with 60 = middle C, velocity 1-127, channel 0-15) and controller events. Notes \
         past the clip's end are kept but don't play. describe_midi_clip reads the same notes \
         as music (bars and beats, names, key, chords).",
        Safety::Read,
        |host, input: ClipArg| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            Ok(view(s, input.clip))
        },
    );
    r.add(
        "describe_midi_clip",
        "Read a MIDI clip as music, to write a part that fits it: where it starts and ends \
         (`start`, `end` as \"bar:beat:tick\": bars and beats from 1 as the tempo map's time \
         signatures count them, then ticks into the beat, 960 per quarter note), the bars it \
         spans, the tempo (quarter notes per minute) and time signature where it plays and each \
         change inside it (`meters`), its estimated `key` and `runner_up` (Krumhansl-Kessler \
         profiles over the notes' lengths; `score` is the fit, -1 to 1, with the key \
         signature and scale), its `range` (lowest and highest note), its chords bar by bar \
         (`progression` like \"Am | F | C G\", and `chords` with each one's place, length in \
         ticks, root, quality, bass and tones; one per bar, or per half bar or beat where it \
         changes; `implied` when only some tones are heard, as in a bass line; \"N\" for none) \
         and every note as get_midi_clip gives it plus `at` (its song position, \
         \"bar:beat:tick\") and `name` (C4 = 60). Names use flats in a flat key unless \
         `spelling` is \"sharps\" or \"flats\". add_notes and set_notes take `at` and names \
         back. An estimate: check it by ear or against the other parts.",
        Safety::Read,
        |host, input: DescribeClip| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            let (track, clip) = s.project.find_midi_clip(input.clip).expect("checked");
            let map = s.tempo_map();
            Ok(Described {
                clip: input.clip,
                track,
                track_name: s.project.track(track).map(|t| t.name.clone()),
                name: clip.name.clone(),
                start_tick: clip.start,
                length: clip.length,
                reading: midi_analysis::describe(clip, &map, input.spelling.unwrap_or_default()),
            })
        },
    );
    r.add(
        "set_midi_clip",
        "Move, resize or rename a MIDI clip: `start` and `length` in ticks (960 per quarter \
         note). Only the fields given change. Shortening hides the notes past the new end \
         without deleting them.",
        Safety::Edit,
        set_midi_clip,
    );
    r.add(
        "remove_midi_clip",
        "Delete a MIDI clip with its notes and controller events.",
        Safety::Destructive,
        |host, input: ClipArg| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            s.project.remove_midi_clip(input.clip);
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
    r.add(
        "add_notes",
        "Add notes to a MIDI clip. Each note: where it starts, as `tick` (from the clip start; \
         960 per quarter note) or `at` (its song position \"bar:beat:tick\", bars and beats \
         from 1 as describe_midi_clip gives them, e.g. \"5:3:000\" or \"5:3\"; it must fall \
         in or after the clip's start), `length` in ticks (960 a quarter note, 480 an eighth), \
         `pitch` 0-127 (60 = middle C) or a name (\"C4\", \"Bb3\", \"F#2\"), `velocity` 1-127 \
         (default 100), `channel` 0-15 (default 0). All or nothing; an error names the note \
         (from 0) and the bad value. Returns the new notes' ids, in the order given.",
        Safety::Edit,
        add_notes,
    );
    r.add(
        "set_notes",
        "Change notes in a MIDI clip by id: `tick` (from the clip start, 960 per quarter \
         note) or `at` (song position \"bar:beat:tick\"), `length` in ticks, `pitch` (0-127 or \
         a name like \"Bb3\"), `velocity`, `channel` (only the fields given change). All or \
         nothing. Returns the changed notes.",
        Safety::Edit,
        set_notes,
    );
    r.add(
        "resize_notes",
        "Resize notes in a MIDI clip by one edge, as dragging a note's edge in the piano roll \
         does: `edge` \"start\" or \"end\" moves by `by` ticks (960 per quarter note; \
         negative is earlier) and the other edge stays. A note keeps at least 1 tick and never \
         starts before the clip's start. `notes` are note ids; all or nothing. Returns the \
         changed notes.",
        Safety::Edit,
        resize_notes,
    );
    r.add(
        "remove_notes",
        "Delete notes from a MIDI clip by id. All or nothing.",
        Safety::Destructive,
        |host, input: RemoveNotes| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            s.project
                .remove_notes(input.clip, &input.notes)
                .map_err(error)?;
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
    r.add(
        "copy_notes",
        "Copy notes of a MIDI clip (`clip`, `notes` as note ids) to the note clipboard, as          Ctrl+C in the piano roll does, replacing what it held: their distances from the          earliest one (in ticks, 960 per quarter note), pitch, length, velocity and channel.          The note clipboard is the session's, apart from the clip clipboard (copy_clips): it          isn't saved and isn't part of undo. All or nothing. Returns how many.",
        Safety::Edit,
        |host, input: RemoveNotes| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            let notes = s.copy_notes(input.clip, &input.notes)?;
            Ok(NoteCount { notes })
        },
    );
    r.add(
        "cut_notes",
        "Copy notes of a MIDI clip (`clip`, `notes` as note ids) to the note clipboard, as          copy_notes does, and delete them from the clip, as Ctrl+X in the piano roll does.          All or nothing. Returns how many.",
        Safety::Destructive,
        |host, input: RemoveNotes| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            let notes = s.cut_notes(input.clip, &input.notes)?;
            Ok(NoteCount { notes })
        },
    );
    r.add(
        "paste_notes",
        "Paste the note clipboard (copy_notes, cut_notes, or Ctrl+C in the piano roll) into          MIDI clip `clip`, as Ctrl+V in the piano roll does: the earliest copied note starts at          `position` (samples, at the tick nearest it; default: the playhead) or `at` (song          position \"bar:beat:tick\", bars and beats from 1), not snapped, and every other          note at its same distance from it and its same pitch, length, velocity and channel.          Refused when that point is before the clip's start. Notes reaching past the clip's          end lengthen it to the bar line after them. Copied notes may be pasted into any MIDI          clip. Returns the new notes' ids, in time order.",
        Safety::Edit,
        paste_notes,
    );
    r.add(
        "add_controller_events",
        "Add controller events to a MIDI clip. Each: `tick` (from the clip start), `channel` \
         0-15 (default 0) and `kind`: {\"type\":\"cc\",\"number\":1,\"value\":64} (control \
         change 0-127, e.g. 1 mod wheel, 64 sustain pedal), {\"type\":\"pitch_bend\",\
         \"value\":0} (-8192..8191) or {\"type\":\"program_change\",\"program\":0}. All or \
         nothing. Returns the new events' ids.",
        Safety::Edit,
        add_controller_events,
    );
    r.add(
        "read_midi_file",
        "Look inside a Standard MIDI file (absolute `path`) without importing it: its tracks \
         with notes (name, `import_as`: the name import_midi_file gives its track, note count, \
         length in ticks at 960 per quarter note), and the \
         tempo map it asks for (opening tempo and time signature, later changes in bars and \
         beats from the file's start), so you can decide on import_midi_file's `set_tempo`.",
        Safety::Read,
        |host, input: PathArg| {
            let _ = host;
            let file = crate::smf::read_path(&input.path)?;
            let stem = input
                .path
                .file_stem()
                .map_or_else(|| "MIDI".into(), |s| s.to_string_lossy().into_owned());
            let tracks: Vec<FileTrackView> = file
                .tracks
                .iter()
                .zip(file.track_names(&stem))
                .map(|(t, import_as)| FileTrackView {
                    name: t.name.clone(),
                    import_as,
                    notes: t.notes.len(),
                    controllers: t.controllers.len(),
                    length: t.end,
                })
                .collect();
            let tempo = file.tempo_settings().as_ref().map(FileTempoView::new);
            Ok(FileView { tracks, tempo })
        },
    );
    r.add(
        "import_midi_file",
        "Import a Standard MIDI file (absolute `path`, type 0 or 1) as MIDI clips, placed at \
         `bar` (and `beat`, from 1) or `start` in ticks (960 per quarter note); bar 1 by \
         default. Its first track with notes goes on `track` if that's an instrument track; \
         the others (and the first, otherwise) on new instrument tracks after it, named from \
         the file (tracks that share a name, like a piano's two staves, become \"Name \
         (treble)\" and \"Name (bass)\" by register, else \"Name 1\", \"Name 2\"). \
         `same_instrument` (default true) gives each new track its own copy of `track`'s \
         instrument (its first insert), with its settings. `set_tempo` (default false) first \
         takes the file's tempo and time \
         signatures as the project's (opening tempo from bar 1, later changes from the clip's \
         bar on), keeping the clips on the bar they were placed at. Returns the tracks and \
         clips made.",
        Safety::Edit,
        import_midi_file,
    );
    r.add(
        "export_midi_clip",
        "Save a MIDI clip as a Standard MIDI file (type 0, 960 ticks per quarter note) at \
         absolute `path`: the notes that play (cropped at the clip's end) and its controller \
         events, with the tempo and time signature at the clip's start. Overwrites `path`.",
        Safety::Destructive,
        |host, input: ExportClip| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            s.export_midi_clip(input.clip, &input.path)?;
            Ok(Done { done: true })
        },
    );
    r.add(
        "export_midi_track",
        "Save every MIDI clip of instrument track `track` as one Standard MIDI file (type 0, \
         960 ticks per quarter note) at absolute `path`, each clip at its place in the song \
         (tick 0 is the project's start), with the project's tempo and time signature \
         changes: the notes as programmed (cropped at each clip's end, as they play) and \
         controller events, nothing from plug-ins. For another program to import. Overwrites \
         `path`.",
        Safety::Destructive,
        |host, input: ExportTrack| {
            let s = host.session();
            check_track(s, input.track)?;
            s.export_midi_track(input.track, &input.path)?;
            Ok(Done { done: true })
        },
    );
    r.add(
        "quantize_notes",
        "Move a MIDI clip's notes toward the grid: each start goes `strength` (0-100 %, \
         default 100) of the way to its nearest line of `division` (\"1/16\", \"1/8t\", \
         \"beat\", \"bar\"…), exactly in ticks, following the time signature (lines restart \
         each bar). Lengths are kept. `notes` picks some by id; all by default.",
        Safety::Edit,
        |host, input: QuantizeNotes| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            let division: crate::grid::Division = input.division.parse().map_err(error)?;
            let strength = input.strength.unwrap_or(100.0);
            if !(0.0..=100.0).contains(&strength) {
                return Err(error(format!("strength {strength} is outside 0..100")));
            }
            s.quantize_notes(
                input.clip,
                input.notes.as_deref(),
                division,
                strength / 100.0,
            )?;
            Ok(view(s, input.clip))
        },
    );
    r.add(
        "preview_note",
        "Play a note on an instrument track's plug-in now, whether or not the track is armed: \
         `pitch` 0-127 (60 = middle C), `velocity` 1-127 (default 100), released after \
         `duration_ms` (default 500, up to 10000). Nothing is recorded or changed.",
        Safety::Transport,
        |host, input: PreviewNote| {
            let duration = input.duration_ms.unwrap_or(500);
            if duration == 0 || duration > 10_000 {
                return Err(error(format!("duration_ms {duration} is outside 1..10000")));
            }
            let s = host.session();
            check_track(s, input.track)?;
            s.preview_note(
                input.track,
                input.pitch,
                input.velocity.unwrap_or(100),
                Some(std::time::Duration::from_millis(duration)),
            )?;
            Ok(Done { done: true })
        },
    );
    r.add(
        "set_midi_record_options",
        "How recording on armed instrument tracks lands: `replace` (true: first delete what \
         the track has in the recorded range; false, the default: merge, overdubbing into a \
         clip that holds the range, or a new clip) and `quantize` (a grid division like \
         \"1/16\", \"1/8t\", \"beat\" to snap recorded note starts to, or \"off\"). Only the \
         fields given change. Saved with the project.",
        Safety::Edit,
        |host, input: RecordOptions| {
            let s = host.session();
            if let Some(q) = input.quantize.as_deref() {
                if q != "off" {
                    q.parse::<crate::grid::Division>().map_err(error)?;
                }
            }
            let settings = &mut s.project.midi_record;
            if let Some(replace) = input.replace {
                settings.replace = replace;
            }
            if let Some(q) = input.quantize {
                settings.quantize = (q != "off").then_some(q);
            }
            Ok(settings.clone())
        },
    );
    r.add(
        "set_chase_notes",
        "Chase notes (on by default): when playback starts, loops back or jumps into the \
         middle of a MIDI note, strike it there instead of waiting for the next note. Saved \
         with the project.",
        Safety::Edit,
        |host, input: ChaseArg| {
            let s = host.session();
            s.project.chase_notes = input.on;
            s.mark_dirty();
            Ok(ChaseArg::view(input.on))
        },
    );
    r.add(
        "remove_controller_events",
        "Delete controller events from a MIDI clip by id. All or nothing.",
        Safety::Destructive,
        |host, input: RemoveEvents| {
            let s = host.session();
            check_midi_clip(s, input.clip)?;
            s.project
                .remove_controller_events(input.clip, &input.events)
                .map_err(error)?;
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ChaseArg {
    on: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathArg {
    /// Absolute path of the file.
    path: std::path::PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct FileTrackView {
    name: Option<String>,
    /// The name import_midi_file gives its track (tracks that share a name are told apart).
    import_as: String,
    notes: usize,
    controllers: usize,
    /// Ticks (960 per quarter note) to the track's last event.
    length: u64,
}

#[derive(Serialize, JsonSchema)]
struct FileView {
    tracks: Vec<FileTrackView>,
    /// `null` when the file sets no tempo or time signature.
    tempo: Option<FileTempoView>,
}

#[derive(Serialize, JsonSchema)]
struct FileTempoView {
    /// Quarter notes per minute at the start.
    bpm: f64,
    time_signature: (u32, u32),
    /// Later changes, bars and beats from 1 counted from the file's start.
    changes: Vec<FileTempoChange>,
}

#[derive(Serialize, JsonSchema)]
struct FileTempoChange {
    bar: u32,
    beat: u32,
    bpm: Option<f64>,
    time_signature: Option<(u32, u32)>,
}

impl FileTempoView {
    fn new(settings: &crate::smf::TempoSettings) -> Self {
        Self {
            bpm: settings.tempo,
            time_signature: settings.time_signature,
            changes: settings
                .changes
                .iter()
                .map(|c| FileTempoChange {
                    bar: c.bar + 1,
                    beat: c.beat + 1,
                    bpm: c.tempo,
                    time_signature: c.time_signature,
                })
                .collect(),
        }
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImportMidi {
    /// Absolute path of the MIDI file.
    path: std::path::PathBuf,
    #[serde(default)]
    track: Option<TrackId>,
    /// Bar to place it at, from 1.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1.
    #[serde(default)]
    beat: Option<u32>,
    /// Ticks from the start of the song, instead of `bar`.
    #[serde(default)]
    start: Option<u64>,
    /// Take the file's tempo map as the project's.
    #[serde(default)]
    set_tempo: bool,
    /// Give each new track a copy of `track`'s instrument.
    #[serde(default = "default_same_instrument")]
    same_instrument: bool,
}

fn default_same_instrument() -> bool {
    true
}

#[derive(Serialize, JsonSchema)]
struct Imported {
    clips: Vec<ImportedClip>,
}

#[derive(Serialize, JsonSchema)]
struct ImportedClip {
    track: TrackId,
    clip: ClipId,
    notes: usize,
}

fn import_midi_file(host: &mut dyn Host, input: ImportMidi) -> ApiResult<Imported> {
    let s = host.session();
    if let Some(track) = input.track {
        check_track(s, track)?;
    }
    let map = s.tempo_map();
    let start = match (input.bar, input.start) {
        (Some(_), Some(_)) => return Err(error("give `bar` or `start`, not both")),
        (Some(0), _) => return Err(error("bars count from 1")),
        (Some(bar), None) => map.tick_of_bar(
            i64::from(bar) - 1,
            input.beat.unwrap_or(1).saturating_sub(1),
        ),
        (None, start) => start.unwrap_or(0),
    };
    let made = s.import_midi_file(
        &input.path,
        input.track,
        start,
        input.set_tempo,
        input.same_instrument,
    )?;
    s.mark_dirty();
    let clips = made
        .into_iter()
        .map(|(track, clip)| ImportedClip {
            track,
            clip,
            notes: s
                .project
                .find_midi_clip(clip)
                .map_or(0, |(_, c)| c.notes.len()),
        })
        .collect();
    Ok(Imported { clips })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportTrack {
    track: TrackId,
    /// Absolute path to write (.mid); overwritten.
    path: std::path::PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportClip {
    clip: ClipId,
    /// Absolute path to write (.mid); overwritten.
    path: std::path::PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QuantizeNotes {
    clip: ClipId,
    /// "1/16", "1/8t", "beat", "bar"…
    division: String,
    /// 0-100 %; 100 when omitted.
    #[serde(default)]
    strength: Option<f64>,
    /// Note ids; every note when omitted.
    #[serde(default)]
    notes: Option<Vec<NoteId>>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PreviewNote {
    track: TrackId,
    /// 0-127; 60 is middle C.
    pitch: u8,
    /// 1-127; 100 when omitted.
    #[serde(default)]
    velocity: Option<u8>,
    /// Milliseconds before the release; 500 when omitted.
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RecordOptions {
    #[serde(default)]
    replace: Option<bool>,
    /// A grid division ("1/16", "1/8t", "beat", "bar") or "off".
    #[serde(default)]
    quantize: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct ChaseView {
    chase_notes: bool,
}

impl ChaseArg {
    fn view(on: bool) -> ChaseView {
        ChaseView { chase_notes: on }
    }
}

fn check_midi_clip(s: &Session, id: ClipId) -> ApiResult<()> {
    if s.project.find_midi_clip(id).is_some() {
        return Ok(());
    }
    if s.project.find_clip(id).is_some() {
        return Err(error(format!(
            "clip {id} is an audio clip; this action works on MIDI clips"
        )));
    }
    Err(error(format!(
        "no MIDI clip with id {id} (see get_project)"
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClipArg {
    clip: ClipId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DescribeClip {
    clip: ClipId,
    /// "auto" (the default: flats in a flat key), "sharps" or "flats".
    #[serde(default)]
    spelling: Option<Spelling>,
}

#[derive(Serialize, JsonSchema)]
struct Described {
    clip: ClipId,
    track: TrackId,
    track_name: Option<String>,
    name: String,
    /// Ticks from the start of the song (960 per quarter note), and the length in ticks.
    start_tick: u64,
    length: u64,
    #[serde(flatten)]
    reading: midi_analysis::Description,
}

/// A pitch as a number (60 = middle C) or a name ("C4", "Bb3", "F#2").
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum Pitch {
    /// 0-127; 60 is middle C.
    Number(i64),
    /// A note name with its octave: "C4" (60), "A#3" or "Bb3" (58).
    Name(String),
}

impl Pitch {
    /// The note number, or why not, for note `i` of the call.
    fn resolve(&self, i: usize) -> ApiResult<u8> {
        match self {
            Self::Number(n) => u8::try_from(*n)
                .ok()
                .filter(|&p| p <= 127)
                .ok_or_else(|| error(format!("note {i}: pitch {n} is outside 0..127"))),
            Self::Name(name) => {
                midi_analysis::parse_note_name(name).map_err(|e| error(format!("note {i}: {e}")))
            }
        }
    }
}

/// Where note `i` starts in its clip, in ticks from the clip's start: `tick` as given, or
/// `at` ("bar:beat:tick" in the song) through the tempo map. `None` when neither is given.
fn note_tick(
    map: &crate::tempo::TempoMap,
    clip: &MidiClip,
    i: usize,
    tick: Option<u64>,
    at: Option<&str>,
) -> ApiResult<Option<u64>> {
    match (tick, at) {
        (Some(_), Some(_)) => Err(error(format!(
            "note {i}: give `tick` (from the clip's start) or `at` (\"bar:beat:tick\" in the \
             song), not both"
        ))),
        (Some(tick), None) => Ok(Some(tick)),
        (None, None) => Ok(None),
        (None, Some(at)) => {
            let song = midi_analysis::parse_position(map, at)
                .map_err(|e| error(format!("note {i}: {e}")))?;
            let tick = song.checked_sub(clip.start).ok_or_else(|| {
                error(format!(
                    "note {i}: \"{at}\" is before the clip, which starts at {}",
                    midi_analysis::format_position(map, clip.start)
                ))
            })?;
            Ok(Some(tick))
        }
    }
}

/// A MIDI clip as `get_project` lists it.
#[derive(Serialize, JsonSchema)]
pub(super) struct MidiClipSummary {
    id: ClipId,
    name: String,
    /// Ticks from the start of the song (960 per quarter note).
    start: u64,
    length: u64,
    notes: usize,
    controllers: usize,
}

impl MidiClipSummary {
    pub(super) fn new(clip: &MidiClip) -> Self {
        Self {
            id: clip.id,
            name: clip.name.clone(),
            start: clip.start,
            length: clip.length,
            notes: clip.notes.len(),
            controllers: clip.controllers.len(),
        }
    }
}

#[derive(Serialize, JsonSchema)]
pub(super) struct MidiClipView {
    clip: ClipId,
    track: TrackId,
    name: String,
    start: u64,
    length: u64,
    /// Where the clip falls now, in samples (it follows the tempo map).
    start_sample: u64,
    end_sample: u64,
    notes: Vec<Note>,
    controllers: Vec<ControllerEvent>,
}

pub(super) fn view(s: &Session, id: ClipId) -> MidiClipView {
    let (track, clip) = s.project.find_midi_clip(id).expect("checked by the caller");
    let map = s.tempo_map();
    MidiClipView {
        clip: id,
        track,
        name: clip.name.clone(),
        start: clip.start,
        length: clip.length,
        start_sample: map.sample_at_tick(clip.start as f64).round() as u64,
        end_sample: map.sample_at_tick(clip.end() as f64).round() as u64,
        notes: clip.notes.clone(),
        controllers: clip.controllers.clone(),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddMidiClip {
    track: TrackId,
    /// Bar to start at, from 1.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
    /// Start in ticks from the start of the song, instead of `bar`.
    #[serde(default)]
    start: Option<u64>,
    /// Length in bars (with `bar`).
    #[serde(default)]
    bars: Option<u32>,
    /// Length in ticks, instead of `bars`.
    #[serde(default)]
    length: Option<u64>,
    #[serde(default)]
    name: Option<String>,
}

fn add_midi_clip(host: &mut dyn Host, input: AddMidiClip) -> ApiResult<MidiClipView> {
    let s = host.session();
    check_track(s, input.track)?;
    let map = s.tempo_map();
    let bar = match (input.bar, input.start) {
        (Some(_), Some(_)) => return Err(error("give `bar` or `start`, not both")),
        (Some(0), _) => return Err(error("bars count from 1")),
        (Some(bar), None) => Some(i64::from(bar) - 1),
        (None, _) => None,
    };
    let start = match (bar, input.start) {
        (Some(bar), _) => {
            let beat = input.beat.unwrap_or(1);
            let beats_in_bar = map.segment_at(map.bar_start(bar)).numerator;
            if beat == 0 || beat > beats_in_bar {
                return Err(error(format!(
                    "bar {} has beats 1 to {beats_in_bar}; beat {beat} is not one of them",
                    bar + 1
                )));
            }
            map.tick_of_bar(bar, beat - 1)
        }
        (None, start) => {
            if input.beat.is_some() {
                return Err(error("`beat` goes with `bar`"));
            }
            start.unwrap_or(0)
        }
    };
    let bar_ticks = |bar: i64, bars: i64| {
        map.tick_of_bar(bar + bars, 0)
            .saturating_sub(map.tick_of_bar(bar, 0))
    };
    let length = match (input.length, input.bars, bar) {
        (Some(_), Some(_), _) => return Err(error("give `bars` or `length`, not both")),
        (Some(length), None, _) => length,
        (None, Some(_), None) => {
            return Err(error(
                "`bars` goes with `bar`; give `length` in ticks instead",
            ))
        }
        (None, bars, Some(bar)) => bar_ticks(bar, i64::from(bars.unwrap_or(1))),
        // One bar: the length of the bar the clip starts in.
        (None, None, None) => {
            let (bar, _) = map.bar_at(map.sample_at_tick(start as f64));
            bar_ticks(bar, 1)
        }
    };
    let id = s
        .project
        .add_midi_clip(input.track, start, length)
        .map_err(error)?;
    if let Some(name) = input.name.filter(|n| !n.trim().is_empty()) {
        s.project.find_midi_clip_mut(id).expect("just added").name = name.trim().into();
    }
    s.mark_dirty();
    Ok(view(s, id))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InsertMidiTrack {
    /// Where the clip starts, samples; or give `bar`.
    #[serde(default)]
    start: Option<u64>,
    /// Where it ends, samples (with `start`); omit for two bars.
    #[serde(default)]
    end: Option<u64>,
    /// The bar the clip starts on, from 1, instead of `start`.
    #[serde(default)]
    bar: Option<u32>,
    /// Its length in bars (with `bar`); 2 when omitted.
    #[serde(default)]
    bars: Option<u32>,
    /// The track to insert after; omit for the end.
    #[serde(default)]
    after: Option<TrackId>,
    /// An instrument to load: installed plug-in name (or a unique part of it), or a path.
    #[serde(default)]
    instrument: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct InsertedMidiTrack {
    track: TrackId,
    insert: Option<crate::project::InsertId>,
    clip: MidiClipView,
}

fn insert_midi_track(host: &mut dyn Host, input: InsertMidiTrack) -> ApiResult<InsertedMidiTrack> {
    let s = host.session();
    if let Some(after) = input.after {
        check_track(s, after)?;
    }
    let instrument = input
        .instrument
        .map(|name| crate::plugins::resolve(&name, s.installed_plugins()))
        .transpose()?;
    let after = input.after;
    let (track, clip, insert) = match (input.start, input.bar) {
        (Some(_), Some(_)) => return Err(error("give `start` (samples) or `bar`, not both")),
        (None, None) => return Err(error("give `start` (samples) or `bar` (from 1)")),
        (Some(start), None) => {
            if input.bars.is_some() {
                return Err(error(
                    "`bars` goes with `bar`; give `end` in samples instead",
                ));
            }
            s.insert_midi_track(start, input.end, after, instrument.as_deref())?
        }
        (None, Some(bar)) => {
            if input.end.is_some() {
                return Err(error("`end` goes with `start`; give `bars` instead"));
            }
            let bars = input.bars.unwrap_or(2);
            if bar == 0 || bars == 0 {
                return Err(error(format!(
                    "bar {bar}, bars {bars}: bars count from 1 and a clip lasts at least one"
                )));
            }
            let map = s.tempo_map();
            let first = i64::from(bar) - 1;
            let (from, to) = (
                map.tick_of_bar(first, 0),
                map.tick_of_bar(first + i64::from(bars), 0),
            );
            s.insert_midi_track_ticks(from, to, after, instrument.as_deref())?
        }
    };
    Ok(InsertedMidiTrack {
        track,
        insert,
        clip: view(s, clip),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMidiClip {
    clip: ClipId,
    /// Ticks from the start of the song.
    #[serde(default)]
    start: Option<u64>,
    /// In ticks, at least 1.
    #[serde(default)]
    length: Option<u64>,
    #[serde(default)]
    name: Option<String>,
}

fn set_midi_clip(host: &mut dyn Host, input: SetMidiClip) -> ApiResult<MidiClipView> {
    let s = host.session();
    check_midi_clip(s, input.clip)?;
    if input.length == Some(0) {
        return Err(error("a MIDI clip's length must be at least 1 tick"));
    }
    if input.name.as_ref().is_some_and(|n| n.trim().is_empty()) {
        return Err(error("a clip name cannot be empty"));
    }
    let clip = s.project.find_midi_clip_mut(input.clip).expect("checked");
    if let Some(start) = input.start {
        clip.start = start;
    }
    if let Some(length) = input.length {
        clip.length = length;
    }
    if let Some(name) = input.name {
        clip.name = name.trim().into();
    }
    s.mark_dirty();
    Ok(view(s, input.clip))
}

fn default_velocity() -> u8 {
    100
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NewNote {
    /// Ticks from the clip start (960 per quarter note); or give `at`.
    #[serde(default)]
    tick: Option<u64>,
    /// The song position "bar:beat:tick" (from 1; describe_midi_clip's form), instead of
    /// `tick`.
    #[serde(default)]
    at: Option<String>,
    /// In ticks, at least 1.
    length: u64,
    /// 0-127 (60 is middle C) or a name: "C4", "Bb3", "F#2".
    pitch: Pitch,
    /// 1-127.
    #[serde(default = "default_velocity")]
    velocity: u8,
    /// 0-15.
    #[serde(default)]
    channel: u8,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddNotes {
    clip: ClipId,
    notes: Vec<NewNote>,
}

#[derive(Serialize, JsonSchema)]
struct NoteIds {
    notes: Vec<NoteId>,
}

fn add_notes(host: &mut dyn Host, input: AddNotes) -> ApiResult<NoteIds> {
    let s = host.session();
    check_midi_clip(s, input.clip)?;
    let map = s.tempo_map();
    let clip = &s.project.find_midi_clip(input.clip).expect("checked").1;
    let notes = input
        .notes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let tick = note_tick(&map, clip, i, n.tick, n.at.as_deref())?.ok_or_else(|| {
                error(format!(
                    "note {i}: give `tick` (from the clip's start) or `at` (\"bar:beat:tick\")"
                ))
            })?;
            Ok(Note {
                id: 0,
                tick,
                length: n.length,
                pitch: n.pitch.resolve(i)?,
                velocity: n.velocity,
                channel: n.channel,
            })
        })
        .collect::<ApiResult<Vec<Note>>>()?;
    let ids = s.project.add_notes(input.clip, &notes).map_err(error)?;
    s.mark_dirty();
    Ok(NoteIds { notes: ids })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoteChange {
    note: NoteId,
    /// Ticks from the clip start.
    #[serde(default)]
    tick: Option<u64>,
    /// The song position "bar:beat:tick", instead of `tick`.
    #[serde(default)]
    at: Option<String>,
    #[serde(default)]
    length: Option<u64>,
    /// 0-127 or a name ("C4", "Bb3").
    #[serde(default)]
    pitch: Option<Pitch>,
    #[serde(default)]
    velocity: Option<u8>,
    #[serde(default)]
    channel: Option<u8>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetNotes {
    clip: ClipId,
    notes: Vec<NoteChange>,
}

#[derive(Serialize, JsonSchema)]
struct ChangedNotes {
    notes: Vec<Note>,
}

fn set_notes(host: &mut dyn Host, input: SetNotes) -> ApiResult<ChangedNotes> {
    let s = host.session();
    check_midi_clip(s, input.clip)?;
    let mut clip = s
        .project
        .find_midi_clip(input.clip)
        .expect("checked")
        .1
        .clone();
    let map = s.tempo_map();
    for (i, change) in input.notes.iter().enumerate() {
        let tick = note_tick(&map, &clip, i, change.tick, change.at.as_deref())?;
        let pitch = change.pitch.as_ref().map(|p| p.resolve(i)).transpose()?;
        let note = clip.note_mut(change.note).ok_or_else(|| {
            error(format!(
                "MIDI clip {} has no note with id {}",
                input.clip, change.note
            ))
        })?;
        note.tick = tick.unwrap_or(note.tick);
        note.length = change.length.unwrap_or(note.length);
        note.pitch = pitch.unwrap_or(note.pitch);
        note.velocity = change.velocity.unwrap_or(note.velocity);
        note.channel = change.channel.unwrap_or(note.channel);
    }
    let changed: Vec<Note> = clip
        .notes
        .iter()
        .filter(|n| input.notes.iter().any(|c| c.note == n.id))
        .copied()
        .collect();
    s.project.set_notes(input.clip, clip.notes).map_err(error)?;
    s.mark_dirty();
    Ok(ChangedNotes { notes: changed })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ResizeNotes {
    clip: ClipId,
    /// Note ids.
    notes: Vec<NoteId>,
    /// Which edge moves: "start" or "end".
    edge: NoteEdge,
    /// Ticks to move it by; negative is earlier.
    by: i64,
}

fn resize_notes(host: &mut dyn Host, input: ResizeNotes) -> ApiResult<ChangedNotes> {
    let s = host.session();
    check_midi_clip(s, input.clip)?;
    let clip = &s.project.find_midi_clip(input.clip).expect("checked").1;
    if let Some(missing) = input
        .notes
        .iter()
        .find(|id| !clip.notes.iter().any(|n| n.id == **id))
    {
        return Err(error(format!(
            "MIDI clip {} has no note with id {missing}",
            input.clip
        )));
    }
    let ids = input.notes.iter().copied().collect();
    let notes = crate::midi_clip::resize_notes(&clip.notes, &ids, input.edge, input.by, 1);
    let changed = notes
        .iter()
        .filter(|n| ids.contains(&n.id))
        .copied()
        .collect();
    s.project.set_notes(input.clip, notes).map_err(error)?;
    s.mark_dirty();
    Ok(ChangedNotes { notes: changed })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveNotes {
    clip: ClipId,
    notes: Vec<NoteId>,
}

#[derive(Serialize, JsonSchema)]
struct NoteCount {
    /// How many notes were copied (or cut).
    notes: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PasteNotes {
    clip: ClipId,
    /// Where the earliest copied note starts: a song position in samples (the tick nearest
    /// it). Default: the playhead.
    #[serde(default)]
    position: Option<u64>,
    /// Or the song position "bar:beat:tick" (from 1; describe_midi_clip's form), instead of
    /// `position`.
    #[serde(default)]
    at: Option<String>,
}

fn paste_notes(host: &mut dyn Host, input: PasteNotes) -> ApiResult<NoteIds> {
    let s = host.session();
    check_midi_clip(s, input.clip)?;
    let tick = match (input.position, input.at.as_deref()) {
        (Some(_), Some(_)) => {
            return Err(error(
                "give `position` (samples) or `at` (\"bar:beat:tick\"), not both",
            ))
        }
        (_, Some(at)) => midi_analysis::parse_position(&s.tempo_map(), at).map_err(error)?,
        (position, None) => s.tick_at_sample(position.unwrap_or_else(|| s.position())),
    };
    let notes = s.paste_notes_at_tick(input.clip, tick)?;
    Ok(NoteIds { notes })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NewEvent {
    /// Ticks from the clip start.
    tick: u64,
    /// 0-15.
    #[serde(default)]
    channel: u8,
    kind: Controller,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddEvents {
    clip: ClipId,
    events: Vec<NewEvent>,
}

#[derive(Serialize, JsonSchema)]
struct EventIds {
    events: Vec<EventId>,
}

fn add_controller_events(host: &mut dyn Host, input: AddEvents) -> ApiResult<EventIds> {
    let s = host.session();
    check_midi_clip(s, input.clip)?;
    let events: Vec<ControllerEvent> = input
        .events
        .iter()
        .map(|e| ControllerEvent {
            id: 0,
            tick: e.tick,
            channel: e.channel,
            kind: e.kind,
        })
        .collect();
    let ids = s
        .project
        .add_controller_events(input.clip, &events)
        .map_err(error)?;
    s.mark_dirty();
    Ok(EventIds { events: ids })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemoveEvents {
    clip: ClipId,
    events: Vec<EventId>,
}
