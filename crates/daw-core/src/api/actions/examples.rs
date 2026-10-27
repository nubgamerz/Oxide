//! The MCP manual's examples (27.6): every example in `docs/manual/mcp/examples/*.toml` run
//! against a headless session holding the example project, and its answer compared with the
//! one the manual shows, so the examples can't rot. `UPDATE_DOCS=1` writes the answers into
//! the files instead (keeping their wildcards); the format is `manual::examples`', the
//! authors' guide `docs/manual/README.md` ("Writing the MCP manual").
//!
//! The example project ([`build_fixture`], 48 kHz, 120 BPM, 4/4: a bar is 96,000 samples),
//! made once per run, saved, and copied afresh for each example, so no example sees
//! another's changes:
//!
//! | id | track | what is on it |
//! |---|---|---|
//! | 1 | Guitars (folder) | Guitar L and Guitar R |
//! | 2 | Guitar L (audio, in Guitars, pan -0.5) | clip 7 `guitar-l.wav` at 0 (one bar); insert 17 Compressor (47 samples of latency) |
//! | 3 | Guitar R (audio, in Guitars, pan 0.5) | clip 8 `guitar-r.wav` at 0 (one bar) |
//! | 4 | Keys (instrument, armed, no plug-in) | MIDI clip 10 "Chords", bars 1-2: notes 11-13 C4 E4 G4 in bar 1, 14-16 A3 C4 E4 in bar 2, whole notes |
//! | 5 | Vocal (audio, -3 dB) | clip 9 `vocal.wav` at bar 2 (96000, one bar); send 19 to Reverb at -6 dB |
//! | 6 | Reverb (bus) | insert 18 Room Reverb |
//!
//! and the marker 20 "Chorus" at bar 2; the next new id is 21. The files are in the project's
//! `Imported` folder. The ids are the registry's own, checked by [`the_example_project_is_as_documented`]; a
//! change to the fixture changes them for every example, so keep it stable.

use super::super::{title, ActionOutput, Registry, SessionHost, Undo};
use crate::device::AudioSettings;
use crate::session::{ExportStatus, Session};
use crate::wav;
use manual::examples::{self, Outcome, PROJECT};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::Duration;

const RATE: u32 = 48_000;
/// One bar at 120 BPM in 4/4.
const BAR: usize = 96_000;

fn headless() -> Session {
    Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        threads: Some(0),
        ..Default::default()
    })
}

fn call(r: &Registry, s: &mut Session, name: &str, args: Value) -> Outcome {
    let outcome = match r.call_now(&mut SessionHost(s), name, args) {
        Ok(ActionOutput::Json(v)) => Outcome::Result(v),
        Ok(ActionOutput::Png(_)) => Outcome::Result(json!("(a PNG image)")),
        Err(e) => Outcome::Error(e.0),
    };
    // A job an action started (an export, a render) ends before the session does.
    if matches!(s.export_status(), ExportStatus::Running { .. }) {
        let _ = s.wait_for_export(Duration::from_secs(60));
    }
    outcome
}

/// A call the fixture needs: its answer, or a panic naming it.
fn must(r: &Registry, s: &mut Session, name: &str, args: Value) -> Value {
    match call(r, s, name, args.clone()) {
        Outcome::Result(v) => v,
        Outcome::Error(e) => panic!("the example project's {name} {args}: {e}"),
    }
}

/// A mono test file: `pitch` Hz plucked on each beat (a decaying sawtooth), with a slow
/// vibrato when `sung`.
fn write_file(path: &Path, pitch: f32, sung: bool) {
    let beat = BAR / 4;
    let samples: Vec<f32> = (0..BAR)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            if sung {
                let vibrato = 1.0 + 0.01 * (2.0 * std::f32::consts::PI * 5.0 * t).sin();
                0.3 * (2.0 * std::f32::consts::PI * pitch * vibrato * t).sin()
            } else {
                let since = (i % beat) as f32 / RATE as f32;
                let phase = (pitch * t).fract();
                0.4 * (2.0 * phase - 1.0) * (-6.0 * since).exp()
            }
        })
        .collect();
    wav::write_f32(
        path,
        &wav::Audio {
            sample_rate: RATE,
            channels: vec![samples],
        },
    )
    .unwrap();
}

