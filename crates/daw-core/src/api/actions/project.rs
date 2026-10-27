//! The project as a whole: reading it, files, tempo and meter.

use super::{error, NoArgs};
use crate::api::{ActionOutput, ApiError, ApiResult, Host, Registry, Safety};
use crate::grid::Grid;
use crate::project::{InsertSlot, Mixer, TempoChange, TrackId, TrackInput, TrackKind, TrackOutput};
use crate::session::{check_tempo, check_time_signature, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "get_project",
        "Describe the open project: tracks (with ids, kind, folder parent, input, output, \
         mixer settings, inserts, clips and automation lanes), the master bus, the opening tempo and time \
         signature (changes: get_tempo_map) and sample rate. Positions and lengths are in samples at `sample_rate`. Call this first to learn \
         the ids other actions take.",
        Safety::Read,
        get_project,
    );
    r.add_deferred(
        "new_project",
        "Close the open project and start a new one. Unsaved changes are lost (save_project \
         first). With `name` and `folder` (an absolute path), the project gets a folder of \
         its own, `folder/<name>/`, holding `<name>.oxide` and the folders its files go in \
         (Recorded, Imported, Rendered, Exported, Backups, Temp), and is saved there at once; \
         refused if that folder exists and isn't empty. Without them it is an unsaved, empty \
         project, which can't record or import files until it is saved into a folder; in the \
         app it is no project at all (the start screen), and changes to it are refused until \
         save_project gives it a path, so give `name` and `folder`. With \
         `template` (a name from list_templates; needs `name` and `folder`) it starts as that \
         template's setup: its tracks, buses, routing and plug-ins with their settings, no \
         clips. Returns the project file's path.",
        Safety::Destructive,
        |host, input: NewProject, reply| match (input.name, input.folder) {
            (Some(name), Some(folder)) => {
                let made = host
                    .session()
                    .new_project_in(&name, &folder, input.template.as_deref());
                match made {
                    // A template's plug-ins load as an open's do: answered once they are in.
                    Ok(path) => host
                        .when_opened(reply, Box::new(move |_| output(Saved { path: Some(path) }))),
                    Err(e) => reply(Err(e.into())),
                }
            }
            (None, None) if input.template.is_some() => reply(Err(error(
                "a project from a template needs `name` and `folder` to be made in",
            ))),
            (None, None) => {
                host.session().new_project();
                reply(output(Saved { path: None }));
            }
            _ => reply(Err(error("give both `name` and `folder`, or neither"))),
        },
    );
    r.returns::<Saved>();
    r.add_deferred(
        "open_project",
        "Open a project file (.oxide, or an older .dawproj; absolute `path`), replacing the \
         open project (unsaved changes are lost). Plug-ins reload with their saved state, which \
         can take a few seconds: the call answers once the project is in (the app shows its \
         loading bar meanwhile; get_busy reports the progress, and if the open is cancelled \
         (cancel_open, or Cancel on the bar) or replaced by another, this call answers with \
         an error). `newer_backup` names a backup written after the file was last \
         saved (Oxide closed without saving, or crashed): open_backup opens it instead.",
        Safety::Destructive,
        |host, input: PathArg, reply| {
            if let Err(e) = host.session().open(&input.path) {
                return reply(Err(e.into()));
            }
            // An agent gets the project whole: answered once its plug-ins are all in (in the
            // app, behind the loading bar, as File > Open). Its newer backup is known then.
            host.when_opened(
                reply,
                Box::new(|s| {
                    output(Opened {
                        path: s.path.clone(),
                        newer_backup: s.newer_backup().map(Path::to_path_buf),
                    })
                }),
            );
        },
    );
    r.returns::<Opened>();
    r.add(
        "list_backups",
        "The open project's backups in its Backups folder, newest first: `path`, `modified` \
         (seconds since 1970), `age_seconds` and `size` in bytes. Oxide backs a project up \
         every few minutes while it has unsaved changes and the transport is stopped, keeping \
         the newest (both set in the app's settings); the project file itself is only written \
         by save_project.",
        Safety::Read,
        |host, _: NoArgs| Ok(backup_list(host.session())),
    );
    r.add(
        "save_backup",
        "Back the open (saved) project up now into its Backups folder, as autosave does, \
         keeping the newest backups; the project file is untouched. Returns the backup's path.",
        Safety::Edit,
        |host, _: NoArgs| {
            let path = host.session().save_backup()?;
            Ok(Saved { path: Some(path) })
        },
    );
    r.add_deferred(
        "open_backup",
        "Open one of the open project's backups (absolute `path`, from list_backups or \
         open_project's `newer_backup`) in its place: it becomes the open project, unsaved \
         until save_project writes it over the project file. Unsaved changes are lost.",
        Safety::Destructive,
        |host, input: PathArg, reply| {
            if let Err(e) = host.session().open_backup(&input.path) {
                return reply(Err(e.into()));
            }
            host.when_opened(
                reply,
                Box::new(|s| {
                    output(Saved {
                        path: s.path.clone(),
                    })
                }),
            );
        },
    );
    r.returns::<Saved>();
    r.add(
        "get_busy",
        "What Oxide is busy with, for an agent to wait on: `loading` (a project being opened \
         in the app, in stages: `stage` is `closing` (the last project's plug-ins destroyed), \
         `reading` (its audio files read on workers) or `loading` (its plug-ins loaded a few a \
         frame); `done` of `total` over the whole open, `stage_done` of `stage_total` within \
         the stage; `next`, the plug-in or file it is on, and `track`, the loading plug-in's \
         track; cancel_open stops it; `kind` says what the plug-ins load for: `open` (a \
         project), `rack` (only the Waveform view's effects rack, the last run's or a preset, \
         which leaves the project open; `rack` is then true too), `undo` or `redo` (a step \
         that brought plug-ins back; `label` is its name; the project plays as before the step \
         until they are in) or `sample_rate` (every plug-in reloading at a new rate); only an \
         open can be cancelled), `saving` (the project file a save is writing in the \
         background), `save` (how far it has come: `done` of `total` plug-in states read, \
         `next` and `track` the plug-in read next, `writing` once the file is being written; \
         also while the app looks for unsaved changes before closing, with `saving` null) \
         and `exporting` (an export or render running; get_export_status has its \
         progress). All null/false when idle. open_project and the other actions wait for \
         their own work. While the app is opening or saving a project, every other call waits \
         until it is done (except get_busy, cancel_open and screenshot, which answer at once), \
         so a call never sees a project half loaded or changes one being saved.",
        Safety::Read,
        |host, _: NoArgs| {
            let s = host.session();
            Ok(Busy {
                loading: s.loading().map(|l| LoadingView {
                    stage: l.stage,
                    done: l.done,
                    total: l.total,
                    stage_done: l.stage_done,
                    stage_total: l.stage_total,
                    next: l.next,
                    track: l.track,
                    rack: l.kind == crate::session::LoadKind::Rack,
                    kind: l.kind,
                    label: l.label,
                }),
                saving: s.saving().map(Path::to_path_buf),
                save: s.save_progress().map(|p| SaveView {
                    done: p.done,
                    total: p.total,
                    next: p.next,
                    track: p.track,
                    writing: p.writing,
                }),
                exporting: matches!(
                    s.export_status(),
                    crate::session::ExportStatus::Running { .. }
                ),
            })
        },
    );
    r.add_deferred(
        "cancel_open",
        "Stop opening the project the app is opening (get_busy's `loading`) and go back to \
         the project open before it, reopened from its file with its plug-ins (changes not \
         saved there were given up when the open was asked for), or, with none before, to no \
         project (the start screen). Returns `reopened`: that project's path, or null, once it \
         is back whole; the open_project call it cancelled answers with an error. Refused \
         when nothing is being opened. A plug-in already loading finishes first.",
        Safety::Destructive,
        |host, _: NoArgs, reply| match host.session().cancel_loading() {
            // As open_project: answered once the project reopened is back whole.
            Ok(reopened) => host.when_opened(reply, Box::new(|_| output(Cancelled { reopened }))),
            Err(e) => reply(Err(e.into())),
        },
    );
    r.returns::<Cancelled>();
    r.add(
        "get_unsaved_changes",
        "What closing the project now would lose, as the app's \"Save the changes?\" question sees it: `project` is true when the project differs from its file as last saved or opened (every plug-in's state read now; an edit undone again is no change, a plug-in turned in its editor is one; a backup opened in its place always is), and `files` lists the Waveform view's files with unsaved edits (`document` ids and paths). False while a project's plug-ins are still loading.",
        Safety::Read,
        |host, _: NoArgs| {
            let s = host.session();
            Ok(Unsaved {
                project: s.has_unsaved_changes(),
                files: s
                    .documents()
                    .iter()
                    .filter(|d| d.is_dirty())
                    .map(|d| UnsavedFile {
                        document: d.id,
                        path: d.path.clone(),
                    })
                    .collect(),
            })
        },
    );
    r.add_deferred(
        "save_project",
        "Save the project, with every plug-in's state. Without `path` it saves to the file it \
         was opened from or last saved to; give an absolute path (.oxide) the first time or to \
         save a copy elsewhere: saved into another folder, the project's files (recordings, \
         imported copies, renders) are copied along to the same places inside it, so the new \
         folder is complete. Overwrites that file. In the app the save runs behind its busy \
         bar (each plug-in's state read in a frame of its own, the file written on a worker; \
         get_busy reports it) and the call answers once the file is written; other calls wait \
         meanwhile. Refused while another save is being written or plug-ins are loading.",
        Safety::Destructive,
        save_project,
    );
    r.returns::<Saved>();
    r.add_deferred(
        "undo",
        "Undo the last edit: every action that changes the project (tracks, clips, takes, \
         routing, mixer, inserts, tempo map, loop, punch, metronome) is one undo step, \
         recorded only if it changed something; repeated set_ calls on the same object in \
         quick succession are one step, and a recording is one step. Plug-in parameter \
         changes are not undo steps. A step that brings plug-ins back (a removed insert, a \
         replaced plug-in) loads them a plug-in a frame behind the app's busy bar (get_busy's \
         `kind` is `undo`); the call answers once they are in. Returns what was undone and the \
         history. Refused while recording, while a project opens and while an export or render \
         runs.",
        Safety::Edit,
        |host, _: NoArgs, reply| match host.session().undo() {
            Ok(undone) => host.when_opened(
                reply,
                Box::new(move |s| output(HistoryView::of(s, Some(undone)))),
            ),
            Err(e) => reply(Err(e.into())),
        },
    );
    r.returns::<HistoryView>();
    r.add_deferred(
        "redo",
        "Redo the last undone edit. Any new edit after an undo clears what could be redone. \
         Plug-ins it brings back load as undo's do, and the call answers once they are in. \
         Returns what was redone and the history.",
        Safety::Edit,
        |host, _: NoArgs, reply| match host.session().redo() {
            Ok(redone) => host.when_opened(
                reply,
                Box::new(move |s| output(HistoryView::of(s, Some(redone)))),
            ),
            Err(e) => reply(Err(e.into())),
        },
    );
    r.returns::<HistoryView>();
    r.add(
        "get_history",
        "The undo history: what undo would undo (`undo`, most recent first) and what redo \
         would redo (`redo`), with the history's estimated memory use. Opening or starting a \
         project, or changing the sample rate, starts it afresh.",
        Safety::Read,
        |host, _: NoArgs| Ok(HistoryView::of(host.session(), None)),
    );
    r.add(
        "set_tempo",
        "Set the opening tempo, in quarter notes per minute (20 to 300), which holds until the \
         first tempo change (see get_tempo_map). Moves the bar/beat grid and MIDI clips; audio \
         clips stay where they are in time unless they follow the tempo (set_clip \
         `source_tempo`), which move and stretch with it.",
        Safety::Edit,
        |host, input: Tempo| {
            check_tempo(input.bpm)?;
            host.session().project.tempo = input.bpm;
            Ok(tempo_map(host.session()))
        },
    );
    r.add(
        "set_time_signature",
        "Set the opening time signature, e.g. 4/4 or 7/8 (`numerator` 1 to 16; \
         `denominator` 2, 4, 8 or 16), which holds until the first time-signature change.",
        Safety::Edit,
        |host, input: TimeSignature| {
            check_time_signature(input.numerator, input.denominator)?;
            host.session().project.time_signature = (input.numerator, input.denominator);
            Ok(tempo_map(host.session()))
        },
    );
    r.add(
        "set_grid",
        "Set the song's grid division (`grid`), saved with the project: \"1/4\", \"1/8\" \
         (a new project's), \"1/16\", \"1/32\" or \"1/64\" of a whole note (1/4 is a beat in \
         4/4, 1/8 a beat in 6/8), each with a \"t\" for triplets (\"1/8t\"), or \"auto\": the \
         division follows each view's zoom (the finest of 1/64 to 1/4 whose lines are at \
         least 14 points apart, then the beat or every 1, 2, 4… bars; get_view's \
         `grid_division` is the one the Multitrack shows now). Lines run from each bar's \
         start, so in a meter whose bar isn't a whole number of divisions (7/8 at 1/4) the \
         bar's last one is shorter. The Multitrack draws its lines and, with snap on \
         (set_view `snap`), snaps the playhead, clips, selections, markers and automation \
         points to them; the piano roll and the Waveform view's bars and beats show the same \
         grid. Returns the grid.",
        Safety::Edit,
        |host, input: GridArg| {
            let grid: Grid = input.grid.parse().map_err(error)?;
            host.session().set_grid(grid)?;
            Ok(serde_json::json!({ "grid": host.session().project.grid }))
        },
    );
    r.add(
        "get_tempo_map",
        "The tempo map: the opening tempo (quarter notes per minute) and time signature, and \
         every change after it with its bar and beat (from 1) and where it falls in samples \
         (`position`) and ticks (`tick`, 960 per quarter note, as MIDI clips count them). \
         Each change sets a tempo, a time signature, or both, from there on.",
        Safety::Read,
        |host, _: NoArgs| Ok(tempo_map(host.session())),
    );
    r.add(
        "add_tempo_change",
        "Change the tempo (`bpm`, quarter notes per minute) and/or time signature \
         (`numerator`, `denominator`) from bar `bar` (from 2; bar 1 is the opening, see \
         set_tempo) and beat `beat` (from 1, default 1). A time signature changes only on \
         beat 1. Changes sit in bars and beats, so they stay on their bar when an earlier \
         tempo changes; changes are instant (no ramps). A change already at that place is \
         updated. Audio clips stay where they are in time (unless they follow the tempo: \
         set_clip `source_tempo`); MIDI clips stay on their beats.",
        Safety::Edit,
        |host, input: AddTempoChange| {
            let both = (input.numerator, input.denominator);
            let time_signature = match both {
                (Some(n), Some(d)) => Some((n, d)),
                (None, None) => None,
                _ => return Err(error("give both numerator and denominator, or neither")),
            };
            let s = host.session();
            s.add_tempo_change(TempoChange {
                bar: from_one(input.bar, "bar")?,
                beat: from_one(input.beat.unwrap_or(1), "beat")?,
                tempo: input.bpm,
                time_signature,
            })?;
            Ok(tempo_map(s))
        },
    );
    r.add(
        "remove_tempo_change",
        "Remove the tempo/time-signature change at `bar` and `beat` (from 1; beat defaults to \
         1).",
        Safety::Edit,
        |host, input: BarBeat| {
            let s = host.session();
            s.remove_tempo_change(
                from_one(input.bar, "bar")?,
                from_one(input.beat.unwrap_or(1), "beat")?,
            )?;
            Ok(tempo_map(s))
        },
    );
}