/// Makes the example project in `root/Example` and saves it; returns its file.
fn build_fixture(r: &Registry, root: &Path) -> PathBuf {
    let sources = root.join("sources");
    std::fs::create_dir_all(&sources).unwrap();
    write_file(&sources.join("guitar-l.wav"), 110.0, false);
    write_file(&sources.join("guitar-r.wav"), 146.83, false);
    write_file(&sources.join("vocal.wav"), 440.0, true);

    let mut s = headless();
    let made = must(
        r,
        &mut s,
        "new_project",
        json!({"name": "Example", "folder": root}),
    );
    let file = PathBuf::from(made["path"].as_str().expect("a path"));
    must(r, &mut s, "set_tempo", json!({"bpm": 120.0}));
    let id = |v: Value, field: &str| v[field].as_u64().unwrap_or_else(|| panic!("{v}"));
    let folder = id(
        must(
            r,
            &mut s,
            "add_track",
            json!({"kind": "folder", "name": "Guitars"}),
        ),
        "track",
    );
    let audio = |s: &mut Session, name: &str, parent: Option<u64>| {
        id(
            must(
                r,
                s,
                "add_track",
                json!({"kind": "audio", "name": name, "parent": parent}),
            ),
            "track",
        )
    };
    let left = audio(&mut s, "Guitar L", Some(folder));
    let right = audio(&mut s, "Guitar R", Some(folder));
    let keys = id(
        must(
            r,
            &mut s,
            "add_track",
            json!({"kind": "instrument", "name": "Keys"}),
        ),
        "track",
    );
    let vocal = audio(&mut s, "Vocal", None);
    let reverb = id(
        must(
            r,
            &mut s,
            "add_track",
            json!({"kind": "bus", "name": "Reverb"}),
        ),
        "track",
    );
    must(
        r,
        &mut s,
        "set_track_mixer",
        json!({"track": left, "pan": -0.5}),
    );
    must(
        r,
        &mut s,
        "set_track_mixer",
        json!({"track": right, "pan": 0.5}),
    );
    must(
        r,
        &mut s,
        "set_track_mixer",
        json!({"track": vocal, "gain_db": -3.0}),
    );
    for (track, file, at) in [
        (left, "guitar-l.wav", 0),
        (right, "guitar-r.wav", 0),
        (vocal, "vocal.wav", BAR),
    ] {
        must(
            r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": sources.join(file), "position": at}),
        );
    }
    let clip = id(
        must(
            r,
            &mut s,
            "add_midi_clip",
            json!({"track": keys, "bar": 1, "bars": 2, "name": "Chords"}),
        ),
        "clip",
    );
    let chord = |at: &str, pitches: [&str; 3]| -> Vec<Value> {
        pitches
            .iter()
            .map(|p| json!({"pitch": p, "at": at, "length": 3840, "velocity": 90}))
            .collect()
    };
    let mut notes = chord("1:1:0", ["C4", "E4", "G4"]);
    notes.extend(chord("2:1:0", ["A3", "C4", "E4"]));
    must(
        r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": notes}),
    );
    must(
        r,
        &mut s,
        "add_insert",
        json!({"track": left, "plugin": "native:compressor"}),
    );
    must(
        r,
        &mut s,
        "add_insert",
        json!({"track": reverb, "plugin": "native:room-reverb"}),
    );
    must(
        r,
        &mut s,
        "add_send",
        json!({"track": vocal, "target": reverb, "gain_db": -6.0}),
    );
    must(r, &mut s, "add_marker", json!({"bar": 2, "name": "Chorus"}));
    must(r, &mut s, "save_project", Value::Null);
    file
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The run's folder: the fixture, each example's copy, the manual built for `search_manual`.
fn run_root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("daw-mcp-examples-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    root
}

/// The manual built from `docs/manual` in memory, its search index written into `root/manual`
/// as the generator writes it, for `search_manual` and `open_manual`.
fn manual_dir(root: &Path) -> PathBuf {
    let dir = root.join("manual");
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
    let source = manual::Source::read(&src, &src.join("shots")).unwrap();
    let built = manual::build(
        &source,
        &manual::Options::default(),
        &manual::mcp::Reference::load(),
    );
    // A manual with errors still has its index; its own check (the quick tier's) fails on
    // them, so a broken page doesn't hide what the examples did.
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join(manual::search::JSON_FILE),
        &built.files[manual::search::JSON_FILE],
    )
    .unwrap();
    dir
}