#[derive(Serialize, JsonSchema)]
struct HistoryView {
    /// The step just undone or redone.
    #[serde(skip_serializing_if = "Option::is_none")]
    done: Option<String>,
    undo: Vec<String>,
    redo: Vec<String>,
    bytes: usize,
}

impl HistoryView {
    fn of(s: &Session, done: Option<String>) -> Self {
        let (undo, redo, bytes) = s.history();
        Self {
            done,
            undo,
            redo,
            bytes,
        }
    }
}

/// Bars and beats are numbered from 1 for agents (as on screen), from 0 inside.
fn from_one(n: u32, what: &str) -> ApiResult<u32> {
    n.checked_sub(1)
        .ok_or_else(|| error(format!("{what} numbers start at 1")))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddTempoChange {
    /// Bar, from 2.
    bar: u32,
    /// Beat within the bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
    /// Quarter notes per minute, 20 to 300.
    #[serde(default)]
    bpm: Option<f64>,
    #[serde(default)]
    numerator: Option<u32>,
    #[serde(default)]
    denominator: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct BarBeat {
    bar: u32,
    #[serde(default)]
    beat: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
struct TempoMapView {
    /// Quarter notes per minute.
    tempo: f64,
    time_signature: (u32, u32),
    changes: Vec<TempoChangeView>,
}

#[derive(Serialize, JsonSchema)]
struct TempoChangeView {
    /// From 1.
    bar: u32,
    beat: u32,
    /// Sample position.
    position: u64,
    /// Ticks from the song's start (960 per quarter note), as MIDI clips count them.
    tick: u64,
    tempo: Option<f64>,
    time_signature: Option<(u32, u32)>,
}

fn tempo_map(s: &Session) -> TempoMapView {
    let map = s.tempo_map();
    TempoMapView {
        tempo: s.project.tempo,
        time_signature: s.project.time_signature,
        changes: s
            .project
            .tempo_changes
            .iter()
            .map(|c| {
                let bar = map.bar_start(i64::from(c.bar));
                let at = map.sample_of_beat(map.beats_at(bar) + f64::from(c.beat));
                TempoChangeView {
                    bar: c.bar + 1,
                    beat: c.beat + 1,
                    position: at.round().max(0.0) as u64,
                    tick: map.tick_of_bar(i64::from(c.bar), c.beat),
                    tempo: c.tempo,
                    time_signature: c.time_signature,
                }
            })
            .collect(),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathArg {
    /// Path of the .dawproj file.
    path: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveArgs {
    /// Where to save (.dawproj); defaults to the project's current file.
    #[serde(default)]
    path: Option<PathBuf>,
}

#[derive(Serialize, JsonSchema)]
struct Busy {
    loading: Option<LoadingView>,
    saving: Option<PathBuf>,
    save: Option<SaveView>,
    exporting: bool,
}

/// How far a save (or the look for unsaved changes) has come.
#[derive(Serialize, JsonSchema)]
struct SaveView {
    done: usize,
    total: usize,
    next: Option<String>,
    track: Option<String>,
    writing: bool,
}

#[derive(Serialize, JsonSchema)]
struct Unsaved {
    project: bool,
    files: Vec<UnsavedFile>,
}

#[derive(Serialize, JsonSchema)]
struct UnsavedFile {
    document: crate::session::DocumentId,
    path: PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct LoadingView {
    stage: crate::session::LoadStage,
    done: usize,
    total: usize,
    stage_done: usize,
    stage_total: usize,
    next: Option<String>,
    track: Option<String>,
    rack: bool,
    kind: crate::session::LoadKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct Cancelled {
    reopened: Option<PathBuf>,
}

/// An answer as the action's JSON.
fn output(answer: impl Serialize) -> ApiResult<ActionOutput> {
    serde_json::to_value(answer)
        .map(ActionOutput::Json)
        .map_err(|e| ApiError(format!("encoding the result: {e}")))
}

#[derive(Serialize, JsonSchema)]
struct Saved {
    path: Option<PathBuf>,
}

#[derive(Serialize, JsonSchema)]
struct Opened {
    path: Option<PathBuf>,
    newer_backup: Option<PathBuf>,
}

#[derive(Serialize, JsonSchema)]
struct BackupView {
    path: PathBuf,
    modified: u64,
    age_seconds: u64,
    size: u64,
}

#[derive(Serialize, JsonSchema)]
struct BackupList {
    backups: Vec<BackupView>,
}

fn backup_list(s: &Session) -> BackupList {
    let now = std::time::SystemTime::now();
    let secs = |t: std::time::SystemTime| {
        t.duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs())
    };
    BackupList {
        backups: s
            .backups()
            .into_iter()
            .map(|b| BackupView {
                modified: secs(b.modified),
                age_seconds: now.duration_since(b.modified).map_or(0, |d| d.as_secs()),
                size: b.size,
                path: b.path,
            })
            .collect(),
    }
}

/// Begun as the app's Ctrl+S is (`save_in_background`), answered once written.
fn save_project(host: &mut dyn Host, input: SaveArgs, reply: crate::api::Reply) {
    let s = host.session();
    let Some(path) = input.path.or_else(|| s.path.clone()) else {
        return reply(Err(error("the project has never been saved; give a path")));
    };
    if let Err(e) = s.save_in_background(&path) {
        return reply(Err(e.into()));
    }
    host.when_saved(
        reply,
        Box::new(|s| {
            output(Saved {
                path: s.path.clone(),
            })
        }),
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Tempo {
    bpm: f64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GridArg {
    /// "auto", or "1/4", "1/8", "1/16", "1/32", "1/64" (a "t" after it for triplets).
    grid: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TimeSignature {
    numerator: u32,
    denominator: u32,
}

// ------------------------------------------------------------------- reading

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NewProject {
    /// The project's name: its folder and file are named after it.
    #[serde(default)]
    name: Option<String>,
    /// Absolute path of the folder the project's own folder is made in.
    #[serde(default)]
    folder: Option<PathBuf>,
    /// The template to start from, by name (list_templates).
    #[serde(default)]
    template: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct ProjectView {
    name: String,
    /// The project file; none until it is saved.
    path: Option<PathBuf>,
    /// The project's folder, where its files are kept; none with no project open.
    folder: Option<PathBuf>,
    sample_rate: u32,
    tempo: f64,
    time_signature: (u32, u32),
    /// End of the last clip, in samples.
    length: u64,
    /// MIDI notes are struck when playback starts inside them (see set_chase_notes).
    chase_notes: bool,
    /// How recorded MIDI lands (see set_midi_record_options).
    midi_record: crate::project::MidiRecordSettings,
    /// The integrated loudness the mix aims at, LUFS (see set_loudness_target).
    loudness_target: f64,
    /// The song's grid division, or "auto" (see set_grid).
    #[schemars(with = "String")]
    grid: Grid,
    /// Named places, in time order (see get_markers).
    markers: Vec<super::markers::MarkerView>,
    tracks: Vec<TrackView>,
    master: MasterView,
}

#[derive(Serialize, JsonSchema)]
struct TrackView {
    id: TrackId,
    name: String,
    kind: TrackKind,
    color: [u8; 3],
    /// Folder track this one sits in.
    parent: Option<TrackId>,
    input: TrackInput,
    output: TrackOutput,
    monitor: crate::project::Monitor,
    mixer: Mixer,
    arm: bool,
    /// Row height in the arrange view, in points.
    height: f32,
    /// Copies of the signal into folders and buses (see add_send).
    sends: Vec<super::sends::SendView>,
    inserts: Vec<InsertView>,
    clips: Vec<ClipView>,
    /// Instrument tracks: MIDI clips, positions in ticks (see get_midi_clip).
    midi_clips: Vec<super::midi_clips::MidiClipSummary>,
    /// Automation lanes (see get_automation_lane).
    automation: Vec<super::automation::LaneSummary>,
    /// What its controls do to its lanes while playing (see set_automation_mode).
    automation_mode: crate::automation::AutomationMode,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct InsertView {
    id: u32,
    name: String,
    bypass: bool,
    loaded: bool,
    latency: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct ClipView {
    id: u32,
    name: String,
    start: u64,
    length: u64,
    /// The playing take's offset and file. Offsets count samples of the audio as heard (for
    /// a stretched clip, its render).
    offset: u64,
    gain_db: f32,
    /// Time-stretch ratio (1 = untouched) and transposition in semitones (0 = untouched).
    stretch: f64,
    pitch: f64,
    /// The recording's tempo when the clip follows the project tempo, else absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_tempo: Option<f64>,
    /// A stretch keeps the attacks unstretched and on time.
    transients: bool,
    /// Still waiting for its stretched render (it plays unchanged meanwhile).
    rendering: bool,
    /// Fades at each end, in samples.
    fade_in: crate::project::Fade,
    fade_out: crate::project::Fade,
    file: PathBuf,
    /// Every take (one per pass of a loop recording), by index; `active` plays.
    takes: Vec<TakeView>,
    active: usize,
}

#[derive(Serialize, JsonSchema)]
struct TakeView {
    name: String,
    file: PathBuf,
    offset: u64,
}

#[derive(Serialize, JsonSchema)]
struct MasterView {
    mixer: Mixer,
    output_channel: u16,
    inserts: Vec<InsertView>,
    /// Automation lanes (see get_automation_lane).
    automation: Vec<super::automation::LaneSummary>,
    automation_mode: crate::automation::AutomationMode,
    /// Its row in the arrangement: height in points, and whether its lanes are shown.
    height: f32,
    show_automation: bool,
}

pub(super) fn inserts(session: &Session, slots: &[InsertSlot]) -> Vec<InsertView> {
    slots
        .iter()
        .map(|s| {
            let info = session.insert_info(s.id);
            InsertView {
                id: s.id,
                name: s.name.clone(),
                bypass: s.bypass,
                loaded: info.is_some(),
                latency: info.map(|(_, latency, _)| latency),
            }
        })
        .collect()
}

fn get_project(host: &mut dyn Host, _: NoArgs) -> ApiResult<ProjectView> {
    let s = host.session();
    let p = &s.project;
    let tracks = p
        .tracks
        .iter()
        .map(|t| TrackView {
            id: t.id,
            name: t.name.clone(),
            kind: t.kind,
            color: t.color,
            parent: t.parent,
            input: t.input.clone(),
            output: t.output,
            monitor: t.monitor,
            mixer: t.mixer,
            arm: t.arm,
            height: t.height(),
            sends: t
                .sends
                .iter()
                .map(|send| super::sends::SendView::new(t.id, send))
                .collect(),
            inserts: inserts(s, &t.inserts),
            clips: t
                .clips
                .iter()
                .map(|c| ClipView {
                    id: c.id,
                    name: c.name.clone(),
                    start: c.start,
                    length: c.length,
                    offset: c.offset(),
                    gain_db: c.gain_db,
                    stretch: c.stretch,
                    pitch: c.pitch,
                    source_tempo: c.follow.as_ref().map(|f| f.tempo),
                    transients: c.transients,
                    rendering: s.clip_rendering(c),
                    fade_in: c.fade_in,
                    fade_out: c.fade_out,
                    file: s.resolve(c.file()),
                    takes: c
                        .takes
                        .iter()
                        .map(|t| TakeView {
                            name: t.name.clone(),
                            file: s.resolve(&t.file),
                            offset: t.offset,
                        })
                        .collect(),
                    active: c.active,
                })
                .collect(),
            midi_clips: t
                .midi_clips
                .iter()
                .map(super::midi_clips::MidiClipSummary::new)
                .collect(),
            automation: super::automation::LaneSummary::all(&t.automation),
            automation_mode: t.automation_mode,
        })
        .collect();
    Ok(ProjectView {
        name: p.name.clone(),
        path: s.path.clone(),
        folder: s.project_dir(),
        sample_rate: s.sample_rate(),
        tempo: p.tempo,
        time_signature: p.time_signature,
        length: s.content_end(),
        chase_notes: p.chase_notes,
        midi_record: p.midi_record.clone(),
        loudness_target: p.loudness_target,
        grid: p.grid,
        markers: super::markers::marker_views(s),
        tracks,
        master: MasterView {
            mixer: p.master.mixer,
            output_channel: p.master.output_channel,
            inserts: inserts(s, &p.master.inserts),
            automation: super::automation::LaneSummary::all(&p.master.automation),
            automation_mode: p.master.automation_mode,
            height: p.master.height(),
            show_automation: p.master.show_automation,
        },
    })
}