/// A fresh copy (number `n`) of the example project, open in a new headless session: the
/// session and the copy's folder.
fn fresh(r: &Registry, fixture: &Path, root: &Path, n: usize) -> (Session, PathBuf) {
    let folder = fixture.parent().expect("the project's folder");
    let copy = root.join(format!("run-{n}")).join("Example");
    copy_dir(folder, &copy);
    let mut s = headless();
    s.manual_dir = Some(root.join("manual"));
    let file = copy.join(fixture.file_name().expect("a file"));
    must(r, &mut s, "open_project", json!({ "path": file }));
    (s, copy)
}

/// `<project>` in an example's input made the copy's folder.
fn with_folder(input: &Value, folder: &Path) -> Value {
    examples::replace_in_strings(input, PROJECT, &folder.to_string_lossy())
}

/// The copy's folder in an answer made `<project>`.
fn without_folder(outcome: Outcome, folder: &Path) -> Outcome {
    let text = folder.to_string_lossy().to_string();
    let forward = text.replace('\\', "/");
    match outcome {
        Outcome::Result(v) => {
            let v = examples::replace_in_strings(&v, &text, PROJECT);
            Outcome::Result(examples::replace_in_strings(&v, &forward, PROJECT))
        }
        Outcome::Error(e) => Outcome::Error(e.replace(&text, PROJECT).replace(&forward, PROJECT)),
    }
}

/// What a call did to the project and its undo history.
#[derive(Debug, Clone)]
struct UndoSeen {
    /// The undo list's names (most recent first) before and after the call.
    before: Vec<String>,
    after: Vec<String>,
    /// The project differs after the call (view state such as track heights aside, as the
    /// history compares it).
    changed: bool,
}

/// The call did to the undo history what the manual says (`Registry::undo`, the badge):
///
/// - `never`: no step added (`undo` takes one off, `redo` puts one back, an open starts
///   afresh: those move through the history, they add nothing of their own);
/// - `when_changed`: one step named after the action when the project changed (or none, the
///   step merged into the one before, for a `set_` action repeated on the same object), and
///   none when it didn't: an example that sets a value to what it was records nothing, as
///   documented;
/// - `always`: as `when_changed`, and the project must have changed.
///
/// A refusal adds no step, whatever the action.
fn undo_as_documented(
    r: &Registry,
    action: &str,
    outcome: &Outcome,
    seen: &UndoSeen,
) -> Result<(), String> {
    let added = seen.after.len() as i64 - seen.before.len() as i64;
    let top = seen.after.first().cloned().unwrap_or_default();
    if let Outcome::Error(_) = outcome {
        return if added > 0 {
            Err(format!(
                "{action} was refused, but left the undo step \"{top}\""
            ))
        } else {
            Ok(())
        };
    }
    let undo = r.undo(action);
    match undo {
        Undo::Never => {
            if added > 0 && action != "redo" {
                return Err(format!(
                    "{action} is documented as not an undo step, but recorded \"{top}\""
                ));
            }
        }
        Undo::WhenChanged | Undo::Always => {
            if seen.changed {
                let step = added == 1 && top == title(action);
                let merged = added == 0 && seen.after == seen.before && action.starts_with("set_");
                if !step && !merged {
                    return Err(format!(
                        "{action} changed the project, but the undo history went from {:?} to \
                         {:?} (one step, \"{}\", was documented)",
                        seen.before,
                        seen.after,
                        title(action)
                    ));
                }
            } else if added != 0 {
                return Err(format!(
                    "{action} changed nothing, but the undo history went from {:?} to {:?}",
                    seen.before, seen.after
                ));
            } else if undo == Undo::Always {
                return Err(format!(
                    "{action} is documented as an undo step on every call, but this example \
                     changed nothing (move it to the actions that are one only when they \
                     change something, or make the example change something)"
                ));
            }
        }
    }
    Ok(())
}

/// Undo-step actions the example project can't show changing anything, with why: the check
/// that some example of each undo-step action changes the project skips them, and the run
/// lists them as not seen changing it.
const CANT_CHANGE_HERE: &[(&str, &str)] = &[(
    "set_active_take",
    "the example project's clips have one take each (takes come from loop recording)",
)];

/// The undo list's names, most recent first.
fn undo_names(r: &Registry, s: &mut Session) -> Vec<String> {
    let history = must(r, s, "get_history", Value::Null);
    history["undo"]
        .as_array()
        .map(|names| {
            names
                .iter()
                .filter_map(|n| n.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// One example run: its setup, then its call, on a fresh copy. The answer and what it did to
/// the undo history, or why it couldn't be had.
fn run_one(
    r: &Registry,
    fixture: &Path,
    root: &Path,
    n: usize,
    example: &examples::Example,
) -> Result<(Outcome, UndoSeen), String> {
    if r.safety(&example.action).is_none() {
        return Err(format!("no action is called {}", example.action));
    }
    let (mut s, folder) = fresh(r, fixture, root, n);
    for step in &example.setup {
        if let Outcome::Error(e) = call(r, &mut s, &step.action, with_folder(&step.input, &folder))
        {
            return Err(format!("its setup's {} was refused: {e}", step.action));
        }
    }
    let before = undo_names(r, &mut s);
    let project = s.project.clone();
    let outcome = call(
        r,
        &mut s,
        &example.action,
        with_folder(&example.input, &folder),
    );
    let mut was = project;
    was.copy_view_from(&s.project);
    let seen = UndoSeen {
        before,
        after: undo_names(r, &mut s),
        changed: was != s.project,
    };
    Ok((without_folder(outcome, &folder), seen))
}

/// Sessions are slow to make (each scans the installed plug-ins), so the examples run on a
/// few threads, each making its own sessions as tests do side by side.
fn workers() -> usize {
    std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, 8)
}

/// Every example in `docs/manual/mcp/examples` gives the answer it shows (27.6). With
/// `UPDATE_DOCS` set, the answers are written into the files instead (read them before
/// keeping them). Prints how many ran, how many are only shown, and how many actions have
/// none yet.
#[test]
fn the_manuals_mcp_examples_give_the_answers_they_show() {
    let r = Registry::new();
    let root = run_root("run");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual/mcp/examples");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e == "toml"))
                .collect()
        })
        .unwrap_or_default();
    paths.sort();
    let mut failures = Vec::new();
    let mut files = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(path).unwrap();
        match examples::parse(&name, &text) {
            Ok(file) => files.push((path.clone(), name, text, file)),
            Err(errors) => failures.extend(
                errors
                    .into_iter()
                    .map(|(line, message)| format!("{name}:{line}: {message}")),
            ),
        }
    }
    // Every example to run, as (file, example).
    let jobs: Vec<(usize, usize)> = files
        .iter()
        .enumerate()
        .flat_map(|(f, (.., file))| {
            file.examples
                .iter()
                .enumerate()
                .filter(|(_, e)| e.shown_only.is_none())
                .map(move |(n, _)| (f, n))
        })
        .collect();
    let shown: usize = files
        .iter()
        .map(|(.., file)| {
            file.examples
                .iter()
                .filter(|e| e.shown_only.is_some())
                .count()
        })
        .sum();
    let fixture = build_fixture(&r, &root.join("fixture"));
    manual_dir(&root);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let answers = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|scope| {
        for _ in 0..workers().min(jobs.len()) {
            scope.spawn(|| {
                let r = Registry::new();
                loop {
                    let job = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(&(f, n)) = jobs.get(job) else {
                        break;
                    };
                    let example = &files[f].3.examples[n];
                    let answer = run_one(&r, &fixture, &root, job, example);
                    answers.lock().unwrap().push((f, n, answer));
                }
            });
        }
    });
    let mut answers = answers.into_inner().unwrap();
    answers.sort_by_key(|(f, n, _)| (*f, *n));

    let update = std::env::var_os("UPDATE_DOCS").is_some();
    let mut written: Vec<Vec<(usize, Outcome)>> = vec![Vec::new(); files.len()];
    // Per action: its examples that answered, and how many of them changed the project.
    let mut changes: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    for (f, n, answer) in answers {
        let (_, name, _, file) = &files[f];
        let example = &file.examples[n];
        let at = format!(
            "{name}:{} {} \"{}\"",
            example.line, example.action, example.title
        );
        let outcome = match answer {
            Ok((outcome, seen)) => {
                if let Err(e) = undo_as_documented(&r, &example.action, &outcome, &seen) {
                    failures.push(format!("{at}: {e}"));
                }
                if let Outcome::Result(_) = outcome {
                    let entry = changes.entry(example.action.clone()).or_default();
                    entry.0 += 1;
                    entry.1 += usize::from(seen.changed);
                }
                outcome
            }
            Err(e) => {
                failures.push(format!("{at}: {e}"));
                continue;
            }
        };
        let kind_fits = matches!(
            (&outcome, &example.error),
            (Outcome::Result(_), None) | (Outcome::Error(_), Some(_))
        );
        if update && kind_fits {
            written[f].push((n, outcome));
        } else if let Err(e) = examples::check(example, &outcome) {
            failures.push(format!("{at}: {e}"));
        }
    }
    if update {
        for ((path, _, text, file), outcomes) in files.iter().zip(&written) {
            let new = examples::rewrite(text, file, outcomes);
            if &new != text {
                std::fs::write(path, new).unwrap();
            }
        }
    }
    let with: std::collections::BTreeSet<&str> = files
        .iter()
        .flat_map(|(.., file)| file.examples.iter().map(|e| e.action.as_str()))
        .collect();
    let specs = r.specs();
    let missing = specs.iter().filter(|s| !with.contains(s.name)).count();
    // An action documented as an undo step must be seen recording one: one whose examples all
    // answered without changing the project is either one that never can (it belongs in
    // `NO_PROJECT_CHANGE`, "not an undo step") or needs an example that changes something.
    let mut unproven = Vec::new();
    for spec in &specs {
        if r.undo(spec.name) == Undo::Never {
            continue;
        }
        match changes.get(spec.name) {
            _ if CANT_CHANGE_HERE.iter().any(|(name, _)| *name == spec.name) => {
                unproven.push(spec.name)
            }
            Some(&(_, 0)) => failures.push(format!(
                "{}: documented as an undo step, but none of its examples changed the project: \
                 if it never can, list it in api::NO_PROJECT_CHANGE; otherwise add an example \
                 that changes something",
                spec.name
            )),
            Some(_) => {}
            None => unproven.push(spec.name),
        }
    }
    println!(
        "MCP examples: {} ran, {shown} shown only (not run); {missing} of {} actions have no \
         example yet{}",
        jobs.len(),
        specs.len(),
        if update { "; results written" } else { "" }
    );
    println!(
        "Undo steps: {} undo-step actions not seen changing the project (their examples are \
         refusals or shown only): {}",
        unproven.len(),
        unproven.join(", ")
    );
    let _ = std::fs::remove_dir_all(&root);
    assert!(
        failures.is_empty(),
        "{} example(s) failed (UPDATE_DOCS=1 writes what came back, for an answer that \
         changed on purpose; read it before keeping it):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The example project's ids and contents are the ones the authors' guide lists.
#[test]
fn the_example_project_is_as_documented() {
    let r = Registry::new();
    let root = run_root("documented");
    let fixture = build_fixture(&r, &root.join("fixture"));
    let (mut s, _) = fresh(&r, &fixture, &root, 0);
    let project = must(&r, &mut s, "get_project", Value::Null);
    let tracks: Vec<(u64, &str, &str)> = project["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| {
            (
                t["id"].as_u64().unwrap(),
                t["name"].as_str().unwrap(),
                t["kind"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        tracks,
        [
            (1, "Guitars", "Folder"),
            (2, "Guitar L", "Audio"),
            (3, "Guitar R", "Audio"),
            (4, "Keys", "Instrument"),
            (5, "Vocal", "Audio"),
            (6, "Reverb", "Bus"),
        ],
        "{project:#}"
    );
    let t = &project["tracks"];
    let ids = |v: &Value, field: &str| -> Vec<u64> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|x| x[field].as_u64().unwrap())
            .collect()
    };
    assert_eq!(ids(&t[1]["clips"], "id"), [7]);
    assert_eq!(ids(&t[2]["clips"], "id"), [8]);
    assert_eq!(ids(&t[4]["clips"], "id"), [9]);
    assert_eq!(t[4]["clips"][0]["start"], BAR);
    assert_eq!(ids(&t[3]["midi_clips"], "id"), [10]);
    assert_eq!(ids(&t[1]["inserts"], "id"), [17]);
    assert_eq!(ids(&t[5]["inserts"], "id"), [18]);
    assert_eq!(ids(&t[4]["sends"], "send"), [19]);
    assert_eq!(ids(&project["markers"], "id"), [20]);
    let clip = must(&r, &mut s, "get_midi_clip", json!({"clip": 10}));
    let notes: Vec<(u64, u64)> = clip["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["id"].as_u64().unwrap(), n["pitch"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        notes,
        [(11, 60), (12, 64), (13, 67), (14, 57), (15, 60), (16, 64)],
        "{clip:#}"
    );
    let next = must(&r, &mut s, "add_track", json!({"kind": "audio"}));
    assert_eq!(next["track"], 21, "the next id");
    drop(s);
    let _ = std::fs::remove_dir_all(&root);
}
