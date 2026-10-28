//! Every action, called the way an MCP client would, on a headless session.

use crate::api::{markdown, ActionOutput, Registry, SessionHost};
use crate::device::AudioSettings;
use crate::session::{LoadStage, Session};
use crate::wav;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn headless() -> Session {
    Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        threads: Some(0),
        ..Default::default()
    })
}

fn call(r: &Registry, s: &mut Session, name: &str, args: Value) -> Result<Value, String> {
    match r.call_now(&mut SessionHost(s), name, args) {
        Ok(ActionOutput::Json(v)) => Ok(v),
        Ok(ActionOutput::Png(_)) => Err("unexpected image".into()),
        Err(e) => Err(e.0),
    }
}

fn id(v: &Value, field: &str) -> u64 {
    v[field]
        .as_u64()
        .unwrap_or_else(|| panic!("no {field} in {v}"))
}

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-api-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// One second of a 440 Hz tone at 48 kHz, silent for its first quarter.
fn tone(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("tone.wav");
    let samples: Vec<f32> = (0..48_000)
        .map(|i| {
            if i < 12_000 {
                0.0
            } else {
                0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
            }
        })
        .collect();
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![samples],
        },
    )
    .unwrap();
    path
}

/// What `mixdown` renders of the first `frames` samples.
fn mixdown(r: &Registry, s: &mut Session, dir: &std::path::Path, name: &str) -> Vec<f32> {
    let out = dir.join(name);
    call(
        r,
        s,
        "export_mixdown",
        json!({"path": out, "start": 0, "end": 48_000, "tail_seconds": 0}),
    )
    .unwrap();
    s.wait_for_export(Duration::from_secs(20)).unwrap();
    wav::read(&out).unwrap().channels[0].clone()
}

#[test]
fn loop_takes_pick_split_delete_and_stack_up() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("takes");
    // A "recording" of 4 passes of a 2000-sample loop at 1000: the tone file is 48000 long.
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let passes = crate::record::slice_passes(8000, 1000, (1000, 3000), None).unwrap();
    s.add_loop_takes(track as u32, &file, &passes).unwrap();
    let clip = &call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"][0];
    assert_eq!(
        (clip["start"].as_u64(), clip["length"].as_u64()),
        (Some(1000), Some(2000))
    );
    assert_eq!(clip["takes"].as_array().unwrap().len(), 4);
    assert_eq!(clip["active"], 3, "the last complete pass plays");
    let clip_id = clip["id"].as_u64().unwrap();

    // A second round over the same loop adds to the clip rather than stacking another.
    s.add_loop_takes(track as u32, &file, &passes).unwrap();
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    let clips = project["tracks"][0]["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0]["takes"].as_array().unwrap().len(), 8);
    assert_eq!(clips[0]["active"], 7);
    assert_eq!(clips[0]["takes"][7]["offset"], 6000);

    assert!(call(
        &r,
        &mut s,
        "set_active_take",
        json!({"clip": clip_id, "take": 8})
    )
    .is_err());
    let picked = call(
        &r,
        &mut s,
        "set_active_take",
        json!({"clip": clip_id, "take": 1}),
    )
    .unwrap();
    assert_eq!(
        (picked["active"].as_u64(), picked["offset"].as_u64()),
        (Some(1), Some(2000))
    );

    // Splitting keeps every take on both halves (comping: pick per piece).
    let split = call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip_id, "position": 1500}),
    )
    .unwrap();
    assert_eq!(split["first"]["takes"], 8);
    assert_eq!(split["second"]["takes"], 8);
    assert_eq!(split["second"]["offset"], 2500, "take 2, 500 in");
    let second = split["second"]["clip"].as_u64().unwrap();
    call(
        &r,
        &mut s,
        "set_active_take",
        json!({"clip": second, "take": 7}),
    )
    .unwrap();

    let deleted = call(
        &r,
        &mut s,
        "delete_take",
        json!({"clip": clip_id, "take": 1}),
    )
    .unwrap();
    assert_eq!(
        (deleted["takes"].as_u64(), deleted["active"].as_u64()),
        (Some(7), Some(0))
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_edit_is_an_undo_step_and_undo_restores_exactly() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("undo");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "set_track",
        json!({"track": track, "name": "DI"}),
    )
    .unwrap();
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap(),
        "clip",
    );
    let before_move = call(&r, &mut s, "get_project", Value::Null).unwrap();
    call(
        &r,
        &mut s,
        "move_clip",
        json!({"clip": clip, "start": 48_000}),
    )
    .unwrap();
    let after_move = call(&r, &mut s, "get_project", Value::Null).unwrap();

    // A failed edit, a read and a save leave nothing behind.
    assert!(call(&r, &mut s, "move_clip", json!({"clip": 9999, "start": 1})).is_err());
    call(&r, &mut s, "get_transport", Value::Null).unwrap();
    call(
        &r,
        &mut s,
        "save_project",
        json!({"path": dir.join("undo.dawproj")}),
    )
    .unwrap();
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(
        history["undo"],
        json!(["Move clip", "Import audio", "Set track", "Add track"])
    );

    let undone = call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(undone["done"], "Move clip");
    assert_eq!(undone["redo"], json!(["Move clip"]));
    // Exactly as before the move, except what the save since then changed for good: the
    // project's file and folder, the name it took from it, and the clips playing the copies
    // the save took into the new folder (JSON escapes the folder's backslashes).
    let saved = |mut view: Value| -> Value {
        let folder = view["folder"]
            .as_str()
            .unwrap_or_default()
            .replace('\\', "\\\\");
        view["path"] = Value::Null;
        view["folder"] = Value::Null;
        view["name"] = json!("undo");
        serde_json::from_str(&view.to_string().replace(&folder, "<folder>")).unwrap()
    };
    let now = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(saved(now), saved(before_move));
    call(&r, &mut s, "redo", Value::Null).unwrap();
    let now = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(saved(now), saved(after_move));
    assert_eq!(
        s.project.name, "undo",
        "undoing past a save keeps the saved name"
    );

    // Undo everything: an empty project, and nothing more to undo.
    for _ in 0..4 {
        call(&r, &mut s, "undo", Value::Null).unwrap();
    }
    assert!(s.project.tracks.is_empty());
    assert!(call(&r, &mut s, "undo", Value::Null)
        .unwrap_err()
        .contains("nothing"));

    // A new track after undoing the first one gets a new id: ids are never reused.
    let again = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    assert_ne!(again, track);
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["redo"], json!([]), "a new edit clears redo");

    call(&r, &mut s, "new_project", Value::Null).unwrap();
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(
        (history["undo"].clone(), history["redo"].clone()),
        (json!([]), json!([]))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// What the app does every frame: the frame is one edit, named by what the UI changed. An
/// undo and a save inside a frame are not taken for edits.
#[test]
fn ui_frames_are_undo_steps_named_by_what_changed() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("ui-frames");
    let track = s.add_track(crate::project::TrackKind::Audio, None);
    // A fader dragged over three frames, as the mixer does it.
    for gain in [-2.0, -4.0, -6.0] {
        s.begin_edit("", None);
        s.project.track_mut(track).unwrap().mixer.gain_db = gain;
        s.end_edit();
    }
    // A rename in another frame, then a frame that saves, then one that changes nothing.
    s.begin_edit("", None);
    s.project.track_mut(track).unwrap().name = "Bass DI".into();
    s.end_edit();
    s.begin_edit("", None);
    s.save(&dir.join("frames.dawproj")).unwrap();
    s.end_edit();
    s.begin_edit("", None);
    s.end_edit();
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"], json!(["Rename track", "Volume"]));

    // Ctrl+Z inside a frame undoes the rename and records nothing itself.
    s.begin_edit("", None);
    assert_eq!(s.undo().unwrap(), "Rename track");
    s.end_edit();
    assert_eq!(s.project.tracks[0].name, "Audio 1");
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"], json!(["Volume"]));
    assert_eq!(history["redo"], json!(["Rename track"]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn fader_moves_merge_and_undo_waits_for_recording_to_stop() {
    let r = Registry::new();
    let mut s = headless();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    for gain in [-1.0, -2.0, -3.0, -6.0] {
        call(
            &r,
            &mut s,
            "set_track_mixer",
            json!({"track": track, "gain_db": gain}),
        )
        .unwrap();
    }
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"], json!(["Set track mixer", "Add track"]));
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(
        s.project.tracks[0].mixer.gain_db, 0.0,
        "back before the first move"
    );

    s.pretend_recording();
    let err = call(&r, &mut s, "undo", Value::Null).unwrap_err();
    assert!(err.contains("stop recording first"), "{err}");
}

/// Track heights are view state: saved with the project, but never an undo step, and undo
/// leaves them as they are.
#[test]
fn track_heights_are_saved_but_never_undo_steps() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("heights");
    let a = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let folder = id(
        &call(&r, &mut s, "add_track", json!({"kind": "folder"})).unwrap(),
        "track",
    );
    let heights = |r: &Registry, s: &mut Session| -> Vec<f64> {
        let project = call(r, s, "get_project", Value::Null).unwrap();
        project["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["height"].as_f64().unwrap())
            .collect()
    };
    assert_eq!(heights(&r, &mut s), [64.0, 44.0], "the defaults");

    call(&r, &mut s, "set_track", json!({"track": a, "height": 150})).unwrap();
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"], json!(["Add track", "Add track"]));
    assert_eq!(heights(&r, &mut s), [150.0, 44.0]);

    // A rename alongside a resize is one step that undoes only the rename.
    call(
        &r,
        &mut s,
        "set_track",
        json!({"track": a, "name": "Gtr", "height": 200, "show_automation": true}),
    )
    .unwrap();
    // And a UI frame that only resizes records nothing.
    s.begin_edit("", None);
    s.project.track_mut(folder as u32).unwrap().height = Some(90.0);
    s.project.track_mut(folder as u32).unwrap().show_automation = true;
    s.end_edit();
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Set track");
    assert_eq!(history["undo"].as_array().unwrap().len(), 3);
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(s.project.tracks[0].name, "Audio 1");
    assert_eq!(heights(&r, &mut s), [200.0, 90.0], "undo keeps the heights");
    let shown = |s: &Session| {
        s.project
            .tracks
            .iter()
            .map(|t| t.show_automation)
            .collect::<Vec<_>>()
    };
    assert_eq!(shown(&s), [true, true], "and the lanes shown");
    call(&r, &mut s, "redo", Value::Null).unwrap();
    assert_eq!(heights(&r, &mut s), [200.0, 90.0], "and so does redo");

    // Saved and reopened; 0 goes back to the default.
    let path = dir.join("heights.dawproj");
    call(&r, &mut s, "save_project", json!({"path": path})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    assert_eq!(heights(&r, &mut s), [200.0, 90.0]);
    assert_eq!(shown(&s), [true, true]);
    call(&r, &mut s, "set_track", json!({"track": a, "height": 0})).unwrap();
    assert_eq!(heights(&r, &mut s), [64.0, 90.0]);

    for bad in [10.0, 500.0] {
        let err = call(&r, &mut s, "set_track", json!({"track": a, "height": bad})).unwrap_err();
        assert!(err.contains("height"), "{err}");
    }
}

/// Renaming and typed levels (22.1, 22.2): the UI's frame edits and `set_track` /
/// `set_track_mixer` share `Session::rename_track` and `set_mix_level`. A blank name is refused
/// with the old one kept; a rename is one "Rename track" step; typed levels clamp to the
/// fader, and two typed in a row are two steps, not merged as a drag's frames are.
#[test]
fn renames_and_typed_levels_are_one_step_each() {
    let r = Registry::new();
    let mut s = headless();
    let a = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    ) as u32;
    let bus = id(
        &call(&r, &mut s, "add_track", json!({"kind": "bus"})).unwrap(),
        "track",
    ) as u32;

    // As the header's field commits: inside the frame's edit.
    s.begin_edit("", None);
    s.rename_track(a, "  Lead guitar ").unwrap();
    s.end_edit();
    assert_eq!(s.project.track(a).unwrap().name, "Lead guitar");
    let err = s.rename_track(a, "   ").unwrap_err().to_string();
    assert!(err.contains("empty"), "{err}");
    assert_eq!(s.project.track(a).unwrap().name, "Lead guitar", "kept");
    let err = call(&r, &mut s, "set_track", json!({"track": a, "name": ""})).unwrap_err();
    assert!(err.contains("empty"), "{err}");
    assert!(s.rename_track(999, "x").is_err());
    // Buses (and folders) rename the same way.
    call(
        &r,
        &mut s,
        "set_track",
        json!({"track": bus, "name": "Reverb"}),
    )
    .unwrap();
    assert_eq!(s.project.track(bus).unwrap().name, "Reverb");
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][1], "Rename track");
    // The same name again is no step.
    s.begin_edit("", None);
    s.rename_track(a, "Lead guitar").unwrap();
    s.end_edit();
    let steps = |r: &Registry, s: &mut Session| {
        call(r, s, "get_history", Value::Null).unwrap()["undo"]
            .as_array()
            .unwrap()
            .len()
    };
    let before = steps(&r, &mut s);

    // Two volumes typed one after the other: two steps.
    for db in [-6.5, -3.0] {
        s.begin_edit("", None);
        s.set_mix_level(Some(a), Some(db), None).unwrap();
        s.end_edit();
    }
    assert_eq!(steps(&r, &mut s), before + 2);
    assert_eq!(s.project.track(a).unwrap().mixer.gain_db, -3.0);
    s.begin_edit("", None);
    s.set_mix_level(Some(a), None, Some(-0.3)).unwrap();
    s.end_edit();
    assert_eq!(s.project.track(a).unwrap().mixer.pan, -0.3);
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Pan");
    assert_eq!(history["undo"][1], "Volume");
    assert_eq!(history["undo"][2], "Volume");
    // Clamped to the fader; "-inf" is its bottom; not a number is refused.
    let mixer = s.set_mix_level(Some(a), Some(40.0), Some(5.0)).unwrap();
    assert_eq!((mixer.gain_db, mixer.pan), (12.0, 1.0));
    let mixer = s
        .set_mix_level(None, Some(f32::NEG_INFINITY), None)
        .unwrap();
    assert_eq!(mixer.gain_db, -60.0);
    assert!(s.set_mix_level(None, Some(f32::NAN), None).is_err());
    assert!(s.set_mix_level(Some(999), Some(0.0), None).is_err());
}

/// A project is a folder (13.1): made with its subfolders; imports are copied into it (the
/// original untouched); with no project open nothing is written anywhere; the folder copied
/// elsewhere opens and plays the same; Save As takes the files along; old `.dawproj` opens.
#[test]
fn a_project_is_a_folder_that_holds_its_files() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("project-folder");
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let file = tone(&outside);
    let original = std::fs::read(&file).unwrap();

    // With no project open (as in the app), importing is refused.
    s.scratch = None;
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let refused = call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap_err();
    assert!(refused.contains("no project is open"), "{refused}");

    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "First Song", "folder": dir}),
    )
    .unwrap();
    let project = dir.join("First Song").join("First Song.oxide");
    assert_eq!(made["path"], json!(project));
    for sub in crate::folder::ALL {
        assert!(dir.join("First Song").join(sub).is_dir(), "{sub}");
    }
    let view = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(
        (&view["name"], &view["folder"]),
        (&json!("First Song"), &json!(dir.join("First Song")))
    );
    for (args, says) in [
        (json!({"name": "First Song", "folder": dir}), "isn't empty"),
        (json!({"name": "x"}), "or neither"),
    ] {
        let err = call(&r, &mut s, "new_project", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }

    // An import is the project's own copy; the original is never touched.
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let clip = s.project.tracks[0].clips[0].file().to_path_buf();
    assert_eq!(clip, PathBuf::from("Imported").join("tone.wav"), "relative");
    let before = mixdown(&r, &mut s, &dir, "first.wav");
    call(&r, &mut s, "save_project", Value::Null).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), original);

    // The folder copied elsewhere opens, and plays the same.
    let moved = dir.join("moved");
    copy_tree(&dir.join("First Song"), &moved);
    call(
        &r,
        &mut s,
        "open_project",
        json!({"path": moved.join("First Song.oxide")}),
    )
    .unwrap();
    assert_eq!(s.resolve(&clip), moved.join("Imported").join("tone.wav"));
    assert_eq!(mixdown(&r, &mut s, &dir, "moved.wav"), before);

    // Save As into another folder takes the files along; the old folder keeps its own.
    let other = dir.join("other").join("Copy.oxide");
    call(&r, &mut s, "save_project", json!({"path": other})).unwrap();
    assert!(dir
        .join("other")
        .join("Imported")
        .join("tone.wav")
        .is_file());
    assert_eq!(mixdown(&r, &mut s, &dir, "copy.wav"), before);

    // A project saved before the name still opens.
    let old = dir.join("old").join("Old.dawproj");
    call(&r, &mut s, "save_project", json!({"path": old})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": old})).unwrap();
    assert_eq!(s.project.tracks.len(), 1);
    assert_eq!(mixdown(&r, &mut s, &dir, "old.wav"), before);
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// An update, and more until an autosave it began (its states read an update at a time, the
/// backup written on a worker, 20.1) is done.
fn autosaved(s: &mut Session) {
    s.update();
    let deadline = Instant::now() + Duration::from_secs(10);
    while s.autosaving() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(2));
        s.update();
    }
    assert!(!s.autosaving(), "the autosave never finished");
}

/// Saving without a hang (20.1): the app's save only begins (no state read yet), reads behind
/// `hold_saving` until let go, says how far it has come (states read of all, the plug-in next
/// and its track; get_busy too), refuses a second save meanwhile, and writes what a plain save
/// writes. save_project answers once written; the look for unsaved changes answers as
/// has_unsaved_changes does, a plug-in at a time.
#[test]
fn a_save_reads_states_over_updates_behind_the_bar() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("staged-save");
    call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Song", "folder": dir}),
    )
    .unwrap();
    let path = s.path.clone().unwrap();
    let bass = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "audio", "name": "Bass"}),
        )
        .unwrap(),
        "track",
    );
    for plugin in ["amplify", "parametric-eq"] {
        call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": bass, "plugin": plugin}),
        )
        .unwrap();
    }
    call(&r, &mut s, "add_insert", json!({"plugin": "limiter"})).unwrap();
    s.update();

    s.hold_saving(true);
    s.save_in_background(&path).unwrap();
    assert_eq!(s.saving(), Some(path.as_path()));
    let progress = s.save_progress().unwrap();
    assert_eq!((progress.done, progress.total), (0, 3), "{progress:?}");
    assert_eq!(progress.next.as_deref(), Some("Amplify"));
    assert_eq!(progress.track.as_deref(), Some("Bass"));
    assert_eq!(progress.path.as_deref(), Some(path.as_path()));
    assert!(!progress.writing);
    for _ in 0..3 {
        s.update();
    }
    let busy = call(&r, &mut s, "get_busy", Value::Null).unwrap();
    assert_eq!(busy["save"]["done"], json!(0), "held: {busy}");
    assert_eq!(busy["save"]["total"], json!(3));
    assert_eq!(busy["saving"], json!(path));
    let again = s.save_in_background(&path).unwrap_err();
    assert!(again.to_string().contains("already"), "{again}");
    let refused = call(&r, &mut s, "save_backup", Value::Null);
    assert!(refused.is_ok(), "a backup may be taken meanwhile");

    s.hold_saving(false);
    let deadline = Instant::now() + Duration::from_secs(10);
    while s.saving().is_some() && Instant::now() < deadline {
        s.update();
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(s.saving().is_none() && s.save_progress().is_none());
    assert_eq!(s.take_saved(), Some(Ok(path.clone())));
    let staged = std::fs::read(&path).unwrap();
    let plain = dir.join("Plain").join("Song.oxide");
    std::fs::create_dir_all(plain.parent().unwrap()).unwrap();
    s.save(&plain).unwrap();
    assert_eq!(
        std::fs::read(&plain).unwrap(),
        staged,
        "the staged save writes what a plain one does"
    );

    // An agent's save answers once written; the look for changes agrees, staged (as with a
    // VST3, held until let go, then read and compared on a worker) or at once (only built-in
    // effects: nothing to wait on).
    let changes = |s: &mut Session| {
        s.hold_saving(true);
        assert_eq!(s.check_unsaved_with(true), None, "begun, not answered");
        for _ in 0..3 {
            s.update();
            let progress = s.save_progress().expect("looking");
            assert_eq!((progress.path, progress.done), (None, 0), "held");
        }
        s.hold_saving(false);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(answer) = s.check_unsaved_with(true) {
                assert!(s.save_progress().is_none());
                assert_eq!(s.check_unsaved(), Some(answer), "at once, the same");
                return answer;
            }
            assert!(
                Instant::now() < deadline,
                "the look for changes never answered"
            );
            s.update();
            std::thread::sleep(Duration::from_millis(2));
        }
    };
    assert!(!changes(&mut s), "just saved");
    call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap();
    assert!(changes(&mut s));
    assert!(s.has_unsaved_changes());
    let saved = call(&r, &mut s, "save_project", Value::Null).unwrap();
    assert_eq!(saved["path"], json!(plain));
    assert!(s.saving().is_none(), "answered once written");
    assert!(!changes(&mut s));
    let _ = std::fs::remove_dir_all(dir);
}

/// Backups (13.2): autosave writes one when due and something changed (while stopped), never
/// the project file; only the newest are kept; a backup opens as the project it was; and a
/// backup newer than the saved file is reported when the project opens.
#[test]
fn autosave_backs_up_changes_and_keeps_the_newest() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("backups");
    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Song", "folder": dir}),
    )
    .unwrap();
    let project = PathBuf::from(made["path"].as_str().unwrap());
    let saved_file = std::fs::read(&project).unwrap();
    let count = |r: &Registry, s: &mut Session| {
        let list = call(r, s, "list_backups", Value::Null).unwrap();
        list["backups"].as_array().unwrap().len()
    };
    let add_track = |s: &mut Session| {
        call(&r, s, "add_track", json!({"kind": "audio"})).unwrap();
    };

    // Due, but nothing changed since the save: no backup.
    s.autosave = Some(Duration::from_millis(30));
    std::thread::sleep(Duration::from_millis(40));
    autosaved(&mut s);
    assert_eq!(count(&r, &mut s), 0);
    // A change: backed up once, and not again until something else changes.
    add_track(&mut s);
    std::thread::sleep(Duration::from_millis(40));
    autosaved(&mut s);
    assert_eq!(count(&r, &mut s), 1);
    std::thread::sleep(Duration::from_millis(40));
    autosaved(&mut s);
    assert_eq!(count(&r, &mut s), 1, "nothing new");
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"], json!(["Add track"]), "not an undo step");
    assert_eq!(
        std::fs::read(&project).unwrap(),
        saved_file,
        "the project file untouched"
    );

    // The newest three kept.
    s.keep_backups = 3;
    s.autosave = None;
    for _ in 0..4 {
        add_track(&mut s);
        call(&r, &mut s, "save_backup", Value::Null).unwrap();
    }
    let list = call(&r, &mut s, "list_backups", Value::Null).unwrap();
    let backups = list["backups"].as_array().unwrap();
    assert_eq!(backups.len(), 3);
    let newest = PathBuf::from(backups[0]["path"].as_str().unwrap());
    let at_newest = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert!(newest.starts_with(dir.join("Song").join("Backups")));

    // A later edit, then the newest backup opened: the project as it was then.
    add_track(&mut s);
    call(&r, &mut s, "open_backup", json!({"path": newest})).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_project", Value::Null).unwrap(),
        at_newest
    );
    assert_eq!(
        s.path.as_deref(),
        Some(project.as_path()),
        "it opens as the project"
    );
    let elsewhere = dir.join("x.oxide");
    std::fs::write(&elsewhere, "{}").unwrap();
    let err = call(&r, &mut s, "open_backup", json!({"path": elsewhere})).unwrap_err();
    assert!(err.contains("isn't one of this project's backups"), "{err}");

    // Opening the project (saved before these backups) points at the newest one; saving
    // clears that.
    let opened = call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    let newer = PathBuf::from(opened["newer_backup"].as_str().unwrap());
    assert_eq!(newer, newest);
    call(&r, &mut s, "save_project", Value::Null).unwrap();
    let opened = call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    assert!(opened["newer_backup"].is_null(), "{opened}");
}

/// Multi-out racks (14.4), headless: a built-in effect has no extra outputs, so it lists none
/// and is refused a rack; a track listening to an output that isn't there (its plug-in gone,
/// or one without it) is silent without breaking the graph, and a render of it is refused
/// for want of clips rather than failing. Real outputs need a multi-out VST3 (the
/// maintainer's checks).
#[test]
fn multi_out_racks_need_a_plug_in_with_outputs() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("multi-out");
    let file = tone(&dir);
    let guitar = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let import = json!({"track": guitar, "path": file, "position": 0});
    call(&r, &mut s, "import_audio", import).unwrap();
    let amp = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": guitar, "plugin": "amplify"}),
        )
        .unwrap(),
        "insert",
    );
    let outputs = call(&r, &mut s, "list_plugin_outputs", json!({"insert": amp})).unwrap();
    assert_eq!(outputs["outputs"], json!([]));
    let err = call(&r, &mut s, "create_multi_out_rack", json!({"insert": amp})).unwrap_err();
    assert!(err.contains("no outputs besides its main one"), "{err}");
    let err = call(&r, &mut s, "remove_multi_out_rack", json!({"insert": amp})).unwrap_err();
    assert!(err.contains("no tracks on its outputs"), "{err}");
    let err = call(&r, &mut s, "list_plugin_outputs", json!({"insert": 9999})).unwrap_err();
    assert!(err.contains("9999"), "{err}");

    // A listener on Amplify's (missing) second output: the mix is the guitar's alone.
    let alone = mixdown(&r, &mut s, &dir, "alone.wav");
    let listener = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    s.project.track_mut(listener as u32).unwrap().input = crate::project::TrackInput::Plugin {
        insert: amp as u32,
        output: 1,
    };
    s.mark_dirty();
    assert_eq!(s.project.listeners(amp as u32), [(listener as u32, 1)]);
    assert_eq!(mixdown(&r, &mut s, &dir, "with-listener.wav"), alone);
    let view = call(&r, &mut s, "get_project", Value::Null).unwrap();
    let shown = view["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == listener)
        .unwrap()["input"]
        .clone();
    assert!(shown.to_string().contains("Plugin"), "{shown}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Project templates (14.2): a project's setup (a folder, a bus with a send, plug-ins with
/// changed settings, lanes, markers, tempo, loudness target) saved without its clips; a new
/// project from it has the same setup, its plug-ins' states byte for byte, and plays the same
/// clip exactly as the original did. Then renaming, replacing and deleting templates.
#[test]
fn templates_keep_the_setup_without_the_song() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("templates");
    s.templates_dir = Some(dir.join("Templates"));
    let file = tone(&dir);
    let original = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Original", "folder": dir}),
    )
    .unwrap();
    let original = PathBuf::from(original["path"].as_str().unwrap());
    let add = |s: &mut Session, args: Value| id(&call(&r, s, "add_track", args).unwrap(), "track");
    let folder = add(&mut s, json!({"kind": "folder", "name": "Guitars"}));
    let guitar = add(
        &mut s,
        json!({"kind": "audio", "name": "Guitar", "parent": folder}),
    );
    let reverb = add(&mut s, json!({"kind": "bus", "name": "Reverb"}));
    let insert = |s: &mut Session, track: u64, plugin: &str| {
        let args = json!({"track": track, "plugin": plugin});
        id(&call(&r, s, "add_insert", args).unwrap(), "insert")
    };
    let amp = insert(&mut s, guitar, "amplify");
    insert(&mut s, guitar, "hard limiter");
    let echo = insert(&mut s, reverb, "amplify");
    for (insert, db) in [(amp, 9.0), (echo, -4.5)] {
        let params = call(
            &r,
            &mut s,
            "list_plugin_parameters",
            json!({"insert": insert}),
        );
        let left = params.unwrap()["parameters"][0]["id"].clone();
        let value = (db + 96.0) / 144.0;
        let set = json!({"insert": insert, "parameter": left, "value": value});
        call(&r, &mut s, "set_plugin_parameter", set).unwrap();
    }
    let send = json!({"track": guitar, "target": reverb, "gain_db": -6, "pre_fader": true});
    call(&r, &mut s, "add_send", send).unwrap();
    let mixer = json!({"track": guitar, "gain_db": -2.5, "pan": -0.3});
    call(&r, &mut s, "set_track_mixer", mixer).unwrap();
    call(&r, &mut s, "set_tempo", json!({"bpm": 100})).unwrap();
    call(&r, &mut s, "set_loudness_target", json!({"lufs": -16.0})).unwrap();
    call(
        &r,
        &mut s,
        "add_marker",
        json!({"position": 24_000, "name": "Verse"}),
    )
    .unwrap();
    let import = json!({"track": guitar, "path": file, "position": 0});
    call(&r, &mut s, "import_audio", import).unwrap();
    let played = mixdown(&r, &mut s, &dir, "original.wav");
    assert!(played.iter().any(|x| x.abs() > 0.1), "it plays");
    // A lane with moves: the lane goes into the template, its moves don't.
    let lane = json!({"track": guitar, "target": {"type": "volume"},
        "points": [{"tick": 0, "value": -40.0}]});
    call(&r, &mut s, "add_automation_lane", lane).unwrap();
    call(&r, &mut s, "save_project", Value::Null).unwrap();

    let saved = call(&r, &mut s, "save_template", json!({"name": "Rock: DI"})).unwrap();
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(path.file_name().unwrap(), "Rock_ DI.oxide-template");
    let listed = call(&r, &mut s, "list_templates", Value::Null).unwrap();
    assert_eq!(listed["templates"][0]["name"], "Rock_ DI");
    let template: crate::project::Project =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert!(template
        .tracks
        .iter()
        .all(|t| t.clips.is_empty() && t.midi_clips.is_empty()));
    let lanes = &template.track(guitar as u32).unwrap().automation;
    assert!(lanes.len() == 1 && lanes[0].points.is_empty(), "{lanes:?}");
    let states = |p: &crate::project::Project| -> Vec<Option<Vec<u8>>> {
        p.all_inserts()
            .map(|s| s.state.as_deref().cloned())
            .collect()
    };
    assert!(states(&template).iter().all(Option::is_some));
    assert_eq!(states(&template), states(&s.project), "as they are now");
    // The open project is untouched.
    assert_eq!(s.project.track(guitar as u32).unwrap().clips.len(), 1);
    let err = call(&r, &mut s, "save_template", json!({"name": "Rock: DI"})).unwrap_err();
    assert!(err.contains("exists already"), "{err}");

    // A new project from it.
    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Song", "folder": dir, "template": "Rock_ DI"}),
    )
    .unwrap();
    let song = PathBuf::from(made["path"].as_str().unwrap());
    assert_eq!(song, dir.join("Song").join("Song.oxide"));
    assert_eq!(s.project.name, "Song");
    let names: Vec<&str> = s.project.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, ["Guitars", "Guitar", "Reverb"]);
    let track = s.project.track(guitar as u32).unwrap();
    assert_eq!(track.parent, Some(folder as u32));
    assert_eq!((track.sends.len(), track.automation.len()), (1, 1));
    assert!(track.clips.is_empty());
    assert_eq!(
        (
            s.project.tempo,
            s.project.loudness_target,
            s.project.markers.len()
        ),
        (100.0, -16.0, 1)
    );
    let saved_song: crate::project::Project =
        serde_json::from_str(&std::fs::read_to_string(&song).unwrap()).unwrap();
    assert_eq!(states(&saved_song), states(&template), "byte for byte");
    let shown = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp})).unwrap();
    assert_eq!(shown["parameters"][0]["display"], "+9.00 dB");
    // The same clip plays exactly as it did (the lane, without its moves, does nothing).
    // Imported onto the template's track by the call a file dropped onto it makes (16.2:
    // `add_clip_from_file`), it plays through that track's plug-ins.
    let import = json!({"track": guitar, "path": file, "position": 0});
    call(&r, &mut s, "import_audio", import).unwrap();
    assert_eq!(mixdown(&r, &mut s, &dir, "from-template.wav"), played);
    assert!(
        std::fs::read_to_string(&original).unwrap().contains("tone"),
        "the original keeps its clip"
    );

    // Replace, rename, delete; a template that isn't there makes no folder.
    call(
        &r,
        &mut s,
        "save_template",
        json!({"name": "Rock_ DI", "replace": true}),
    )
    .unwrap();
    let renamed = call(
        &r,
        &mut s,
        "rename_template",
        json!({"name": "Rock_ DI", "new_name": "Rock"}),
    )
    .unwrap();
    assert!(renamed["path"]
        .as_str()
        .unwrap()
        .ends_with("Rock.oxide-template"));
    assert!(!path.exists());
    let listed = call(&r, &mut s, "list_templates", Value::Null).unwrap();
    assert_eq!(listed["templates"].as_array().unwrap().len(), 1);
    let err = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Other", "folder": dir, "template": "Jazz"}),
    )
    .unwrap_err();
    assert!(err.contains("no template called \"Jazz\""), "{err}");
    assert!(!dir.join("Other").exists());
    let err = call(&r, &mut s, "new_project", json!({"template": "Rock"})).unwrap_err();
    assert!(err.contains("needs `name` and `folder`"), "{err}");
    call(&r, &mut s, "delete_template", json!({"name": "Rock"})).unwrap();
    let listed = call(&r, &mut s, "list_templates", Value::Null).unwrap();
    assert!(listed["templates"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// Plug-in order (13.8): the chain runs top to bottom, so +12 dB into a limiter stays under
/// the ceiling and the limiter into +12 dB doesn't; a plug-in moved to another track takes its
/// settings and lanes and plays the same there; a copy has the original's settings.
#[test]
fn plug_in_order_is_the_signals_and_moves_across_tracks() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("insert-order");
    let file = tone(&dir);
    let add = |s: &mut Session| {
        let track = id(
            &call(&r, s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let import = json!({"track": track, "path": file, "position": 0});
        call(&r, s, "import_audio", import).unwrap();
        track
    };
    let (first, second) = (add(&mut s), add(&mut s));
    let insert = |s: &mut Session, track: u64, plugin: &str| {
        let args = json!({"track": track, "plugin": plugin});
        call(&r, s, "add_insert", args).unwrap()["insert"]
            .as_u64()
            .unwrap()
    };
    let amp = insert(&mut s, first, "amplify");
    let limiter = insert(&mut s, first, "hard limiter");
    let params = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp})).unwrap();
    let left = params["parameters"][0]["id"].clone();
    let plus_12 = (12.0 + 96.0) / 144.0;
    let set = json!({"insert": amp, "parameter": left, "value": plus_12});
    call(&r, &mut s, "set_plugin_parameter", set).unwrap();
    let alone = |s: &mut Session, track: u64, name: &str| {
        let other = if track == first { second } else { first };
        call(
            &r,
            s,
            "set_track_mixer",
            json!({"track": other, "mute": true}),
        )
        .unwrap();
        call(
            &r,
            s,
            "set_track_mixer",
            json!({"track": track, "mute": false}),
        )
        .unwrap();
        mixdown(&r, s, &dir, name)
    };
    let peak = |x: &[f32]| x.iter().fold(0.0f32, |m, v| m.max(v.abs()));

    // Amplify then the limiter: held under the ceiling. The limiter first: 12 dB over it.
    let limited = alone(&mut s, first, "amp-limiter.wav");
    let move_to = |s: &mut Session, args: Value| {
        call(&r, s, "move_insert", args).unwrap();
    };
    move_to(&mut s, json!({"insert": limiter, "position": 0}));
    let loud = alone(&mut s, first, "limiter-amp.wav");
    assert!(peak(&limited) <= 1.0, "limited: {}", peak(&limited));
    assert!(peak(&loud) > 1.5, "then amplified: {}", peak(&loud));
    move_to(&mut s, json!({"insert": limiter, "position": 1}));
    assert_eq!(
        alone(&mut s, first, "again.wav"),
        limited,
        "back in order, the same"
    );

    // Amplify moved to the second track, with a lane on its gain: it plays the same there.
    let lane =
        json!({"track": first, "target": {"type": "parameter", "insert": amp, "parameter": left}});
    call(&r, &mut s, "add_automation_lane", lane).unwrap();
    call(&r, &mut s, "remove_insert", json!({"insert": limiter})).unwrap();
    let amplified = alone(&mut s, first, "amp.wav");
    move_to(&mut s, json!({"insert": amp, "track": second}));
    assert!(s.project.track(first as u32).unwrap().inserts.is_empty());
    assert_eq!(
        s.project.track(second as u32).unwrap().inserts[0].id,
        amp as u32
    );
    assert!(s.project.track(first as u32).unwrap().automation.is_empty());
    assert_eq!(
        s.project.track(second as u32).unwrap().automation.len(),
        1,
        "its lane too"
    );
    assert_eq!(alone(&mut s, second, "moved.wav"), amplified);
    // Undo the mutes above, then the move.
    for _ in 0..4 {
        if !s.project.track(first as u32).unwrap().inserts.is_empty() {
            break;
        }
        call(&r, &mut s, "undo", Value::Null).unwrap();
    }
    assert_eq!(
        s.project.track(first as u32).unwrap().inserts[0].id,
        amp as u32,
        "undone"
    );

    // A copy onto the second track: its own instance, the same settings.
    let copied = call(
        &r,
        &mut s,
        "copy_insert",
        json!({"insert": amp, "track": second}),
    )
    .unwrap();
    let copy = copied["insert"].as_u64().unwrap();
    assert_ne!(copy, amp);
    let shown = |s: &mut Session, insert: u64| {
        call(&r, s, "list_plugin_parameters", json!({"insert": insert})).unwrap()["parameters"][0]
            ["display"]
            .clone()
    };
    assert_eq!(shown(&mut s, copy), shown(&mut s, amp));
    assert_eq!(shown(&mut s, copy), "+12.00 dB");
    assert_eq!(
        alone(&mut s, second, "copy.wav"),
        alone(&mut s, first, "original.wav")
    );
    let err = call(
        &r,
        &mut s,
        "move_insert",
        json!({"insert": amp, "track": second, "master": true}),
    )
    .unwrap_err();
    assert!(err.contains("not both"), "{err}");
}

/// Busy work (15.2): a staged open (the app's) loads no plug-in until `update` runs, says how
/// far it is, and then plays exactly as the project was saved; a background save returns
/// before the file is written and writes what a plain save does.
#[test]
fn staged_opens_and_background_saves_finish_as_plain_ones() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("staged-open");
    let file = tone(&dir);
    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Staged", "folder": dir}),
    )
    .unwrap();
    let path = PathBuf::from(made["path"].as_str().unwrap());
    let mut inserts = Vec::new();
    for _ in 0..3 {
        let track = id(
            &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let import = json!({"track": track, "path": file, "position": 0});
        call(&r, &mut s, "import_audio", import).unwrap();
        for plugin in ["amplify", "hard limiter"] {
            let added = call(
                &r,
                &mut s,
                "add_insert",
                json!({"track": track, "plugin": plugin}),
            );
            inserts.push(id(&added.unwrap(), "insert"));
        }
    }
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": inserts[0]}),
    );
    let left = params.unwrap()["parameters"][0]["id"].clone();
    let set = json!({"insert": inserts[0], "parameter": left, "value": (9.0 + 96.0) / 144.0});
    call(&r, &mut s, "set_plugin_parameter", set).unwrap();
    let before = mixdown(&r, &mut s, &dir, "before.wav");
    call(&r, &mut s, "save_project", Value::Null).unwrap();

    // Staged: nothing read or loaded on return; the file (the three clips share one), then
    // the plug-ins, all six in one update.
    let mut staged = headless();
    staged.stage_loads = true;
    staged.hold_file_reads(Some(0));
    staged.hold_loading(true);
    staged.open(&path).unwrap();
    let progress = staged.loading().expect("loading");
    assert_eq!(progress.stage, LoadStage::Reading);
    assert_eq!((progress.done, progress.total), (0, 1 + 6));
    assert_eq!((progress.stage_done, progress.stage_total), (0, 1));
    assert!(inserts
        .iter()
        .all(|&i| staged.insert_info(i as u32).is_none()));
    let busy = call(&r, &mut staged, "get_busy", Value::Null).unwrap();
    assert_eq!(busy["loading"]["total"], 7);
    assert_eq!(busy["loading"]["stage"], "reading");
    assert_eq!(
        (&busy["saving"], &busy["exporting"]),
        (&Value::Null, &json!(false))
    );
    staged.hold_file_reads(None);
    let started = Instant::now();
    while staged
        .loading()
        .is_some_and(|l| l.stage != LoadStage::Loading)
    {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the file never arrived"
        );
        staged.update();
    }
    let progress = staged.loading().expect("plug-ins held");
    assert_eq!((progress.done, progress.total), (1, 7));
    assert_eq!((progress.stage_done, progress.stage_total), (0, 6));
    assert_eq!(progress.next.as_deref(), Some("Amplify"));
    assert!(progress.track.is_some());
    // Held after two (the manual's shot of the bar on its plug-ins): the third is next.
    staged.hold_plugin_loads(Some(2));
    staged.hold_loading(false);
    staged.update();
    staged.update();
    let progress = staged.loading().expect("held after two");
    assert_eq!((progress.stage_done, progress.stage_total), (2, 6));
    assert_eq!(progress.next.as_deref(), Some("Amplify"));
    assert_eq!(
        inserts
            .iter()
            .filter(|&&i| staged.insert_info(i as u32).is_some())
            .count(),
        2
    );
    staged.hold_plugin_loads(None);
    staged.update();
    let idle = call(&r, &mut staged, "get_busy", Value::Null).unwrap();
    assert!(idle["loading"].is_null(), "{idle}");
    assert!(
        staged.loading().is_none(),
        "six built-in effects load in one frame's budget"
    );
    assert!(inserts
        .iter()
        .all(|&i| staged.insert_info(i as u32).is_some()));
    assert_eq!(mixdown(&r, &mut staged, &dir, "staged.wav"), before);

    // In the background: back before it is written (or just after), done after updates, and
    // the same bytes as a plain save of the same project.
    let other = dir.join("Elsewhere");
    std::fs::create_dir_all(&other).unwrap();
    let background = other.join("Staged.oxide");
    staged.save_in_background(&background).unwrap();
    assert_eq!(staged.saving(), Some(background.as_path()));
    let err = staged.save_in_background(&background).unwrap_err();
    assert!(err.to_string().contains("already being written"), "{err}");
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while staged.saving().is_some() && std::time::Instant::now() < deadline {
        staged.update();
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(staged.saving().is_none());
    assert_eq!(staged.path.as_deref(), Some(background.as_path()));
    let plain = dir.join("Plain").join("Staged.oxide");
    staged.save(&plain).unwrap();
    assert_eq!(
        std::fs::read(&background).unwrap(),
        std::fs::read(&plain).unwrap(),
        "a background save writes what a plain one does"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Tracks reordered (15.5) through `move_track_to`, the call a header drag makes: to the top,
/// the bottom, into a folder at a position and out again, the folder's sum following (its
/// -6 dB applies only while the track is inside); the impossible places are refused.
#[test]
fn tracks_move_to_any_place_and_their_folders_follow() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("move-track-to");
    let file = tone(&dir);
    let track = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let (a, b) = (track(&mut s, "audio"), track(&mut s, "audio"));
    let (f, g) = (track(&mut s, "folder"), track(&mut s, "folder"));
    let mixer = json!({"track": f, "gain_db": -6.0});
    call(&r, &mut s, "set_track_mixer", mixer).unwrap();
    let import = json!({"track": a, "path": file, "position": 0});
    call(&r, &mut s, "import_audio", import).unwrap();
    let order = |s: &Session| -> Vec<(u64, Option<u64>)> {
        s.project
            .tracks
            .iter()
            .map(|t| (u64::from(t.id), t.parent.map(u64::from)))
            .collect()
    };
    let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let to = |s: &mut Session, args: Value| call(&r, s, "move_track_to", args);
    let dry = peak(&mixdown(&r, &mut s, &dir, "dry.wav"));

    // Into f (at its end): its -6 dB applies.
    to(&mut s, json!({"track": a, "folder": f})).unwrap();
    assert_eq!(order(&s), [(b, None), (f, None), (a, Some(f)), (g, None)]);
    let inside = peak(&mixdown(&r, &mut s, &dir, "inside.wav"));
    let gain = 20.0 * (inside / dry).log10();
    assert!((gain + 6.0).abs() < 0.01, "the folder's sum: {gain:.3} dB");
    // Before a: into f too, first.
    to(&mut s, json!({"track": b, "before": a})).unwrap();
    assert_eq!(
        order(&s),
        [(f, None), (b, Some(f)), (a, Some(f)), (g, None)]
    );
    // To the top, before the folder; the folder takes its sub-tracks to the bottom.
    to(&mut s, json!({"track": g, "before": f})).unwrap();
    assert_eq!(
        order(&s),
        [(g, None), (f, None), (b, Some(f)), (a, Some(f))]
    );
    to(&mut s, json!({"track": f})).unwrap();
    assert_eq!(
        order(&s),
        [(g, None), (f, None), (b, Some(f)), (a, Some(f))]
    );
    to(&mut s, json!({"track": g, "after": f})).unwrap();
    assert_eq!(
        order(&s),
        [(f, None), (b, Some(f)), (a, Some(f)), (g, None)]
    );
    // Into f at the top, then out after it: dry again.
    to(&mut s, json!({"track": a, "folder": f, "position": 0})).unwrap();
    assert_eq!(
        order(&s),
        [(f, None), (a, Some(f)), (b, Some(f)), (g, None)]
    );
    to(&mut s, json!({"track": a, "after": f})).unwrap();
    assert_eq!(order(&s), [(f, None), (b, Some(f)), (a, None), (g, None)]);
    let out = peak(&mixdown(&r, &mut s, &dir, "out.wav"));
    assert_eq!(out, dry, "out of the folder, no -6 dB");

    let before = s.project.clone();
    for (args, says) in [
        (json!({"track": f, "folder": f}), "inside itself"),
        (json!({"track": f, "before": b}), "inside it"),
        (json!({"track": a, "before": 999}), "999"),
        (json!({"track": a, "folder": b}), "not a folder"),
        (json!({"track": a, "before": b, "after": g}), "at most one"),
    ] {
        let err = to(&mut s, args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    assert_eq!(s.project, before, "refused moves change nothing");
    // One undo step each.
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(
        order(&s),
        [(f, None), (a, Some(f)), (b, Some(f)), (g, None)]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Clips between tracks (15.4), through the calls the arrangement's drag makes: an audio clip
/// moved onto another track plays through that track's effects, a MIDI clip goes to another
/// instrument track, the wrong kinds are refused and change nothing, and an Alt-drag's copy
/// plays beside the original.
#[test]
fn clips_move_and_copy_between_tracks_and_play_from_there() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("clips-between-tracks");
    let file = tone(&dir);
    let track = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let (a, b) = (track(&mut s, "audio"), track(&mut s, "audio"));
    let (keys, synth) = (track(&mut s, "instrument"), track(&mut s, "instrument"));
    let folder = track(&mut s, "folder");
    // Track b's Amplify: +9 dB on the left.
    let amp = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": b, "plugin": "amplify"}),
        )
        .unwrap(),
        "insert",
    );
    let params = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp}));
    let left = params.unwrap()["parameters"][0]["id"].clone();
    let set = json!({"insert": amp, "parameter": left, "value": (9.0 + 96.0) / 144.0});
    call(&r, &mut s, "set_plugin_parameter", set).unwrap();
    let import = json!({"track": a, "path": file, "position": 0});
    let clip = id(&call(&r, &mut s, "import_audio", import).unwrap(), "clip");
    let midi = id(
        &call(
            &r,
            &mut s,
            "add_midi_clip",
            json!({"track": keys, "bar": 1, "bars": 1}),
        )
        .unwrap(),
        "clip",
    );
    let peak = |samples: &[f32]| samples.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let dry = peak(&mixdown(&r, &mut s, &dir, "on-a.wav"));
    assert!(dry > 0.1, "{dry}");

    let moves = |s: &mut Session, clips: Value, rows: i64| {
        let args = json!({"clips": clips, "delta": 0, "track_delta": rows});
        call(&r, s, "move_clips", args)
    };
    moves(&mut s, json!([clip]), 1).unwrap();
    assert_eq!(s.project.clip_track(clip as u32), Some(b as u32));
    let wet = peak(&mixdown(&r, &mut s, &dir, "on-b.wav"));
    let gain = 20.0 * (wet / dry).log10();
    assert!(
        (gain - 9.0).abs() < 0.01,
        "plays through b's Amplify: {gain:.3} dB"
    );

    moves(&mut s, json!([midi]), 1).unwrap();
    assert_eq!(s.project.clip_track(midi as u32), Some(synth as u32));
    let before = s.project.clone();
    let not_audio = moves(&mut s, json!([midi]), -2).unwrap_err();
    assert!(
        not_audio.contains("isn't an instrument track"),
        "{not_audio}"
    );
    let not_folder = moves(&mut s, json!([clip]), 3).unwrap_err();
    assert!(not_folder.contains("folder or bus"), "{not_folder}");
    let copy_args = json!({"clips": [clip], "delta": 0, "track_delta": 3});
    let refused = call(&r, &mut s, "copy_clips_to", copy_args).unwrap_err();
    assert!(refused.contains("folder or bus"), "{refused}");
    assert_eq!(s.project, before, "refused moves and copies change nothing");
    assert!(s.project.track(folder as u32).unwrap().clips.is_empty());

    // An Alt-drag back up: a copy on a, the clip still on b; both play.
    let copy_args = json!({"clips": [clip], "delta": 0, "track_delta": -1});
    let copied = call(&r, &mut s, "copy_clips_to", copy_args).unwrap();
    let copy = copied["clips"][0].as_u64().unwrap();
    assert_ne!(copy, clip);
    assert_eq!(s.project.clip_track(copy as u32), Some(a as u32));
    assert_eq!(s.project.clip_track(clip as u32), Some(b as u32));
    let both = peak(&mixdown(&r, &mut s, &dir, "both.wav"));
    let expected = dry * (1.0 + 10f32.powf(9.0 / 20.0));
    assert!((both / expected - 1.0).abs() < 1e-3, "{both} vs {expected}");
    // One undo takes the copy back.
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert!(s.project.find_clip(copy as u32).is_none());
    assert_eq!(s.project.clip_track(clip as u32), Some(b as u32));
    let _ = std::fs::remove_dir_all(dir);
}

/// Save before closing (15.3): the project has unsaved changes exactly when it differs from
/// its file: an edit has, the same edit undone hasn't, a plug-in turned by hand (no undo step)
/// has, saving (plain or in the background) or reopening clears it, a backup opened in its
/// place always has; and a Waveform view file with edits is listed.
#[test]
fn unsaved_changes_follow_the_project_not_its_history() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("unsaved");
    let unsaved = |s: &mut Session| {
        let got = call(&r, s, "get_unsaved_changes", Value::Null).unwrap();
        got["project"].as_bool().unwrap()
    };
    assert!(!unsaved(&mut s), "a fresh session has nothing to lose");
    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Unsaved", "folder": dir}),
    )
    .unwrap();
    let path = PathBuf::from(made["path"].as_str().unwrap());
    assert!(!unsaved(&mut s), "a new project is saved as it starts");

    call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap();
    assert!(unsaved(&mut s));
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert!(!unsaved(&mut s), "an edit undone again is no change");

    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let insert = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": track, "plugin": "amplify"}),
        )
        .unwrap(),
        "insert",
    );
    call(&r, &mut s, "save_project", Value::Null).unwrap();
    assert!(!unsaved(&mut s), "saved");
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    );
    let left = params.unwrap()["parameters"][0]["id"].clone();
    let set = json!({"insert": insert, "parameter": left, "value": 0.9});
    call(&r, &mut s, "set_plugin_parameter", set).unwrap();
    assert!(
        unsaved(&mut s),
        "a plug-in turned is a change, though no undo step"
    );

    s.save_in_background(&path).unwrap();
    s.finish_saving().unwrap();
    assert!(!unsaved(&mut s), "saved in the background");
    let mixer = json!({"track": track, "gain_db": -3.0});
    call(&r, &mut s, "set_track_mixer", mixer).unwrap();
    assert!(unsaved(&mut s));
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    assert!(!unsaved(&mut s), "just opened");

    let backup = s.save_backup().unwrap();
    call(&r, &mut s, "open_backup", json!({"path": backup})).unwrap();
    assert!(unsaved(&mut s), "a backup is never the project's file");
    s.new_project();
    assert!(!unsaved(&mut s), "a new project has nothing to lose");

    let file = tone(&dir);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let clean = call(&r, &mut s, "get_unsaved_changes", Value::Null).unwrap();
    assert_eq!(clean["files"], json!([]));
    let args = json!({"document": doc, "title": "Tone"});
    call(&r, &mut s, "set_metadata", args).unwrap();
    let dirty = call(&r, &mut s, "get_unsaved_changes", Value::Null).unwrap();
    assert_eq!(dirty["files"][0]["document"], json!(doc));
    assert_eq!(dirty["project"], json!(false));
    let _ = std::fs::remove_dir_all(dir);
}

/// Copy and paste (15.1): a plug-in copied with its settings pastes onto other tracks as often
/// as asked, plays the same, and still pastes after the original is removed; copying is not
/// an undo step, pasting is.
#[test]
fn plug_ins_copy_and_paste_with_their_settings() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("plugin-paste");
    let file = tone(&dir);
    let add = |s: &mut Session| {
        let track = id(
            &call(&r, s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let import = json!({"track": track, "path": file, "position": 0});
        call(&r, s, "import_audio", import).unwrap();
        track
    };
    let (a, b, c) = (add(&mut s), add(&mut s), add(&mut s));
    let err = call(&r, &mut s, "paste_plugin", json!({"track": b})).unwrap_err();
    assert!(err.contains("no plug-in has been copied"), "{err}");
    let amp = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": a, "plugin": "amplify"}),
        )
        .unwrap(),
        "insert",
    );
    let params = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp})).unwrap();
    let left = params["parameters"][0]["id"].clone();
    let set = json!({"insert": amp, "parameter": left, "value": (7.5 + 96.0) / 144.0});
    call(&r, &mut s, "set_plugin_parameter", set).unwrap();
    let undo_before = call(&r, &mut s, "get_history", Value::Null).unwrap()["undo"]
        .as_array()
        .unwrap()
        .len();
    let copied = call(&r, &mut s, "copy_plugin", json!({"insert": amp})).unwrap();
    assert_eq!(copied["name"], "Amplify");
    let undo_after = call(&r, &mut s, "get_history", Value::Null).unwrap()["undo"]
        .as_array()
        .unwrap()
        .len();
    assert_eq!(undo_after, undo_before, "copying is no edit");
    let shown = |s: &mut Session, insert: u64| {
        call(&r, s, "list_plugin_parameters", json!({"insert": insert})).unwrap()["parameters"][0]
            ["display"]
            .clone()
    };
    let first = id(
        &call(&r, &mut s, "paste_plugin", json!({"track": b})).unwrap(),
        "insert",
    );
    let second = id(
        &call(
            &r,
            &mut s,
            "paste_plugin",
            json!({"track": b, "position": 0}),
        )
        .unwrap(),
        "insert",
    );
    assert_ne!(first, second);
    assert_eq!(shown(&mut s, first), "+7.50 dB");
    let order: Vec<u64> = s
        .project
        .track(b as u32)
        .unwrap()
        .inserts
        .iter()
        .map(|i| u64::from(i.id))
        .collect();
    assert_eq!(order, [second, first], "the second at position 0");
    // One pasted Amplify plays like the original.
    call(&r, &mut s, "remove_insert", json!({"insert": second})).unwrap();
    let alone = |s: &mut Session, track: u64, name: &str| {
        for t in [a, b, c] {
            call(
                &r,
                s,
                "set_track_mixer",
                json!({"track": t, "mute": t != track}),
            )
            .unwrap();
        }
        mixdown(&r, s, &dir, name)
    };
    assert_eq!(
        alone(&mut s, b, "pasted.wav"),
        alone(&mut s, a, "original.wav")
    );
    // The original gone, it still pastes; undo takes a paste back.
    call(&r, &mut s, "remove_insert", json!({"insert": amp})).unwrap();
    let third = id(
        &call(&r, &mut s, "paste_plugin", json!({"track": c})).unwrap(),
        "insert",
    );
    assert_eq!(shown(&mut s, third), "+7.50 dB");
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert!(s.project.track(c as u32).unwrap().inserts.is_empty());
    let err = call(&r, &mut s, "paste_plugin", json!({})).unwrap_err();
    assert!(err.contains("give a `track`"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

/// The Diagnostics panel (11.5b): clicks and clipped runs found at their places, repaired by
/// id or all as steps of the file's own history (never the project's), the rest keeping their
/// ids; the list goes stale when the file changes, and undo brings the file back exactly.
#[test]
fn audio_problems_are_found_listed_and_repaired_by_id() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("diagnostics");
    // Stereo, 48 kHz: a faded 440 Hz tone on both, clicks of 1 to 12 samples on the left at
    // 6000, 9000, ..., and two on the right; a clipped tone after them on both.
    let n = 96_000;
    let tone = |i: usize| {
        let fade = (i.min(n - 1 - i) as f32 / 480.0).min(1.0);
        fade * 0.25 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
    };
    let mut channels: Vec<Vec<f32>> = (0..2).map(|_| (0..n).map(tone).collect()).collect();
    let clicks = [
        (0, 6000, 1),
        (0, 9000, 3),
        (1, 10_500, 2),
        (0, 12_000, 7),
        (0, 15_000, 12),
    ];
    for (k, &(c, at, len)) in clicks.iter().enumerate() {
        let sign = if k % 2 == 0 { 0.5 } else { -0.5 };
        for x in &mut channels[c][at..at + len] {
            *x += sign;
        }
    }
    let original = channels.clone();
    let file = dir.join("clicks.wav");
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels,
        },
    )
    .unwrap();
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let project_history =
        |s: &mut Session| call(&r, s, "get_history", Value::Null).unwrap()["undo"].clone();
    let before = project_history(&mut s);

    let none = call(&r, &mut s, "get_audio_problems", json!({"document": doc})).unwrap();
    assert_eq!((&none["stale"], &none["count"]), (&json!(true), &json!(0)));
    let found = call(
        &r,
        &mut s,
        "find_audio_problems",
        json!({"document": doc, "kind": "click-repair"}),
    )
    .unwrap();
    assert_eq!(found["kind"], "click-repair");
    let list = found["problems"].as_array().unwrap().clone();
    assert_eq!(list.len(), clicks.len(), "{found}");
    for (p, &(c, at, len)) in list.iter().zip(&clicks) {
        let (start, length) = (id(p, "start") as usize, id(p, "length") as usize);
        assert_eq!(id(p, "channel") as usize, c);
        assert!(
            start <= at && at - start <= 4 && start + length >= at + len,
            "{p}"
        );
        assert!(p["level_db"].as_f64().unwrap() > -10.0, "{p}");
    }
    let ids: Vec<u64> = list.iter().map(|p| id(p, "id")).collect();
    // The same file scanned again, the kind by its name before 0.3.1 ("declicker", still
    // read): the same problems, under new ids, and the kind by today's name.
    let again = call(
        &r,
        &mut s,
        "find_audio_problems",
        json!({"document": doc, "kind": "declicker"}),
    )
    .unwrap();
    assert_eq!(again["kind"], "click-repair");
    let again_ids: Vec<u64> = again["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| id(p, "id"))
        .collect();
    assert_eq!(again_ids.len(), ids.len());
    assert!(
        again_ids.iter().all(|i| !ids.contains(i)),
        "ids never reused"
    );
    let ids = again_ids;

    // Two repaired: one step of the file's history, the rest keep their ids.
    let repaired = call(
        &r,
        &mut s,
        "repair_audio_problems",
        json!({"document": doc, "ids": [ids[1], ids[3]]}),
    )
    .unwrap();
    assert_eq!(repaired["repaired"], 2);
    let left: Vec<u64> = repaired["left"]["problems"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| id(p, "id"))
        .collect();
    assert_eq!(left, [ids[0], ids[2], ids[4]]);
    let view = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    assert_eq!(
        view["documents"][0]["history"]["steps"],
        json!(["Open", "Repair 2 clicks"])
    );
    let audio = s.document(doc as u32).unwrap().audio.clone();
    let moved = |c: usize| {
        (0..n)
            .filter(|&i| audio.channels[c][i] != original[c][i])
            .count()
    };
    assert!(moved(0) > 0 && moved(0) <= 7 + 12 + 8, "{}", moved(0));
    assert_eq!(moved(1), 0, "only the chosen ones");
    // A repaired id is refused by name; so is a scan's id after the file changed.
    let e = call(
        &r,
        &mut s,
        "repair_audio_problems",
        json!({"document": doc, "ids": [ids[1]]}),
    )
    .unwrap_err();
    assert!(e.contains(&ids[1].to_string()), "{e}");
    // The rest, all at once: no click is left.
    let all = call(
        &r,
        &mut s,
        "repair_audio_problems",
        json!({"document": doc, "all": true}),
    )
    .unwrap();
    assert_eq!(all["repaired"], 3);
    assert_eq!(all["left"]["count"], 0);
    let rescan = call(
        &r,
        &mut s,
        "find_audio_problems",
        json!({"document": doc, "kind": "click-repair"}),
    )
    .unwrap();
    assert_eq!(rescan["count"], 0, "{rescan}");
    assert_eq!(
        project_history(&mut s),
        before,
        "the project's history untouched"
    );

    // Undo twice: the file exactly as it was, the list stale; scanned, the clicks are back.
    for _ in 0..2 {
        call(&r, &mut s, "undo_audio_edit", json!({"document": doc})).unwrap();
    }
    assert_eq!(s.document(doc as u32).unwrap().audio.channels, original);
    let stale = call(&r, &mut s, "get_audio_problems", json!({"document": doc})).unwrap();
    assert_eq!(stale["stale"], true);
    let back = call(
        &r,
        &mut s,
        "find_audio_problems",
        json!({"document": doc, "kind": "click-repair", "start": 0, "end": 11_000}),
    )
    .unwrap();
    assert_eq!(back["count"], 3, "{back}");
    // Any edit leaves the list behind.
    call(
        &r,
        &mut s,
        "edit_audio",
        json!({"document": doc, "op": "amplify", "db": -1.0}),
    )
    .unwrap();
    let stale = call(&r, &mut s, "get_audio_problems", json!({"document": doc})).unwrap();
    assert_eq!(stale["stale"], true);
    let e = call(
        &r,
        &mut s,
        "repair_audio_problems",
        json!({"document": doc, "all": true}),
    )
    .unwrap_err();
    assert!(e.contains("scan it first"), "{e}");

    // Clip Repair: the left clipped at 0.2 from 40 000 to 50 000 (the tone's peaks there
    // are its loudest, so the scan's levels): every half cycle a run, repaired over the clip.
    let mut clipped = original.clone();
    for c in &mut clipped {
        for x in &mut c[40_000..50_000] {
            *x = x.clamp(-0.2, 0.2) * 3.0;
        }
    }
    let file = dir.join("clipped.wav");
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: clipped.clone(),
        },
    )
    .unwrap();
    let clipped_doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let runs = call(
        &r,
        &mut s,
        "find_audio_problems",
        json!({"document": clipped_doc, "kind": "declipper", "start": 40_000, "end": 50_000,
               "tolerance": 5, "min_clip_size": 4}),
    )
    .unwrap();
    // 440 Hz over 10 000 samples: about 183 half cycles a channel.
    let count = runs["count"].as_u64().unwrap();
    println!("{count} clipped runs found in 40 000..50 000 of the two channels");
    assert!((2 * 180..=2 * 186).contains(&count), "{count}");
    let first = &runs["problems"][0];
    assert_eq!(first["kind"], "clip-repair");
    assert!((first["level_db"].as_f64().unwrap() - 20.0 * 0.6f64.log10()).abs() < 1e-3);
    let done = call(
        &r,
        &mut s,
        "repair_audio_problems",
        json!({"document": clipped_doc, "all": true}),
    )
    .unwrap();
    assert_eq!(done["repaired"].as_u64(), Some(count));
    let view = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    assert_eq!(
        view["documents"][1]["history"]["steps"],
        json!(["Open", format!("Repair {count} clipped runs")])
    );
    let rebuilt = &s.document(clipped_doc as u32).unwrap().audio.channels[0];
    assert!(rebuilt[40_000..50_000].iter().any(|x| x.abs() > 0.61));

    // Bad input, each refused by name.
    for (name, args, says) in [
        (
            "find_audio_problems",
            json!({"document": 99, "kind": "click-repair"}),
            "99",
        ),
        (
            "find_audio_problems",
            json!({"document": doc, "kind": "decrackler"}),
            "decrackler",
        ),
        (
            "find_audio_problems",
            json!({"document": doc, "kind": "click-repair", "threshold": 120}),
            "Threshold",
        ),
        (
            "find_audio_problems",
            json!({"document": doc, "kind": "click-repair", "start": 5}),
            "both",
        ),
        (
            "find_audio_problems",
            json!({"document": doc, "kind": "click-repair", "start": 0, "end": 1_000_000}),
            "1000000",
        ),
        (
            "repair_audio_problems",
            json!({"document": doc, "ids": [1], "all": true}),
            "not both",
        ),
        ("repair_audio_problems", json!({"document": doc}), "ids"),
    ] {
        let e = call(&r, &mut s, name, args).unwrap_err();
        assert!(e.contains(says), "{name}: {e}");
    }
}

/// Metadata (13.7): a file's tags set (only the fields given), a step of its own history
/// that undo takes back, written by Save and by Export, and read again when it opens.
#[test]
fn tags_are_edited_saved_and_exported() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("tags");
    let file = tone(&dir);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let empty = call(&r, &mut s, "get_metadata", json!({"document": doc})).unwrap();
    assert_eq!(
        (&empty["title"], &empty["cover"]),
        (&json!(""), &Value::Null)
    );
    let png = dir.join("cover.png");
    std::fs::write(&png, [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 9, 9]).unwrap();
    let args = json!({
        "document": doc, "title": "Tone", "artist": "Oxide", "track": "1/2", "cover": png,
    });
    let set = call(&r, &mut s, "set_metadata", args).unwrap();
    assert_eq!(
        (&set["title"], &set["artist"], &set["track"]),
        (&json!("Tone"), &json!("Oxide"), &json!("1/2"))
    );
    assert_eq!(set["cover"], json!({"mime": "image/png", "bytes": 10}));
    let set = call(
        &r,
        &mut s,
        "set_metadata",
        json!({"document": doc, "album": "Tests"}),
    )
    .unwrap();
    assert_eq!(
        (&set["title"], &set["album"]),
        (&json!("Tone"), &json!("Tests")),
        "the rest kept"
    );

    // A step of the file's history: undo goes back to the tags before it.
    let undone = call(&r, &mut s, "undo_audio_edit", json!({"document": doc})).unwrap();
    assert_eq!(undone["document"]["dirty"], true);
    let now = call(&r, &mut s, "get_metadata", json!({"document": doc})).unwrap();
    assert_eq!((&now["title"], &now["album"]), (&json!("Tone"), &json!("")));
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert!(history["undo"]
        .as_array()
        .unwrap()
        .iter()
        .all(|h| h != "Set metadata"));

    // Exported as an MP3, and saved in place (a WAV): both read back with the tags.
    let mp3 = dir.join("tone.mp3");
    let args = json!({"document": doc, "path": mp3, "format": {"type": "mp3", "bitrate": 128}});
    call(&r, &mut s, "export_audio", args).unwrap();
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    let read = crate::metadata::read(&mp3);
    assert_eq!((read.title.as_str(), read.track.as_str()), ("Tone", "1/2"));
    assert_eq!(read.cover.map(|c| c.data.len()), Some(10));
    call(&r, &mut s, "save_audio", json!({"document": doc})).unwrap();
    let saved = s.document(doc as u32).unwrap().path.clone();
    let read = crate::metadata::read(&saved);
    assert_eq!(
        (read.title.as_str(), read.artist.as_str()),
        ("Tone", "Oxide")
    );
    call(&r, &mut s, "close_audio_editor", json!({"document": doc})).unwrap();
    let again = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": saved})).unwrap(),
        "document",
    );
    let reopened = call(&r, &mut s, "get_metadata", json!({"document": again})).unwrap();
    assert_eq!(reopened["title"], "Tone");

    let gif = dir.join("cover.gif");
    std::fs::write(&gif, b"GIF89a").unwrap();
    let err = call(
        &r,
        &mut s,
        "set_metadata",
        json!({"document": again, "cover": gif}),
    )
    .unwrap_err();
    assert!(err.contains("JPEG or PNG"), "{err}");
}

/// Export formats (13.6): a mixdown and a Waveform view document in other formats, decoded
/// back at the right length and rate; the extension follows the format; bad settings refused.
#[test]
fn exports_take_a_format() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("formats");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let import = json!({"track": track, "path": file, "position": 0});
    call(&r, &mut s, "import_audio", import).unwrap();

    // The mix as a 24-bit FLAC at 44.1 kHz (the path given no extension).
    let args = json!({
        "path": dir.join("mix"), "end": 48_000, "tail_seconds": 0,
        "format": {"type": "flac", "bits": 24, "level": 5}, "sample_rate": 44_100,
    });
    let started = call(&r, &mut s, "export_mixdown", args).unwrap();
    // Running, it says what it is making (it may be done already: then it's finished).
    if started["state"] == "running" {
        assert_eq!(started["kind"], "export");
        assert_eq!(started["what"], "Exporting mix.flac");
    }
    let flac = s.wait_for_export(Duration::from_secs(30)).unwrap();
    assert_eq!(flac, dir.join("mix.flac"));
    let read = crate::media::read_audio(&flac).unwrap();
    assert_eq!((read.sample_rate, read.channels[0].len()), (44_100, 44_100));

    // A document as an MP3, its file untouched.
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let before = std::fs::read(&file).unwrap();
    let args = json!({
        "document": doc, "path": dir.join("tone.mp3"),
        "format": {"type": "mp3", "bitrate": 192},
    });
    let made = call(&r, &mut s, "export_audio", args).unwrap();
    let mp3 = s.wait_for_export(Duration::from_secs(30)).unwrap();
    assert_eq!(made["path"], json!(mp3));
    let read = crate::media::read_audio(&mp3).unwrap();
    assert_eq!((read.sample_rate, read.channels[0].len()), (48_000, 48_000));
    assert_eq!(std::fs::read(&file).unwrap(), before);

    for (args, says) in [
        (
            json!({"document": doc, "path": dir.join("x.wav"), "format": {"type": "ogg", "quality": 5}}),
            "use .ogg",
        ),
        (
            json!({"document": doc, "path": dir.join("x.mp3"), "format": {"type": "mp3"}, "sample_rate": 96_000}),
            "can't be 96000 Hz",
        ),
        (
            json!({"document": doc, "path": dir.join("x.flac"), "format": {"type": "flac", "bits": 32, "level": 5}}),
            "16 or 24-bit",
        ),
    ] {
        let err = call(&r, &mut s, "export_audio", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
}

/// Mixdown to a new file (13.5): bit for bit an export of the same range (not normalised),
/// written into the project's Rendered folder, and opened as a document for the Waveform view.
#[test]
fn a_mixdown_is_the_export_and_opens_for_editing() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("mixdown");
    let made = json!({"name": "Mix", "folder": dir});
    call(&r, &mut s, "new_project", made).unwrap();
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let import = json!({"track": track, "path": file, "position": 0});
    call(&r, &mut s, "import_audio", import).unwrap();
    call(&r, &mut s, "add_insert", json!({"plugin": "hard limiter"})).unwrap();
    let mix = json!({"track": track, "gain_db": 4.0, "pan": 0.5});
    call(&r, &mut s, "set_track_mixer", mix).unwrap();

    let export = dir.join("export.wav");
    let args = json!({"path": export, "start": 6000, "end": 30_000, "tail_seconds": 0.25});
    call(&r, &mut s, "export_mixdown", args).unwrap();
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    let args = json!({"start": 6000, "end": 30_000, "tail_seconds": 0.25});
    let made = call(&r, &mut s, "mixdown_to_new_file", args).unwrap();
    let path = PathBuf::from(made["path"].as_str().unwrap());
    assert!(
        path.starts_with(dir.join("Mix").join("Rendered")),
        "{path:?}"
    );
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    s.update();
    let exported = wav::read(&export).unwrap();
    let mixed = wav::read(&path).unwrap();
    assert_eq!(mixed.channels.len(), 2);
    assert_eq!(mixed.channels[0].len(), 24_000 + 12_000);
    assert!(
        mixed.channels == exported.channels,
        "the mixdown is the export"
    );

    let doc = s.take_opened_mixdown().expect("opened");
    // A muted master would mix down to silence: refused, saying so.
    call(&r, &mut s, "set_master", json!({"mute": true})).unwrap();
    let err = call(&r, &mut s, "mixdown_to_new_file", Value::Null).unwrap_err();
    assert!(err.contains("the master is muted: unmute it"), "{err}");
    let editor = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    let docs = editor["documents"].as_array().unwrap();
    assert!(
        docs.iter()
            .any(|d| d["document"] == doc && d["path"] == json!(path)),
        "{editor}"
    );
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert!(
        !history["undo"]
            .as_array()
            .unwrap()
            .iter()
            .any(|h| h == "Mixdown to new file"),
        "not an edit of the project"
    );
}

/// A track's MIDI as one file (13.4): every clip at its place in the song, cropped as it
/// plays, with the tempo and time signature changes; read back, and imported again, every note
/// lands on the same tick.
#[test]
fn a_tracks_midi_exports_as_one_file_with_its_tempo_map() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("midi-track");
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    let clip_at = |s: &mut Session, bar: u64, bars: u64| {
        let args = json!({"track": keys, "bar": bar, "bars": bars});
        id(&call(&r, s, "add_midi_clip", args).unwrap(), "clip")
    };
    let first = clip_at(&mut s, 2, 1);
    let second = clip_at(&mut s, 4, 1);
    let notes = |clip: u64, notes: Value| json!({"clip": clip, "notes": notes});
    call(
        &r,
        &mut s,
        "add_notes",
        notes(
            first,
            json!([
                {"tick": 0, "length": 960, "pitch": 60},
                {"tick": 3000, "length": 2000, "pitch": 64},
            ]),
        ),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "add_notes",
        notes(
            second,
            json!([{"tick": 480, "length": 240, "pitch": 67, "velocity": 90}]),
        ),
    )
    .unwrap();
    // From bar 3, 90 quarter notes a minute in 3/4.
    call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 3, "bpm": 90, "numerator": 3, "denominator": 4}),
    )
    .unwrap();
    let path = dir.join("keys.mid");
    call(
        &r,
        &mut s,
        "export_midi_track",
        json!({"track": keys, "path": path}),
    )
    .unwrap();

    let file = crate::smf::read_path(&path).unwrap();
    assert_eq!(file.tracks.len(), 1);
    let bar = 3840;
    let starts = s
        .project
        .track(keys as u32)
        .unwrap()
        .midi_clips
        .iter()
        .map(|c| c.start)
        .collect::<Vec<_>>();
    assert_eq!(starts[0], bar, "bar 2");
    let read: Vec<(u64, u64, u8)> = file.tracks[0]
        .notes
        .iter()
        .map(|n| (n.tick, n.length, n.pitch))
        .collect();
    // The second note is cropped at its clip's end (one 4/4 bar), as it plays.
    assert_eq!(
        read,
        [
            (bar, 960, 60),
            (bar + 3000, bar - 3000, 64),
            (starts[1] + 480, 240, 67)
        ]
    );
    assert_eq!(file.tempos, [(0, 120.0), (2 * bar, 90.0)]);
    assert_eq!(file.meters, [(0, (4, 4)), (2 * bar, (3, 4))]);

    // Imported again at bar 1 on a new track, the notes land where they were.
    let other = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    let imported = call(
        &r,
        &mut s,
        "import_midi_file",
        json!({"path": path, "track": other, "bar": 1}),
    )
    .unwrap();
    let clip = &s
        .project
        .find_midi_clip(imported["clips"][0]["clip"].as_u64().unwrap() as u32)
        .unwrap()
        .1;
    let landed: Vec<u64> = clip.notes.iter().map(|n| clip.start + n.tick).collect();
    assert_eq!(landed, [bar, bar + 3000, starts[1] + 480]);

    let audio = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let err = call(
        &r,
        &mut s,
        "export_midi_track",
        json!({"track": audio, "path": path}),
    )
    .unwrap_err();
    assert!(err.contains("has no MIDI clips"), "{err}");
}

/// A track's render (13.3) is the track as it plays: the new track alone exports the same as
/// the original alone, bit for bit (an effect, a fader and a pan on it, a folder over it with
/// its own effect), an effect whose output depends on where its blocks fall too (the Graphic
/// EQ's FFT: a render runs from the song's start, as the export it is compared with); the
/// original is switched off (out of the graph) in the same undo step, which undo takes back; a
/// disabled track is silent and a disabled folder silences its tracks.
#[test]
fn a_rendered_track_plays_exactly_like_the_track() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("render-track");
    call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Render", "folder": dir}),
    )
    .unwrap();
    let file = tone(&dir);
    let add = |s: &mut Session, args: Value| id(&call(&r, s, "add_track", args).unwrap(), "track");
    let folder = add(&mut s, json!({"kind": "folder"}));
    let guitar = add(&mut s, json!({"kind": "audio", "parent": folder}));
    let other = add(&mut s, json!({"kind": "audio"}));
    for track in [guitar, other] {
        call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 4800}),
        )
        .unwrap();
    }
    call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": guitar, "plugin": "parametric equalizer"}),
    )
    .unwrap();
    for (track, plugin) in [
        (folder, "hard limiter"),
        (other, "graphic equalizer (10 bands)"),
    ] {
        let args = json!({"track": track, "plugin": plugin});
        call(&r, &mut s, "add_insert", args).unwrap();
    }
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": guitar, "gain_db": -3.0, "pan": 0.4}),
    )
    .unwrap();
    // The master's effects and level stay out of a track's render.
    call(&r, &mut s, "add_insert", json!({"plugin": "amplify"})).unwrap();
    let export_alone = |s: &mut Session, solo: u64, name: &str| {
        call(
            &r,
            s,
            "set_track_mixer",
            json!({"track": other, "mute": solo != other}),
        )
        .unwrap();
        mixdown(&r, s, &dir, name)
    };

    // Render the folder (its guitar and its limiter), leaving it on; the tail is the file's.
    let rendered = call(
        &r,
        &mut s,
        "render_track",
        json!({"track": folder, "tail_seconds": 0.5, "disable": false}),
    )
    .unwrap();
    let path = PathBuf::from(rendered["path"].as_str().unwrap());
    assert!(
        path.starts_with(dir.join("Render").join("Rendered")),
        "{path:?}"
    );
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    s.update();
    let new = s.take_rendered_track().expect("a new track");
    let file_len = wav::read(&path).unwrap().channels[0].len();
    assert_eq!(
        file_len,
        4_800 + 48_000 + 24_000,
        "from the song's start to the clip's end, plus the tail"
    );
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    let names: Vec<&str> = project["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names[..3],
        ["Folder 1", "Audio 1", "Folder 1 (rendered)"][..],
        "under it"
    );
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Render track");

    // The folder alone (its render off), and its render alone (the folder off): the same,
    // bit for bit.
    let enable = |s: &mut Session, track: u64, enabled: bool| {
        let args = json!({"track": track, "enabled": enabled});
        call(&r, s, "set_track_enabled", args).unwrap();
    };
    enable(&mut s, new as u64, false);
    let original = export_alone(&mut s, folder, "folder.wav");
    enable(&mut s, new as u64, true);
    enable(&mut s, folder, false);
    let render = export_alone(&mut s, folder, "render.wav");
    assert!(original.iter().any(|x| x.abs() > 0.01), "it plays");
    let first = render.iter().zip(&original).position(|(a, b)| a != b);
    let worst = render
        .iter()
        .zip(&original)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        first.is_none() && render.len() == original.len(),
        "the render differs from the folder: first at {first:?} ({:?} vs {:?}), worst {worst}",
        first.map(|i| render[i]),
        first.map(|i| original[i])
    );
    // Off, the folder and its guitar are silent: with the render off too, nothing plays.
    enable(&mut s, new as u64, false);
    let silent = export_alone(&mut s, folder, "silent.wav");
    assert!(silent.iter().all(|x| *x == 0.0));
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Set track enabled");

    // A muted track isn't rendered: it says to unmute it. A solo elsewhere doesn't matter.
    // Rendering with `disable` switches the original off in the same step; undo puts it back.
    call(
        &r,
        &mut s,
        "set_track_enabled",
        json!({"track": folder, "enabled": true}),
    )
    .unwrap();
    let err = call(&r, &mut s, "render_track", json!({"track": other})).unwrap_err();
    assert!(err.contains("is muted: unmute it to render it"), "{err}");
    let set = |s: &mut Session, args: Value| {
        call(&r, s, "set_track_mixer", args).unwrap();
    };
    set(&mut s, json!({"track": other, "mute": false}));
    set(&mut s, json!({"track": folder, "solo": true}));
    call(&r, &mut s, "render_track", json!({"track": other})).unwrap();
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    set(&mut s, json!({"track": folder, "solo": false}));
    s.update();
    let rendered = s.take_rendered_track().expect("a new track") as u64;
    assert!(!s.project.track(other as u32).unwrap().enabled);
    let tracks = s.project.tracks.len();
    let render = export_alone(&mut s, rendered, "eq-render.wav");
    enable(&mut s, rendered, false);
    enable(&mut s, other, true);
    let original = export_alone(&mut s, other, "eq.wav");
    let worst = render
        .iter()
        .zip(&original)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst == 0.0, "the Graphic EQ's render is {worst} off");
    // Undo the switches and mutes above, then the render itself.
    while s.project.tracks.len() == tracks {
        call(&r, &mut s, "undo", Value::Null).unwrap();
    }
    assert!(s.project.track(other as u32).unwrap().enabled);
    assert_eq!(s.project.tracks.len(), tracks - 1);

    let empty = add(&mut s, json!({"kind": "audio"}));
    let err = call(&r, &mut s, "render_track", json!({"track": empty})).unwrap_err();
    assert!(err.contains("no clips to render"), "{err}");
}

/// A track's render starts at the song's start: the file runs from sample 0 to the last
/// clip's end plus the tail, exactly silent before the first clip (nothing upstream sounds
/// there), its clip on the new track at 0, lined up with the original; from the first clip on
/// it is what a render from there gave (an export of the track alone from the clip's start),
/// and the whole of it what an export of the track alone from 0 gives.
#[test]
fn a_tracks_render_starts_at_the_songs_start() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("render-start");
    call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Start", "folder": dir}),
    )
    .unwrap();
    let file = tone(&dir);
    let add = |s: &mut Session, args: Value| id(&call(&r, s, "add_track", args).unwrap(), "track");
    let guitar = add(&mut s, json!({"kind": "audio"}));
    let other = add(&mut s, json!({"kind": "audio"}));
    let clip_at = 30_000u64;
    for (track, at) in [(guitar, clip_at), (other, 0)] {
        let args = json!({"track": track, "path": file, "position": at});
        call(&r, &mut s, "import_audio", args).unwrap();
    }
    for plugin in ["parametric equalizer", "hard limiter"] {
        let args = json!({"track": guitar, "plugin": plugin});
        call(&r, &mut s, "add_insert", args).unwrap();
    }
    let args = json!({"track": guitar, "gain_db": -2.0, "pan": -0.3});
    call(&r, &mut s, "set_track_mixer", args).unwrap();

    let rendered = call(
        &r,
        &mut s,
        "render_track",
        json!({"track": guitar, "tail_seconds": 0.5, "disable": false}),
    )
    .unwrap();
    let path = PathBuf::from(rendered["path"].as_str().unwrap());
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    s.update();
    let new = s.take_rendered_track().expect("a new track");
    let render = wav::read(&path).unwrap().channels;
    let len = clip_at as usize + 48_000 + 24_000;
    assert!(render.iter().all(|c| c.len() == len), "not {len} long");
    let lead = clip_at as usize;
    for (k, channel) in render.iter().enumerate() {
        let loud = channel[..lead].iter().position(|x| *x != 0.0);
        assert_eq!(loud, None, "channel {k} sounds before the clip");
        assert!(channel[lead..].iter().any(|x| x.abs() > 0.01), "it plays");
    }
    let clips = &s.project.track(new).expect("the render's track").clips;
    assert_eq!(clips.len(), 1);
    assert_eq!(
        clips[0].start, 0,
        "the render's clip lines up with the song"
    );

    // The track alone (its render off, the other track muted), exported.
    call(
        &r,
        &mut s,
        "set_track_enabled",
        json!({"track": new, "enabled": false}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": other, "mute": true}),
    )
    .unwrap();
    let export = |s: &mut Session, start: u64, name: &str| -> Vec<Vec<f32>> {
        let out = dir.join(name);
        let end = clip_at + 48_000;
        let args = json!({"path": out, "start": start, "end": end, "tail_seconds": 0.5});
        call(&r, s, "export_mixdown", args).unwrap();
        s.wait_for_export(Duration::from_secs(20)).unwrap();
        wav::read(&out).unwrap().channels
    };
    // From the clip on: what the render from the first clip gave before.
    let from_clip = export(&mut s, clip_at, "from-clip.wav");
    for (k, (ours, theirs)) in render.iter().zip(&from_clip).enumerate() {
        assert_eq!(theirs.len(), len - lead);
        let first = ours[lead..].iter().zip(theirs).position(|(a, b)| a != b);
        assert_eq!(first, None, "channel {k} differs from the clip on");
    }
    // The whole file: an export of the track from 0.
    let from_start = export(&mut s, 0, "from-start.wav");
    assert_eq!(
        render, from_start,
        "the render differs from an export from 0"
    );
}

/// The CPU panel's numbers (12.5): every track and the master with their inserts, from the
/// timing in the graph, after processing (here an export) has run.
#[test]
fn cpu_usage_lists_tracks_and_inserts_by_cost() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("cpu-usage");
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let insert = |s: &mut Session, args: Value| {
        call(&r, s, "add_insert", args).unwrap()["insert"]
            .as_u64()
            .unwrap()
    };
    let amplify = insert(&mut s, json!({"track": track, "plugin": "amplify"}));
    let eq = insert(
        &mut s,
        json!({"track": track, "plugin": "graphic equalizer (10 bands)"}),
    );
    let limiter = insert(&mut s, json!({"plugin": "hard limiter"}));
    // Read once to start a window; then process, and read when the window has passed.
    call(&r, &mut s, "get_cpu_usage", Value::Null).unwrap();
    mixdown(&r, &mut s, &dir, "cpu.wav");
    std::thread::sleep(crate::usage::WINDOW);
    let usage = call(&r, &mut s, "get_cpu_usage", Value::Null).unwrap();
    assert!(usage["window"].as_f64().unwrap() >= 0.5, "{usage}");
    let tracks = usage["tracks"].as_array().unwrap();
    assert_eq!(tracks.len(), 2, "the master and the track");
    let costs: Vec<f64> = tracks
        .iter()
        .map(|t| t["load"]["per_buffer_us"].as_f64().unwrap())
        .collect();
    assert!(
        costs.windows(2).all(|w| w[0] >= w[1]),
        "most costly first: {costs:?}"
    );
    let mine = tracks.iter().find(|t| t["track"] == track).unwrap();
    let master = tracks.iter().find(|t| t["track"].is_null()).unwrap();
    assert_eq!(master["name"], "Master");
    assert_eq!(master["inserts"][0]["insert"], limiter);
    let inserts: Vec<u64> = mine["inserts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["insert"].as_u64().unwrap())
        .collect();
    // The 10-band Graphic EQ (an FFT) costs more than Amplify (a gain).
    assert_eq!(inserts, [eq, amplify], "{mine}");
    for t in [mine, master] {
        let load = &t["load"];
        assert!(load["buffers"].as_u64().unwrap() > 0, "{t}");
        assert!(load["share"].as_f64().unwrap() > 0.0, "{t}");
        let inserts_us: f64 = t["inserts"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["load"]["per_buffer_us"].as_f64().unwrap())
            .sum();
        assert!(load["per_buffer_us"].as_f64().unwrap() >= inserts_us, "{t}");
    }
    // The whole app's CPU counts in cycles (TD156), so the mixdown's alone is in the window.
    assert!(
        usage["process"].as_f64().unwrap() > 0.0,
        "the whole app: {usage}"
    );
    // Nothing held a plug-in: no insert missed a block (18.4).
    for t in [mine, master] {
        for i in t["inserts"].as_array().unwrap() {
            assert_eq!(i["misses"], 0, "{i}");
        }
    }

    // Bypassed (15.7): marked, and its DSP not called (only the passthrough that keeps its
    // latency runs), so it costs a fraction. The times are wall time per buffer, and under a
    // busy machine one preemption inside one of the window's buffers adds a whole quantum to
    // the average; preemption only ever adds, so the cheapest of a few windows is the cost.
    let before = mine["inserts"][0]["load"]["per_buffer_us"]
        .as_f64()
        .unwrap();
    let bypass = json!({"insert": eq, "bypass": true});
    call(&r, &mut s, "set_insert", bypass).unwrap();
    s.update();
    let mut readings = Vec::new();
    for i in 0..5 {
        mixdown(&r, &mut s, &dir, &format!("cpu-bypassed-{i}.wav"));
        std::thread::sleep(crate::usage::WINDOW);
        let usage = call(&r, &mut s, "get_cpu_usage", Value::Null).unwrap();
        let mine = usage["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["track"] == track)
            .unwrap();
        let bypassed = mine["inserts"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["insert"] == eq)
            .unwrap();
        assert_eq!(bypassed["bypass"], true, "{bypassed}");
        // A window with no buffers (a reading taken during a slow mixdown) proves nothing.
        if bypassed["load"]["buffers"].as_u64().unwrap() == 0 {
            continue;
        }
        let after = bypassed["load"]["per_buffer_us"].as_f64().unwrap();
        if after < before / 10.0 {
            return;
        }
        readings.push(after);
    }
    panic!("{before} us on, {readings:?} us bypassed");
}

/// The master's row (12.3) is view state like a track's: its height and lanes shown are saved
/// with the project, never undo steps, and a project saved before it had a row opens with the
/// defaults.
#[test]
fn the_master_row_is_view_state() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("master-row");
    let row = |r: &Registry, s: &mut Session| {
        let project = call(r, s, "get_project", Value::Null).unwrap();
        (
            project["master"]["height"].as_f64().unwrap(),
            project["master"]["show_automation"].as_bool().unwrap(),
        )
    };
    assert_eq!(row(&r, &mut s), (84.0, false), "the defaults");

    let set = json!({"height": 150, "show_automation": true});
    let view = call(&r, &mut s, "set_master", set).unwrap();
    assert_eq!(view["height"], 150.0);
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"], json!([]), "no undo step");

    // A mute alongside is one step, and undoing it keeps the row as it is.
    let set = json!({"mute": true, "height": 200});
    call(&r, &mut s, "set_master", set).unwrap();
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert!(!s.project.master.mixer.mute);
    assert_eq!(row(&r, &mut s), (200.0, true), "undo keeps the row");

    let path = dir.join("master.dawproj");
    call(&r, &mut s, "save_project", json!({"path": path})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    assert_eq!(row(&r, &mut s), (200.0, true), "saved");

    // A project from before the row: the master without its view fields.
    let mut old: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let master = old["master"].as_object_mut().unwrap();
    assert!(master.remove("height").is_some() && master.remove("show_automation").is_some());
    std::fs::write(&path, old.to_string()).unwrap();
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    assert_eq!(
        row(&r, &mut s),
        (84.0, false),
        "an old project opens with the defaults"
    );

    let err = call(&r, &mut s, "set_master", json!({"height": 10})).unwrap_err();
    assert!(err.contains("height"), "{err}");
}

/// A track whose main output goes into a muted folder is heard only through its send to a
/// reverb bus: the export proves the send reaches the mix, at its level, pre- or post-fader.
#[test]
fn a_pre_fader_send_reaches_the_mix_past_the_fader() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("sends-mix");
    let file = tone(&dir);
    let add = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let guitar = add(&mut s, "audio");
    let silent = add(&mut s, "folder");
    let reverb = add(&mut s, "bus");
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": guitar, "path": file, "position": 0}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_track_output",
        json!({"track": guitar, "output": {"type": "bus", "track": silent}}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": silent, "mute": true}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": guitar, "gain_db": -60}),
    )
    .unwrap();
    let send = id(
        &call(
            &r,
            &mut s,
            "add_send",
            json!({"track": guitar, "target": reverb, "gain_db": -6, "pre_fader": true}),
        )
        .unwrap(),
        "send",
    );
    let peak = |s: &mut Session, name: &str| {
        mixdown(&r, s, &dir, name)
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    };
    let expected = 0.5 * crate::util::db_to_gain(-6.0);
    let pre = peak(&mut s, "pre.wav");
    assert!(
        (pre - expected).abs() < 0.005,
        "pre-fader peak {pre}, expected {expected}"
    );

    call(
        &r,
        &mut s,
        "set_send",
        json!({"send": send, "pre_fader": false}),
    )
    .unwrap();
    let post = peak(&mut s, "post.wav");
    assert!(post < 0.001, "post-fader follows the -60 dB fader: {post}");

    call(
        &r,
        &mut s,
        "set_send",
        json!({"send": send, "pre_fader": true, "mute": true}),
    )
    .unwrap();
    assert_eq!(peak(&mut s, "send-muted.wav"), 0.0);
    call(&r, &mut s, "set_send", json!({"send": send, "mute": false})).unwrap();
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": guitar, "mute": true}),
    )
    .unwrap();
    assert_eq!(
        peak(&mut s, "track-muted.wav"),
        0.0,
        "muting the track silences its pre-fader send too"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// A lane added without points starts with one holding the current setting (5.3), so adding
/// it changes nothing, and the track shows its lanes. A point drag over several UI frames is
/// one undo step; the value at the playhead follows the lane.
#[test]
fn new_lanes_start_at_the_current_setting() {
    let r = Registry::new();
    let mut s = headless();
    let ok = |s: &mut Session, name: &str, args: Value| {
        call(&r, s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    let guitar = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
    let send = id(
        &ok(
            &mut s,
            "add_send_to_new_bus",
            json!({"track": guitar, "gain_db": -9}),
        ),
        "send",
    );
    ok(
        &mut s,
        "set_track_mixer",
        json!({"track": guitar, "gain_db": -6.5, "pan": 0.25, "mute": true}),
    );
    ok(&mut s, "set_master", json!({"pan": -0.5}));
    let seeded = |s: &mut Session, track: Value, target: Value| {
        let lane = ok(
            s,
            "add_automation_lane",
            json!({"track": track, "target": target}),
        );
        assert_eq!(lane["points"].as_array().unwrap().len(), 1, "{lane}");
        assert_eq!(lane["points"][0]["tick"], 0);
        (id(&lane, "lane") as u32, lane["points"][0]["value"].clone())
    };
    assert!(!s.project.track(guitar as u32).unwrap().show_automation);
    let (volume, value) = seeded(&mut s, json!(guitar), json!({"type": "volume"}));
    assert_eq!(value, -6.5);
    assert!(
        s.project.track(guitar as u32).unwrap().show_automation,
        "adding a lane shows the track's lanes"
    );
    assert_eq!(
        seeded(&mut s, json!(guitar), json!({"type": "pan"})).1,
        0.25
    );
    assert_eq!(
        seeded(&mut s, json!(guitar), json!({"type": "mute"})).1,
        1.0
    );
    let level = json!({"type": "send_level", "send": send});
    assert_eq!(seeded(&mut s, json!(guitar), level).1, -9.0);
    assert_eq!(seeded(&mut s, Value::Null, json!({"type": "pan"})).1, -0.5);
    // Points given are the points: no seed.
    let given = ok(
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "volume"}, "points": []}),
    );
    assert_eq!(given["points"], json!([]));

    // The value at the playhead (bar 1) follows the lane, not the fader.
    let map = s.tempo_map();
    use crate::automation::AutomationTarget::Volume;
    let owner = Some(guitar as u32);
    assert_eq!(s.playing_value(owner, Volume, &map), Some(-6.5));
    s.project
        .set_point(
            s.project.find_lane(volume).unwrap().1.points[0].id,
            None,
            Some(-20.0),
            None,
        )
        .unwrap();
    assert_eq!(s.playing_value(owner, Volume, &map), Some(-20.0));
    assert_eq!(s.fixed_value(owner, Volume), Some(-6.5));

    // A point dragged over three frames is one step, named for undo.
    let point = s.project.find_lane(volume).unwrap().1.points[0].id;
    for (tick, value) in [(100, -10.0), (200, -12.0), (300, -14.0)] {
        s.begin_edit("", None);
        s.project
            .set_point(point, Some(tick), Some(value), None)
            .unwrap();
        s.end_edit();
    }
    let history = ok(&mut s, "get_history", Value::Null);
    assert_eq!(history["undo"][0], "Automation");
    assert_eq!(history["undo"][1], "Add automation lane");
    ok(&mut s, "undo", Value::Null);
    let p = s.project.find_lane(volume).unwrap().1.points[0];
    assert_eq!(
        (p.tick, p.value),
        (0, -20.0),
        "one undo takes the whole drag back"
    );

    // A delete and then a drag on the same lane are two steps: only drags merge. The app
    // names its discrete edits, which breaks the merge.
    let added = ok(
        &mut s,
        "add_automation_points",
        json!({"lane": volume, "points": [{"tick": 960, "value": -3.0}, {"tick": 1920, "value": -4.0}]}),
    );
    let ids: Vec<u32> = added["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p.as_u64().unwrap() as u32)
        .collect();
    s.begin_edit("", None);
    s.project.remove_points(volume, &[ids[0]], None).unwrap();
    s.label_edit("Delete automation point", None);
    s.end_edit();
    s.begin_edit("", None);
    s.project.set_point(ids[1], Some(2000), None, None).unwrap();
    s.end_edit();
    let history = ok(&mut s, "get_history", Value::Null);
    assert_eq!(history["undo"][0], "Automation");
    assert_eq!(history["undo"][1], "Delete automation point");
}

/// Loudness (6.1): the live meter reads nothing headless; files are measured with the same
/// meter, mono as one channel; resetting is no edit.
#[test]
fn loudness_is_measured_live_and_from_files() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("loudness");
    let ok = |s: &mut Session, name: &str, args: Value| {
        call(&r, s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    let live = ok(&mut s, "get_loudness", Value::Null);
    assert_eq!(live["integrated"], Value::Null, "no device ran: {live}");
    assert_eq!(live["momentary"], Value::Null);
    assert_eq!(live["seconds"], 0.0);
    assert_eq!(live["target"], -14.0, "streaming by default");
    assert_eq!(live["offset"], Value::Null, "nothing to compare yet");

    // The target (6.2): presets, a value in range, refusals, and undo steps.
    let target =
        |s: &mut Session, args: Value| ok(s, "set_loudness_target", args)["target"].clone();
    assert_eq!(target(&mut s, json!({"preset": "broadcast"})), -23.0);
    assert_eq!(target(&mut s, json!({"lufs": -16.0})), -16.0);
    for (args, says) in [
        (json!({"lufs": -50.0}), "outside -40..-5"),
        (json!({"preset": "streaming", "lufs": -14.0}), "not both"),
        (json!({}), "give `preset` or `lufs`"),
    ] {
        let err = call(&r, &mut s, "set_loudness_target", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    assert_eq!(
        ok(&mut s, "get_project", Value::Null)["loudness_target"],
        -16.0
    );
    assert_eq!(
        ok(&mut s, "get_history", Value::Null)["undo"][0],
        "Set loudness target"
    );
    // A custom value dragged in the app over several frames is one step.
    for lufs in [-15.0, -14.5, -14.0] {
        s.begin_edit("", None);
        s.project.loudness_target = lufs;
        s.end_edit();
    }
    let history = ok(&mut s, "get_history", Value::Null);
    assert_eq!(history["undo"][0], "Loudness target");
    assert_eq!(history["undo"][1], "Set loudness target");
    ok(&mut s, "undo", Value::Null);
    assert_eq!(s.project.loudness_target, -16.0);
    let before = ok(&mut s, "get_history", Value::Null)["undo"].clone();
    ok(&mut s, "reset_loudness", Value::Null);
    assert_eq!(
        ok(&mut s, "get_history", Value::Null)["undo"],
        before,
        "not an undo step"
    );

    // Five seconds of a 1 kHz tone at -23 dBFS peak.
    let tone: Vec<f32> = (0..240_000)
        .map(|i| {
            let amplitude = 10f64.powf(-23.0 / 20.0);
            (amplitude * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32
        })
        .collect();
    let write = |name: &str, channels: Vec<Vec<f32>>| {
        let path = dir.join(name);
        wav::write_f32(
            &path,
            &wav::Audio {
                sample_rate: 48_000,
                channels,
            },
        )
        .unwrap();
        path
    };
    let near = |v: &Value, expected: f64, tolerance: f64| {
        (v.as_f64().unwrap_or(f64::NAN) - expected).abs() <= tolerance
    };
    let stereo = ok(
        &mut s,
        "measure_loudness",
        json!({"path": write("stereo.wav", vec![tone.clone(), tone.clone()])}),
    );
    assert!(near(&stereo["integrated"], -23.0, 0.1), "{stereo}");
    assert!(near(&stereo["true_peak"], -23.0, 0.3), "{stereo}");
    assert!(near(&stereo["max_momentary"], -23.0, 0.1), "{stereo}");
    let mono = ok(
        &mut s,
        "measure_loudness",
        json!({"path": write("mono.wav", vec![tone.clone()])}),
    );
    assert!(
        near(&mono["integrated"], -26.0, 0.1),
        "one channel at weight 1, 3 dB below the stereo pair: {mono}"
    );
    let err = call(
        &r,
        &mut s,
        "measure_loudness",
        json!({"path": write("three.wav", vec![tone.clone(), tone.clone(), tone])}),
    )
    .unwrap_err();
    assert!(err.contains("3 channels"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

/// Export reports and normalising (6.3). Every export reports the file's loudness; normalising
/// is one constant gain (the null test: normalised = plain x gain, sample for sample), held
/// back by the true-peak ceiling; silence is left alone.
#[test]
fn exports_report_their_loudness_and_normalise_by_one_gain() {
    use crate::export::gain_factor;
    let r = Registry::new();
    let dir = temp_dir("export-loudness");
    let ok = |s: &mut Session, name: &str, args: Value| {
        call(&r, s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    // A project of one mono clip, centre-panned: `peak` adds one sample at -2 dBFS.
    let project = |name: &str, level: Option<f64>, peak: bool| {
        let mut samples: Vec<f32> = (0..240_000)
            .map(|i| match level {
                Some(db) => {
                    let amplitude = 10f64.powf(db / 20.0);
                    (amplitude * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin())
                        as f32
                }
                None => 0.0,
            })
            .collect();
        if peak {
            samples[100_000] = 10f32.powf(-2.0 / 20.0);
        }
        let file = dir.join(format!("{name}-source.wav"));
        wav::write_f32(
            &file,
            &wav::Audio {
                sample_rate: 48_000,
                channels: vec![samples],
            },
        )
        .unwrap();
        let mut s = headless();
        let track = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
        ok(
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        );
        s
    };
    let export = |s: &mut Session, name: &str, normalize: bool| {
        let path = dir.join(name);
        ok(
            s,
            "export_mixdown",
            json!({"path": path, "tail_seconds": 0, "normalize": normalize}),
        );
        s.wait_for_export(Duration::from_secs(30)).unwrap();
        let status = ok(s, "get_export_status", Value::Null);
        assert_eq!(status["state"], "finished", "{status}");
        (status, wav::read(&path).unwrap(), path)
    };

    // -23 dBFS stereo: the report is the file's own measurement.
    let mut s = project("tone", Some(-23.0), false);
    let (plain, plain_audio, plain_path) = export(&mut s, "plain.wav", false);
    let measured = crate::loudness::measure_file(&plain_path).unwrap();
    assert_eq!(plain["loudness"], serde_json::to_value(measured).unwrap());
    assert_eq!(plain["normalized"], Value::Null);
    let integrated = plain["loudness"]["integrated"].as_f64().unwrap();
    assert!((integrated + 23.0).abs() < 0.1, "{plain}");

    // Normalised to the -14 target: +9 dB, exactly that gain on every sample.
    let (normalised, audio, _) = export(&mut s, "normalised.wav", true);
    let gain = normalised["normalized"]["gain_db"].as_f64().unwrap();
    assert_eq!(normalised["normalized"]["held_by_peak"], false);
    assert!((gain - (-14.0 - integrated)).abs() < 1e-9, "{normalised}");
    let factor = gain_factor(gain);
    for (n, p) in audio.channels.iter().zip(&plain_audio.channels) {
        assert_eq!(n.len(), p.len());
        assert!(
            n.iter()
                .zip(p)
                .all(|(n, p)| n.to_bits() == (p * factor).to_bits()),
            "the normalised file is the plain one times the gain, sample for sample"
        );
    }
    let reached = normalised["loudness"]["integrated"].as_f64().unwrap();
    assert!((reached + 14.0).abs() < 0.1, "reached {reached}");

    // One sample at -2 dBFS: +9 dB would put the true peak near +7, so the ceiling holds it.
    let mut s = project("peak", Some(-23.0), true);
    let (held, _, _) = export(&mut s, "held.wav", true);
    assert_eq!(held["normalized"]["held_by_peak"], true, "{held}");
    let tp = held["loudness"]["true_peak"].as_f64().unwrap();
    assert!(
        tp <= -1.0 + 1e-3 && tp > -1.5,
        "true peak {tp} at the ceiling"
    );
    assert!(
        held["loudness"]["integrated"].as_f64().unwrap() < -14.5,
        "{held}"
    );

    // Silence: nothing to normalise, and nothing done to it.
    let mut s = project("silence", None, false);
    let (quiet, audio, _) = export(&mut s, "silence.wav", true);
    assert_eq!(quiet["normalized"]["gain_db"], Value::Null, "{quiet}");
    assert!(audio.channels.iter().flatten().all(|v| *v == 0.0));
    let _ = std::fs::remove_dir_all(dir);
}

/// Write modes (5.4): while a control is written, it plays rather than its lane, through the
/// session's live values; letting go hands back to the lane; the pass lands in the lanes as one
/// undo step, a lane-less control getting a lane. The app drives the writer while playing; a
/// headless session can't play, so the test drives it the same way.
#[test]
fn a_write_pass_plays_the_control_and_lands_as_one_undo_step() {
    use crate::automation::AutomationMode::Touch;
    use crate::automation::AutomationTarget::{Pan, Volume};
    use crate::automation_write::WriteKey;
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("automation-write");
    let file = tone(&dir);
    let ok = |s: &mut Session, name: &str, args: Value| {
        call(&r, s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    let guitar = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
    ok(
        &mut s,
        "import_audio",
        json!({"track": guitar, "path": file, "position": 0}),
    );
    ok(
        &mut s,
        "set_automation_mode",
        json!({"track": guitar, "mode": "touch"}),
    );
    let project = ok(&mut s, "get_project", Value::Null);
    assert_eq!(project["tracks"][0]["automation_mode"], "touch");
    assert_eq!(project["master"]["automation_mode"], "read");
    // A mode changed in the app is an undo step named for what it is.
    s.begin_edit("", None);
    s.project.master.automation_mode = crate::automation::AutomationMode::Latch;
    s.end_edit();
    assert_eq!(
        ok(&mut s, "get_history", Value::Null)["undo"][0],
        "Automation mode"
    );
    s.project.master.automation_mode = crate::automation::AutomationMode::Read;
    let lane = ok(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "volume"}, "points": [{"tick": 0, "value": -40.0}]}),
    );
    let lane = id(&lane, "lane") as u32;
    let peak = |out: Vec<f32>| out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak(mixdown(&r, &mut s, &dir, "lane.wav")) < 0.01, "-40 dB");

    // The fader held at -6 dB from the start: the control plays, not the lane (nor the fixed
    // fader, at 0 dB).
    let owner = Some(guitar as u32);
    let volume = WriteKey {
        owner,
        target: Volume,
    };
    s.writer.touch(volume, Touch, -6.0, 0, -40.0);
    s.writer.advance(960, false);
    // Pan, which has no lane, through the session as the app does. The fixed pan moving later
    // in the pass must not change where its new lane starts.
    s.touch_control_at(owner, Pan, -1.0, 960);
    s.project.track_mut(guitar as u32).unwrap().mixer.pan = 0.7;
    assert_eq!(s.written_value(owner, Volume), Some(-6.0));
    let held = peak(mixdown(&r, &mut s, &dir, "held.wav"));
    let expected = 0.5 * crate::util::db_to_gain(-6.0);
    assert!(
        (held - expected).abs() < 0.005,
        "the held fader plays: {held}"
    );
    // Let go: the lane plays again.
    s.writer.advance(1920, false);
    s.writer.release(volume, 1920);
    assert!(
        peak(mixdown(&r, &mut s, &dir, "let-go.wav")) < 0.01,
        "back to the lane"
    );

    // Playback stops: one undo step, a pan lane where there was none.
    s.finish_writing();
    let history = ok(&mut s, "get_history", Value::Null);
    assert_eq!(history["undo"][0], "Write automation");
    let points = |s: &Session, target| {
        s.project
            .lanes(owner)
            .unwrap()
            .iter()
            .find(|l| l.target == target)
            .map(|l| {
                l.points
                    .iter()
                    .map(|p| (p.tick, p.value))
                    .collect::<Vec<_>>()
            })
    };
    let glide = crate::session::WRITE_GLIDE;
    assert_eq!(
        points(&s, Volume).unwrap(),
        [
            (0, -40.0),
            (0, -6.0),
            (960, -6.0),
            (1920, -6.0),
            (1920 + glide, -40.0)
        ]
        .iter()
        .copied()
        .filter(|p| p.0 != 960)
        .collect::<Vec<_>>(),
        "led in from the lane, held, then a glide back to it"
    );
    assert_eq!(
        points(&s, Pan).unwrap(),
        [
            (0, 0.0),
            (960, 0.0),
            (960, -1.0),
            (1920, -1.0),
            (1920 + glide, 0.0)
        ],
        "a new lane, seeded at the fixed pan, written from where it was touched"
    );
    ok(&mut s, "undo", Value::Null);
    assert_eq!(points(&s, Pan), None, "undo takes the whole pass back");

    // An export runs the transport but writes nothing, even in write mode.
    ok(
        &mut s,
        "set_automation_mode",
        json!({"track": guitar, "mode": "write"}),
    );
    assert!(
        peak(mixdown(&r, &mut s, &dir, "export.wav")) < 0.01,
        "the lane plays"
    );
    s.update();
    let history = ok(&mut s, "get_history", Value::Null);
    assert_ne!(history["undo"][0], "Write automation");
    assert_eq!(points(&s, Volume).unwrap(), [(0, -40.0)]);
    assert_eq!(s.project.find_lane(lane).unwrap().1.points.len(), 1);
    let _ = std::fs::remove_dir_all(dir);
}

/// Automation plays (5.2): a lane added over MCP changes the export with no other call, a
/// jump lands on its sample, and a mute lane takes over from the mute button while solo still
/// wins.
#[test]
fn automation_lanes_play_in_the_mix() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("automation-mix");
    let file = tone(&dir);
    let ok = |s: &mut Session, name: &str, args: Value| {
        call(&r, s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    let guitar = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
    let other = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
    ok(
        &mut s,
        "import_audio",
        json!({"track": guitar, "path": file, "position": 0}),
    );
    let peak = |out: &[f32], from: usize, to: usize| {
        out[from..to].iter().fold(0.0f32, |m, x| m.max(x.abs()))
    };
    // The tone (0.5 from sample 12000) at 0 dB, then -60 dB from tick 960: sample 24000 at
    // 120 BPM.
    let volume = id(
        &ok(
            &mut s,
            "add_automation_lane",
            json!({"track": guitar, "target": {"type": "volume"},
                   "points": [{"tick": 0, "value": 0.0}, {"tick": 960, "value": 0.0},
                              {"tick": 960, "value": -60.0}]}),
        ),
        "lane",
    );
    let out = mixdown(&r, &mut s, &dir, "jump.wav");
    assert!(
        peak(&out, 12_000, 23_968) > 0.49,
        "full level before the jump"
    );
    assert!(
        peak(&out, 24_000, 48_000) < 0.001,
        "-60 dB from its sample on"
    );
    ok(
        &mut s,
        "set_automation_lane",
        json!({"lane": volume, "enabled": false}),
    );
    let out = mixdown(&r, &mut s, &dir, "off.wav");
    assert!(
        peak(&out, 24_000, 48_000) > 0.49,
        "a lane switched off moves nothing"
    );

    // The mute button is down, but a mute lane says play: the lane decides.
    ok(
        &mut s,
        "set_track_mixer",
        json!({"track": guitar, "mute": true}),
    );
    assert_eq!(
        peak(&mixdown(&r, &mut s, &dir, "button.wav"), 0, 48_000),
        0.0
    );
    ok(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "mute"}, "points": [{"tick": 0, "value": 0.0}]}),
    );
    let out = mixdown(&r, &mut s, &dir, "lane.wav");
    assert!(
        peak(&out, 12_000, 48_000) > 0.49,
        "the mute lane overrules the button"
    );
    // Soloing another track still silences it.
    ok(
        &mut s,
        "set_track_mixer",
        json!({"track": other, "solo": true}),
    );
    assert_eq!(peak(&mixdown(&r, &mut s, &dir, "solo.wav"), 0, 48_000), 0.0);
    let _ = std::fs::remove_dir_all(dir);
}

/// Solo in place: soloing a reverb bus plays it with what feeds it, and nothing else. The
/// guitar is heard through its send, not its own output; the bass, which doesn't send, is
/// silent.
#[test]
fn soloing_a_bus_keeps_its_senders_feeding_it() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("solo-bus");
    let file = tone(&dir);
    let add = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let guitar = add(&mut s, "audio");
    let bass = add(&mut s, "audio");
    let reverb = add(&mut s, "bus");
    for track in [guitar, bass] {
        call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap();
    }
    call(
        &r,
        &mut s,
        "add_send",
        json!({"track": guitar, "target": reverb, "gain_db": -6}),
    )
    .unwrap();
    let solo = |s: &mut Session, track: u64, on: bool| {
        call(
            &r,
            s,
            "set_track_mixer",
            json!({"track": track, "solo": on}),
        )
        .unwrap();
    };
    let peak = |s: &mut Session, name: &str| {
        mixdown(&r, s, &dir, name)
            .iter()
            .fold(0.0f32, |m, x| m.max(x.abs()))
    };
    // The same tone through every path, in phase: the peaks add.
    let send = 0.5 * crate::util::db_to_gain(-6.0);
    let near = |got: f32, want: f32| (got - want).abs() < 0.005;
    let all = peak(&mut s, "all.wav");
    assert!(near(all, 0.5 + 0.5 + send), "everything: {all}");

    solo(&mut s, reverb, true);
    let bus = peak(&mut s, "bus.wav");
    assert!(
        near(bus, send),
        "the reverb alone, fed by the guitar's send: {bus}"
    );

    solo(&mut s, reverb, false);
    solo(&mut s, guitar, true);
    let dry_and_wet = peak(&mut s, "guitar.wav");
    assert!(
        near(dry_and_wet, 0.5 + send),
        "a soloed guitar keeps its reverb: {dry_and_wet}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn sends_refuse_loops_save_and_undo() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("sends");
    let add = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let guitar = add(&mut s, "audio");
    let reverb = add(&mut s, "bus");
    let drums = add(&mut s, "folder");
    let other = add(&mut s, "folder");
    let send = id(
        &call(
            &r,
            &mut s,
            "add_send",
            json!({"track": guitar, "target": reverb}),
        )
        .unwrap(),
        "send",
    );
    // A level drag over MCP is one step.
    for gain in [-1.0, -2.0, -3.0] {
        call(
            &r,
            &mut s,
            "set_send",
            json!({"send": send, "gain_db": gain}),
        )
        .unwrap();
    }
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Set send");
    assert_eq!(history["undo"][1], "Add send");

    // Loops and bad targets are refused, naming the tracks.
    let refused = |s: &mut Session, action: &str, args: Value, says: &str| {
        let err = call(&r, s, action, args).unwrap_err();
        assert!(err.contains(says), "{action}: {err}");
    };
    refused(
        &mut s,
        "add_send",
        json!({"track": reverb, "target": guitar}),
        "not a folder or bus",
    );
    refused(
        &mut s,
        "add_send",
        json!({"track": reverb, "target": reverb}),
        "into itself would create a loop",
    );
    call(
        &r,
        &mut s,
        "add_send",
        json!({"track": drums, "target": reverb}),
    )
    .unwrap();
    refused(
        &mut s,
        "set_track_output",
        json!({"track": reverb, "output": {"type": "bus", "track": drums}}),
        &format!("{drums} already feeds {reverb}"),
    );
    refused(
        &mut s,
        "set_track_parent",
        json!({"track": reverb, "folder": drums}),
        "would create a loop",
    );
    // Two folders feeding each other by main output: refused now (it used to be cut
    // silently when the graph was built).
    call(
        &r,
        &mut s,
        "set_track_output",
        json!({"track": other, "output": {"type": "bus", "track": drums}}),
    )
    .unwrap();
    refused(
        &mut s,
        "set_track_output",
        json!({"track": drums, "output": {"type": "bus", "track": other}}),
        "would create a loop",
    );
    refused(
        &mut s,
        "set_send",
        json!({"send": send, "target": guitar}),
        "not a folder or bus",
    );
    refused(
        &mut s,
        "set_send",
        json!({"send": 9999}),
        "no send with id 9999",
    );
    refused(
        &mut s,
        "set_send",
        json!({"send": send, "gain_db": 40}),
        "outside",
    );

    // Saved and reopened.
    let sends_of = |s: &mut Session, track: u64| {
        let project = call(&r, s, "get_project", Value::Null).unwrap();
        project["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == track)
            .unwrap()["sends"]
            .clone()
    };
    let before = sends_of(&mut s, guitar);
    assert_eq!(before[0]["target"], reverb);
    assert_eq!(before[0]["gain_db"], -3.0);
    let path = dir.join("sends.dawproj");
    call(&r, &mut s, "save_project", json!({"path": path})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    assert_eq!(sends_of(&mut s, guitar), before);

    // Deleting the bus removes the sends into it; undo brings both back.
    call(&r, &mut s, "remove_track", json!({"track": reverb})).unwrap();
    assert_eq!(sends_of(&mut s, guitar), json!([]));
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(sends_of(&mut s, guitar), before);
    call(&r, &mut s, "remove_send", json!({"send": send})).unwrap();
    assert_eq!(sends_of(&mut s, guitar), json!([]));

    // "Add send → new bus" is one step: a bus at the end, fed by the send; undo removes both.
    let tracks = |s: &mut Session| {
        call(&r, s, "get_project", Value::Null).unwrap()["tracks"]
            .as_array()
            .unwrap()
            .len()
    };
    let count = tracks(&mut s);
    let made = call(
        &r,
        &mut s,
        "add_send_to_new_bus",
        json!({"track": guitar, "gain_db": -10, "pre_fader": true}),
    )
    .unwrap();
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    let bus = project["tracks"]
        .as_array()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    assert_eq!(
        (bus["kind"].clone(), bus["id"].clone()),
        (json!("Bus"), made["target"].clone())
    );
    assert_eq!(
        (made["gain_db"].clone(), made["pre_fader"].clone()),
        (json!(-10.0), json!(true))
    );
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Add send to new bus");
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(tracks(&mut s), count);
    assert_eq!(sends_of(&mut s, guitar), json!([]));
    let _ = std::fs::remove_dir_all(dir);
}

/// Automation lanes (5.1): one per target on tracks and the master, checked values, points in
/// time order by tick or bar with jumps, lanes leaving with the send or plug-in they move, a
/// UI edit named for undo, save, reopen and undo.
#[test]
fn automation_lanes_hold_points_and_follow_their_targets() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("automation");
    let ok = |s: &mut Session, name: &str, args: Value| {
        call(&r, s, name, args).unwrap_or_else(|e| panic!("{name}: {e}"))
    };
    let refused = |s: &mut Session, name: &str, args: Value, says: &str| {
        let err = call(&r, s, name, args).unwrap_err();
        assert!(err.contains(says), "{name}: {err}");
    };
    let value_at = |s: &mut Session, lane: u64, at: Value| {
        let mut args = at;
        args["lane"] = json!(lane);
        ok(s, "get_automation_value", args)["value"].clone()
    };
    let guitar = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
    let keys = id(&ok(&mut s, "add_track", json!({"kind": "audio"})), "track");
    let made = ok(&mut s, "add_send_to_new_bus", json!({"track": guitar}));
    let (send, bus) = (id(&made, "send"), id(&made, "target"));

    // Points given out of order come back in time order; bar 2 of 4/4 is tick 3840.
    let lane = ok(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "volume"},
               "points": [{"bar": 2, "value": 0.0}, {"tick": 0, "value": -12.0}]}),
    );
    assert_eq!(lane["units"], "dB, -60 to +12");
    assert_eq!(lane["track"], guitar);
    let ticks: Vec<u64> = lane["points"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["tick"].as_u64().unwrap())
        .collect();
    assert_eq!(ticks, [0, 3840]);
    let volume = id(&lane, "lane");
    assert_eq!(value_at(&mut s, volume, json!({"tick": 1920})), -6.0);
    assert_eq!(value_at(&mut s, volume, json!({"tick": 9999})), 0.0);
    // A second point on bar 2 makes a jump there.
    ok(
        &mut s,
        "add_automation_points",
        json!({"lane": volume, "points": [{"bar": 2, "value": -30.0}]}),
    );
    assert_eq!(value_at(&mut s, volume, json!({"bar": 2})), -30.0);
    assert!(
        value_at(&mut s, volume, json!({"tick": 3839}))
            .as_f64()
            .unwrap()
            > -0.01
    );

    // Refused: a second lane for one target, values out of range, sends and plug-ins that
    // aren't the owner's. A bad point refuses the whole call.
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "volume"}}),
        &format!("already has a volume (dB) lane (lane {volume})"),
    );
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "mute"}, "points": [{"tick": 0, "value": 0.5}]}),
        "0 (plays) or 1",
    );
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "mute"}, "points": [{"tick": 0, "value": 1.0, "curve": 2.0}]}),
        "curve 2 is outside",
    );
    assert_eq!(
        s.project.track(guitar as u32).unwrap().automation.len(),
        1,
        "a refused lane leaves nothing behind"
    );
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": keys, "target": {"type": "send_level", "send": send}}),
        &format!("send {send} belongs to track {guitar}"),
    );
    refused(
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "send_level", "send": send}}),
        "the master has no sends",
    );
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "parameter", "insert": 9999, "parameter": 1}}),
        "no insert with id 9999",
    );
    refused(
        &mut s,
        "add_automation_points",
        json!({"lane": volume, "points": [{"tick": 10, "value": -1.0}, {"tick": 20, "value": 40.0}]}),
        "outside -60..12",
    );
    refused(
        &mut s,
        "add_automation_points",
        json!({"lane": volume, "points": [{"tick": 10, "bar": 1, "value": -1.0}]}),
        "not both",
    );
    assert_eq!(
        s.project.find_lane(volume as u32).unwrap().1.points.len(),
        3
    );

    // A point moves and changes; a range of points goes.
    let pan = ok(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "pan"},
               "points": [{"tick": 0, "value": -1.0}, {"tick": 960, "value": 1.0, "curve": 0.5}]}),
    );
    let (pan, last) = (id(&pan, "lane"), id(&pan["points"][1], "point"));
    let moved = ok(
        &mut s,
        "set_automation_point",
        json!({"point": last, "tick": 1920, "value": 0.5}),
    );
    assert_eq!(
        (moved["tick"].clone(), moved["value"].clone()),
        (json!(1920), json!(0.5))
    );
    assert_eq!(moved["curve"], 0.5);
    refused(
        &mut s,
        "set_automation_point",
        json!({"point": last, "curve": 2.0}),
        "curve 2 is outside",
    );
    let gone = ok(
        &mut s,
        "remove_automation_points",
        json!({"lane": pan, "from_tick": 0, "to_tick": 1000}),
    );
    assert_eq!(gone["removed"], 1);
    assert_eq!(
        value_at(&mut s, pan, json!({"tick": 0})),
        0.5,
        "the last point holds"
    );

    // The master's lanes; a frame of UI editing them is named for undo.
    let master = ok(
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "mute"}}),
    );
    assert_eq!(master["track"], Value::Null);
    let master = id(&master, "lane") as u32;
    s.begin_edit("", None);
    s.project
        .add_points(
            master,
            &[crate::automation::NewPoint {
                tick: 0,
                value: 1.0,
                curve: 0.0,
            }],
        )
        .unwrap();
    s.end_edit();
    let history = ok(&mut s, "get_history", Value::Null);
    assert_eq!(history["undo"][0], "Automation");
    assert_eq!(history["undo"][1], "Add automation lane");

    // A send lane and a lane on a plug-in that isn't loaded (its parameters are unknown, so
    // only an id will do).
    let send_lane = ok(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "send_level", "send": send},
               "points": [{"tick": 0, "value": -6.0}]}),
    );
    let insert = s.project.next_id();
    s.project
        .track_mut(guitar as u32)
        .unwrap()
        .inserts
        .push(crate::project::InsertSlot {
            id: insert,
            name: "Missing".into(),
            path: dir.join("Missing.vst3"),
            bypass: false,
            state: None,
        });
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "parameter", "insert": insert, "parameter": "Gain"}}),
        "give the parameter by id",
    );
    refused(
        &mut s,
        "add_automation_lane",
        json!({"track": keys, "target": {"type": "parameter", "insert": insert, "parameter": 7}}),
        &format!("plug-in {insert} is not in track {keys}'s inserts"),
    );
    let param = ok(
        &mut s,
        "add_automation_lane",
        json!({"track": guitar, "target": {"type": "parameter", "insert": insert, "parameter": 7},
               "points": [{"tick": 0, "value": 0.25}]}),
    );
    assert_eq!(
        param["target"],
        json!({"type": "parameter", "insert": insert, "parameter": 7})
    );

    // Saved and reopened.
    let lanes_of = |s: &mut Session, track: u64| {
        let project = call(&r, s, "get_project", Value::Null).unwrap();
        project["tracks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == track)
            .unwrap()["automation"]
            .clone()
    };
    let before = lanes_of(&mut s, guitar);
    assert_eq!(before.as_array().unwrap().len(), 4, "{before}");
    let detail = ok(&mut s, "get_automation_lane", json!({"lane": volume}));
    let path = dir.join("automation.dawproj");
    ok(&mut s, "save_project", json!({"path": path}));
    ok(&mut s, "new_project", Value::Null);
    ok(&mut s, "open_project", json!({"path": path}));
    assert_eq!(lanes_of(&mut s, guitar), before);
    assert_eq!(
        ok(&mut s, "get_automation_lane", json!({"lane": volume})),
        detail
    );
    assert_eq!(s.project.master.automation.len(), 1);

    // Deleting the bus takes the send with it, and the send's lane; undo brings all back.
    let send_lane = id(&send_lane, "lane");
    ok(&mut s, "remove_track", json!({"track": bus}));
    refused(
        &mut s,
        "get_automation_lane",
        json!({"lane": send_lane}),
        "no automation lane",
    );
    ok(&mut s, "undo", Value::Null);
    assert_eq!(lanes_of(&mut s, guitar), before);
    // So does removing the send itself, and the plug-in its lane moves.
    ok(&mut s, "remove_send", json!({"send": send}));
    ok(&mut s, "remove_insert", json!({"insert": insert}));
    let left: Vec<Value> = lanes_of(&mut s, guitar)
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["target"]["type"].clone())
        .collect();
    assert_eq!(left, [json!("volume"), json!("pan")]);
    ok(
        &mut s,
        "set_automation_lane",
        json!({"lane": pan, "enabled": false}),
    );
    ok(&mut s, "remove_automation_lane", json!({"lane": volume}));
    assert_eq!(lanes_of(&mut s, guitar)[0]["enabled"], false);
    let history = ok(&mut s, "get_history", Value::Null);
    assert_eq!(history["undo"][0], "Remove automation lane");
    assert_eq!(history["undo"][1], "Set automation lane");
    let _ = std::fs::remove_dir_all(dir);
}

/// MIDI clips over MCP: notes and controller events by id, in ticks; checked values, all or
/// nothing; a tempo change inside the clip moves it in samples but not in ticks; save, reopen
/// and undo.
#[test]
fn midi_clips_hold_notes_in_musical_time() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("midi-clips");
    let add = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let keys = add(&mut s, "instrument");
    let audio = add(&mut s, "audio");
    let err = call(
        &r,
        &mut s,
        "add_midi_clip",
        json!({"track": audio, "bar": 1}),
    )
    .unwrap_err();
    assert!(err.contains("instrument"), "{err}");

    // Bars 2 and 3 of 4/4 at 120: ticks 3840 to 11520, one bar is 96000 samples.
    let made = call(
        &r,
        &mut s,
        "add_midi_clip",
        json!({"track": keys, "bar": 2, "bars": 2, "name": "Chords"}),
    )
    .unwrap();
    assert_eq!(
        (
            &made["start"],
            &made["length"],
            &made["start_sample"],
            &made["name"]
        ),
        (&json!(3840), &json!(7680), &json!(96_000), &json!("Chords"))
    );
    let clip = id(&made, "clip");
    let notes = |s: &mut Session| {
        call(&r, s, "get_midi_clip", json!({"clip": clip})).unwrap()["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| {
                (
                    n["id"].as_u64().unwrap(),
                    n["tick"].as_u64().unwrap(),
                    n["pitch"].as_u64().unwrap(),
                )
            })
            .collect::<Vec<_>>()
    };

    let added = call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [
            {"tick": 1920, "length": 960, "pitch": 64},
            {"tick": 0, "length": 960, "pitch": 60, "velocity": 90},
            {"tick": 0, "length": 480, "pitch": 67},
        ]}),
    )
    .unwrap();
    let ids: Vec<u64> = added["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let (e, c, g) = (ids[0], ids[1], ids[2]);
    assert_eq!(
        notes(&mut s),
        [(c, 0, 60), (g, 0, 67), (e, 1920, 64)],
        "time, then pitch"
    );

    // Checked, all or nothing.
    let refused = |s: &mut Session, action: &str, args: Value, says: &str| {
        let err = call(&r, s, action, args).unwrap_err();
        assert!(err.contains(says), "{action}: {err}");
    };
    refused(
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [{"tick": 0, "length": 10, "pitch": 50}, {"tick": 0, "length": 10, "pitch": 128}]}),
        "note 1: pitch 128",
    );
    refused(
        &mut s,
        "remove_notes",
        json!({"clip": clip, "notes": [g, 9999]}),
        "no note with id 9999",
    );
    assert_eq!(notes(&mut s).len(), 3);
    refused(
        &mut s,
        "add_controller_events",
        json!({"clip": clip, "events": [{"tick": 960, "kind": {"type": "pitch_bend", "value": -9000}}]}),
        "pitch bend -9000",
    );
    // Audio actions name the MIDI ones.
    refused(
        &mut s,
        "delete_clip",
        json!({"clip": clip}),
        "is a MIDI clip",
    );

    // Resized by either edge, both ways (12.4), as the piano roll's edge drags do: the
    // other edge stays, one undo step each.
    let span = |s: &mut Session, note: u64| {
        let clip = call(&r, s, "get_midi_clip", json!({"clip": clip})).unwrap();
        let n = clip["notes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["id"] == note)
            .cloned()
            .unwrap();
        let tick = n["tick"].as_u64().unwrap();
        (tick, tick + n["length"].as_u64().unwrap())
    };
    let resize = |s: &mut Session, edge: &str, by: i64| {
        let args = json!({"clip": clip, "notes": [e], "edge": edge, "by": by});
        call(&r, s, "resize_notes", args).unwrap()
    };
    assert_eq!(span(&mut s, e), (1920, 2880));
    let changed = resize(&mut s, "end", 480);
    assert_eq!(changed["notes"][0]["length"], 1440);
    resize(&mut s, "end", -960);
    assert_eq!(span(&mut s, e), (1920, 2400));
    resize(&mut s, "start", -1920);
    assert_eq!(span(&mut s, e), (0, 2400));
    resize(&mut s, "start", 3000);
    assert_eq!(span(&mut s, e), (2399, 2400), "a tick at least");
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Resize notes");
    for _ in 0..4 {
        call(&r, &mut s, "undo", Value::Null).unwrap();
    }
    assert_eq!(span(&mut s, e), (1920, 2880));
    refused(
        &mut s,
        "resize_notes",
        json!({"clip": clip, "notes": [e, 9999], "edge": "end", "by": 10}),
        "no note with id 9999",
    );
    assert_eq!(span(&mut s, e), (1920, 2880), "all or nothing");

    // A moved note re-sorts.
    call(
        &r,
        &mut s,
        "set_notes",
        json!({"clip": clip, "notes": [{"note": e, "tick": 0, "pitch": 62}]}),
    )
    .unwrap();
    assert_eq!(notes(&mut s), [(c, 0, 60), (e, 0, 62), (g, 0, 67)]);
    call(
        &r,
        &mut s,
        "remove_notes",
        json!({"clip": clip, "notes": [g]}),
    )
    .unwrap();
    let events = call(
        &r,
        &mut s,
        "add_controller_events",
        json!({"clip": clip, "events": [
            {"tick": 0, "kind": {"type": "cc", "number": 64, "value": 127}},
            {"tick": 960, "channel": 1, "kind": {"type": "pitch_bend", "value": -4096}},
        ]}),
    )
    .unwrap();
    assert_eq!(events["events"].as_array().unwrap().len(), 2);

    // A tempo change inside the clip: from bar 3, 60 quarter notes a minute. The clip's
    // second bar now lasts twice as long in samples; no tick changes.
    call(&r, &mut s, "add_tempo_change", json!({"bar": 3, "bpm": 60})).unwrap();
    let after = call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap();
    assert_eq!(
        (
            &after["start"],
            &after["length"],
            &after["start_sample"],
            &after["end_sample"]
        ),
        (
            &json!(3840),
            &json!(7680),
            &json!(96_000),
            &json!(192_000 + 192_000)
        )
    );
    assert_eq!(notes(&mut s), [(c, 0, 60), (e, 0, 62)]);
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(
        project["length"], 384_000,
        "the song's end counts MIDI clips"
    );
    let summary = &project["tracks"][0]["midi_clips"][0];
    assert_eq!(
        (&summary["notes"], &summary["controllers"]),
        (&json!(2), &json!(2))
    );

    // Undo names, and save and reopen.
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    let names: Vec<&str> = history["undo"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    for name in [
        "Add MIDI clip",
        "Add notes",
        "Set notes",
        "Remove notes",
        "Add controller events",
    ] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    let before = call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap();
    let path = dir.join("midi.dawproj");
    call(&r, &mut s, "save_project", json!({"path": path})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap(),
        before
    );

    // Shortening hides notes without deleting them; removing the clip, then undoing.
    call(
        &r,
        &mut s,
        "set_midi_clip",
        json!({"clip": clip, "length": 1}),
    )
    .unwrap();
    assert_eq!(notes(&mut s).len(), 2);
    call(&r, &mut s, "remove_midi_clip", json!({"clip": clip})).unwrap();
    assert!(call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).is_err());
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(notes(&mut s).len(), 2);
    let _ = std::fs::remove_dir_all(dir);
}

/// Every MIDI edit reaches the engine: the graph (which holds the played sequence) is rebuilt.
/// Without that, notes an agent adds would stay silent until something else rebuilt it.
#[test]
fn every_midi_edit_rebuilds_the_graph() {
    let r = Registry::new();
    let mut s = headless();
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    let edit = |s: &mut Session, action: &str, args: Value| -> Value {
        s.update();
        assert!(!s.graph_dirty());
        let out = call(&r, s, action, args).unwrap();
        assert!(s.graph_dirty(), "{action} did not rebuild the graph");
        out
    };
    let clip = id(
        &edit(&mut s, "add_midi_clip", json!({"track": keys, "bar": 1})),
        "clip",
    );
    let note = edit(
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [{"tick": 0, "length": 10, "pitch": 60}]}),
    )["notes"][0]
        .clone();
    edit(
        &mut s,
        "set_notes",
        json!({"clip": clip, "notes": [{"note": note, "pitch": 61}]}),
    );
    let event = edit(
        &mut s,
        "add_controller_events",
        json!({"clip": clip, "events": [{"tick": 0, "kind": {"type": "cc", "number": 1, "value": 9}}]}),
    )["events"][0]
        .clone();
    edit(
        &mut s,
        "remove_controller_events",
        json!({"clip": clip, "events": [event]}),
    );
    edit(
        &mut s,
        "remove_notes",
        json!({"clip": clip, "notes": [note]}),
    );
    edit(&mut s, "set_midi_clip", json!({"clip": clip, "start": 960}));
    // Half a bar in at 120 BPM (one second at the session's rate).
    let at = s.sample_rate();
    edit(&mut s, "split_clip", json!({"clip": clip, "position": at}));
    edit(&mut s, "undo", Value::Null);
    edit(&mut s, "set_chase_notes", json!({"on": false}));
    edit(&mut s, "remove_midi_clip", json!({"clip": clip}));
    edit(&mut s, "undo", Value::Null);
    assert!(!s.project.chase_notes);
}

/// The piano roll's Ctrl+C / Ctrl+X / Ctrl+V on notes, as actions: copied notes keep their
/// distances from the earliest one, which a paste puts at the tick nearest the position (through
/// the tempo map, across a tempo change, not snapped); past the clip's end the clip grows to the
/// bar line after them; before its start a paste is refused; a cut is one undo step; notes paste
/// into another clip; the clip clipboard is a separate one.
#[test]
fn notes_copy_cut_and_paste_at_a_position_keeping_their_offsets() {
    let r = Registry::new();
    let mut s = headless();
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    // Bars 2-3 (ticks 3840 to 11520) and bar 6 (19200, one bar).
    let a = id(
        &call(
            &r,
            &mut s,
            "add_midi_clip",
            json!({"track": keys, "bar": 2, "bars": 2}),
        )
        .unwrap(),
        "clip",
    );
    let b = id(
        &call(
            &r,
            &mut s,
            "add_midi_clip",
            json!({"track": keys, "bar": 6}),
        )
        .unwrap(),
        "clip",
    );
    let added = call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": a, "notes": [
            {"tick": 0, "length": 480, "pitch": 60, "velocity": 90, "channel": 2},
            {"tick": 480, "length": 240, "pitch": 64},
            {"tick": 1920, "length": 960, "pitch": 67, "velocity": 70},
        ]}),
    )
    .unwrap();
    let (c, g) = (
        added["notes"][0].as_u64().unwrap(),
        added["notes"][2].as_u64().unwrap(),
    );
    // From bar 3 (tick 7680, sample 192,000 at 120), 60 quarter notes a minute: 50 samples a
    // tick there instead of 25.
    call(&r, &mut s, "add_tempo_change", json!({"bar": 3, "bpm": 60})).unwrap();
    let clip = |s: &Session, id: u64| s.project.find_midi_clip(id as u32).unwrap().1.clone();
    let undo_len = |s: &mut Session| {
        call(&r, s, "get_history", Value::Null).unwrap()["undo"]
            .as_array()
            .unwrap()
            .len()
    };
    let refused = |s: &mut Session, action: &str, args: Value, says: &str| {
        let err = call(&r, s, action, args).unwrap_err();
        assert!(err.contains(says), "{action}: {err}");
    };

    refused(
        &mut s,
        "paste_notes",
        json!({"clip": a}),
        "the note clipboard is empty",
    );
    refused(
        &mut s,
        "copy_notes",
        json!({"clip": a, "notes": [c, 9999]}),
        "no note with id 9999",
    );
    // The clip clipboard is apart: copying notes leaves it alone.
    call(&r, &mut s, "copy_clips", json!({"clips": [b]})).unwrap();
    let steps = undo_len(&mut s);
    let copied = call(
        &r,
        &mut s,
        "copy_notes",
        json!({"clip": a, "notes": [g, c]}),
    )
    .unwrap();
    assert_eq!(copied, json!({"notes": 2}));
    assert_eq!(undo_len(&mut s), steps, "copying is no undo step");
    assert_eq!(s.clipboard_len(), 1, "the copied clip is still there");

    // 25,007 samples into bar 3 at 50 a tick: tick 7680 + 500 (500.14, the nearest).
    s.update();
    let pasted = call(
        &r,
        &mut s,
        "paste_notes",
        json!({"clip": a, "position": 192_000 + 25_007}),
    )
    .unwrap();
    assert!(s.graph_dirty(), "a paste rebuilds the graph");
    let new: Vec<u32> = pasted["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    let shape = |s: &Session, id: u64, ids: &[u32]| -> Vec<(u64, u64, u8, u8, u8)> {
        ids.iter()
            .map(|n| {
                let n = clip(s, id).notes.into_iter().find(|x| x.id == *n).unwrap();
                (n.tick, n.length, n.pitch, n.velocity, n.channel)
            })
            .collect()
    };
    // The earliest copied (C, tick 0) at 8180 - 3840; G 1920 after it, as it was.
    assert_eq!(
        shape(&s, a, &new),
        [(4340, 480, 60, 90, 2), (6260, 960, 67, 70, 0)]
    );
    assert_eq!(clip(&s, a).notes.len(), 5);
    assert_eq!(clip(&s, a).length, 7680, "inside the clip: its length kept");
    assert_eq!(undo_len(&mut s), steps + 1);
    assert_eq!(
        call(&r, &mut s, "get_history", Value::Null).unwrap()["undo"][0],
        "Paste notes"
    );

    // Cut: copied again and removed, one undo step; undone, they're back.
    s.update();
    let ids: Vec<u64> = new.iter().map(|&n| u64::from(n)).collect();
    let cut = call(&r, &mut s, "cut_notes", json!({"clip": a, "notes": ids})).unwrap();
    assert_eq!(cut, json!({"notes": 2}));
    assert!(s.graph_dirty(), "a cut rebuilds the graph");
    assert_eq!(clip(&s, a).notes.len(), 3);
    assert_eq!(undo_len(&mut s), steps + 2);
    assert_eq!(
        call(&r, &mut s, "get_history", Value::Null).unwrap()["undo"][0],
        "Cut notes"
    );
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(
        clip(&s, a).notes.len(),
        5,
        "one undo brings the cut notes back"
    );
    assert_eq!(s.note_clipboard_len(), 2);

    // Past the end: from bar 3 beat 4 (tick 10560, 6720 into the clip), G ends at 9600 in it:
    // the clip grows to the next bar line (bar 5, tick 15360).
    let late = call(
        &r,
        &mut s,
        "paste_notes",
        json!({"clip": a, "at": "3:4:000"}),
    )
    .unwrap();
    let late: Vec<u32> = late["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    assert_eq!(
        shape(&s, a, &late),
        [(6720, 480, 60, 90, 2), (8640, 960, 67, 70, 0)]
    );
    assert_eq!(clip(&s, a).length, 15360 - 3840);

    // Before the clip's start: refused, nothing changes.
    let before = clip(&s, a);
    refused(
        &mut s,
        "paste_notes",
        json!({"clip": a, "position": 0}),
        "can't paste notes at 1:1:000: MIDI clip",
    );
    refused(
        &mut s,
        "paste_notes",
        json!({"clip": a, "at": "1:4"}),
        "starts later, at 2:1:000",
    );
    refused(
        &mut s,
        "paste_notes",
        json!({"clip": a, "position": 1, "at": "2:1"}),
        "not both",
    );
    assert_eq!(clip(&s, a), before);

    // Into another clip, at its start.
    let other = call(&r, &mut s, "paste_notes", json!({"clip": b, "at": "6:1"})).unwrap();
    let other: Vec<u32> = other["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    assert_eq!(
        shape(&s, b, &other),
        [(0, 480, 60, 90, 2), (1920, 960, 67, 70, 0)]
    );
    assert_eq!(clip(&s, b).length, 3840);
    // No clip or track was added by any of it.
    assert_eq!(s.project.tracks.len(), 1);
    assert_eq!(s.project.track(keys as u32).unwrap().midi_clips.len(), 2);
}

/// Recorded MIDI (stamped as the engine does, with heard positions) lands where it was heard:
/// output latency off, the count-in dropped, channel filters respected, held notes ended at
/// stop; merge, replace and quantize as set; loop passes fold and overdub.
#[test]
fn recorded_midi_lands_where_it_was_heard() {
    use crate::engine::RecordedMidi;
    use vst3_host::{MidiChannel, MidiEvent};

    let r = Registry::new();
    let mut s = headless();
    let add = |s: &mut Session| {
        id(
            &call(&r, s, "add_track", json!({"kind": "instrument"})).unwrap(),
            "track",
        ) as u32
    };
    let (keys, bass) = (add(&mut s), add(&mut s));
    // The bass listens to channel 2 only. Instrument tracks are armed when added.
    call(
        &r,
        &mut s,
        "set_track_input",
        json!({"track": bass, "input": {"type": "midi", "channel": 1}}),
    )
    .unwrap();
    let note = |heard: i64, wraps: u32, channel: MidiChannel, pitch: u8, on: bool| RecordedMidi {
        heard,
        wraps,
        port: 0,
        event: if on {
            MidiEvent::NoteOn {
                channel,
                note: pitch,
                velocity: 100,
            }
        } else {
            MidiEvent::NoteOff {
                channel,
                note: pitch,
                velocity: 0,
            }
        },
    };
    let ch1 = MidiChannel::Ch1;
    let ch2 = MidiChannel::Ch2;
    // 25 samples a tick; output latency 480. Recorded from 24000 (tick 960), stopped at
    // 120000 (tick 4800).
    let events = vec![
        note(20_480, 0, ch1, 48, true), // during the count-in: dropped
        note(25_480, 0, ch1, 60, true),
        note(49_480, 0, ch1, 60, false),
        note(60_480, 0, ch2, 64, true),
        note(72_480, 0, ch2, 64, false),
        note(100_480, 0, ch1, 67, true), // still held at stop
    ];
    s.finish_midi_recording(events, (24_000, u64::MAX), 120_000, 480);
    let clips = |s: &mut Session, track: u32| s.project.track(track).unwrap().midi_clips.clone();
    let keys_clips = clips(&mut s, keys);
    assert_eq!(keys_clips.len(), 1);
    let take = &keys_clips[0];
    assert_eq!((take.start, take.length), (960, 3840));
    let pitches = |c: &crate::midi_clip::MidiClip| -> Vec<(u64, u64, u8)> {
        c.notes
            .iter()
            .map(|n| (n.tick, n.length, n.pitch))
            .collect()
    };
    assert_eq!(
        pitches(take),
        [(40, 960, 60), (1440, 480, 64), (3040, 800, 67)]
    );
    assert_eq!(
        pitches(&clips(&mut s, bass)[0]),
        [(1440, 480, 64)],
        "channel 2 only"
    );
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Record");

    // Merge (the default): a second pass over the same range goes into the same clip.
    s.finish_midi_recording(
        vec![
            note(30_480, 0, ch1, 72, true),
            note(31_480, 0, ch1, 72, false),
        ],
        (24_000, u64::MAX),
        120_000,
        480,
    );
    assert_eq!(clips(&mut s, keys).len(), 1);
    assert_eq!(clips(&mut s, keys)[0].notes.len(), 4);
    // Replace, quantized to sixteenths (240 ticks): only the new, snapped note is left.
    call(
        &r,
        &mut s,
        "set_midi_record_options",
        json!({"replace": true, "quantize": "1/16"}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "set_midi_record_options",
        json!({"quantize": "1/7x"})
    )
    .is_err());
    s.finish_midi_recording(
        vec![
            note(36_580, 0, ch1, 55, true),
            note(40_480, 0, ch1, 55, false),
        ],
        (24_000, u64::MAX),
        120_000,
        480,
    );
    assert_eq!(pitches(&clips(&mut s, keys)[0]), [(480, 156, 55)]);

    // Loop recording over 0..96000: just after the wrap, a note heard at 100 (less 480 of
    // output latency) was the end of the loop; in the first pass it was before it.
    call(
        &r,
        &mut s,
        "set_midi_record_options",
        json!({"replace": false, "quantize": "off"}),
    )
    .unwrap();
    s.project.loop_region = crate::project::Region {
        start: 0,
        end: 96_000,
        enabled: true,
    };
    call(&r, &mut s, "remove_track", json!({"track": bass})).unwrap();
    s.finish_midi_recording(
        vec![
            note(100, 0, ch1, 40, true),
            note(200, 0, ch1, 40, false),
            note(100, 1, ch1, 41, true),
            note(200, 1, ch1, 41, false),
        ],
        (0, u64::MAX),
        0,
        480,
    );
    let looped: Vec<_> = clips(&mut s, keys)
        .iter()
        .flat_map(|c| c.notes.iter().map(|n| (c.start + n.tick, n.pitch)))
        .filter(|(_, p)| *p < 50)
        .collect();
    assert_eq!(
        looped,
        // 100 − 480 + 96000 = 95620 samples = tick 3824.8.
        [(3825, 41)],
        "folded to the loop's end; the first pass's dropped"
    );
}

/// The piano roll's operations: quantize (all, some, with a strength), previewing notes with its
/// guards and timed release, and a note drag over several frames being one undo step while a
/// draw and a delete are two.
#[test]
fn piano_roll_edits_quantize_and_preview_notes() {
    let r = Registry::new();
    let mut s = headless();
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    let audio = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "add_midi_clip",
            json!({"track": keys, "bar": 1, "bars": 2}),
        )
        .unwrap(),
        "clip",
    );
    // Off the sixteenths (240 ticks) by 30, 100 and 200 ticks.
    let ids: Vec<u64> = call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [
            {"tick": 270, "length": 100, "pitch": 60},
            {"tick": 580, "length": 100, "pitch": 62},
            {"tick": 1160, "length": 100, "pitch": 64},
        ]}),
    )
    .unwrap()["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap())
        .collect();
    let ticks = |s: &mut Session| -> Vec<u64> {
        let mut t: Vec<u64> = s
            .project
            .find_midi_clip(clip as u32)
            .unwrap()
            .1
            .notes
            .iter()
            .map(|n| n.tick)
            .collect();
        t.sort();
        t
    };
    // Half strength, only the first two: 270 → 255, 580 → 530 (halfway to 480).
    call(
        &r,
        &mut s,
        "quantize_notes",
        json!({"clip": clip, "division": "1/16", "strength": 50, "notes": [ids[0], ids[1]]}),
    )
    .unwrap();
    assert_eq!(ticks(&mut s), [255, 530, 1160]);
    call(
        &r,
        &mut s,
        "quantize_notes",
        json!({"clip": clip, "division": "1/16"}),
    )
    .unwrap();
    assert_eq!(ticks(&mut s), [240, 480, 1200]);
    assert!(call(
        &r,
        &mut s,
        "quantize_notes",
        json!({"clip": clip, "division": "1/16", "notes": [9999]})
    )
    .is_err());

    // A note preview: only an instrument track with a plug-in; its note is released on time.
    let refused = |s: &mut Session, track: u64, says: &str| {
        let err = call(&r, s, "preview_note", json!({"track": track, "pitch": 60})).unwrap_err();
        assert!(err.contains(says), "{err}");
    };
    refused(&mut s, audio, "not an instrument");
    refused(&mut s, keys, "no plug-in");
    s.project
        .track_mut(keys as u32)
        .unwrap()
        .inserts
        .push(crate::project::InsertSlot {
            id: 77,
            name: "stand-in".into(),
            path: "stand-in.vst3".into(),
            bypass: false,
            state: None,
        });
    assert!(call(
        &r,
        &mut s,
        "preview_note",
        json!({"track": keys, "pitch": 60, "duration_ms": 20000})
    )
    .is_err());
    call(
        &r,
        &mut s,
        "preview_note",
        json!({"track": keys, "pitch": 60, "duration_ms": 5}),
    )
    .unwrap();
    assert_eq!(s.note_previews_pending(), 1);
    std::thread::sleep(Duration::from_millis(20));
    s.update();
    assert_eq!(s.note_previews_pending(), 0, "released after its duration");

    // The editor's undo steps: a three-frame drag merges; a draw and a delete don't.
    let before = call(&r, &mut s, "get_history", Value::Null).unwrap()["undo"]
        .as_array()
        .unwrap()
        .len();
    let mut notes = s
        .project
        .find_midi_clip(clip as u32)
        .unwrap()
        .1
        .notes
        .clone();
    for frame in 1..=3 {
        s.begin_edit("", None);
        s.label_edit("Move notes", Some("piano-drag:1".into()));
        notes[0].tick = 240 + frame * 120;
        s.project.set_notes(clip as u32, notes.clone()).unwrap();
        s.end_edit();
    }
    s.begin_edit("", None);
    s.label_edit("Add note", None);
    let added = s.project.add_notes(clip as u32, &[notes[0]]).unwrap();
    s.end_edit();
    s.begin_edit("", None);
    s.label_edit("Delete notes", None);
    s.project.remove_notes(clip as u32, &added).unwrap();
    s.end_edit();
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    let undo = history["undo"].as_array().unwrap();
    assert_eq!(undo.len(), before + 3);
    assert_eq!(
        &undo[..3],
        &[
            json!("Delete notes"),
            json!("Add note"),
            json!("Move notes")
        ]
    );
}

/// MIDI files over MCP: read one, import it with and without its tempo map (the clips keep
/// their bar through a meter change, and the file's changes move with the drop bar), and
/// export a clip and read it back.
#[test]
fn midi_files_import_with_or_without_their_tempo_and_export() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("midi-files");
    let path = dir.join("song.mid");
    std::fs::write(&path, crate::smf::tests::sample_file(480)).unwrap();
    let keys = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "instrument", "name": "Keys"}),
        )
        .unwrap(),
        "track",
    );
    let after = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "audio", "name": "After"}),
        )
        .unwrap(),
        "track",
    );

    let info = call(&r, &mut s, "read_midi_file", json!({"path": path})).unwrap();
    assert_eq!(info["tracks"].as_array().unwrap().len(), 2);
    assert_eq!(info["tracks"][0]["name"], "Lead");
    assert_eq!(
        (&info["tempo"]["bpm"], &info["tempo"]["time_signature"]),
        (&json!(140.0), &json!([4, 4]))
    );
    assert_eq!(
        info["tempo"]["changes"][0],
        json!({"bar": 2, "beat": 1, "bpm": null, "time_signature": [3, 4]})
    );
    assert_eq!(info["tempo"]["changes"][1]["bpm"], 70.0);

    // Without its tempo: at bar 3 of the project's 4/4 (tick 7680). The second track goes on
    // a new instrument track right after Keys, before "After".
    let made = call(
        &r,
        &mut s,
        "import_midi_file",
        json!({"path": path, "track": keys, "bar": 3}),
    )
    .unwrap();
    let clips = made["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!(clips[0]["track"], keys);
    let order: Vec<u64> = s.project.tracks.iter().map(|t| u64::from(t.id)).collect();
    assert_eq!(order, [keys, clips[1]["track"].as_u64().unwrap(), after]);
    let lead = s
        .project
        .find_midi_clip(clips[0]["clip"].as_u64().unwrap() as u32)
        .unwrap()
        .1
        .clone();
    assert_eq!(
        (lead.start, lead.length, lead.name.as_str()),
        (7680, 3840, "Lead")
    );
    assert_eq!(s.project.tempo, 120.0);
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert!(s.project.tracks.iter().all(|t| t.midi_clips.is_empty()));

    // With it, in a 3/4 project: bar 3 was tick 5760; under the file's 4/4 it's 7680, and the
    // clip stays on bar 3. The file's changes (its bars 2 and 3) land on bars 4 and 5.
    s.project.time_signature = (3, 4);
    let made = call(
        &r,
        &mut s,
        "import_midi_file",
        json!({"path": path, "track": keys, "bar": 3, "set_tempo": true}),
    )
    .unwrap();
    assert_eq!((s.project.tempo, s.project.time_signature), (140.0, (4, 4)));
    let changes = &s.project.tempo_changes;
    assert_eq!(changes.len(), 2);
    assert_eq!(
        (changes[0].bar, changes[0].tempo, changes[0].time_signature),
        (3, None, Some((3, 4)))
    );
    assert_eq!(
        (changes[1].bar, changes[1].tempo, changes[1].time_signature),
        (4, Some(70.0), None)
    );
    let clip = made["clips"][0]["clip"].as_u64().unwrap();
    assert_eq!(s.project.find_midi_clip(clip as u32).unwrap().1.start, 7680);

    // Export and read back: the notes that play, at 960 ticks a quarter note.
    let out = dir.join("lead.mid");
    call(
        &r,
        &mut s,
        "export_midi_clip",
        json!({"clip": clip, "path": out}),
    )
    .unwrap();
    let back = call(&r, &mut s, "read_midi_file", json!({"path": out})).unwrap();
    assert_eq!(back["tracks"][0]["notes"], 2);
    assert_eq!(back["tempo"]["bpm"], 140.0);
    let err = call(
        &r,
        &mut s,
        "read_midi_file",
        json!({"path": dir.join("missing.mid")}),
    )
    .unwrap_err();
    assert!(err.contains("missing.mid"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A multi-track MIDI file (14.1) as Guitar Pro exports a piano: two staves, both "Acoustic
/// Piano". Imported onto an instrument track, each staff lands on its own track, named by
/// register, every note on its tick, and the new track plays its own copy of the instrument
/// (Amplify stands in for one: it is the first insert, settings included).
#[test]
fn a_pianos_two_staves_import_onto_two_named_tracks() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("midi-staves");
    let path = dir.join("piano.mid");
    std::fs::write(&path, crate::smf::tests::staves_file(480)).unwrap();
    let piano = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "instrument", "name": "Piano"}),
        )
        .unwrap(),
        "track",
    );
    let amp = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": piano, "plugin": "amplify"}),
        )
        .unwrap(),
        "insert",
    );
    let params = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp})).unwrap();
    let left = params["parameters"][0]["id"].clone();
    let set = json!({"insert": amp, "parameter": left, "value": (6.0 + 96.0) / 144.0});
    call(&r, &mut s, "set_plugin_parameter", set).unwrap();

    let info = call(&r, &mut s, "read_midi_file", json!({"path": path})).unwrap();
    let names: Vec<&str> = info["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["import_as"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["Acoustic Piano (treble)", "Acoustic Piano (bass)"]);
    assert_eq!(info["tracks"][0]["name"], "Acoustic Piano");

    let made = call(
        &r,
        &mut s,
        "import_midi_file",
        json!({"path": path, "track": piano, "bar": 1, "set_tempo": true}),
    )
    .unwrap();
    assert_eq!(s.project.tempo, 90.0);
    let clips = made["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 2);
    assert_eq!(
        clips[0]["track"], piano,
        "the first staff on the chosen track"
    );
    let bass = clips[1]["track"].as_u64().unwrap();
    let order: Vec<u64> = s.project.tracks.iter().map(|t| u64::from(t.id)).collect();
    assert_eq!(order, [piano, bass]);
    let track = s.project.track(bass as u32).unwrap();
    assert_eq!(track.name, "Acoustic Piano (bass)");
    assert_eq!(track.kind, crate::project::TrackKind::Instrument);
    // Its own Amplify with the same settings.
    assert_eq!(track.inserts.len(), 1);
    let copy = u64::from(track.inserts[0].id);
    assert_ne!(copy, amp);
    let shown = |s: &mut Session, insert: u64| {
        call(&r, s, "list_plugin_parameters", json!({"insert": insert})).unwrap()["parameters"][0]
            ["display"]
            .clone()
    };
    assert_eq!(shown(&mut s, copy), "+6.00 dB");
    let notes = |clip: &Value| {
        let c = &s
            .project
            .find_midi_clip(clip["clip"].as_u64().unwrap() as u32)
            .unwrap()
            .1;
        let notes: Vec<(u64, u64, u8)> = c
            .notes
            .iter()
            .map(|n| (n.tick, n.length, n.pitch))
            .collect();
        (c.name.clone(), notes)
    };
    let (name, treble) = notes(&clips[0]);
    assert_eq!(name, "Acoustic Piano (treble)");
    let expected: Vec<(u64, u64, u8)> = [64, 65, 67, 69, 71, 72, 74, 76]
        .iter()
        .enumerate()
        .map(|(k, &p)| (k as u64 * 960, 960, p))
        .collect();
    assert_eq!(treble, expected);
    let (name, low) = notes(&clips[1]);
    assert_eq!(name, "Acoustic Piano (bass)");
    assert_eq!(
        low,
        [
            (0, 1920, 36),
            (1920, 1920, 43),
            (3840, 1920, 41),
            (5760, 1920, 48)
        ]
    );

    // One undo step takes it all back, the new track and its instrument too.
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(s.project.tracks.len(), 1);
    assert!(s.project.tracks[0].midi_clips.is_empty());

    // Without `same_instrument`, or onto no track: the new tracks are empty instruments.
    let made = call(
        &r,
        &mut s,
        "import_midi_file",
        json!({"path": path, "track": piano, "same_instrument": false}),
    )
    .unwrap();
    let bass = made["clips"][1]["track"].as_u64().unwrap();
    assert!(s.project.track(bass as u32).unwrap().inserts.is_empty());
    let made = call(&r, &mut s, "import_midi_file", json!({"path": path})).unwrap();
    for clip in made["clips"].as_array().unwrap() {
        let t = s
            .project
            .track(clip["track"].as_u64().unwrap() as u32)
            .unwrap();
        assert!(t.inserts.is_empty() && t.name.starts_with("Acoustic Piano ("));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn tempo_changes_move_the_grid_and_survive_a_save() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("tempo-map");
    // Two 4/4 bars at 120 (96000 samples each), then 3/4 at 90 from bar 3.
    let map = call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 3, "bpm": 90, "numerator": 3, "denominator": 4}),
    )
    .unwrap();
    assert_eq!(map["changes"][0]["position"], 192_000);
    // Refused: the opening bar, a meter mid-bar, a beat the bar lacks, a wild tempo.
    for (args, why) in [
        (json!({"bar": 1, "bpm": 100}), "opening"),
        (
            json!({"bar": 4, "beat": 2, "numerator": 5, "denominator": 4}),
            "first beat",
        ),
        (json!({"bar": 4, "beat": 4, "bpm": 100}), "no beat 4"),
        (json!({"bar": 5, "bpm": 400}), "outside"),
    ] {
        let err = call(&r, &mut s, "add_tempo_change", args).unwrap_err();
        assert!(err.contains(why), "{err}");
    }
    // A tempo change on beat 2 of bar 4: in 3/4 at 90 a beat is 32000.
    let map = call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 4, "beat": 2, "bpm": 180}),
    )
    .unwrap();
    assert_eq!(map["changes"][1]["position"], 192_000 + 96_000 + 32_000);

    let t = call(&r, &mut s, "locate", json!({"position": 200_000})).unwrap();
    assert_eq!(t["tempo"], 90.0);
    assert_eq!(t["time_signature"], json!([3, 4]));
    assert_eq!(t["bar_beat"], "3.1");
    // Quantize across the change: beats are 24000 apart before it, 32000 after.
    let q = call(
        &r,
        &mut s,
        "quantize_positions",
        json!({"positions": [30_000, 240_000], "division": "beat"}),
    )
    .unwrap();
    let snapped: Vec<u64> = q["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["quantized"].as_u64().unwrap())
        .collect();
    assert_eq!(snapped, vec![24_000, 256_000]);

    let project = dir.join("tempo.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    let map = call(&r, &mut s, "get_tempo_map", Value::Null).unwrap();
    assert_eq!(map["changes"].as_array().unwrap().len(), 2);
    let map = call(
        &r,
        &mut s,
        "remove_tempo_change",
        json!({"bar": 4, "beat": 2}),
    )
    .unwrap();
    assert_eq!(map["changes"].as_array().unwrap().len(), 1);
    assert!(call(
        &r,
        &mut s,
        "remove_tempo_change",
        json!({"bar": 4, "beat": 2})
    )
    .is_err());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn projects_from_before_quarter_note_tempo_keep_their_speed() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("old-tempo");
    let project = dir.join("old.dawproj");
    call(&r, &mut s, "set_tempo", json!({"bpm": 120})).unwrap();
    call(
        &r,
        &mut s,
        "set_time_signature",
        json!({"numerator": 6, "denominator": 8}),
    )
    .unwrap();
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    // As saved before 1.8: no version, and 120 meant eighth notes in 6/8.
    let mut json: Value =
        serde_json::from_str(&std::fs::read_to_string(&project).unwrap()).unwrap();
    json.as_object_mut().unwrap().remove("version");
    std::fs::write(&project, json.to_string()).unwrap();
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    let map = call(&r, &mut s, "get_tempo_map", Value::Null).unwrap();
    assert_eq!(map["tempo"], 60.0, "120 eighths a minute is 60 quarters");
    // A beat (an eighth) still lasts half a second.
    let t = call(&r, &mut s, "locate", json!({"position": 24_000})).unwrap();
    assert_eq!(t["position_beats"], 1.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn record_offset_adds_to_the_record_shift() {
    let r = Registry::new();
    let mut s = headless();
    let view = call(&r, &mut s, "get_audio_settings", Value::Null).unwrap();
    // No device here: no round trip measured, no delay compensation.
    assert_eq!(view["round_trip"], Value::Null);
    assert_eq!(
        (
            view["record_offset"].as_i64(),
            view["record_shift"].as_u64()
        ),
        (Some(0), Some(0))
    );
    let view = call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"record_offset": 120}),
    )
    .unwrap();
    assert_eq!(view["record_shift"], 120);
    // Negative moves takes later; the shift never goes below zero.
    let view = call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"record_offset": -50}),
    )
    .unwrap();
    assert_eq!(
        (
            view["record_offset"].as_i64(),
            view["record_shift"].as_u64()
        ),
        (Some(-50), Some(0))
    );
    assert!(call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"record_offset": 100_000})
    )
    .is_err());
    // The offset alone does not reopen the device.
    let before = s.settings.clone();
    assert!(!before.needs_restart(&crate::device::AudioSettings {
        record_offset: 7,
        ..before.clone()
    }));
}

#[test]
fn a_processing_block_gathers_device_buffers_and_moves_takes_by_its_latency() {
    let r = Registry::new();
    let mut s = headless();
    s.start_virtual_device(16).unwrap();
    let view = call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"processing_block": 64}),
    )
    .unwrap();
    assert_eq!(view["processing_block"], 64);
    // The engine picks it up at its next callback: the virtual device's run every 1/3 ms.
    let waited = std::time::Instant::now();
    while s.stats().block_size != 64 && waited.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let stats = call(&r, &mut s, "get_stats", Value::Null).unwrap();
    assert_eq!(
        (stats["block_size"].as_u64(), stats["device_block"].as_u64()),
        (Some(64), Some(16))
    );
    assert_eq!(stats["cadence"], 16, "the virtual device calls back evenly");
    let view = call(&r, &mut s, "get_audio_settings", Value::Null).unwrap();
    assert_eq!(view["stage_latency"], 48);
    assert_eq!(
        view["record_shift"], 48,
        "takes move earlier by what the stage adds"
    );
    // No restart for it.
    let before = s.settings.clone();
    assert!(!before.needs_restart(&crate::device::AudioSettings {
        processing_block: None,
        ..before.clone()
    }));
    assert!(call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"processing_block": 4096})
    )
    .unwrap_err()
    .contains("4096"));
    let view = call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"processing_block": 0}),
    )
    .unwrap();
    assert_eq!(view["processing_block"], Value::Null);
    let waited = std::time::Instant::now();
    while s.stats().block_size != 16 && waited.elapsed().as_secs() < 5 {
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(s.stage_latency(), 0);
    assert_eq!(s.record_shift(), 0);
}

#[test]
fn loop_punch_and_playhead_are_fixed_while_recording() {
    let r = Registry::new();
    let mut s = headless();
    call(
        &r,
        &mut s,
        "set_loop",
        json!({"start": 0, "end": 96_000, "enabled": true}),
    )
    .unwrap();
    s.pretend_recording();
    for (action, args) in [
        ("set_loop", json!({"enabled": false})),
        ("set_loop", json!({"start": 0, "end": 48_000})),
        ("set_punch", json!({"enabled": true})),
        ("locate", json!({"position": 1000})),
    ] {
        let err = call(&r, &mut s, action, args).unwrap_err();
        assert!(err.contains("stop recording first"), "{action}: {err}");
    }
    // Once stopped, they move again (while the files are still being finished).
    call(&r, &mut s, "stop", Value::Null).unwrap();
    call(&r, &mut s, "locate", json!({"position": 1000})).unwrap();
    call(&r, &mut s, "set_loop", json!({"enabled": false})).unwrap();
}

#[test]
fn projects_saved_before_takes_load_and_play_the_same() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("old-project");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let before = mixdown(&r, &mut s, &dir, "before.wav");
    let project = dir.join("old.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();

    // Rewrite the clip the way projects stored it before takes: file and offset on the clip.
    let mut json: Value =
        serde_json::from_str(&std::fs::read_to_string(&project).unwrap()).unwrap();
    let clip = &mut json["tracks"][0]["clips"][0];
    let take = clip["takes"][0].clone();
    let fields = clip.as_object_mut().unwrap();
    fields.remove("takes");
    fields.remove("active");
    fields.insert("file".into(), take["file"].clone());
    fields.insert("offset".into(), take["offset"].clone());
    std::fs::write(&project, serde_json::to_string_pretty(&json).unwrap()).unwrap();

    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    let clip = &call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"][0];
    assert_eq!(clip["takes"].as_array().unwrap().len(), 1);
    let after = mixdown(&r, &mut s, &dir, "after.wav");
    assert!(before.iter().any(|v| *v != 0.0));
    assert_eq!(before, after, "the old project plays exactly as before");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_action_has_an_object_schema_and_a_description() {
    let specs = Registry::new().specs();
    assert!(specs.len() >= 40, "only {} actions registered", specs.len());
    for spec in specs {
        assert_eq!(spec.input_schema["type"], "object", "{}", spec.name);
        assert!(
            spec.description.len() > 20,
            "{} needs a real description",
            spec.name
        );
        assert!(
            spec.name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '_'),
            "{} is not snake_case",
            spec.name
        );
    }
}

/// `docs/mcp-tools.md` must match the registry. Set UPDATE_DOCS=1 to rewrite it.
#[test]
fn docs_are_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/mcp-tools.md");
    let expected = markdown(&Registry::new());
    if std::env::var_os("UPDATE_DOCS").is_some() {
        std::fs::write(path, &expected).unwrap();
    }
    let actual = std::fs::read_to_string(path)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    assert!(
        actual == expected,
        "docs/mcp-tools.md is stale: regenerate it with UPDATE_DOCS=1 set (PowerShell: $env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\\UPDATE_DOCS)"
    );

    // The bridge's catalog (23.3): what `oxide-mcp.exe` lists while Oxide is closed. Compared
    // as JSON, so line endings and whitespace don't matter.
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../mcp-link/src/catalog.json");
    let expected = crate::api::catalog(&Registry::new());
    if std::env::var_os("UPDATE_DOCS").is_some() {
        let mut text = serde_json::to_string_pretty(&expected).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }
    let actual: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    assert!(
        actual == expected,
        "crates/mcp-link/src/catalog.json (the bridge's tools) is stale: regenerate it with UPDATE_DOCS=1 set (PowerShell: $env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\\UPDATE_DOCS)"
    );
}

/// The built-in effects as the manual's parameter tables show them (27.5): written to
/// `crates/manual/src/effects.json` with `UPDATE_DOCS` set (the same command as
/// `docs_are_current`, whose name this one contains), compared as JSON otherwise.
#[test]
fn effect_docs_are_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../manual/src/effects.json");
    let expected = crate::native::describe::effects();
    if std::env::var_os("UPDATE_DOCS").is_some() {
        let mut text = serde_json::to_string_pretty(&expected).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }
    let actual: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    assert!(
        actual == expected,
        "crates/manual/src/effects.json (the manual's effect tables) is stale: regenerate it with UPDATE_DOCS=1 set (PowerShell: $env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\\UPDATE_DOCS)"
    );
}

/// What the manual's MCP chapter is generated from (27.6): written to
/// `crates/manual/src/mcp.json` with `UPDATE_DOCS` set (the same command as
/// `docs_are_current`, whose name this one contains), compared as JSON otherwise.
#[test]
fn mcp_docs_are_current() {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../manual/src/mcp.json");
    let registry = Registry::new();
    let expected = crate::api::describe(&registry);
    if std::env::var_os("UPDATE_DOCS").is_some() {
        let mut text = serde_json::to_string_pretty(&expected).unwrap();
        text.push('\n');
        std::fs::write(path, text).unwrap();
    }
    let actual: Value = std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    assert!(
        actual == expected,
        "crates/manual/src/mcp.json (the manual's MCP reference) is stale: regenerate it with UPDATE_DOCS=1 set (PowerShell: $env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\\UPDATE_DOCS)"
    );
    // Every action has an area of the table, and every area has actions.
    let specs = registry.specs();
    for spec in &specs {
        assert!(
            crate::api::AREAS.iter().any(|a| a.0 == spec.area),
            "{} has no area",
            spec.name
        );
    }
    for (area, ..) in crate::api::AREAS {
        assert!(specs.iter().any(|s| s.area == *area), "{area} is empty");
    }
    // The undo lists name wrapped actions only, and none twice.
    for name in crate::api::NO_PROJECT_CHANGE
        .iter()
        .chain(crate::api::ALWAYS_CHANGES)
    {
        assert!(
            registry.is_undo_step(name),
            "{name} isn't wrapped in an edit"
        );
    }
    for name in crate::api::NO_PROJECT_CHANGE {
        assert!(!crate::api::ALWAYS_CHANGES.contains(name), "{name} in both");
    }
}

#[test]
fn tracks_routing_and_mixer() {
    let r = Registry::new();
    let mut s = headless();
    let folder = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "folder", "name": "Gtrs"}),
        )
        .unwrap(),
        "track",
    );
    let other = id(
        &call(&r, &mut s, "add_track", json!({"kind": "folder"})).unwrap(),
        "track",
    );
    let gtr = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "audio", "parent": folder}),
        )
        .unwrap(),
        "track",
    );
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );

    let mixed = call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": gtr, "gain_db": -6.5, "pan": -0.5, "mute": true}),
    )
    .unwrap();
    assert_eq!(mixed["mixer"]["gain_db"], -6.5);
    let named = call(
        &r,
        &mut s,
        "set_track",
        json!({"track": gtr, "name": "Rhythm", "color": "#1a2b3c"}),
    )
    .unwrap();
    assert_eq!(named["color"], "#1a2b3c");
    assert!(call(
        &r,
        &mut s,
        "set_track",
        json!({"track": gtr, "color": "blue"})
    )
    .unwrap_err()
    .contains("#rrggbb"));
    assert!(call(
        &r,
        &mut s,
        "set_track",
        json!({"track": gtr, "collapsed": true})
    )
    .unwrap_err()
    .contains("not a folder"));
    call(
        &r,
        &mut s,
        "set_track",
        json!({"track": folder, "collapsed": true}),
    )
    .unwrap();

    // Inputs must suit the track kind.
    call(
        &r,
        &mut s,
        "set_track_input",
        json!({"track": gtr, "input": {"type": "audio", "channel": 0}}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "set_track_input",
        json!({"track": gtr, "input": {"type": "midi"}})
    )
    .unwrap_err()
    .contains("instrument"));
    call(
        &r,
        &mut s,
        "set_track_input",
        json!({"track": keys, "input": {"type": "midi", "channel": 9}}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "set_track_input",
        json!({"track": keys, "input": {"type": "midi", "channel": 16}})
    )
    .unwrap_err()
    .contains("0-15"));

    // Outputs: buses must exist and not loop.
    call(
        &r,
        &mut s,
        "set_track_output",
        json!({"track": gtr, "output": {"type": "bus", "track": other}}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "set_track_output",
        json!({"track": folder, "output": {"type": "bus", "track": folder}})
    )
    .unwrap_err()
    .contains("loop"));
    call(
        &r,
        &mut s,
        "set_track_output",
        json!({"track": gtr, "output": {"type": "hardware", "channel": 0}}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "set_track_output",
        json!({"track": gtr, "output": {"type": "hardware", "channel": 8}})
    )
    .unwrap_err()
    .contains("outputs"));
    call(
        &r,
        &mut s,
        "set_track_monitor",
        json!({"track": gtr, "monitor": "on"}),
    )
    .unwrap();

    // Nesting and order.
    call(
        &r,
        &mut s,
        "set_track_parent",
        json!({"track": keys, "folder": other}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "set_track_parent",
        json!({"track": other, "folder": other})
    )
    .is_err());
    call(
        &r,
        &mut s,
        "move_track",
        json!({"track": other, "direction": "up"}),
    )
    .unwrap();

    let master = call(
        &r,
        &mut s,
        "set_master",
        json!({"gain_db": -1.0, "mute": false}),
    )
    .unwrap();
    assert_eq!(master["mixer"]["gain_db"], -1.0);

    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    let names: Vec<&str> = project["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        ["Folder 2", "Instrument 1", "Gtrs", "Rhythm"],
        "moved folder came first with its child"
    );
    let rhythm = &project["tracks"][3];
    assert_eq!(rhythm["mixer"]["mute"], true);
    assert_eq!(rhythm["monitor"], "On");

    call(&r, &mut s, "remove_track", json!({"track": gtr})).unwrap();
    assert!(call(&r, &mut s, "remove_track", json!({"track": gtr}))
        .unwrap_err()
        .contains(&gtr.to_string()));
}

#[test]
fn clips_import_split_move_and_waveform() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("clips");
    let file = tone(&dir);
    let a = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let b = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let folder = id(
        &call(&r, &mut s, "add_track", json!({"kind": "folder"})).unwrap(),
        "track",
    );

    let clip = call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": a, "path": file, "position": 1000}),
    )
    .unwrap();
    let clip_id = id(&clip, "clip");
    assert_eq!(clip["length"], 48_000);
    assert!(call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": folder, "path": file})
    )
    .unwrap_err()
    .contains("not an audio track"));

    // 8 segments of 6000 samples: the first is well inside the silent quarter (peaks are
    // cached per 256 samples, so a segment touching the boundary may see the tone).
    let wave = call(
        &r,
        &mut s,
        "get_clip_waveform",
        json!({"clip": clip_id, "points": 8}),
    )
    .unwrap();
    let points = wave["points"].as_array().unwrap();
    assert_eq!(points.len(), 8);
    assert_eq!(points[0][0], 1000, "segments carry timeline positions");
    assert_eq!(points[0][2], 0.0, "the start is silent");
    assert!(points[6][2].as_f64().unwrap() > 0.4, "the tone is loud");

    assert!(call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip_id, "position": 999})
    )
    .unwrap_err()
    .contains("not inside"));
    let split = call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip_id, "position": 13_000}),
    )
    .unwrap();
    let second = id(&split["second"], "clip");
    assert_eq!(split["first"]["length"], 12_000);
    assert_eq!(split["second"]["offset"], 12_000);

    let moved = call(
        &r,
        &mut s,
        "move_clip",
        json!({"clip": second, "start": 50_000, "track": b}),
    )
    .unwrap();
    assert_eq!(
        (moved["track"].as_u64(), moved["start"].as_u64()),
        (Some(b), Some(50_000))
    );
    let gained = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": second, "gain_db": -3.0, "name": "hit"}),
    )
    .unwrap();
    assert_eq!(gained["name"], "hit");

    call(&r, &mut s, "delete_clip", json!({"clip": clip_id})).unwrap();
    assert!(
        call(&r, &mut s, "get_clip_waveform", json!({"clip": clip_id}))
            .unwrap_err()
            .contains("no clip")
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn metronome_settings_and_never_in_exports() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("metronome");
    assert!(call(&r, &mut s, "set_metronome", json!({"volume_db": 12})).is_err());
    let t = call(&r, &mut s, "set_metronome", json!({"enabled": true})).unwrap();
    assert_eq!(
        t["metronome"],
        json!({"enabled": true, "volume_db": -6.0, "only_while_recording": false, "count_in_bars": 0})
    );
    assert!(call(&r, &mut s, "set_metronome", json!({"count_in_bars": 3})).is_err());
    let t = call(
        &r,
        &mut s,
        "set_metronome",
        json!({"volume_db": 0, "only_while_recording": true}),
    )
    .unwrap();
    assert_eq!(t["metronome"]["enabled"], true, "fields left out stay");
    call(
        &r,
        &mut s,
        "set_metronome",
        json!({"only_while_recording": false, "count_in_bars": 2}),
    )
    .unwrap();

    // Two seconds of an empty project with the click on at full level: silence.
    let out = dir.join("click.wav");
    call(
        &r,
        &mut s,
        "export_mixdown",
        json!({"path": out, "start": 0, "end": 96_000, "tail_seconds": 0}),
    )
    .unwrap();
    s.wait_for_export(Duration::from_secs(20)).unwrap();
    let audio = wav::read(&out).unwrap();
    assert_eq!(audio.len(), 96_000);
    assert!(audio.channels.iter().flatten().all(|v| *v == 0.0));

    let project = dir.join("click.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_transport", Value::Null).unwrap()["metronome"],
        json!({"enabled": true, "volume_db": 0.0, "only_while_recording": false, "count_in_bars": 2})
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn punch_range_defaults_to_the_loop_and_is_saved() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("punch");
    assert!(call(&r, &mut s, "set_punch", json!({"end": 10})).is_err());
    assert!(call(&r, &mut s, "set_punch", json!({"start": 20, "end": 10})).is_err());
    assert!(call(&r, &mut s, "set_punch", json!({})).is_err());

    // On with nothing set and a loop region: punch the loop.
    call(
        &r,
        &mut s,
        "set_loop",
        json!({"start": 48_000, "end": 96_000}),
    )
    .unwrap();
    let t = call(&r, &mut s, "set_punch", json!({"enabled": true})).unwrap();
    assert_eq!(
        t["punch"],
        json!({"start": 48_000, "end": 96_000, "enabled": true})
    );
    assert_eq!(t["loop"]["enabled"], false, "the loop itself stays off");
    let t = call(&r, &mut s, "set_punch", json!({"start": 1000, "end": 2000})).unwrap();
    assert_eq!(
        t["punch"]["enabled"], true,
        "changing the range keeps punch on"
    );

    // Recording headless fails before the punch check (no inputs), so the check of the
    // playhead against punch-out is covered by record::punch's unit test.
    let project = dir.join("punch.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_transport", Value::Null).unwrap()["punch"],
        json!({"start": 1000, "end": 2000, "enabled": true})
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn loop_region_is_set_toggled_and_saved() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("loop");
    assert!(call(&r, &mut s, "set_loop", json!({"start": 10})).is_err());
    assert!(call(&r, &mut s, "set_loop", json!({"start": 10, "end": 10})).is_err());
    assert!(call(&r, &mut s, "set_loop", json!({})).is_err());

    // On with nothing set: the 4 bars from the playhead's bar (2 s bars at 120 in 4/4).
    call(&r, &mut s, "locate", json!({"position": 100_000})).unwrap();
    let t = call(&r, &mut s, "set_loop", json!({"enabled": true})).unwrap();
    assert_eq!(
        t["loop"],
        json!({"start": 96_000, "end": 96_000 + 4 * 96_000, "enabled": true})
    );

    let t = call(
        &r,
        &mut s,
        "set_loop",
        json!({"start": 48_000, "end": 72_000}),
    )
    .unwrap();
    assert_eq!(
        t["loop"]["enabled"], true,
        "changing the range keeps looping on"
    );
    let t = call(&r, &mut s, "set_loop", json!({"enabled": false})).unwrap();
    assert_eq!(
        t["loop"],
        json!({"start": 48_000, "end": 72_000, "enabled": false})
    );
    // A range alone (as the ruler draws one, 20.4) leaves looping off too.
    let t = call(
        &r,
        &mut s,
        "set_loop",
        json!({"start": 24_000, "end": 72_000}),
    )
    .unwrap();
    assert_eq!(
        t["loop"],
        json!({"start": 24_000, "end": 72_000, "enabled": false}),
        "changing the range keeps looping off"
    );
    call(
        &r,
        &mut s,
        "set_loop",
        json!({"start": 48_000, "end": 72_000}),
    )
    .unwrap();
    call(&r, &mut s, "set_loop", json!({"enabled": true})).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_transport", Value::Null).unwrap()["loop"]["start"],
        48_000,
        "turning it back on keeps the range"
    );

    let project = dir.join("loop.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_transport", Value::Null).unwrap()["loop"]["enabled"],
        false
    );
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_transport", Value::Null).unwrap()["loop"],
        json!({"start": 48_000, "end": 72_000, "enabled": true})
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn project_files_tempo_and_export() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("files");
    let file = tone(&dir);
    let t = id(
        &call(
            &r,
            &mut s,
            "add_track",
            json!({"kind": "audio", "name": "Tone"}),
        )
        .unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": t, "path": file, "position": 0}),
    )
    .unwrap();

    assert!(call(&r, &mut s, "set_tempo", json!({"bpm": 400})).is_err());
    assert_eq!(
        call(&r, &mut s, "set_tempo", json!({"bpm": 90})).unwrap()["tempo"],
        90.0
    );
    assert!(call(
        &r,
        &mut s,
        "set_time_signature",
        json!({"numerator": 7, "denominator": 3})
    )
    .is_err());
    call(
        &r,
        &mut s,
        "set_time_signature",
        json!({"numerator": 7, "denominator": 8}),
    )
    .unwrap();

    assert!(call(&r, &mut s, "save_project", Value::Null)
        .unwrap_err()
        .contains("never been saved"));
    let project = dir.join("song.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"]
            .as_array()
            .unwrap()
            .len(),
        0
    );
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    let reopened = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(reopened["tracks"][0]["name"], "Tone");
    assert_eq!(reopened["tempo"], 90.0);

    assert_eq!(
        call(&r, &mut s, "get_export_status", Value::Null).unwrap()["state"],
        "idle"
    );
    let out = dir.join("mix.wav");
    call(
        &r,
        &mut s,
        "export_mixdown",
        json!({"path": out, "tail_seconds": 0}),
    )
    .unwrap();
    let deadline = Instant::now() + Duration::from_secs(20);
    let status = loop {
        s.update();
        let status = call(&r, &mut s, "get_export_status", Value::Null).unwrap();
        if status["state"] != "running" || Instant::now() > deadline {
            break status;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(status["state"], "finished", "{status}");
    assert_eq!(wav::read(&out).unwrap().len(), 48_000);

    let t = call(&r, &mut s, "locate", json!({"position": 96_000})).unwrap();
    assert_eq!(t["position"], 96_000);
    // Tempo counts quarter notes: 2 s at 90 is 3 quarter notes, 6 eighth-note beats, so in
    // 7/8 that is bar 1, beat 7.
    assert_eq!(t["bar_beat"], "1.7");
    assert_eq!(t["position_quarter_notes"], 3.0);
    assert_eq!(t["tempo"], 90.0);
    let _ = std::fs::remove_dir_all(dir);
}

/// The dev plan's 0.4 "done when": cut a clip at every transient and quantize the pieces,
/// using only actions, then check the *exported audio* has its attacks on the grid.
#[test]
fn cut_at_transients_and_quantize_lands_hits_on_the_grid() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("quantize");
    let file = dir.join("performance.wav");
    // Eight notes, each up to 40 ms off a 1/8 grid at 120 BPM (250 ms).
    let (signal, starts) = crate::analysis::tests::performance(48_000);
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![signal],
        },
    )
    .unwrap();
    call(&r, &mut s, "set_tempo", json!({"bpm": 120})).unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap(),
        "clip",
    );

    let found = call(&r, &mut s, "detect_transients", json!({"clip": clip})).unwrap();
    let positions: Vec<u64> = found["transients"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["position"].as_u64().unwrap())
        .collect();
    assert_eq!(positions.len(), starts.len(), "{positions:?}");

    let split = call(
        &r,
        &mut s,
        "split_clip_at",
        json!({"clip": clip, "positions": positions}),
    )
    .unwrap();
    let pieces = split["clips"].as_array().unwrap().clone();
    assert_eq!(
        pieces.len(),
        starts.len() + 1,
        "a piece before the first note, then one per note"
    );

    // The leading piece holds only silence; quantize the note pieces.
    let notes: Vec<Value> = pieces[1..].to_vec();
    let moved = call(
        &r,
        &mut s,
        "quantize_clips",
        json!({"clips": notes, "division": "1/8"}),
    )
    .unwrap();
    assert_eq!(moved.as_array().unwrap().len(), starts.len());

    let grid = call(
        &r,
        &mut s,
        "quantize_positions",
        json!({"positions": starts, "division": "1/8"}),
    )
    .unwrap();
    let expected: Vec<u64> = grid["positions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["quantized"].as_u64().unwrap())
        .collect();
    // An eighth note at 120 is 12000 samples: every grid line is a multiple of it.
    assert!(expected.iter().all(|p| p % 12_000 == 0), "{expected:?}");

    // Render and listen: the attacks in the mix must now sit on the grid.
    let out = dir.join("quantized.wav");
    call(
        &r,
        &mut s,
        "export_mixdown",
        json!({"path": out, "tail_seconds": 0}),
    )
    .unwrap();
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    let mix = wav::read(&out).unwrap();
    let mono = crate::analysis::mono(&mix.channels, 0, mix.len());
    let heard: Vec<usize> = crate::analysis::detect_onsets(
        &mono,
        crate::analysis::OnsetSettings::new(0.5, 50.0, 48_000),
    )
    .iter()
    .map(|o| o.position)
    .collect();
    assert_eq!(
        heard.len(),
        expected.len(),
        "heard {heard:?}, grid {expected:?}"
    );
    for (h, e) in heard.iter().zip(&expected) {
        assert!(
            h.abs_diff(*e as usize) <= 48,
            "attack at {h}, grid line at {e}"
        );
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn devices_stats_and_headless_ui() {
    let r = Registry::new();
    let mut s = headless();
    let audio = call(&r, &mut s, "get_audio_settings", Value::Null).unwrap();
    assert!(audio["running"].is_null());
    assert!(audio["error"].as_str().unwrap().contains("<none>"));
    let devices = call(&r, &mut s, "list_audio_devices", Value::Null).unwrap();
    assert!(!devices["hosts"].as_array().unwrap().is_empty());
    assert!(call(
        &r,
        &mut s,
        "list_audio_devices",
        json!({"host": "NotAHost"})
    )
    .unwrap_err()
    .contains("unknown host"));
    assert!(
        call(&r, &mut s, "set_audio_settings", json!({"threads": "lots"}))
            .unwrap_err()
            .contains("auto")
    );
    let stats = call(&r, &mut s, "get_stats", Value::Null).unwrap();
    // No device yet: no blocks, and a cadence that falls back to the block size.
    assert_eq!(stats["cadence"], stats["block_size"]);
    call(&r, &mut s, "get_midi_inputs", Value::Null).unwrap();
    for ui in ["get_meters", "get_view", "screenshot", "get_layout"] {
        let err = call(&r, &mut s, ui, Value::Null).unwrap_err();
        assert!(err.contains("app window"), "{ui}: {err}");
    }
    for ui in ["set_layout", "save_layout", "delete_layout"] {
        let err = call(&r, &mut s, ui, json!({"name": "Mixing"})).unwrap_err();
        assert!(err.contains("app window"), "{ui}: {err}");
    }
    // Help > About's facts: headless, the engine's version (the workspace's, as the app's);
    // the commit and folder are the app's to give.
    let info = call(&r, &mut s, "get_app_info", Value::Null).unwrap();
    assert_eq!(info["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(
        info["version"].as_str().unwrap().split('.').count(),
        3,
        "{info}"
    );
    assert!(
        info["commit"].is_null() && info["folder"].is_null(),
        "{info}"
    );
    // The recent projects are the app's settings: the app answers (daw-app's `recent` tests).
    let err = call(&r, &mut s, "list_recent_projects", Value::Null).unwrap_err();
    assert!(err.contains("app window"), "list_recent_projects: {err}");
    let err = call(
        &r,
        &mut s,
        "remove_recent_project",
        json!({"path": "D:\\Music\\Song\\Song.oxide"}),
    )
    .unwrap_err();
    assert!(err.contains("app window"), "remove_recent_project: {err}");
    assert!(call(&r, &mut s, "remove_recent_project", json!({}))
        .unwrap_err()
        .contains("path"));
    assert!(
        call(&r, &mut s, "list_plugins", json!({"filter": "zzz-no-such"}))
            .unwrap()
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn bad_input_is_explained() {
    let r = Registry::new();
    let mut s = headless();
    let err = call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": 999, "gain_db": 0}),
    )
    .unwrap_err();
    assert!(err.contains("999"), "{err}");
    let err = call(&r, &mut s, "add_track", json!({"kind": "piano"})).unwrap_err();
    assert!(err.contains("invalid arguments"), "{err}");
    let err = call(
        &r,
        &mut s,
        "add_track",
        json!({"kind": "audio", "colour": 1}),
    )
    .unwrap_err();
    assert!(err.contains("colour"), "unknown fields are rejected: {err}");
    let err = call(&r, &mut s, "no_such_action", Value::Null).unwrap_err();
    assert!(err.contains("unknown action"), "{err}");
    let err = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": 5, "parameter": "x", "value": 2}),
    )
    .unwrap_err();
    assert!(err.contains("0..1"), "{err}");
    let err = call(&r, &mut s, "list_plugin_parameters", json!({"insert": 5})).unwrap_err();
    assert!(err.contains("no insert"), "{err}");
}

/// Two seconds at 48 kHz of drum-like hits every 250 ms over a quiet tone: attacks and
/// sustain, so a stretch has something to smear and a split something to break.
fn hits(dir: &std::path::Path) -> PathBuf {
    let path = dir.join("hits.wav");
    let mut x = 0x2545_f491u32;
    let samples: Vec<f32> = (0..96_000)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = x as f32 / u32::MAX as f32 * 2.0 - 1.0;
            let since = (i % 12_000) as f32;
            let tone = 0.2 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin();
            tone + noise * 0.6 * (-since / 2_400.0).exp()
        })
        .collect();
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![samples],
        },
    )
    .unwrap();
    path
}

/// Export `0..end` and read the left channel back.
fn bounce(r: &Registry, s: &mut Session, out: PathBuf, end: u64) -> Result<Vec<f32>, String> {
    call(
        r,
        s,
        "export_mixdown",
        json!({"path": out, "start": 0, "end": end, "tail_seconds": 0}),
    )?;
    s.wait_for_export(Duration::from_secs(30)).unwrap();
    Ok(wav::read(&out).unwrap().channels[0].clone())
}

/// The same project with `file` as a plain clip at 1000: what a stretched clip must equal.
fn reference(file: &std::path::Path, dir: &std::path::Path, name: &str, end: u64) -> Vec<f32> {
    let r = Registry::new();
    let mut s = headless();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 1000}),
    )
    .unwrap();
    bounce(&r, &mut s, dir.join(name), end).unwrap()
}

#[test]
fn stretched_clips_play_their_render_seamlessly_and_export_waits_for_it() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("stretch");
    let file = hits(&dir);
    let source = wav::read(&file).unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 1000}),
        )
        .unwrap(),
        "clip",
    );

    // Twice as long: the length doubles, and the clip renders before it can be exported.
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 2.0, "transients": false}),
    )
    .unwrap();
    assert_eq!(
        (v["length"].as_u64(), v["stretch"].as_f64()),
        (Some(192_000), Some(2.0))
    );
    assert_eq!(v["rendering"], true);
    let end = 1000 + 192_000 + 1000;
    let refused = bounce(&r, &mut s, dir.join("early.wav"), end).unwrap_err();
    assert!(refused.contains("still rendering"), "{refused}");
    assert!(call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 5.0})
    )
    .is_err());
    assert!(call(&r, &mut s, "set_clip", json!({"clip": clip, "pitch": 30.0})).is_err());
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"][0]["rendering"],
        false
    );

    // The bounce is the render, bit for bit: the same as importing it as a plain clip.
    let stretched = bounce(&r, &mut s, dir.join("stretched.wav"), end).unwrap();
    let rendered = crate::stretch::render(&source.channels, 48_000, 2.0, 0.0).unwrap();
    let rendered_file = dir.join("rendered.wav");
    wav::write_f32(
        &rendered_file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: rendered,
        },
    )
    .unwrap();
    let expected = reference(&rendered_file, &dir, "reference.wav", end);
    assert!(stretched.iter().any(|v| v.abs() > 0.1));
    assert!(stretched == expected, "the export differs from the render");

    // Split anywhere, the two pieces read the one render: nothing changes, not even a sample.
    for at in [1000 + 50_001, 1000 + 123_457] {
        let pieces = call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"]
            .as_array()
            .unwrap()
            .len();
        let last = call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"]
            [pieces - 1]["id"]
            .as_u64()
            .unwrap();
        call(
            &r,
            &mut s,
            "split_clip",
            json!({"clip": last, "position": at}),
        )
        .unwrap();
    }
    let split = bounce(&r, &mut s, dir.join("split.wav"), end).unwrap();
    assert!(split == stretched, "a split changed the stretched audio");

    // Undo the splits; then another ratio and back: the 2x render comes back (made again,
    // since the cache let it go when nothing used it), and the same. (Set back rather than
    // undone: repeated set_clip calls on a clip merge into one undo step.)
    assert_eq!(
        call(&r, &mut s, "undo", Value::Null).unwrap()["done"],
        "Split clip"
    );
    call(&r, &mut s, "undo", Value::Null).unwrap();
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 3.0}),
    )
    .unwrap();
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 2.0}),
    )
    .unwrap();
    assert_eq!(
        (&v["length"], &v["rendering"]),
        (&json!(192_000), &json!(true))
    );
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let again = bounce(&r, &mut s, dir.join("again.wav"), end).unwrap();
    assert!(again == stretched, "the render made again differs");

    // Transposed only: the length stays, and it's the render too.
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 1.0, "pitch": 3.0}),
    )
    .unwrap();
    assert_eq!(v["length"], 96_000);
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let transposed = bounce(&r, &mut s, dir.join("transposed.wav"), 98_000).unwrap();
    let rendered = crate::stretch::render(&source.channels, 48_000, 1.0, 3.0).unwrap();
    wav::write_f32(
        &rendered_file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: rendered,
        },
    )
    .unwrap();
    assert!(transposed == reference(&rendered_file, &dir, "reference3.wav", 98_000));

    // Back to untouched: the file itself again.
    call(&r, &mut s, "set_clip", json!({"clip": clip, "pitch": 0.0})).unwrap();
    let plain = bounce(&r, &mut s, dir.join("plain.wav"), 98_000).unwrap();
    assert!(plain == reference(&file, &dir, "reference0.wav", 98_000));
    let _ = std::fs::remove_dir_all(dir);
}

/// The clip's start and length, from get_project.
fn placed(r: &Registry, s: &mut Session) -> (u64, u64) {
    let c = &call(r, s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"][0];
    (c["start"].as_u64().unwrap(), c["length"].as_u64().unwrap())
}

#[test]
fn clips_follow_the_tempo_map_in_one_render() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("follow");
    // Two seconds recorded at 120: four quarter notes. The project is at 120, 48 kHz, so a
    // quarter note is 24000 samples; the clip starts on quarter note 2.
    let file = hits(&dir);
    let source = wav::read(&file).unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 48_000}),
        )
        .unwrap(),
        "clip",
    );
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "source_tempo": 120, "transients": false}),
    )
    .unwrap();
    assert_eq!(
        (v["start"].as_u64(), v["length"].as_u64()),
        (Some(48_000), Some(96_000))
    );
    assert_eq!(v["rendering"], false, "at its own tempo it plays the file");
    assert!(call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 2.0})
    )
    .is_err());
    assert!(call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "source_tempo": 5})
    )
    .is_err());
    let plain = bounce(&r, &mut s, dir.join("plain.wav"), 150_000).unwrap();
    let reference_at = |file: &std::path::Path, at: u64, name: &str, end: u64| {
        let r = Registry::new();
        let mut other = headless();
        let track = id(
            &call(&r, &mut other, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        call(
            &r,
            &mut other,
            "import_audio",
            json!({"track": track, "path": file, "position": at}),
        )
        .unwrap();
        bounce(&r, &mut other, dir.join(name), end).unwrap()
    };
    assert!(plain == reference_at(&file, 48_000, "plain-ref.wav", 150_000));

    // 150: quarter note 2 is now at 38400 and the clip lasts four of them, 76800 samples.
    call(&r, &mut s, "set_tempo", json!({"bpm": 150})).unwrap();
    assert_eq!(placed(&r, &mut s), (38_400, 76_800));
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let faster = bounce(&r, &mut s, dir.join("faster.wav"), 120_000).unwrap();
    let rendered = crate::stretch::render(&source.channels, 48_000, 0.8, 0.0).unwrap();
    let rendered_file = dir.join("rendered.wav");
    let write = |channels: Vec<Vec<f32>>| {
        wav::write_f32(
            &rendered_file,
            &wav::Audio {
                sample_rate: 48_000,
                channels,
            },
        )
        .unwrap()
    };
    write(rendered);
    assert!(faster == reference_at(&rendered_file, 38_400, "faster-ref.wav", 120_000));

    // Undo: back to exactly where it was, in one step with the tempo.
    let undone = call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(undone["done"], "Set tempo");
    assert_eq!(placed(&r, &mut s), (48_000, 96_000));
    call(&r, &mut s, "redo", Value::Null).unwrap();
    assert_eq!(placed(&r, &mut s), (38_400, 76_800));

    // A change to 100 at bar 2 (quarter note 4), inside the clip: quarter notes 2-4 at 150,
    // 4-6 at 100 (28800 samples each).
    call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 2, "bpm": 100}),
    )
    .unwrap();
    assert_eq!(placed(&r, &mut s), (38_400, 38_400 + 57_600));
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let end = 38_400 + 96_000 + 1000;
    let mixed = bounce(&r, &mut s, dir.join("mixed.wav"), end).unwrap();
    let warp = [(48_000, 38_400), (48_000, 57_600)];
    write(crate::stretch::render_warped(&source.channels, 48_000, &warp, 0.0).unwrap());
    assert!(mixed == reference_at(&rendered_file, 38_400, "mixed-ref.wav", end));
    // Split right at the change and either side of it: one render, so nothing changes.
    for at in [76_800, 60_001, 100_003] {
        let pieces = call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"]
            .as_array()
            .unwrap()
            .clone();
        let holder = pieces
            .iter()
            .find(|c| {
                let start = c["start"].as_u64().unwrap();
                start < at && at < start + c["length"].as_u64().unwrap()
            })
            .unwrap()["id"]
            .clone();
        call(
            &r,
            &mut s,
            "split_clip",
            json!({"clip": holder, "position": at}),
        )
        .unwrap();
    }
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let split = bounce(&r, &mut s, dir.join("split.wav"), end).unwrap();
    assert!(split == mixed, "splitting a following clip changed it");

    // Moved, then the tempo changes: it keeps its new musical place.
    for _ in 0..3 {
        call(&r, &mut s, "undo", Value::Null).unwrap();
    }
    call(&r, &mut s, "undo", Value::Null).unwrap();
    call(&r, &mut s, "set_tempo", json!({"bpm": 120})).unwrap();
    assert_eq!(placed(&r, &mut s), (48_000, 96_000));
    call(
        &r,
        &mut s,
        "move_clip",
        json!({"clip": clip, "start": 72_000}),
    )
    .unwrap();
    call(&r, &mut s, "set_tempo", json!({"bpm": 60})).unwrap();
    assert_eq!(
        placed(&r, &mut s),
        (144_000, 192_000),
        "quarter note 3 at 60"
    );

    // Saved and opened again: still following, in the same place.
    let project = dir.join("follow.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": project})).unwrap();
    assert_eq!(placed(&r, &mut s), (144_000, 192_000));
    let clip = call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"][0]["id"]
        .as_u64()
        .unwrap();
    call(&r, &mut s, "set_tempo", json!({"bpm": 120})).unwrap();
    assert_eq!(
        placed(&r, &mut s),
        (72_000, 96_000),
        "still follows after reopening"
    );
    call(&r, &mut s, "set_tempo", json!({"bpm": 60})).unwrap();

    // Stops following: stays put at the ratio its start had (2x at 60).
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "source_tempo": 0}),
    )
    .unwrap();
    assert_eq!(
        (v["start"].as_u64(), v["length"].as_u64()),
        (Some(144_000), Some(192_000))
    );
    assert_eq!(
        (v["stretch"].as_f64(), v.get("source_tempo")),
        (Some(2.0), None)
    );
    call(&r, &mut s, "set_tempo", json!({"bpm": 120})).unwrap();
    assert_eq!(placed(&r, &mut s), (144_000, 192_000));
    let _ = std::fs::remove_dir_all(dir);
}

/// The clip editor's edge drag (`Session::restretch_clip`): from the clip as the drag began,
/// keeping the start (right edge) or the end (left edge), never for a clip following the tempo.
#[test]
fn edge_drags_stretch_from_the_gestures_start() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("edges");
    let file = hits(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 100_000}),
        )
        .unwrap(),
        "clip",
    ) as u32;
    let origin = s.project.find_clip(clip).unwrap().1.clone();
    // Right edge out to 1.5x: the start stays.
    for ratio in [1.1, 1.3, 1.5] {
        s.restretch_clip(clip, &origin, ratio, false).unwrap();
    }
    assert_eq!(placed(&r, &mut s), (100_000, 144_000));
    // Left edge in to 0.5x: the end stays.
    s.restretch_clip(clip, &origin, 0.5, true).unwrap();
    assert_eq!(placed(&r, &mut s), (100_000 + 48_000, 48_000));
    assert!(s.restretch_clip(clip, &origin, 5.0, false).is_err());
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "source_tempo": 120}),
    )
    .unwrap();
    let following = s.project.find_clip(clip).unwrap().1.clone();
    let refused = s.restretch_clip(clip, &following, 2.0, false).unwrap_err();
    assert!(
        format!("{refused}").contains("follows the tempo"),
        "{refused}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_linear_crossfade_over_a_split_sums_back_to_the_clip() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("crossfade");
    let file = hits(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 1000}),
        )
        .unwrap(),
        "clip",
    );
    let whole = bounce(&r, &mut s, dir.join("whole.wav"), 98_000).unwrap();
    let split = call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip, "position": 49_000}),
    )
    .unwrap();
    let second = split["second"]["clip"].as_u64().unwrap();

    // The pieces only touch: 4800 samples of crossfade extend each by 2400 into the other's
    // audio, and they fade over that.
    let x = call(
        &r,
        &mut s,
        "crossfade_clips",
        json!({"clip": second, "length": 4800, "shape": "linear"}),
    )
    .unwrap();
    assert_eq!(x["earlier"]["length"], 48_000 + 2400);
    assert_eq!(
        x["earlier"]["fade_out"],
        json!({"length": 4800, "shape": "linear"})
    );
    assert_eq!(
        (&x["later"]["start"], &x["later"]["offset"]),
        (&json!(46_600), &json!(45_600))
    );
    assert_eq!(x["later"]["fade_in"]["length"], 4800);
    let faded = bounce(&r, &mut s, dir.join("linear.wav"), 98_000).unwrap();
    let worst = whole
        .iter()
        .zip(&faded)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(
        worst < 1e-5,
        "a linear crossfade of one recording is off by {worst}"
    );

    // Equal power on the same audio rises in the middle: it doesn't sum back.
    call(
        &r,
        &mut s,
        "crossfade_clips",
        json!({"clip": second, "shape": "equal_power"}),
    )
    .unwrap();
    let power = bounce(&r, &mut s, dir.join("power.wav"), 98_000).unwrap();
    let rise = whole
        .iter()
        .zip(&power)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(rise > 0.01, "equal power should differ here, by {rise}");

    // A gap is refused; fades come off with set_clip, and the pieces still overlap.
    call(
        &r,
        &mut s,
        "move_clip",
        json!({"clip": second, "start": 60_000}),
    )
    .unwrap();
    let gap = call(&r, &mut s, "crossfade_clips", json!({"clip": second})).unwrap_err();
    assert!(gap.contains("before clip"), "{gap}");
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": second, "fade_in": 0, "fade_out": 1000, "fade_out_shape": "linear"}),
    )
    .unwrap();
    assert_eq!(
        (&v["fade_in"]["length"], &v["fade_out"]["length"]),
        (&json!(0), &json!(1000))
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Every note and controller id in the project (they must be unique).
fn midi_ids(s: &Session) -> Vec<u32> {
    s.project
        .tracks
        .iter()
        .flat_map(|t| &t.midi_clips)
        .flat_map(|c| {
            c.notes
                .iter()
                .map(|n| n.id)
                .chain(c.controllers.iter().map(|e| e.id))
                .chain([c.id])
        })
        .collect()
}

#[test]
fn clips_copy_paste_duplicate_and_move_as_a_group() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("clipboard");
    let file = hits(&dir);
    let track = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let (a, b, keys) = (
        track(&mut s, "audio"),
        track(&mut s, "audio"),
        track(&mut s, "instrument"),
    );
    let import = |s: &mut Session, track: u64, at: u64| {
        id(
            &call(
                &r,
                s,
                "import_audio",
                json!({"track": track, "path": file, "position": at}),
            )
            .unwrap(),
            "clip",
        )
    };
    let stretched = import(&mut s, a, 1000);
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": stretched, "stretch": 1.5, "fade_in": 500, "fade_out": 900}),
    )
    .unwrap();
    let following = import(&mut s, b, 48_000);
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": following, "source_tempo": 120}),
    )
    .unwrap();
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();

    // Copy both, paste at 300000: fresh ids, same distance apart, every field along.
    assert_eq!(
        call(
            &r,
            &mut s,
            "copy_clips",
            json!({"clips": [stretched, following]})
        )
        .unwrap(),
        json!({"clips": 2})
    );
    let pasted = call(&r, &mut s, "paste_clips", json!({"position": 300_000})).unwrap();
    let new: Vec<u32> = pasted["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_u64().unwrap() as u32)
        .collect();
    assert!(!new.contains(&(stretched as u32)) && !new.contains(&(following as u32)));
    let (original, copy) = (
        s.project.find_clip(stretched as u32).unwrap().1.clone(),
        s.project.find_clip(new[0]).unwrap().1.clone(),
    );
    assert_eq!(copy.start, 300_000);
    assert_eq!(
        (
            copy.length,
            copy.stretch,
            copy.fade_in,
            copy.fade_out,
            copy.offset()
        ),
        (
            original.length,
            original.stretch,
            original.fade_in,
            original.fade_out,
            original.offset()
        )
    );
    assert!(
        !s.clip_rendering(&copy),
        "the copy shares the original's render"
    );
    assert_eq!(
        s.project.find_clip(new[1]).unwrap().1.start,
        300_000 + 47_000
    );
    // The pasted following clip has a musical place of its own: it moves with the tempo.
    call(&r, &mut s, "set_tempo", json!({"bpm": 150})).unwrap();
    assert_eq!(s.project.find_clip(new[1]).unwrap().1.start, 277_600);
    call(&r, &mut s, "set_tempo", json!({"bpm": 120})).unwrap();

    // MIDI: pasted twice, every clip, note and event id stays unique.
    let midi = call(
        &r,
        &mut s,
        "add_midi_clip",
        json!({"track": keys, "bar": 1, "bars": 1}),
    )
    .unwrap();
    let midi = midi
        .get("clip")
        .or(midi.get("id"))
        .unwrap()
        .as_u64()
        .unwrap();
    call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": midi, "notes": [
            {"tick": 0, "length": 480, "pitch": 60},
            {"tick": 960, "length": 480, "pitch": 64}
        ]}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "add_controller_events",
        json!({"clip": midi, "events": [{"tick": 0, "kind": {"type": "cc", "number": 1, "value": 64}}]}),
    )
    .unwrap();
    call(&r, &mut s, "copy_clips", json!({"clips": [midi]})).unwrap();
    for at in [192_000, 384_000] {
        call(&r, &mut s, "paste_clips", json!({"position": at})).unwrap();
    }
    let ids = midi_ids(&s);
    let mut unique = ids.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!((ids.len(), unique.len()), (12, 12), "{ids:?}");
    let starts: Vec<u64> = s
        .project
        .track(keys as u32)
        .unwrap()
        .midi_clips
        .iter()
        .map(|c| c.start)
        .collect();
    assert_eq!(
        starts,
        vec![0, 7680, 15360],
        "192000 samples at 120 is two bars, 7680 ticks"
    );

    // Duplicate: right after the last one ends.
    let dup = call(&r, &mut s, "duplicate_clips", json!({"clips": [stretched]})).unwrap();
    let dup = dup["clips"][0].as_u64().unwrap() as u32;
    assert_eq!(s.project.find_clip(dup).unwrap().1.start, original.end());

    // Cut, paste, undo twice: the original is back under its own id.
    call(&r, &mut s, "cut_clips", json!({"clips": [stretched]})).unwrap();
    assert!(s.project.find_clip(stretched as u32).is_none());
    call(&r, &mut s, "paste_clips", json!({"position": 1000})).unwrap();
    call(&r, &mut s, "undo", Value::Null).unwrap();
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(s.project.find_clip(stretched as u32).unwrap().1, &original);

    // Moves: all or nothing.
    let folder = track(&mut s, "folder");
    let before = s.project.clone();
    let onto_folder = call(
        &r,
        &mut s,
        "move_clips",
        json!({"clips": [following, stretched], "delta": 0, "track_delta": 2}),
    )
    .unwrap_err();
    assert!(
        onto_folder.contains("folder or bus") || onto_folder.contains("below"),
        "{onto_folder}"
    );
    let early = call(
        &r,
        &mut s,
        "move_clips",
        json!({"clips": [following, stretched], "delta": -10_000}),
    )
    .unwrap_err();
    assert!(early.contains("before 0"), "{early}");
    assert_eq!(s.project, before, "a refused move changes nothing");
    call(
        &r,
        &mut s,
        "move_clips",
        json!({"clips": [stretched], "delta": 500, "track_delta": 1}),
    )
    .unwrap();
    assert_eq!(s.project.clip_track(stretched as u32), Some(b as u32));
    assert_eq!(s.project.find_clip(stretched as u32).unwrap().1.start, 1500);
    // A paste whose rows run onto the folder is refused too.
    call(
        &r,
        &mut s,
        "copy_clips",
        json!({"clips": [stretched, midi]}),
    )
    .unwrap();
    let refused = call(
        &r,
        &mut s,
        "paste_clips",
        json!({"position": 0, "track": keys}),
    )
    .unwrap_err();
    assert!(
        refused.contains(&format!("{folder}")) || refused.contains("below"),
        "{refused}"
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// 24.1: a pasted copy is the clip it copies. A trimmed, stretched, transposed clip is split
/// and its second piece (an offset into the take) given fades and gain, copied, and pasted on
/// another track at another position (not on a block boundary): each track exported alone,
/// the copy's span nulls bit for bit against the original's.
#[test]
fn a_pasted_clip_plays_exactly_as_the_one_it_copies() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("paste-null");
    let file = hits(&dir);
    let track = |s: &mut Session| {
        id(
            &call(&r, s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        )
    };
    let (a, b) = (track(&mut s), track(&mut s));
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": a, "path": file, "position": 1000}),
        )
        .unwrap(),
        "clip",
    );
    // 96000 samples at 1.25x: 1000..121000, trimmed to 4000..115000.
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 1.25, "pitch": 1.5}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "trim_start": 4000, "trim_end": 115_000}),
    )
    .unwrap();
    let split = call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip, "position": 40_123}),
    )
    .unwrap();
    let piece = split["second"]["clip"].as_u64().unwrap();
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": piece, "fade_in": 3000, "fade_out": 5000, "gain_db": -4.5,
               "fade_in_shape": "linear"}),
    )
    .unwrap();
    s.wait_for_stretches(Duration::from_secs(60)).unwrap();
    let original = s.project.find_clip(piece as u32).unwrap().1.clone();
    assert!(original.offset() > 0, "the piece starts inside its take");

    // Copied, and pasted on track b at the playhead's place, as Ctrl+V does.
    call(&r, &mut s, "copy_clips", json!({"clips": [piece]})).unwrap();
    let at = 200_777u64;
    let pasted = call(
        &r,
        &mut s,
        "paste_clips",
        json!({"position": at, "track": b}),
    )
    .unwrap();
    let copy_id = pasted["clips"][0].as_u64().unwrap() as u32;
    assert_eq!(s.project.clip_track(copy_id), Some(b as u32));
    let copy = s.project.find_clip(copy_id).unwrap().1.clone();
    assert_eq!(copy.start, at);
    // Everything but its id and place is the original's.
    let same = moved_clip(&original, copy.id, at);
    assert_eq!(
        copy, same,
        "the copy differs from the original beyond id and start"
    );
    s.wait_for_stretches(Duration::from_secs(60)).unwrap();

    // Each track alone, over both spans.
    let end = at + copy.length + 1000;
    let alone = |s: &mut Session, solo: u64, name: &str| {
        for t in [a, b] {
            call(
                &r,
                s,
                "set_track_mixer",
                json!({"track": t, "mute": t != solo}),
            )
            .unwrap();
        }
        bounce(&r, s, dir.join(name), end).unwrap()
    };
    let from_a = alone(&mut s, a, "original.wav");
    let from_b = alone(&mut s, b, "copy.wav");
    let span = |audio: &[f32], start: u64| {
        audio[start as usize..(start + original.length) as usize].to_vec()
    };
    let (heard, copied) = (span(&from_a, original.start), span(&from_b, at));
    assert!(
        heard.iter().any(|v| v.abs() > 0.05),
        "the original is silent"
    );
    assert!(
        heard == copied,
        "the copy's span differs from the original's (first at sample {:?})",
        heard.iter().zip(&copied).position(|(x, y)| x != y)
    );
    // And nothing else on b: silent before and after the copy.
    assert!(from_b[..at as usize].iter().all(|v| *v == 0.0));
    let _ = std::fs::remove_dir_all(dir);
}

/// `clip` as it would be with id `id` at `start`, nothing else changed.
fn moved_clip(clip: &crate::project::Clip, id: u32, start: u64) -> crate::project::Clip {
    let mut moved = clip.clone();
    moved.id = id;
    moved.start = start;
    moved
}

#[test]
fn markers_keep_their_bars_and_take_the_playhead_to_them() {
    let r = Registry::new();
    let mut s = headless();
    // 120 BPM at 48 kHz: a quarter note is 24000 samples and 960 ticks.
    let added = call(
        &r,
        &mut s,
        "add_marker",
        json!({"position": 48_000, "name": "Verse"}),
    )
    .unwrap();
    let verse = added["marker"].as_u64().unwrap();
    assert_eq!(added["markers"][0]["tick"], 1920);
    // Out of order: they come back in time order, numbered from 1.
    let chorus = call(
        &r,
        &mut s,
        "add_marker",
        json!({"bar": 5, "name": "Chorus"}),
    )
    .unwrap()["marker"]
        .as_u64()
        .unwrap();
    let intro = call(&r, &mut s, "add_marker", json!({"position": 0})).unwrap()["marker"]
        .as_u64()
        .unwrap();
    let list = call(&r, &mut s, "get_markers", Value::Null).unwrap()["markers"].clone();
    let order: Vec<u64> = list
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_u64().unwrap())
        .collect();
    assert_eq!(order, vec![intro, verse, chorus]);
    assert_eq!(
        (&list[0]["name"], &list[2]["position"], &list[2]["bar"]),
        (&json!("Marker 3"), &json!(384_000), &json!(5))
    );

    // At 60 the samples double; the ticks (and bars) stay.
    call(&r, &mut s, "set_tempo", json!({"bpm": 60})).unwrap();
    let list = call(&r, &mut s, "get_project", Value::Null).unwrap()["markers"].clone();
    assert_eq!(
        (&list[1]["tick"], &list[1]["position"]),
        (&json!(1920), &json!(96_000))
    );
    call(&r, &mut s, "undo", Value::Null).unwrap();

    // Going to them: by id, by number, and next / previous strictly from a marker.
    let went = call(&r, &mut s, "go_to_marker", json!({"marker": chorus})).unwrap();
    assert_eq!(went["position"], 384_000);
    assert_eq!(s.position(), 384_000);
    call(&r, &mut s, "go_to_marker", json!({"number": 2})).unwrap();
    assert_eq!(s.position(), 48_000);
    let next = call(&r, &mut s, "go_to_marker", json!({"direction": "next"})).unwrap();
    assert_eq!(next["marker"].as_u64(), Some(chorus));
    call(&r, &mut s, "go_to_marker", json!({"direction": "previous"})).unwrap();
    assert_eq!(s.position(), 48_000);
    call(&r, &mut s, "go_to_marker", json!({"direction": "previous"})).unwrap();
    assert_eq!(s.position(), 0);
    assert!(call(&r, &mut s, "go_to_marker", json!({"direction": "previous"})).is_err());
    assert!(call(&r, &mut s, "go_to_marker", json!({"number": 4})).is_err());
    assert!(call(
        &r,
        &mut s,
        "go_to_marker",
        json!({"number": 1, "marker": intro})
    )
    .is_err());

    // Moving one past another renumbers them; renaming keeps the place.
    call(
        &r,
        &mut s,
        "set_marker",
        json!({"marker": intro, "bar": 9, "name": "Outro"}),
    )
    .unwrap();
    assert_eq!(s.marker_number(3), Some(intro as u32));
    assert_eq!(s.marker_number(1), Some(verse as u32));
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Set marker");
    call(&r, &mut s, "remove_marker", json!({"marker": chorus})).unwrap();
    assert!(call(&r, &mut s, "remove_marker", json!({"marker": chorus})).is_err());
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(s.project.markers.len(), 3, "undo brings the marker back");
}

/// The largest jump between neighbouring samples outside the hits (they're steep anyway).
fn largest_jump(mix: &[f32], hits: &[usize]) -> f32 {
    (1..mix.len())
        .filter(|&i| !hits.iter().any(|&h| i >= h && i < h + 2400))
        .map(|i| (mix[i] - mix[i - 1]).abs())
        .fold(0.0, f32::max)
}

#[test]
fn quantized_slices_crossfade_instead_of_clicking() {
    let r = Registry::new();
    let dir = temp_dir("slice-fades");
    // A steady 220 Hz tone with hits off the 1/16 grid (6000 samples at 120), alternately
    // late and early, so quantizing moves neighbours apart: fill gaps then joins them mid-tone,
    // away from any hit, and a hard cut there jumps between two phases of the tone.
    let hits: Vec<usize> = (1..8)
        .map(|k| k * 12_000 + if k % 2 == 1 { 3500 } else { 2500 })
        .collect();
    let samples: Vec<f32> = (0..100_000)
        .map(|i| {
            let tone = 0.5 * (2.0 * std::f32::consts::PI * 220.0 * i as f32 / 48_000.0).sin();
            let burst = hits
                .iter()
                .filter(|&&h| i >= h && i < h + 2400)
                .map(|&h| 0.4 * (-((i - h) as f32) / 300.0).exp())
                .sum::<f32>();
            tone + burst
        })
        .collect();
    let file = dir.join("tone-hits.wav");
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![samples.clone()],
        },
    )
    .unwrap();
    let render = |strength: f64, crossfade_ms: f64, name: &str| {
        let mut s = headless();
        let track = id(
            &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let clip = id(
            &call(
                &r,
                &mut s,
                "import_audio",
                json!({"track": track, "path": file, "position": 0}),
            )
            .unwrap(),
            "clip",
        );
        let split = call(
            &r,
            &mut s,
            "split_clip_at",
            json!({"clip": clip, "positions": hits}),
        )
        .unwrap();
        // Every slice, the one before the first hit too (as the app's slicer does): only
        // quantized neighbours are joined.
        let pieces = split["clips"].as_array().unwrap().to_vec();
        call(
            &r,
            &mut s,
            "quantize_clips",
            json!({"clips": pieces, "division": "1/16", "strength": strength,
                   "crossfade_ms": crossfade_ms}),
        )
        .unwrap();
        bounce(&r, &mut s, dir.join(name), 100_000).unwrap()
    };
    // Not moved, the crossfaded slices sum back to the original exactly.
    let still = render(0.0, 5.0, "still.wav");
    let worst = samples
        .iter()
        .zip(&still)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(worst < 1e-5, "unmoved slices are off by {worst}");
    // Moved: hard cuts jump far beyond the tone's own slope; crossfades don't.
    let slope = 0.5 * 2.0 * std::f32::consts::PI * 220.0 / 48_000.0;
    // The hits are on the 1/16 grid now (6000 samples at 120): skip them there.
    let landed: Vec<usize> = hits.iter().map(|h| (h + 3000) / 6000 * 6000).collect();
    // Up to where the material ends (earlier if the last slice moved earlier).
    let end = (100_000 + landed[6] as i64 - hits[6] as i64).min(100_000) as usize;
    let hard = largest_jump(&render(1.0, 0.0, "hard.wav")[..end], &landed);
    let faded = largest_jump(&render(1.0, 5.0, "faded.wav")[..end], &landed);
    println!("tone slope {slope:.4}; hard cuts jump {hard:.4}, crossfaded {faded:.4}");
    assert!(hard > 5.0 * slope, "the hard cuts should click ({hard})");
    assert!(faded < 2.5 * slope, "crossfaded joins still jump {faded}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_clip_keeping_attacks_exports_its_protected_render() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("keep-attacks");
    let file = hits(&dir);
    let source = wav::read(&file).unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 1000}),
        )
        .unwrap(),
        "clip",
    );
    let v = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "stretch": 2.0}),
    )
    .unwrap();
    assert_eq!(v["transients"], true, "new clips keep their attacks");
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let end = 1000 + 192_000 + 1000;
    let kept = bounce(&r, &mut s, dir.join("kept.wav"), end).unwrap();

    // The same warp made by hand: the attacks the file has, kept 30 ms before to 60 after.
    let data = crate::media::AudioData::new(48_000, source.channels.clone());
    let ms = |v: f64| (v / 1000.0 * 48_000.0) as usize;
    let warp = crate::stretch::protect_transients(
        &[(96_000, 192_000)],
        data.onsets(),
        ms(crate::stretch::ATTACK_BEFORE_MS),
        ms(crate::stretch::ATTACK_AFTER_MS),
    );
    assert!(warp.len() > 3, "the hits are kept: {warp:?}");
    let rendered = crate::stretch::render_warped(&source.channels, 48_000, &warp, 0.0).unwrap();
    let rendered_file = dir.join("protected.wav");
    wav::write_f32(
        &rendered_file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: rendered,
        },
    )
    .unwrap();
    assert!(kept == reference(&rendered_file, &dir, "kept-ref.wav", end));

    // Split twice: one render still, nothing changes.
    for at in [1000 + 70_001, 1000 + 150_003] {
        let pieces = call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"]
            .as_array()
            .unwrap()
            .clone();
        let last = pieces.last().unwrap()["id"].clone();
        call(
            &r,
            &mut s,
            "split_clip",
            json!({"clip": last, "position": at}),
        )
        .unwrap();
    }
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    assert!(bounce(&r, &mut s, dir.join("split.wav"), end).unwrap() == kept);

    // Off: the plain stretch, as before.
    let ids: Vec<u64> = call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["id"].as_u64().unwrap())
        .collect();
    for clip in ids {
        call(
            &r,
            &mut s,
            "set_clip",
            json!({"clip": clip, "transients": false}),
        )
        .unwrap();
    }
    s.wait_for_stretches(Duration::from_secs(30)).unwrap();
    let plain = bounce(&r, &mut s, dir.join("plain.wav"), end).unwrap();
    let rendered = crate::stretch::render(&source.channels, 48_000, 2.0, 0.0).unwrap();
    wav::write_f32(
        &rendered_file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: rendered,
        },
    )
    .unwrap();
    assert!(plain == reference(&rendered_file, &dir, "plain-ref.wav", end));
    assert!(plain != kept);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn insert_chains_move_to_a_place_and_bypass_together() {
    let r = Registry::new();
    let mut s = headless();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    ) as u32;
    // Three slots whose plug-ins aren't loaded: moving and bypassing only touch the project.
    let ids: Vec<u32> = (0..3)
        .map(|k| {
            let insert = s.project.next_id();
            s.project
                .track_mut(track)
                .unwrap()
                .inserts
                .push(crate::project::InsertSlot {
                    id: insert,
                    name: format!("FX {k}"),
                    path: PathBuf::from(format!("FX {k}.vst3")),
                    bypass: k == 1,
                    state: None,
                });
            insert
        })
        .collect();
    let order = |s: &Session| -> Vec<u32> {
        s.project
            .track(track)
            .unwrap()
            .inserts
            .iter()
            .map(|i| i.id)
            .collect()
    };
    call(
        &r,
        &mut s,
        "move_insert",
        json!({"insert": ids[2], "position": 0}),
    )
    .unwrap();
    assert_eq!(order(&s), [ids[2], ids[0], ids[1]]);
    call(
        &r,
        &mut s,
        "move_insert",
        json!({"insert": ids[2], "position": 99}),
    )
    .unwrap();
    assert_eq!(order(&s), [ids[0], ids[1], ids[2]]);
    call(
        &r,
        &mut s,
        "move_insert",
        json!({"insert": ids[0], "direction": "down"}),
    )
    .unwrap();
    assert_eq!(order(&s), [ids[1], ids[0], ids[2]]);
    let both = call(
        &r,
        &mut s,
        "move_insert",
        json!({"insert": ids[0], "direction": "up", "position": 0}),
    )
    .unwrap_err();
    assert!(both.contains("either"), "{both}");

    // The rack's power button: one call, one undo step, however many inserts.
    let off = call(
        &r,
        &mut s,
        "set_inserts_bypassed",
        json!({"track": track, "bypass": true}),
    )
    .unwrap();
    assert_eq!(off["changed"], 2);
    let bypassed = |s: &Session| -> Vec<bool> {
        s.project
            .track(track)
            .unwrap()
            .inserts
            .iter()
            .map(|i| i.bypass)
            .collect()
    };
    assert_eq!(bypassed(&s), [true, true, true]);
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(bypassed(&s), [true, false, false], "undone in one step");
    let missing = call(
        &r,
        &mut s,
        "set_inserts_bypassed",
        json!({"track": 999, "bypass": true}),
    )
    .unwrap_err();
    assert!(missing.contains("999"), "{missing}");
}

#[test]
fn a_range_deletes_like_cuts_made_by_hand_and_undoes_in_one_step() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("range");
    let file = tone(&dir);
    let add = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        )
    };
    let (a, b) = (add(&mut s, "audio"), add(&mut s, "audio"));
    for track in [a, b] {
        call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap();
    }
    let clips_on = |s: &Session, t: u64| s.project.track(t as u32).unwrap().clips.len();

    // Split only: three pieces on track a, b untouched.
    let split = call(
        &r,
        &mut s,
        "split_range",
        json!({"start": 20_000, "end": 30_000, "tracks": [a]}),
    )
    .unwrap();
    assert_eq!(split["clips"].as_array().unwrap().len(), 2);
    assert_eq!((clips_on(&s, a), clips_on(&s, b)), (3, 1));
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(clips_on(&s, a), 1, "the split undoes in one step");

    // Delete: the middle goes from track a only.
    let deleted = call(
        &r,
        &mut s,
        "delete_range",
        json!({"start": 20_000, "end": 30_000, "tracks": [a]}),
    )
    .unwrap();
    assert_eq!(deleted["removed"], 1);
    assert_eq!((clips_on(&s, a), clips_on(&s, b)), (2, 1));
    let by_range = mixdown(&r, &mut s, &dir, "range.wav");
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(clips_on(&s, a), 1, "the delete undoes in one step");

    // The same by hand: split twice, delete the middle piece.
    let clip = s.project.track(a as u32).unwrap().clips[0].id;
    let second = call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip, "position": 20_000}),
    )
    .unwrap();
    let middle = id(&second["second"], "clip");
    call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": middle, "position": 30_000}),
    )
    .unwrap();
    call(&r, &mut s, "delete_clip", json!({"clip": middle})).unwrap();
    let by_hand = mixdown(&r, &mut s, &dir, "hand.wav");
    assert_eq!(
        by_range, by_hand,
        "the range export equals the cuts by hand"
    );

    // A MIDI clip across the range's edge is cut there too (18.5; it used to refuse).
    let keys = add(&mut s, "instrument");
    call(
        &r,
        &mut s,
        "add_midi_clip",
        json!({"track": keys, "bar": 1, "bars": 1}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "delete_range",
        json!({"start": 20_000, "end": 300_000}),
    )
    .unwrap();
    let midi = &s.project.track(keys as u32).unwrap().midi_clips;
    assert_eq!(midi.len(), 1, "the part before the range stays");
    assert_eq!(midi[0].end(), s.tick_at_sample(20_000));
    let bad = call(&r, &mut s, "delete_range", json!({"start": 5, "end": 5})).unwrap_err();
    assert!(bad.contains("after"), "{bad}");
}

/// Dev plan 18.5: MIDI clips split like audio ones, at a song position in samples turned into
/// the nearest tick through the tempo map: by `split_clip` (on a tempo change, in a clip that
/// starts mid-bar), by a time range's Split and Delete, one undo step each.
#[test]
fn midi_clips_split_at_a_song_position_through_the_tempo_map() {
    let r = Registry::new();
    let mut s = headless();
    let rate = u64::from(s.sample_rate());
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    // 120 BPM for bar 1 (two seconds), then 60 from bar 2 (tick 3840): a second a beat.
    call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 2, "bpm": 60.0}),
    )
    .unwrap();
    // From beat 3 of bar 1 (tick 1920), three bars long.
    let clip = id(
        &call(
            &r,
            &mut s,
            "add_midi_clip",
            json!({"track": keys, "bar": 1, "beat": 3, "bars": 3}),
        )
        .unwrap(),
        "clip",
    );
    call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [
            {"tick": 960, "length": 1920, "pitch": 60, "velocity": 80, "channel": 2},
            {"tick": 1920, "length": 480, "pitch": 64},
            {"tick": 5000, "length": 100, "pitch": 67},
        ]}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "add_controller_events",
        json!({"clip": clip, "events": [{"tick": 0, "kind": {"type": "cc", "number": 64, "value": 127}}]}),
    )
    .unwrap();
    let whole = call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap();
    assert_eq!(
        (whole["start"].as_u64(), whole["length"].as_u64()),
        (Some(1920), Some(11_520))
    );
    let split = |s: &mut Session, position: u64| {
        call(
            &r,
            s,
            "split_clip",
            json!({"clip": clip, "position": position}),
        )
    };

    // On the tempo change: tick 3840, 1920 into the clip, through the first note.
    let done = split(&mut s, 2 * rate).unwrap();
    let (first, second) = (&done["first"], &done["second"]);
    assert_eq!(id(first, "clip"), clip, "the first piece keeps the id");
    assert_eq!(first["length"], 1920);
    assert_eq!(first["notes"].as_array().unwrap().len(), 1);
    assert_eq!(first["notes"][0]["length"], 960, "ends at the cut");
    assert_eq!(
        (second["start"].as_u64(), second["start_sample"].as_u64()),
        (Some(3840), Some(2 * rate))
    );
    assert_eq!(second["length"], 11_520 - 1920);
    assert_eq!(second["name"], first["name"]);
    let notes: Vec<(u64, u64, u64, u64, u64)> = second["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| {
            let f = |k: &str| n[k].as_u64().unwrap();
            (
                f("tick"),
                f("length"),
                f("pitch"),
                f("velocity"),
                f("channel"),
            )
        })
        .collect();
    assert_eq!(
        notes,
        [
            (0, 960, 60, 80, 2),
            (0, 480, 64, 100, 0),
            (3080, 100, 67, 100, 0)
        ],
        "the rest of the crossing note, the note at the cut, the later one"
    );
    let pedal = &second["controllers"];
    assert_eq!(pedal.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            pedal[0]["tick"].as_u64(),
            pedal[0]["kind"]["value"].as_u64()
        ),
        (Some(0), Some(127)),
        "the sustain pedal still down at the cut"
    );
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Split clip");
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap(),
        whole,
        "one undo step brings the whole clip back"
    );
    assert_eq!(s.project.track(keys as u32).unwrap().midi_clips.len(), 1);

    // A sample between ticks goes to the nearest: after the change a tick is rate/960 samples.
    let tick = rate as f64 / 960.0;
    for (offset, want) in [(0.4, 3840), (0.6, 3841)] {
        let done = split(&mut s, 2 * rate + (offset * tick).round() as u64).unwrap();
        assert_eq!(done["second"]["start"], want, "{offset} of a tick");
        call(&r, &mut s, "undo", Value::Null).unwrap();
    }
    // At the clip's start or end: refused, nothing changed.
    let (start, end) = (
        whole["start_sample"].as_u64().unwrap(),
        whole["end_sample"].as_u64().unwrap(),
    );
    for at in [start, end, end + rate] {
        let refused = split(&mut s, at).unwrap_err();
        assert!(refused.contains("not inside"), "{refused}");
    }
    assert_eq!(
        call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap(),
        whole
    );

    // A time range from bar 2 beat 2 to bar 3: Split cuts at both edges, Delete takes the
    // middle and keeps both outside pieces where they were.
    let (from, to) = (3 * rate, 6 * rate); // ticks 4800 and 7680
    let pieces = call(
        &r,
        &mut s,
        "split_range",
        json!({"start": from, "end": to, "tracks": [keys]}),
    )
    .unwrap();
    assert_eq!(pieces["clips"].as_array().unwrap().len(), 2);
    let spans = |s: &Session| -> Vec<(u64, u64)> {
        s.project
            .track(keys as u32)
            .unwrap()
            .midi_clips
            .iter()
            .map(|c| (c.start, c.end()))
            .collect()
    };
    assert_eq!(spans(&s), [(1920, 4800), (4800, 7680), (7680, 13_440)]);
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(spans(&s), [(1920, 13_440)], "the split undoes in one step");
    let deleted = call(
        &r,
        &mut s,
        "delete_range",
        json!({"start": from, "end": to}),
    )
    .unwrap();
    assert_eq!(deleted["removed"], 1);
    assert_eq!(spans(&s), [(1920, 4800), (7680, 13_440)]);
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Delete range");
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(
        call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap(),
        whole,
        "the delete undoes in one step"
    );
}

#[test]
fn a_midi_track_goes_after_its_track_with_a_clip_over_the_range() {
    let r = Registry::new();
    let mut s = headless();
    let add = |s: &mut Session, kind: &str| {
        id(
            &call(&r, s, "add_track", json!({"kind": kind})).unwrap(),
            "track",
        ) as u32
    };
    let (first, last) = (add(&mut s, "audio"), add(&mut s, "audio"));
    let order = |s: &Session| -> Vec<u32> { s.project.tracks.iter().map(|t| t.id).collect() };

    // No end: two bars from the bar the start is in (4/4 at 120 BPM: 3,840 ticks a bar).
    let rate = f64::from(s.sample_rate());
    let start = (rate * 0.5) as u64; // half a second: one beat in
    let inserted = call(
        &r,
        &mut s,
        "insert_midi_track",
        json!({"start": start, "after": first}),
    )
    .unwrap();
    let track = id(&inserted, "track") as u32;
    assert_eq!(
        order(&s),
        [first, track, last],
        "right after the clicked track"
    );
    let t = s.project.track(track).unwrap();
    assert_eq!(t.kind, crate::project::TrackKind::Instrument);
    assert!(t.arm, "armed, so the keyboard plays it");
    assert_eq!(inserted["insert"], Value::Null);
    assert_eq!(inserted["clip"]["start"], 960);
    assert_eq!(inserted["clip"]["length"], 2 * 3840);

    // With an end: the clip spans the range; and the whole insert undoes in one step.
    let ranged = call(
        &r,
        &mut s,
        "insert_midi_track",
        json!({"start": 0, "end": (rate * 4.0) as u64}),
    )
    .unwrap();
    assert_eq!(ranged["clip"]["start"], 0);
    assert_eq!(
        ranged["clip"]["length"],
        2 * 3840,
        "four seconds at 120 BPM: two bars"
    );
    assert_eq!(*order(&s).last().unwrap(), id(&ranged, "track") as u32);
    call(&r, &mut s, "undo", Value::Null).unwrap();
    assert_eq!(order(&s), [first, track, last], "undone in one step");

    // Refusals change nothing.
    let before = call(&r, &mut s, "get_project", Value::Null).unwrap();
    for (args, says) in [
        (json!({"start": 100, "end": 100}), "after"),
        (json!({"start": 0, "after": 999}), "999"),
        (
            json!({"start": 0, "instrument": "zzz-no-such-plugin"}),
            "zzz",
        ),
    ] {
        let err = call(&r, &mut s, "insert_midi_track", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    assert_eq!(
        call(&r, &mut s, "get_project", Value::Null).unwrap(),
        before
    );
}

#[test]
fn audio_files_open_once_play_on_their_own_and_stay_out_of_exports() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("editor");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap(),
        "clip",
    );
    // Muted, so the project itself exports silence: anything heard would be the preview.
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": track, "mute": true}),
    )
    .unwrap();
    let history =
        |s: &mut Session| call(&r, s, "get_history", Value::Null).unwrap()["undo"].clone();
    let before = history(&mut s);

    let by_path = call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap();
    let by_clip = call(&r, &mut s, "open_audio_editor", json!({"clip": clip})).unwrap();
    assert_eq!(
        by_path["document"], by_clip["document"],
        "one document per file"
    );
    assert_eq!(by_path["sample_rate"], 48_000);
    assert_eq!(by_path["channels"], 1);
    assert_eq!(by_path["length"], 48_000);
    assert_eq!(history(&mut s), before, "opening a file isn't an undo step");
    let doc = id(&by_path, "document");
    let state = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    assert_eq!(state["documents"].as_array().unwrap().len(), 1);

    // A preview playing while the project exports: the export doesn't contain it.
    call(
        &r,
        &mut s,
        "play_audio_editor",
        json!({"document": doc, "start": 12_000}),
    )
    .unwrap();
    let exported = mixdown(&r, &mut s, &dir, "editor.wav");
    assert!(
        exported.iter().all(|v| *v == 0.0),
        "no preview in an export"
    );
    call(
        &r,
        &mut s,
        "play_audio_editor",
        json!({"document": doc, "play": false}),
    )
    .unwrap();

    for (name, args, says) in [
        (
            "open_audio_editor",
            json!({"path": file, "clip": clip}),
            "either",
        ),
        ("open_audio_editor", json!({}), "either"),
        (
            "open_audio_editor",
            json!({"path": dir.join("missing.wav")}),
            "missing",
        ),
        ("play_audio_editor", json!({"document": 99}), "99"),
        (
            "play_audio_editor",
            json!({"document": doc, "start": 48_000}),
            "nothing to play",
        ),
        ("close_audio_editor", json!({"document": 99}), "99"),
    ] {
        let err = call(&r, &mut s, name, args).unwrap_err();
        assert!(err.contains(says), "{name}: {err}");
    }
    call(&r, &mut s, "close_audio_editor", json!({"document": doc})).unwrap();
    let state = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    assert!(state["documents"].as_array().unwrap().is_empty());
}

#[test]
fn an_edited_file_saves_in_its_format_and_the_project_plays_it() {
    use crate::wav::{self, Format};
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("save-audio");
    // A 16-bit file of our own: the check never edits anyone's recording.
    let file = dir.join("take.wav");
    let tone = wav::read(&tone(&dir)).unwrap();
    wav::write_with_format(
        &file,
        &tone,
        Format {
            bits: 16,
            float: false,
        },
    )
    .unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap(),
        "clip",
    );
    let before = mixdown(&r, &mut s, &dir, "before.wav");
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"clip": clip})).unwrap(),
        "document",
    );

    // An edit changes the document, not the file or the project.
    let edited = call(
        &r,
        &mut s,
        "edit_audio",
        json!({"document": doc, "op": "delete", "start": 12_000, "end": 24_000}),
    )
    .unwrap();
    assert_eq!(edited["dirty"], true);
    assert_eq!(edited["length"], 36_000);
    assert_eq!(edited["history"]["steps"], json!(["Open", "Delete"]));
    assert_eq!(edited["clips"], json!([clip]));
    assert_eq!(edited["format"], json!({"bits": 16, "float": false}));
    assert_eq!(
        mixdown(&r, &mut s, &dir, "unsaved.wav"),
        before,
        "not saved yet"
    );
    assert!(
        call(&r, &mut s, "close_audio_editor", json!({"document": doc}))
            .unwrap_err()
            .contains("unsaved")
    );

    // Save: still 16-bit, the new length, and the clip plays it (silence after its end). It
    // writes the project's working copy (Imported/), never the file imported from.
    let saved = call(&r, &mut s, "save_audio", json!({"document": doc})).unwrap();
    assert_eq!(saved["dirty"], false);
    let working = s.resolve(s.project.find_clip(clip as u32).unwrap().1.file());
    assert!(working.starts_with(s.project_dir().unwrap().join("Imported")));
    assert_eq!(
        wav::read(&file).unwrap().channels[0].len(),
        48_000,
        "the original"
    );
    assert_eq!(
        wav::read_format(&working),
        Some(Format {
            bits: 16,
            float: false
        })
    );
    let on_disk = wav::read(&working).unwrap().channels.remove(0);
    assert_eq!(on_disk.len(), 36_000);
    let mut expected = on_disk.clone();
    expected.resize(48_000, 0.0);
    assert_eq!(
        mixdown(&r, &mut s, &dir, "after.wav"),
        expected,
        "the project plays the save"
    );

    // Undo and save again: the file is back.
    let undone = call(&r, &mut s, "undo_audio_edit", json!({"document": doc})).unwrap();
    assert_eq!(undone["done"], "Delete");
    assert_eq!(undone["document"]["dirty"], true);
    call(&r, &mut s, "save_audio", json!({"document": doc})).unwrap();
    assert_eq!(wav::read(&file).unwrap().len(), 48_000);
    assert_eq!(mixdown(&r, &mut s, &dir, "restored.wav"), before);

    // Copy and paste; a failed Save As leaves the file untouched.
    call(
        &r,
        &mut s,
        "copy_audio",
        json!({"document": doc, "start": 0, "end": 1_000}),
    )
    .unwrap();
    let pasted = call(
        &r,
        &mut s,
        "edit_audio",
        json!({"document": doc, "op": "paste", "at": 0}),
    )
    .unwrap();
    assert_eq!(pasted["length"], 49_000);
    let bytes = std::fs::read(&file).unwrap();
    let missing = dir.join("no such folder").join("copy.wav");
    assert!(call(
        &r,
        &mut s,
        "save_audio_as",
        json!({"document": doc, "path": missing})
    )
    .is_err());
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "the original is untouched"
    );
    let copy = dir.join("copy.wav");
    let as_copy = call(
        &r,
        &mut s,
        "save_audio_as",
        json!({"document": doc, "path": copy}),
    )
    .unwrap();
    assert_eq!(as_copy["dirty"], false);
    assert_eq!(wav::read(&copy).unwrap().len(), 49_000);
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes,
        "Save As leaves the original alone"
    );

    // Refusals.
    for (args, says) in [
        (
            json!({"document": doc, "op": "delete"}),
            "needs `start` and `end`",
        ),
        (
            json!({"document": doc, "op": "delete", "start": 5, "end": 9, "channels": [0]}),
            "every channel",
        ),
        (json!({"document": doc, "op": "amplify"}), "`db`"),
        (
            json!({"document": doc, "op": "amplify", "db": 3, "start": 1}),
            "both",
        ),
        (json!({"document": 99, "op": "invert"}), "99"),
    ] {
        let err = call(&r, &mut s, "edit_audio", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    call(
        &r,
        &mut s,
        "close_audio_editor",
        json!({"document": doc, "discard": true}),
    )
    .unwrap();
}

#[test]
fn a_files_spectrum_peaks_at_its_tone() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("spectrum");
    let file = tone(&dir);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let spectrum = call(
        &r,
        &mut s,
        "get_spectrum",
        json!({"document": doc, "start": 12_000, "bands": 48}),
    )
    .unwrap();
    let bands = spectrum["bands"].as_array().unwrap();
    assert_eq!(bands.len(), 48);
    let loudest = bands
        .iter()
        .max_by(|a, b| {
            a["db"]
                .as_f64()
                .unwrap()
                .total_cmp(&b["db"].as_f64().unwrap())
        })
        .unwrap();
    let (lo, hi) = (
        loudest["low_hz"].as_f64().unwrap(),
        loudest["high_hz"].as_f64().unwrap(),
    );
    // 440 Hz falls between two FFT bins (433.6 and 445.3 Hz), so the loudest band is the
    // one holding 440 or its neighbour across a band edge within 3%.
    assert!(
        lo <= 440.0 * 1.03 && 440.0 / 1.03 < hi,
        "loudest band {lo:.0}-{hi:.0} Hz"
    );
    // A 0.5 sine off a bin centre: near -6 dB, a little less for the window's scalloping.
    let level = loudest["db"].as_f64().unwrap();
    assert!((-8.0..=-5.5).contains(&level), "{level} dB");
    for (args, says) in [
        (json!({"document": doc, "channel": 1}), "no channel 1"),
        (json!({"document": doc, "fft": 1000}), "power of two"),
        (
            json!({"document": doc, "start": 5, "end": 5}),
            "nothing to measure",
        ),
        (json!({"document": 42}), "42"),
    ] {
        let err = call(&r, &mut s, "get_spectrum", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
}

#[test]
fn the_effects_rack_is_listed_and_apply_refuses_what_it_cannot_do() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("rack");
    let file = tone(&dir);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let state = call(&r, &mut s, "get_audio_editor", json!({})).unwrap();
    assert_eq!(state["rack"], json!([]));
    assert_eq!(state["applying"], Value::Null);
    for (args, says) in [
        (json!({"document": doc}), "no plug-ins switched on"),
        (json!({"document": doc, "start": 5}), "both"),
    ] {
        let err = call(&r, &mut s, "apply_audio_effects", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    for (name, args, says) in [
        (
            "add_insert",
            json!({"editor": true, "track": track, "plugin": "x"}),
            "not both",
        ),
        (
            "add_insert",
            json!({"editor": true, "plugin": "No Such Plug-in"}),
            "No Such Plug-in",
        ),
        (
            "set_inserts_bypassed",
            json!({"editor": true, "track": track, "bypass": true}),
            "not both",
        ),
    ] {
        let err = call(&r, &mut s, name, args).unwrap_err();
        assert!(err.contains(says), "{name}: {err}");
    }
    // Nothing reached the project or its undo history.
    assert!(s.editor_rack().is_empty());
    assert!(s.project.master.inserts.is_empty());
}

#[test]
fn a_spectral_box_is_attenuated_removed_and_healed_as_asked() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("spectral-edit");
    let file = tone(&dir);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let level = |r: &Registry, s: &mut Session| -> f64 {
        let spectrum = call(
            r,
            s,
            "get_spectrum",
            json!({"document": doc, "start": 26_000, "end": 38_000, "bands": 48}),
        )
        .unwrap();
        spectrum["bands"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["db"].as_f64().unwrap())
            .fold(f64::NEG_INFINITY, f64::max)
    };
    let before = level(&r, &mut s);
    // A box over the 440 Hz tone, turned down 60 dB: its loudest band falls by over 40 dB.
    let boxed =
        json!({"document": doc, "start": 20_000, "end": 44_000, "low_hz": 300.0, "high_hz": 600.0});
    let mut args = boxed.clone();
    args["op"] = json!("spectral_attenuate");
    args["db"] = json!(-60.0);
    let edited = call(&r, &mut s, "edit_audio", args).unwrap();
    assert_eq!(edited["length"], 48_000);
    let after = level(&r, &mut s);
    assert!(before - after > 40.0, "{before:.1} dB -> {after:.1} dB");
    // A band above the tone leaves it as it was.
    call(&r, &mut s, "undo_audio_edit", json!({"document": doc})).unwrap();
    let mut args = boxed.clone();
    args["op"] = json!("spectral_remove");
    args["low_hz"] = json!(2000.0);
    args["high_hz"] = json!(6000.0);
    call(&r, &mut s, "edit_audio", args).unwrap();
    let kept = level(&r, &mut s);
    assert!(
        (kept - before).abs() < 0.1,
        "{before:.2} dB -> {kept:.2} dB"
    );
    let mut args = boxed.clone();
    args["op"] = json!("spectral_heal");
    let healed = call(&r, &mut s, "edit_audio", args).unwrap();
    assert_eq!(
        healed["history"]["steps"],
        json!(["Open", "Spectral remove", "Spectral heal"])
    );
    for (args, says) in [
        (
            json!({"document": doc, "op": "spectral_remove", "start": 0, "end": 100}),
            "low_hz",
        ),
        (
            json!({"document": doc, "op": "spectral_remove", "low_hz": 100.0, "high_hz": 200.0}),
            "needs `start` and `end`",
        ),
        (
            json!({"document": doc, "op": "spectral_attenuate", "start": 0, "end": 100, "low_hz": 100.0, "high_hz": 200.0}),
            "needs `db`",
        ),
        (
            json!({"document": doc, "op": "spectral_heal", "start": 0, "end": 100, "low_hz": 900.0, "high_hz": 200.0}),
            "must go up",
        ),
        (
            json!({"document": doc, "op": "spectral_heal", "start": 0, "end": 100, "low_hz": 100.0, "high_hz": 30_000.0}),
            "30000 Hz",
        ),
        (
            json!({"document": doc, "op": "spectral_remove", "start": 0, "end": 100, "low_hz": 100.0, "high_hz": 200.0, "channels": [1]}),
            "no channel 1",
        ),
    ] {
        let err = call(&r, &mut s, "edit_audio", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    }
    call(
        &r,
        &mut s,
        "close_audio_editor",
        json!({"document": doc, "discard": true}),
    )
    .unwrap();
}

#[test]
fn a_native_effect_is_an_insert_like_a_plugin() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("native");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let listed = call(&r, &mut s, "list_plugins", json!({"filter": "built in"})).unwrap();
    assert!(
        listed
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p["name"] == "Amplify" && p["path"] == "native:amplify"),
        "{listed}"
    );
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "amplify"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Amplify"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 3);
    assert_eq!(params["parameters"][0]["display"], "0.00 dB");
    // +6 dB on the left: the normalized value of 6 on its -96..48 scale.
    let six = (6.0 + 96.0) / 144.0;
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "left gain", "value": six}),
    )
    .unwrap();
    assert_eq!(set["display"], "+6.00 dB");
    let wet = mixdown(&r, &mut s, &dir, "wet.wav");
    let gain = 10f32.powf(6.0 / 20.0);
    for (w, d) in wet.iter().zip(&dry).skip(1000).step_by(997) {
        assert!((w - d * gain).abs() < 1e-6, "{w} vs {d} x {gain}");
    }
    // Its setting is saved with the project and comes back on reopening.
    let project = dir.join("native.dawproj");
    call(&r, &mut s, "save_project", json!({"path": project})).unwrap();
    let mut t = headless();
    call(&r, &mut t, "open_project", json!({"path": project})).unwrap();
    let reopened = call(
        &r,
        &mut t,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(reopened["parameters"][0]["display"], "+6.00 dB");
    assert_eq!(mixdown(&r, &mut t, &dir, "reopened.wav"), wet);
    // A lane on Left Gain from 0 dB at the start down to -96 dB at bar 2 (tick 1920, sample
    // 48000 at 120 bpm): linear in its -96..48 dB scale, so -36 dB at 18000 and -72 at 36000.
    call(
        &r,
        &mut t,
        "add_automation_lane",
        json!({"track": track, "target": {"type": "parameter", "insert": insert, "parameter": "Left Gain"},
               "points": [{"tick": 0, "value": 96.0 / 144.0}, {"tick": 1920, "value": 0.0}]}),
    )
    .unwrap();
    let ramp = mixdown(&r, &mut t, &dir, "ramp.wav");
    let rms = |v: &[f32], at: usize| -> f64 {
        (v[at - 400..at + 400]
            .iter()
            .map(|x| f64::from(*x).powi(2))
            .sum::<f64>()
            / 800.0)
            .sqrt()
    };
    for (at, db) in [(18_000, -36.0), (36_000, -72.0)] {
        let measured = 20.0 * (rms(&ramp, at) / rms(&dry, at)).log10();
        assert!(
            (measured - db).abs() < 1.0,
            "{measured:.2} dB at {at}, not {db}"
        );
    }
    // Undo takes it out and puts it back.
    let removed = call(
        &r,
        &mut t,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    call(&r, &mut t, "remove_insert", json!({"insert": insert})).unwrap();
    assert!(call(
        &r,
        &mut t,
        "list_plugin_parameters",
        json!({"insert": insert})
    )
    .is_err());
    call(&r, &mut t, "undo", Value::Null).unwrap();
    let back = call(
        &r,
        &mut t,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    // Back with its knob where the lane left it, as a plug-in's settings come back.
    assert_eq!(
        back["parameters"][0]["display"],
        removed["parameters"][0]["display"]
    );
}

#[test]
fn native_effects_export_bit_for_bit_as_playback() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("native-playback");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let amp = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Amplify"}),
    )
    .unwrap()["insert"]
        .clone();
    call(&r, &mut s, "add_insert", json!({"plugin": "Channel Mixer"})).unwrap();
    // Two equalisers: the Parametric on the track, the FIR Graphic (7193 samples of latency,
    // so the graph compensates it) on the master.
    let peq = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Parametric EQ"}),
    )
    .unwrap()["insert"]
        .clone();
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": peq, "parameter": "Band 3 Gain", "value": 0.6}),
    )
    .unwrap();
    let geq = call(
        &r,
        &mut s,
        "add_insert",
        json!({"plugin": "Graphic EQ (10 bands)"}),
    )
    .unwrap();
    assert_eq!(geq["latency"], 7193);
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": geq["insert"], "parameter": "500 Hz", "value": 0.55}),
    )
    .unwrap();
    // Dynamics with look-ahead: the Compressor and the Limiter (20 ms of delay) on the
    // track, pushed into reduction.
    let comp = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Compressor"}),
    )
    .unwrap()["insert"]
        .clone();
    for (parameter, value) in [("Threshold", 0.6), ("Ratio", 0.2)] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": comp, "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    let limiter = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Limiter"}),
    )
    .unwrap();
    assert_eq!(limiter["latency"], 960);
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": limiter["insert"], "parameter": "Input Boost", "value": 0.8}),
    )
    .unwrap();
    // The Multiband Compressor (693 samples) on the master, its bands working.
    let multi = call(
        &r,
        &mut s,
        "add_insert",
        json!({"plugin": "Multiband Compressor"}),
    )
    .unwrap();
    assert_eq!(multi["latency"], 693);
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": multi["insert"], "parameter": "Band 2 Threshold", "value": 0.5}),
    )
    .unwrap();
    // The noise reducers (4096 samples each for Noise and Hiss Reduction, 512 for Adaptive):
    // Noise Reduction on the track with a print captured from a noise file in the Waveform
    // view, Adaptive on the track, Hiss Reduction on the master.
    let noise = noisy_tone_file(&dir, 48_000, 48_000);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": noise})).unwrap(),
        "document",
    );
    let nr = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Noise Reduction"}),
    )
    .unwrap();
    assert_eq!(nr["latency"], 4096);
    call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 30_000, "insert": nr["insert"]}),
    )
    .unwrap();
    let adaptive = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Auto Noise Reduction"}),
    )
    .unwrap();
    assert_eq!(adaptive["latency"], 512);
    call(
        &r,
        &mut s,
        "add_insert",
        json!({"plugin": "Hiss Reduction"}),
    )
    .unwrap();
    // The Hum Remover on the track, its fundamental on the 440 Hz tone (a -60 dB notch there).
    let hum = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Hum Remover"}),
    )
    .unwrap();
    assert_eq!(hum["latency"], 0);
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": hum["insert"], "parameter": "Frequency", "value": 420.0 / 19_980.0}),
    )
    .unwrap();
    // The Click Remover on the track (4096 samples), its Threshold lowered.
    let clicks = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Click Remover"}),
    )
    .unwrap();
    assert_eq!(clicks["latency"], 4096);
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": clicks["insert"], "parameter": "Threshold", "value": 0.1}),
    )
    .unwrap();
    // The Click & Pop Repair on the track (22016 samples), its levels scanned from the noise
    // file and its curve lowered, so it repairs as it goes.
    let pops = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Click & Pop Repair"}),
    )
    .unwrap();
    assert_eq!(pops["latency"], 22_016);
    call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000, "insert": pops["insert"],
               "scan": "threshold levels"}),
    )
    .unwrap();
    for parameter in ["Max Threshold", "Avg Threshold", "Min Threshold"] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": pops["insert"], "parameter": parameter, "value": 0.0}),
        )
        .unwrap();
    }
    // Clip Repair on a second track playing a clipped sine, its levels scanned from the
    // file and its Gain on a lane (a line in dB, the same at any block size).
    let clipped = clipped_sine_file(&dir);
    let second = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": second, "path": clipped, "position": 0}),
    )
    .unwrap();
    let declipper = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": second, "plugin": "Clip Repair"}),
    )
    .unwrap();
    assert_eq!(declipper["latency"], 1480);
    let clipped_doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": clipped})).unwrap(),
        "document",
    );
    call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": clipped_doc, "start": 0, "end": 48_000,
               "insert": declipper["insert"]}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": second,
               "target": {"type": "parameter", "insert": declipper["insert"], "parameter": "Gain"},
               "points": [{"tick": 0, "value": 0.3}, {"tick": 1920, "value": 0.9}]}),
    )
    .unwrap();
    // The Room Reverb on the master (latency 0), its Decay on a lane (it only sets the
    // gains) and its Width set by hand.
    let verb = call(&r, &mut s, "add_insert", json!({"plugin": "Room Reverb"})).unwrap();
    assert_eq!(verb["latency"], 0);
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": verb["insert"], "parameter": "Width", "value": 0.8}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "parameter", "insert": verb["insert"], "parameter": "Decay"},
               "points": [{"tick": 0, "value": 0.1}, {"tick": 1920, "value": 0.6}]}),
    )
    .unwrap();
    // The Convolution Reverb on the master (256 samples), playing a stereo room impulse chosen
    // by path (prepared off the audio thread; the export waits for it), its Mix on a lane and
    // its Room Size set by hand.
    let conv = call(
        &r,
        &mut s,
        "add_insert",
        json!({"plugin": "Convolution Reverb"}),
    )
    .unwrap();
    assert_eq!(conv["latency"], 256);
    let ir = dir.join("room-ir.wav");
    let decay = |seed: u32| -> Vec<f32> {
        (0..24_000u32)
            .map(|i| {
                let x = ((i.wrapping_mul(2_654_435_761) ^ seed) % 2001) as f32 / 1000.0 - 1.0;
                0.4 * x * (-(i as f32) / 4000.0).exp()
            })
            .collect()
    };
    wav::write_f32(
        &ir,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![decay(7), decay(11)],
        },
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_impulse_file",
        json!({"insert": conv["insert"], "path": ir}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": conv["insert"], "parameter": "Room Size", "value": 0.7}),
    )
    .unwrap();
    let shown = call(
        &r,
        &mut s,
        "get_impulse_file",
        json!({"insert": conv["insert"]}),
    )
    .unwrap();
    assert_eq!(
        (&shown["error"], &shown["file_channels"]),
        (&Value::Null, &json!(2))
    );
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "parameter", "insert": conv["insert"], "parameter": "Mix"},
               "points": [{"tick": 0, "value": 0.1}, {"tick": 1920, "value": 0.35}]}),
    )
    .unwrap();
    // The Space Reverb on the master (256 samples), its impulse made from the settings off the
    // audio thread (Decay shortened by hand: the export waits for it made again), in true stereo
    // and its Reverberation level on a lane.
    let full = call(&r, &mut s, "add_insert", json!({"plugin": "Space Reverb"})).unwrap();
    assert_eq!(full["latency"], 256);
    for (parameter, value) in [
        ("Reverberation Decay Time", (700.0 - 20.0) / 5980.0),
        ("Sum Inputs", 0.0),
    ] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": full["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "parameter", "insert": full["insert"],
                          "parameter": "Output Level (Reverberation)"},
               "points": [{"tick": 0, "value": 0.02}, {"tick": 1920, "value": 0.1}]}),
    )
    .unwrap();
    // The Delay on the track, its left 200 ms early (9599 samples of latency, compensated),
    // the right inverted, and its right Mix on a lane (a line drawn sample by sample).
    let delay = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Delay"}),
    )
    .unwrap();
    for (parameter, value) in [
        ("Left Channel Delay Time", 0.3),
        ("Invert Right Channel", 1.0),
    ] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": delay["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": track,
               "target": {"type": "parameter", "insert": delay["insert"],
                          "parameter": "Right Channel Mix"},
               "points": [{"tick": 0, "value": 0.2}, {"tick": 1920, "value": 0.9}]}),
    )
    .unwrap();
    // The Echo on the master with Bounce on, its delay shortened by hand (it crossfades) and
    // its left Feedback and an EQ band on lanes.
    let echo = call(&r, &mut s, "add_insert", json!({"plugin": "Echo"})).unwrap();
    assert_eq!(echo["latency"], 0);
    for (parameter, value) in [("Echo Bounce", 1.0), ("Delay Time (Left Channel)", 0.06)] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": echo["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    for parameter in ["Feedback (Left Channel)", "EQ Band 5"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"target": {"type": "parameter", "insert": echo["insert"], "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.3}, {"tick": 1920, "value": 0.8}]}),
        )
        .unwrap();
    }
    // The Vintage Delay on the master in Tape mode, synced to the tempo (a dotted eighth), its
    // Wet and Trash on lanes; the tempo halves at bar 2 (0.8 s in at 300 BPM), so it glides
    // from 150 to 300 ms in the middle of the export, wherever the blocks fall.
    call(&r, &mut s, "set_tempo", json!({"bpm": 300})).unwrap();
    call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 2, "bpm": 150}),
    )
    .unwrap();
    let analog = call(&r, &mut s, "add_insert", json!({"plugin": "Vintage Delay"})).unwrap();
    assert_eq!(analog["latency"], 0);
    for (parameter, value) in [("Mode", 0.9), ("Tempo Sync", 1.0), ("Sync Note", 0.43)] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": analog["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    for parameter in ["Wet Out", "Trash"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"target": {"type": "parameter", "insert": analog["insert"], "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.2}, {"tick": 1920, "value": 0.7}]}),
        )
        .unwrap();
    }
    // The Chorus on the track with 4 voices at Highest Quality (its wander reseeded from the
    // settings at every start, made on the main thread), Modulation Depth and Delay Rate set by
    // hand, Dry and Wet on lanes.
    let chorus = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Chorus"}),
    )
    .unwrap();
    assert_eq!(chorus["latency"], 0);
    for (parameter, value) in [
        ("Voices", 0.2),
        ("Highest Quality", 1.0),
        ("Modulation Depth", 0.3),
        ("Delay Rate", 0.2),
    ] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": chorus["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    for parameter in ["Dry Output Level", "Wet Output Level"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"track": track,
                   "target": {"type": "parameter", "insert": chorus["insert"], "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.3}, {"tick": 1920, "value": 0.7}]}),
        )
        .unwrap();
    }
    // The Flanger on the master (5082 samples, compensated) with one tap and Inverted, its
    // Final Delay Time and Feedback on lanes and Stereo Phasing set by hand.
    let flanger = call(&r, &mut s, "add_insert", json!({"plugin": "Flanger"})).unwrap();
    assert_eq!(flanger["latency"], 5082);
    for (parameter, value) in [
        ("Special Effects Mode", 1.0),
        ("Inverted Mode", 1.0),
        ("Stereo Phasing", 0.5),
    ] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": flanger["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    for parameter in ["Final Delay Time", "Feedback"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"target": {"type": "parameter", "insert": flanger["insert"], "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.2}, {"tick": 1920, "value": 0.6}]}),
        )
        .unwrap();
    }
    // The Phaser on the track at 5 stages with feedback, its Upper Freq and Mix on lanes and
    // its Phase Diff set by hand.
    let phaser = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Phaser"}),
    )
    .unwrap();
    assert_eq!(phaser["latency"], 0);
    for (parameter, value) in [("Stages", 0.75), ("Feedback", 0.8), ("Phase Diff", 0.7)] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": phaser["insert"], "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    for parameter in ["Upper Freq", "Mix"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"track": track,
                   "target": {"type": "parameter", "insert": phaser["insert"], "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.3}, {"tick": 1920, "value": 0.6}]}),
        )
        .unwrap();
    }
    // And a lane moving one of its bands: the kernel follows it once per FFT block.
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "parameter", "insert": geq["insert"], "parameter": "2 kHz"},
               "points": [{"tick": 0, "value": 0.4}, {"tick": 1920, "value": 0.65}]}),
    )
    .unwrap();
    // A lane moving the left gain, so automation is in it too.
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": track, "target": {"type": "parameter", "insert": amp, "parameter": "Left Gain"},
               "points": [{"tick": 0, "value": 0.7}, {"tick": 1920, "value": 0.4}]}),
    )
    .unwrap();
    let out = dir.join("export.wav");
    s.export(&out, Some((0, 48_000)), 0.0).unwrap();
    let exported = wav::read(&s.wait_for_export(Duration::from_secs(20)).unwrap()).unwrap();
    for block in [512, 333, 64] {
        let heard = s.capture_playback(0, 48_000, block).unwrap();
        assert_eq!(heard.len(), exported.channels.len());
        for (c, (h, e)) in heard.iter().zip(&exported.channels).enumerate() {
            let first = h.iter().zip(e).position(|(a, b)| a != b);
            assert_eq!(first, None, "channel {c} at blocks of {block}");
        }
    }
}

#[test]
fn the_dehummer_is_an_insert_that_takes_out_its_frequency() {
    // add_insert by name, its parameters through the parameter actions, and an export: the
    // 440 Hz tone loses about 60 dB with the Frequency on it, and nothing with it elsewhere.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("hum-remover");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "hum-remover"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Hum Remover"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 6);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        ["60.0 Hz", "60.0 Q", "-60.0 dB", "50 %", "Off", "8"],
        "{params}"
    );
    let rms = |v: &[f32]| -> f64 {
        let steady = &v[24_000..48_000];
        (steady.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / steady.len() as f64).sqrt()
    };
    // At 60 Hz the 440 Hz tone sits between the 7th and 8th harmonics, in their skirts: it
    // loses what the editor's curve says (about 6.5 dB; the -60 dB cuts are wide at Q 60).
    let at_60 = mixdown(&r, &mut s, &dir, "at-60.wav");
    let kept = 20.0 * (rms(&at_60) / rms(&dry)).log10();
    let kind = crate::native::find("hum-remover").unwrap();
    let defaults: Vec<f64> = kind.params.iter().map(|p| p.default).collect();
    let curve = (kind.response.unwrap())(48_000.0, &defaults, 440.0);
    assert!(
        (kept - curve).abs() < 0.2,
        "{kept:.2} dB, the curve {curve:.2}"
    );
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "frequency", "value": 420.0 / 19_980.0}),
    )
    .unwrap();
    assert_eq!(set["display"], "440.0 Hz");
    let notched = mixdown(&r, &mut s, &dir, "notched.wav");
    let cut = 20.0 * (rms(&notched) / rms(&dry)).log10();
    assert!((cut + 60.0).abs() < 1.5, "{cut:.2} dB");
}

#[test]
fn the_studio_reverb_is_an_insert_that_adds_a_tail() {
    // add_insert by name, its 11 parameters and their displays, and exports of the tone (silent
    // for its first 12000 samples): no latency, the dry part exact, the first echo 31.25 ms
    // after the tone starts at Room Size 70 and 50 ms at 100.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("room-reverb");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "studio reverb"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Room Reverb"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 11);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        [
            "880 Hz",
            "13.80 kHz",
            "70",
            "25",
            "50 %",
            "50 %",
            "2500 ms",
            "52 %",
            "50",
            "75 %",
            "25 %"
        ],
        "{params}"
    );
    let first = |v: &[f32]| v.iter().position(|x| *x != 0.0);
    let start = first(&dry).unwrap();
    // The defaults: nothing before the tone, then the dry tone at 75 % alone until the first
    // comb echo (1500 samples at 48 kHz), then the tail on top.
    let verb = mixdown(&r, &mut s, &dir, "default.wav");
    assert_eq!(first(&verb), Some(start));
    for n in start..start + 1500 {
        assert!((verb[n] - 0.75 * dry[n]).abs() <= 1e-6, "sample {n}");
    }
    let wet: Vec<f32> = verb.iter().zip(&dry).map(|(v, d)| v - 0.75 * d).collect();
    let echo = wet[start + 1500..start + 1600]
        .iter()
        .fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(echo > 1e-4, "the first echo: {echo}");
    // Wet only, Room Size 100: the first echo 50 ms after the tone starts.
    for (parameter, value, display) in [
        ("Dry", 0.0, "0 %"),
        ("Wet", 1.0, "100 %"),
        ("Room Size", 1.0, "100"),
    ] {
        let set = call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
        assert_eq!(set["display"], display);
    }
    let wet = mixdown(&r, &mut s, &dir, "wet.wav");
    assert_eq!(first(&wet), Some(start + 2400));
    // Wet 0, Dry 100: the input back.
    for (parameter, value) in [("Dry", 1.0), ("Wet", 0.0)] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    assert_eq!(mixdown(&r, &mut s, &dir, "unity.wav"), dry);
    // Room Size has no lane (each change empties the reverb).
    assert!(call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": "Room Size"},
               "points": [{"tick": 0, "value": 0.2}]}),
    )
    .is_err());
}

#[test]
fn the_convolution_reverb_plays_an_impulse_file_copied_into_the_project() {
    // add_insert by name, its 7 parameters, the input unchanged without an impulse, then
    // set_impulse_file by absolute path: copied into the project's Imported folder, saved by
    // that path, and exported through (two taps 1000 samples apart, the second faded by Room
    // Size's half cosine, both scaled by the normalisation); export waits for a re-preparation;
    // Save As takes the impulse along and the project reopens playing the same.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("convolution-reverb");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "convolution reverb"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Convolution Reverb"), Some(256))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        ["100 %", "100 %", "70 %", "73 %", "0.0 ms", "100 %", "-3.0 dB"],
        "{params}"
    );
    // No impulse yet: the input, unchanged and in time (the 256 samples are compensated).
    let none = call(&r, &mut s, "get_impulse_file", json!({"insert": insert})).unwrap();
    assert_eq!(
        (&none["file"], &none["preparing"]),
        (&Value::Null, &json!(false))
    );
    assert_eq!(mixdown(&r, &mut s, &dir, "none.wav"), dry);

    // A mono impulse outside the project: 1 at 0 and 0.5 at 1000, 2000 samples long.
    let outside = dir.join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    let ir = outside.join("taps.wav");
    let mut taps = vec![0.0f32; 2000];
    taps[0] = 1.0;
    taps[1000] = 0.5;
    wav::write_f32(
        &ir,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![taps],
        },
    )
    .unwrap();
    let original = std::fs::read(&ir).unwrap();
    let set = call(
        &r,
        &mut s,
        "set_impulse_file",
        json!({"insert": insert, "path": ir}),
    )
    .unwrap();
    assert_eq!(set["file"], "Imported/taps.wav", "{set}");
    assert_eq!(
        (
            &set["preparing"],
            &set["error"],
            &set["file_channels"],
            &set["file_rate"]
        ),
        (&json!(false), &Value::Null, &json!(1), &json!(48_000)),
        "{set}"
    );
    assert!((set["seconds"].as_f64().unwrap() - 2000.0 / 48_000.0).abs() < 1e-9);
    let copy = s.project_dir().unwrap().join("Imported").join("taps.wav");
    assert_eq!(std::fs::read(&copy).unwrap(), original);
    assert_eq!(
        std::fs::read(&ir).unwrap(),
        original,
        "the original untouched"
    );
    // Damping HF neutral and Gain 0 dB, then straight to an export: it waits for the impulse
    // prepared again.
    for (parameter, value) in [("Damping HF", 60.0 / 90.0), ("Gain", 0.5)] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    let pending = call(&r, &mut s, "get_impulse_file", json!({"insert": insert})).unwrap();
    assert_eq!(pending["preparing"], true, "{pending}");
    let wet = mixdown(&r, &mut s, &dir, "taps.wav");
    // y = Mix·k·(x[n] + 0.5·w·x[n − 1000]), w the fade at 1000 of 2000, k = 0.5/(1 + 0.5·w).
    let w = 0.5 + 0.5 * (std::f64::consts::PI * 1000.0 / 2001.0).cos();
    let k = 0.5 / (1.0 + 0.5 * w);
    let mut worst = 0.0f64;
    for n in 0..48_000 {
        let echo = if n >= 1000 {
            f64::from(dry[n - 1000])
        } else {
            0.0
        };
        let want = 100.0 * k * (f64::from(dry[n]) + 0.5 * w * echo);
        worst = worst.max((f64::from(wet[n]) - want).abs());
    }
    assert!(worst < 1e-4, "{worst}");

    // Save As elsewhere takes the impulse along; reopened, it plays the same.
    let other = dir.join("saved").join("Verb.oxide");
    call(&r, &mut s, "save_project", json!({"path": other})).unwrap();
    assert!(dir
        .join("saved")
        .join("Imported")
        .join("taps.wav")
        .is_file());
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": other})).unwrap();
    let insert = json!(s.project.tracks[0].inserts[0].id);
    let back = call(&r, &mut s, "get_impulse_file", json!({"insert": insert})).unwrap();
    assert_eq!(back["file"], "Imported/taps.wav");
    assert_eq!(mixdown(&r, &mut s, &dir, "reopened.wav"), wet);

    // Refused: another effect, a file that isn't audio, an insert that doesn't exist.
    let first = s.project.tracks[0].id;
    let amp = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": first, "plugin": "Amplify"}),
    )
    .unwrap()["insert"]
        .clone();
    let err = call(
        &r,
        &mut s,
        "set_impulse_file",
        json!({"insert": amp, "path": ir}),
    )
    .unwrap_err();
    assert!(err.contains("plays no impulse"), "{err}");
    let text = outside.join("notes.wav");
    std::fs::write(&text, "not audio").unwrap();
    let err = call(
        &r,
        &mut s,
        "set_impulse_file",
        json!({"insert": insert, "path": text}),
    )
    .unwrap_err();
    assert!(err.contains("notes.wav"), "{err}");
    assert!(!dir
        .join("saved")
        .join("Imported")
        .join("notes.wav")
        .exists());
    let err = call(
        &r,
        &mut s,
        "set_impulse_file",
        json!({"insert": 999_999, "path": ir}),
    )
    .unwrap_err();
    assert!(err.contains("999999"), "{err}");
    // Room Size has no lane: each change prepares the impulse again.
    assert!(call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"target": {"type": "parameter", "insert": insert, "parameter": "Room Size"},
               "points": [{"tick": 0, "value": 0.2}]}),
    )
    .is_err());
}

#[test]
fn the_full_reverb_makes_its_impulse_from_its_settings() {
    // add_insert by name: 256 samples of latency and its 29 parameters (the room's
    // dimensions computed, read-only); no impulse file to choose; an export straight after a
    // settings change waits for the impulse made again and equals the effect run offline on the
    // tone, tail included; get_impulse_file then gives its length; "Set Reverb Based on Room
    // Size" moves Decay; Room Size has no lane, the output levels have.
    use crate::native::full_reverb;
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("space-reverb");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "full reverb"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Space Reverb"), Some(256))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 29);
    let list = params["parameters"].as_array().unwrap();
    let shown = |i: usize| list[i]["display"].as_str().unwrap().to_string();
    assert_eq!(
        [0, 4, 7, 15, 19, 25, 28].map(shown),
        [
            "2636 ms",
            "73.1 %",
            "8879.6 m³",
            "16.95 kHz",
            "On",
            "21.82 m",
            "44100 Hz"
        ],
        "{params}"
    );
    let read_only: Vec<u64> = list
        .iter()
        .filter(|p| p["read_only"] == true)
        .map(|p| p["id"].as_u64().unwrap())
        .collect();
    assert_eq!(read_only, [22, 23, 24, 25, 26, 27]);
    // It makes its impulse: there is no file to give it.
    let err = call(
        &r,
        &mut s,
        "set_impulse_file",
        json!({"insert": insert, "path": file}),
    )
    .unwrap_err();
    assert!(err.contains("makes its impulse"), "{err}");
    // Decay 600 ms, Pre-Delay 20 ms, then straight to an export with a second of tail.
    for (parameter, value) in [
        ("Reverberation Decay Time", (600.0 - 20.0) / 5980.0),
        ("Reverberation Pre-Delay Time", 0.1),
    ] {
        call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
    }
    let pending = call(&r, &mut s, "get_impulse_file", json!({"insert": insert})).unwrap();
    assert_eq!(
        (&pending["file"], &pending["preparing"]),
        (&Value::Null, &json!(true)),
        "{pending}"
    );
    let out = dir.join("verb.wav");
    call(
        &r,
        &mut s,
        "export_mixdown",
        json!({"path": out, "start": 0, "end": 48_000, "tail_seconds": 1.0}),
    )
    .unwrap();
    s.wait_for_export(Duration::from_secs(20)).unwrap();
    let exported = wav::read(&out).unwrap().channels;
    assert_eq!(exported[0].len(), 96_000);
    let values = s
        .native(insert.as_u64().unwrap() as u32)
        .unwrap()
        .plain_values();
    let settings: std::collections::BTreeMap<String, f64> = full_reverb::KIND
        .params
        .iter()
        .zip(&values)
        .map(|(p, v)| (p.name.to_string(), *v))
        .collect();
    let mut tone_and_tail = wav::read(&file).unwrap().channels[0].clone();
    tone_and_tail.resize(96_000, 0.0);
    let offline = crate::native::render(
        &full_reverb::KIND,
        48_000.0,
        &settings,
        &[tone_and_tail.clone(), tone_and_tail],
        512,
    )
    .unwrap();
    for c in 0..2 {
        let worst = exported[c]
            .iter()
            .zip(&offline[c])
            .fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(worst < 1e-6, "channel {c}: {worst}");
    }
    let tail = &exported[0][48_256..];
    assert!(tail.iter().take(4800).any(|x| x.abs() > 1e-3), "a tail");
    let made = call(&r, &mut s, "get_impulse_file", json!({"insert": insert})).unwrap();
    assert_eq!(made["preparing"], false, "{made}");
    let length = full_reverb::synthesize(48_000.0, &values).late[0].len();
    assert!((made["seconds"].as_f64().unwrap() - length as f64 / 48_000.0).abs() < 1e-9);
    // "Set Reverb Based on Room Size": Decay and Pre-Delay from the default room by Sabine.
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "Set Reverb Based on Room Size", "value": 1.0}),
    )
    .unwrap();
    assert_eq!(set["display"], "On");
    let (decay, _) = full_reverb::sabine(full_reverb::room(8879.6, 0.736));
    let now = s
        .native(insert.as_u64().unwrap() as u32)
        .unwrap()
        .plain_values();
    assert!((now[full_reverb::DECAY] - decay).abs() < 1e-6, "{decay}");
    // An output is refused by name (and would keep its value anyway).
    let err = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "Room Width", "value": 0.5}),
    )
    .unwrap_err();
    assert!(
        err.contains("Room Width") && err.contains("read-only"),
        "{err}"
    );
    // Lanes: the levels yes, Room Size no.
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": "Output Level (Dry)"},
               "points": [{"tick": 0, "value": 0.2}]}),
    )
    .unwrap();
    assert!(call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": "Room Size"},
               "points": [{"tick": 0, "value": 0.2}]}),
    )
    .is_err());
}

#[test]
fn the_automatic_click_remover_is_an_insert_that_takes_out_clicks() {
    // add_insert by name, its parameters through the parameter actions, and an export of a tone
    // with clicks of 1 to 20 samples: each click at least 20 dB down, the rest untouched.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("click-remover");
    // A 440 Hz tone at -6 dBFS, faded in and out over 10 ms (a tone starting or stopping at
    // full tilt is a step, which the remover would smooth), with clicks: (sample, length, value
    // added).
    let clean: Vec<f32> = (0..48_000)
        .map(|i| {
            let fade = (i.min(47_999 - i) as f32 / 480.0).min(1.0);
            fade * 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
        })
        .collect();
    let clicks = [
        (9_000usize, 1usize, 0.4f32),
        (15_500, 3, -0.4),
        (22_222, 5, 0.3),
        (30_001, 12, -0.4),
        (38_000, 20, 0.35),
    ];
    let mut clicky = clean.clone();
    for &(at, len, v) in &clicks {
        for (k, x) in clicky[at..at + len].iter_mut().enumerate() {
            *x += if k % 3 == 1 { -v } else { v };
        }
    }
    let file = dir.join("clicky.wav");
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![clicky],
        },
    )
    .unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "automatic click remover"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Click Remover"), Some(4096))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 2);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(shown, ["29.3 %", "15.2 %"], "{params}");
    // Medium Reduction's Complexity (S 56, clicks up to 120 samples).
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "complexity", "value": 55.0 / 99.0}),
    )
    .unwrap();
    assert_eq!(set["display"], "55.6 %");
    let wet = mixdown(&r, &mut s, &dir, "wet.wav");
    // Both in time with the file (the export compensates the 4096 samples).
    let near = |i: usize| {
        clicks
            .iter()
            .any(|&(at, len, _)| i + 8 >= at && i < at + len + 8)
    };
    let error = |x: &[f32], at: usize, len: usize| -> f64 {
        (at - 8..at + len + 8)
            .map(|i| (f64::from(x[i]) - f64::from(clean[i])).powi(2))
            .sum()
    };
    for &(at, len, _) in &clicks {
        let depth = 10.0 * (error(&wet, at, len) / error(&dry, at, len)).log10();
        println!("click of {len} at {at}: {depth:.1} dB");
        assert!(depth < -20.0, "click of {len} at {at}: {depth:.1} dB");
    }
    let changed: Vec<usize> = (0..48_000)
        .filter(|&i| !near(i) && wet[i] != dry[i])
        .collect();
    assert!(changed.is_empty(), "{changed:?}");
}

#[test]
fn the_click_pop_eliminator_scans_its_levels_and_takes_out_clicks() {
    // add_insert by name, its 23 parameters and their displays, both scans through
    // capture_noise_print (into a track's insert and the Waveform rack's), its counters, and
    // an export of a tone with clicks of 1 to 15 samples: each at least 20 dB down, the audio
    // away from them untouched.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("click-pop");
    let clean: Vec<f32> = (0..48_000)
        .map(|i| {
            let fade = (i.min(47_999 - i) as f32 / 480.0).min(1.0);
            fade * 0.5 * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 48_000.0).sin()
        })
        .collect();
    let clicks = [
        (6_000usize, 1usize, 0.4f32),
        (12_500, 3, -0.4),
        (19_222, 6, 0.3),
        (27_001, 10, -0.4),
        (36_000, 15, 0.35),
    ];
    let mut clicky = clean.clone();
    for &(at, len, v) in &clicks {
        for (k, x) in clicky[at..at + len].iter_mut().enumerate() {
            *x += if k % 3 == 1 { -v } else { v };
        }
    }
    let file = dir.join("clicky.wav");
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![clicky],
        },
    )
    .unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "click/pop eliminator"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Click & Pop Repair"), Some(22_016))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 23);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown[..13],
        [
            "30.0",
            "12.0",
            "15.0",
            "-12.0 dB",
            "-50.0 dB",
            "-24.0 dB",
            "40.0",
            "18.0",
            "28.0",
            "32 samples",
            "12 samples",
            "10 samples",
            "8 passes"
        ],
        "{params}"
    );
    assert_eq!(shown[16..], ["On", "On", "On", "On", "Off", "Off", "Off"]);
    assert_eq!(params["counters"], json!([0, 0, 0]));
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "pop size", "value": 10.0 / 300.0}),
    )
    .unwrap();
    assert_eq!(set["display"], "10 samples");
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "pop size", "value": 12.0 / 300.0}),
    )
    .unwrap();
    // Scan For Threshold Levels from the file in the Waveform view: the levels move (the
    // maximum to Max Level, the minimum to Avg Level, the mean to Min Level), the thresholds
    // don't.
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let scanned = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000, "insert": insert}),
    )
    .unwrap();
    assert_eq!(scanned["effect"], "Click & Pop Repair");
    let native = s.native(insert.as_u64().unwrap() as u32).unwrap();
    let levels = crate::native::click_pop::scanned(native.print().unwrap()).unwrap();
    let v = native.plain_values();
    use crate::native::click_pop as cp;
    assert!(!levels.all);
    assert_eq!(
        (v[cp::MAX_LEVEL], v[cp::AVG_LEVEL], v[cp::MIN_LEVEL]),
        (levels.max, levels.min, levels.mean)
    );
    assert!(
        levels.min < levels.mean && levels.mean < levels.max,
        "{levels:?}"
    );
    assert_eq!(v[cp::MAX_THRESHOLD], 30.0);
    println!("scanned: {levels:?}");
    // A scan it doesn't have is refused by name; so is a scan for an effect without scans.
    let err = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000, "insert": insert, "scan": "nope"}),
    )
    .unwrap_err();
    assert!(err.contains("no scan 'nope'"), "{err}");
    let nr = call(
        &r,
        &mut s,
        "add_insert",
        json!({"editor": true, "plugin": "Noise Reduction"}),
    )
    .unwrap()["insert"]
        .clone();
    let err = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000, "insert": nr, "scan": "all levels"}),
    )
    .unwrap_err();
    assert!(err.contains("has no scans"), "{err}");
    // Scan For All Levels into the Waveform rack's own Click & Pop Repair, found by its
    // scan's name: the levels and the six thresholds.
    let rack = call(
        &r,
        &mut s,
        "add_insert",
        json!({"editor": true, "plugin": "Click & Pop Repair"}),
    )
    .unwrap()["insert"]
        .clone();
    let all = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000, "scan": "Scan For All Levels"}),
    )
    .unwrap();
    assert_eq!(all["insert"], rack);
    let native = s.native(rack.as_u64().unwrap() as u32).unwrap();
    let learned = cp::scanned(native.print().unwrap()).unwrap();
    let t = learned.thresholds.unwrap();
    let v = native.plain_values();
    assert_eq!(
        [
            v[cp::MAX_THRESHOLD],
            v[cp::AVG_THRESHOLD],
            v[cp::MIN_THRESHOLD],
            v[cp::MAX_REJECT],
            v[cp::AVG_REJECT],
            v[cp::MIN_REJECT]
        ],
        t
    );
    println!("all levels: {learned:?}");
    // The export: every click at least 20 dB down; away from them, the audio untouched.
    let wet = mixdown(&r, &mut s, &dir, "wet.wav");
    let error = |x: &[f32], at: usize, len: usize| -> f64 {
        (at - 8..at + len + 8)
            .map(|i| (f64::from(x[i]) - f64::from(clean[i])).powi(2))
            .sum()
    };
    for &(at, len, _) in &clicks {
        let depth = 10.0 * (error(&wet, at, len) / error(&dry, at, len)).log10();
        println!("click of {len} at {at}: {depth:.1} dB");
        assert!(depth < -20.0, "click of {len} at {at}: {depth:.1} dB");
    }
    // A click's repair runs on over its statistic's tail (Run Size): 60 samples after it.
    let near = |i: usize| {
        clicks
            .iter()
            .any(|&(at, len, _)| i + 8 >= at && i < at + len + 60)
    };
    let changed: Vec<usize> = (0..48_000)
        .filter(|&i| !near(i) && wet[i] != dry[i])
        .collect();
    assert!(changed.is_empty(), "{changed:?}");
    let counters = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert, "limit": 1}),
    )
    .unwrap()["counters"]
        .clone();
    println!("counters after the export: {counters}");
    assert!(
        counters[0].as_u64().unwrap() >= clicks.len() as u64,
        "{counters}"
    );
}

/// One second at 48 kHz of a 480 Hz sine (a cycle every 100 samples) at -3 dBFS
/// hard-clipped at ±0.5 (-6 dBFS), written as `clipped.wav`.
fn clipped_sine_file(dir: &std::path::Path) -> PathBuf {
    let clipped: Vec<f32> = (0..48_000)
        .map(|i| {
            let phase = 2.0 * std::f64::consts::PI * 480.0 * i as f64 / 48_000.0 + 0.3;
            ((10f64.powf(-3.0 / 20.0) * phase.sin()) as f32).clamp(-0.5, 0.5)
        })
        .collect();
    let path = dir.join("clipped.wav");
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![clipped],
        },
    )
    .unwrap();
    path
}

/// The worst difference (dB) between `amp` and each 100-sample cycle's highest and lowest
/// sample in `x`, over `cycles`.
fn worst_peak_error(x: &[f32], amp: f64, cycles: std::ops::Range<usize>) -> f64 {
    cycles
        .map(|k| {
            let cycle = &x[k * 100..(k + 1) * 100];
            let hi = cycle.iter().fold(f32::MIN, |m, s| m.max(*s));
            let lo = cycle.iter().fold(f32::MAX, |m, s| m.min(*s));
            let e = |v: f32| (20.0 * (f64::from(v.abs()) / amp).log10()).abs();
            e(hi).max(e(lo))
        })
        .fold(0.0, f64::max)
}

#[test]
fn the_declipper_is_an_insert_that_learns_the_clip_levels_and_rebuilds_the_peaks() {
    // add_insert by name, its parameters through the parameter actions, the levels scanned
    // through capture_noise_print from the file in the Waveform view, and an export: each
    // clipped peak back within 1 dB of the sine's. Then in the Waveform view's rack, scanned
    // without naming it and applied.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("clip-repair");
    let file = clipped_sine_file(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    // The mix of the clipped file as it is: its level in the mix (the pan law) from the clip.
    let dry = mixdown(&r, &mut s, &dir, "dry.wav");
    let scale = f64::from(dry.iter().fold(0.0f32, |m, v| m.max(v.abs()))) / 0.5;
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "clip-repair"}),
    )
    .unwrap();
    // 520 samples of repair and the limiter's 20 ms.
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Clip Repair"), Some(1480))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 6);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        ["-12.0 dB", "Off", "10.0 %", "3 samples", "Cubic", "32"],
        "{params}"
    );
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "gain", "value": 1.0}),
    )
    .unwrap();
    assert_eq!(set["display"], "0.0 dB");
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let scanned = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000, "insert": insert}),
    )
    .unwrap();
    assert_eq!(scanned["effect"], "Clip Repair");
    let levels = crate::native::declipper::learned(
        s.native(insert.as_u64().unwrap() as u32)
            .unwrap()
            .print()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (levels[0].top, levels[0].bottom, levels[0].runs),
        (0.5, -0.5, 960)
    );
    let wet = mixdown(&r, &mut s, &dir, "wet.wav");
    let amp = 10f64.powf(-3.0 / 20.0) * scale;
    let clipped = worst_peak_error(&dry, amp, 20..460);
    let rebuilt = worst_peak_error(&wet, amp, 20..460);
    println!("clipped peaks {clipped:.2} dB down; rebuilt within {rebuilt:.3} dB");
    assert!(clipped > 2.9 && rebuilt < 1.0, "{clipped} {rebuilt}");

    // The Waveform view's rack: a Clip Repair there takes the scan without being named, and
    // Apply writes its repair (at its default Gain, -12 dB).
    let rack = call(
        &r,
        &mut s,
        "add_insert",
        json!({"editor": true, "plugin": "Clip Repair"}),
    )
    .unwrap()["insert"]
        .clone();
    let scanned = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 48_000}),
    )
    .unwrap();
    assert_eq!(scanned["insert"], rack);
    assert_eq!(scanned["effect"], "Clip Repair");
    s.update();
    call(&r, &mut s, "apply_audio_effects", json!({"document": doc})).unwrap();
    s.wait_for_apply(Duration::from_secs(30)).unwrap();
    let applied = s.document(doc as u32).unwrap().audio.channels[0].clone();
    let g = f64::from(crate::native::declipper::gain_of(-12.0));
    let rebuilt = worst_peak_error(&applied, 10f64.powf(-3.0 / 20.0) * g, 20..460);
    println!("applied in the Waveform view at -12 dB: peaks within {rebuilt:.3} dB");
    assert!(rebuilt < 1.0, "{rebuilt}");
}

/// Both channels of `export_mixdown`'s first second.
fn mixdown_pair(r: &Registry, s: &mut Session, dir: &std::path::Path, name: &str) -> Vec<Vec<f32>> {
    let out = dir.join(name);
    call(
        r,
        s,
        "export_mixdown",
        json!({"path": out, "start": 0, "end": 48_000, "tail_seconds": 0}),
    )
    .unwrap();
    s.wait_for_export(Duration::from_secs(20)).unwrap();
    wav::read(&out).unwrap().channels
}

#[test]
fn the_delay_is_an_insert_whose_early_copies_are_compensated() {
    // add_insert by name, its 7 parameters and their displays, and exports of the tone (silent
    // for its first 12000 samples): the defaults add no latency, a copy 120 ms late on the left
    // and 180 ms on the right; a negative time reports its latency, the graph compensates it
    // and the copy comes early by exactly its amount; delay times have no lanes, Mix has.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("delay");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown_pair(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "delay"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Delay"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 7);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        [
            "120.0 ms",
            "Off",
            "75 %",
            "180.0 ms",
            "Off",
            "65 %",
            "Milliseconds"
        ],
        "{params}"
    );
    // y = m·x[n - d] + (1 - m)·x[n] on each side.
    let expect = |out: &[Vec<f32>], taps: [(i64, f32); 2]| {
        for (c, (d, m)) in taps.iter().enumerate() {
            for n in 0..48_000i64 {
                let back = n - d;
                let wet = if (0..48_000).contains(&back) {
                    dry[c][back as usize]
                } else {
                    0.0
                };
                let want = m * wet + (1.0 - m) * dry[c][n as usize];
                let got = out[c][n as usize];
                assert!(
                    (got - want).abs() <= 1e-6,
                    "channel {c} sample {n}: {got} vs {want}"
                );
            }
        }
    };
    let out = mixdown_pair(&r, &mut s, &dir, "default.wav");
    expect(&out, [(5760, 0.75), (8640, 0.65)]);
    // Left -200 ms and right +200 ms at 60 % (Spatial Echo): 9599 samples of latency, and the
    // left copy 9599 samples ahead of the dry sound in the export.
    for (parameter, value, display) in [
        ("Left Channel Delay Time", 0.3, "-200.0 ms"),
        ("Right Channel Delay Time", 0.7, "200.0 ms"),
        ("Left Channel Mix", 0.6, "60 %"),
        ("Right Channel Mix", 0.6, "60 %"),
    ] {
        let set = call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
        assert_eq!(set["display"], display);
    }
    s.update();
    let id_of = insert.as_u64().unwrap() as u32;
    assert_eq!(s.insert_info(id_of).map(|i| i.1), Some(9599));
    let out = mixdown_pair(&r, &mut s, &dir, "spatial.wav");
    expect(&out, [(-9599, 0.6), (9600, 0.6)]);
    let first = |v: &[f32]| v.iter().position(|x| *x != 0.0).unwrap();
    assert_eq!(first(&out[0]) + 9599, first(&dry[0]));
    // Back to positive times: no latency again.
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "Left Channel Delay Time", "value": 0.6}),
    )
    .unwrap();
    s.update();
    assert_eq!(s.insert_info(id_of).map(|i| i.1), Some(0));
    // A lane on a delay time is refused (it would move the latency while playing); Mix takes one.
    let lane = |parameter: &str| {
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": parameter},
               "points": [{"tick": 0, "value": 0.2}]})
    };
    assert!(call(
        &r,
        &mut s,
        "add_automation_lane",
        lane("Left Channel Delay Time")
    )
    .is_err());
    call(&r, &mut s, "add_automation_lane", lane("Left Channel Mix")).unwrap();
}

#[test]
fn the_echo_is_an_insert_that_plays_as_it_renders_offline() {
    // add_insert by name, its 17 parameters and their displays, and exports of the tone: no
    // latency, the dry sound at unity, and the same samples as the effect run offline on the
    // dry mixdown (at the defaults, and with Bounce on); Feedback takes a lane, Lock doesn't.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("echo");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown_pair(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "echo"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Echo"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 17);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        [
            "348.8 ms",
            "232.6 ms",
            "64.0 %",
            "64.0 %",
            "85.0 %",
            "85.0 %",
            "-9.8 dB",
            "-9.9 dB",
            "-9.6 dB",
            "-9.5 dB",
            "-2.4 dB",
            "-7.0 dB",
            "-1.1 dB",
            "0.0 dB",
            "Off",
            "Off",
            "Milliseconds"
        ],
        "{params}"
    );
    let kind = crate::native::find("Echo").unwrap();
    let offline = |values: &[(&str, f64)]| {
        let settings = values.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        crate::native::render(kind, 48_000.0, &settings, &dry, 512).unwrap()
    };
    let close = |a: &[Vec<f32>], b: &[Vec<f32>], what: &str| {
        for (c, (x, y)) in a.iter().zip(b).enumerate() {
            for (n, (p, q)) in x.iter().zip(y).enumerate() {
                assert!(
                    (p - q).abs() <= 1e-5,
                    "{what}: channel {c} sample {n}: {p} vs {q}"
                );
            }
        }
    };
    let out = mixdown_pair(&r, &mut s, &dir, "default.wav");
    close(&out, &offline(&[]), "the defaults");
    // Before the first echo (348.8 ms after the tone starts on the left) the dry sound alone.
    let first = dry[0].iter().position(|x| *x != 0.0).unwrap();
    for n in first..first + 16_000 {
        assert!((out[0][n] - dry[0][n]).abs() <= 1e-6, "sample {n}");
    }
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "Echo Bounce", "value": 1.0}),
    )
    .unwrap();
    assert_eq!(set["display"], "On");
    let out = mixdown_pair(&r, &mut s, &dir, "bounce.wav");
    close(&out, &offline(&[("Echo Bounce", 1.0)]), "bounce");
    let lane = |parameter: &str| {
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": parameter},
               "points": [{"tick": 0, "value": 0.2}]})
    };
    assert!(call(
        &r,
        &mut s,
        "add_automation_lane",
        lane("Lock Left & Right Channels")
    )
    .is_err());
    call(
        &r,
        &mut s,
        "add_automation_lane",
        lane("Feedback (Left Channel)"),
    )
    .unwrap();
}

#[test]
fn the_analog_delay_is_an_insert_that_syncs_to_the_song_s_tempo() {
    // add_insert by name, its 9 parameters and their displays, an export equal to the effect
    // run offline on the dry mixdown, then Tempo Sync on a quarter note at 100 BPM: the echo
    // 600 ms (28801 samples, give or take the wow) after the tone starts.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("vintage-delay");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown_pair(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "analog delay"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Vintage Delay"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 9);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        ["Analog", "70.0 %", "30.0 %", "200.0 ms", "20.0 %", "25.0 %", "100.0 %", "Off", "1/4"],
        "{params}"
    );
    let kind = crate::native::find("vintage-delay").unwrap();
    let offline = crate::native::render(kind, 48_000.0, &Default::default(), &dry, 512).unwrap();
    let out = mixdown_pair(&r, &mut s, &dir, "default.wav");
    for (c, (x, y)) in out.iter().zip(&offline).enumerate() {
        for (n, (p, q)) in x.iter().zip(y).enumerate() {
            assert!((p - q).abs() <= 1e-5, "channel {c} sample {n}: {p} vs {q}");
        }
    }
    // Synced to a quarter note at 100 BPM, wet alone with no feedback.
    call(&r, &mut s, "set_tempo", json!({"bpm": 100})).unwrap();
    for (parameter, value, display) in [
        ("Tempo Sync", 1.0, "On"),
        ("Dry Out", 0.0, "0.0 %"),
        ("Wet Out", 1.0, "100.0 %"),
        ("Feedback", 0.0, "0.0 %"),
        ("Mode", 0.9, "Tape"),
    ] {
        let set = call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
        assert_eq!(set["display"], display);
    }
    let out = mixdown_pair(&r, &mut s, &dir, "synced.wav");
    let first = |v: &[f32], above: f32| v.iter().position(|x| x.abs() > above).unwrap();
    let start = first(&dry[0], 0.0);
    let echo = first(&out[0], 1e-4);
    let want = start + crate::native::analog_delay::delay_samples(48_000.0, 600.0);
    assert!(
        (want - 8..=want + 6).contains(&echo),
        "the echo at {echo}, {want} expected"
    );
    // A lane on the delay and on Trash.
    for parameter in ["Delay", "Trash"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"track": track,
                   "target": {"type": "parameter", "insert": insert, "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.2}]}),
        )
        .unwrap();
    }
}

#[test]
fn the_chorus_is_an_insert_that_plays_as_it_renders_offline() {
    // add_insert by name, its 13 parameters and their displays, and exports of the tone: no
    // latency, the same samples as the effect run offline on the dry mixdown (its random
    // wander reseeded from the settings at every start), at the defaults and with 4 voices at
    // Highest Quality, and the same again on a second export; Wet takes a lane, the settings
    // the start state is made from (Voices, Delay Time, Modulation Depth...) don't.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("chorus");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown_pair(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "chorus"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Chorus"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 13);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        [
            "31.40 ms", "0.10 Hz", "0.0 %", "70.0 %", "30.0 %", "30.0 ms", "100.0 %", "0.10 Hz",
            "0.0 dB", "2", "Off", "Off", "Off"
        ],
        "{params}"
    );
    let kind = crate::native::find("Chorus").unwrap();
    let offline = |values: &[(&str, f64)]| {
        let settings = values.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        crate::native::render(kind, 48_000.0, &settings, &dry, 512).unwrap()
    };
    let close = |a: &[Vec<f32>], b: &[Vec<f32>], what: &str| {
        for (c, (x, y)) in a.iter().zip(b).enumerate() {
            for (n, (p, q)) in x.iter().zip(y).enumerate() {
                assert!(
                    (p - q).abs() <= 1e-5,
                    "{what}: channel {c} sample {n}: {p} vs {q}"
                );
            }
        }
    };
    let out = mixdown_pair(&r, &mut s, &dir, "default.wav");
    close(&out, &offline(&[]), "the defaults");
    assert_ne!(out, dry);
    for (parameter, value, display) in [
        ("Voices", 3.0 / 15.0, "4"),
        ("Highest Quality", 1.0, "On"),
        ("Modulation Depth", 0.4, "+6.0 dB"),
    ] {
        let set = call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
        assert_eq!(set["display"], display);
    }
    let out = mixdown_pair(&r, &mut s, &dir, "four.wav");
    close(
        &out,
        &offline(&[
            ("Voices", 4.0),
            ("Highest Quality", 1.0),
            ("Modulation Depth", 6.0),
        ]),
        "4 voices",
    );
    assert_eq!(out, mixdown_pair(&r, &mut s, &dir, "four-again.wav"));
    let lane = |parameter: &str| {
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": parameter},
               "points": [{"tick": 0, "value": 0.2}]})
    };
    // What a start copies is made on the main thread from the hashed settings: no lanes.
    for parameter in ["Voices", "Delay Time", "Modulation Depth"] {
        assert!(call(&r, &mut s, "add_automation_lane", lane(parameter)).is_err());
    }
    call(&r, &mut s, "add_automation_lane", lane("Wet Output Level")).unwrap();
}

#[test]
fn the_flanger_is_an_insert_whose_5082_samples_are_compensated() {
    // add_insert by name, its 9 parameters and their displays, its latency of 5082 samples,
    // and exports of the tone: at Mix 0 the tone itself, in place (the graph compensates the
    // latency); at the defaults and with one tap the same samples as the effect run offline on
    // the dry mixdown; every parameter takes a lane.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("flanger");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown_pair(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "flanger"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Flanger"), Some(5082))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 9);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        ["0.00 ms", "4.16 ms", "28 °", "0.232 Hz", "40.0 %", "60.0 %", "Off", "Off", "On"],
        "{params}"
    );
    let kind = crate::native::find("Flanger").unwrap();
    let offline = |values: &[(&str, f64)]| {
        let settings = values.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        crate::native::render(kind, 48_000.0, &settings, &dry, 512).unwrap()
    };
    let close = |a: &[Vec<f32>], b: &[Vec<f32>], what: &str| {
        for (c, (x, y)) in a.iter().zip(b).enumerate() {
            for (n, (p, q)) in x.iter().zip(y).enumerate() {
                assert!(
                    (p - q).abs() <= 1e-5,
                    "{what}: channel {c} sample {n}: {p} vs {q}"
                );
            }
        }
    };
    let out = mixdown_pair(&r, &mut s, &dir, "default.wav");
    close(&out, &offline(&[]), "the defaults");
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "Mix", "value": 0.0}),
    )
    .unwrap();
    assert_eq!(mixdown_pair(&r, &mut s, &dir, "mix-0.wav"), dry);
    for (parameter, value, display) in [
        ("Mix", 1.0, "100.0 %"),
        ("Special Effects Mode", 1.0, "On"),
        ("Sinusoidal Mode", 0.0, "Off"),
    ] {
        let set = call(
            &r,
            &mut s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap();
        assert_eq!(set["display"], display);
    }
    let out = mixdown_pair(&r, &mut s, &dir, "one-tap.wav");
    close(
        &out,
        &offline(&[
            ("Mix", 100.0),
            ("Special Effects Mode", 1.0),
            ("Sinusoidal Mode", 0.0),
        ]),
        "one tap",
    );
    for parameter in ["Final Delay Time", "Mix"] {
        call(
            &r,
            &mut s,
            "add_automation_lane",
            json!({"track": track,
                   "target": {"type": "parameter", "insert": insert, "parameter": parameter},
                   "points": [{"tick": 0, "value": 0.2}]}),
        )
        .unwrap();
    }
}

#[test]
fn the_phaser_is_an_insert_that_plays_as_it_renders_offline() {
    // add_insert by name, its 12 parameters and their displays, and exports of the tone: no
    // latency, at Mix 0 the tone itself, at the defaults and at Phase80 with a sine LFO the same
    // samples as the effect run offline on the dry mixdown; Mod Rate takes a lane.
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("phaser");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    let dry = mixdown_pair(&r, &mut s, &dir, "dry.wav");
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "phaser"}),
    )
    .unwrap();
    assert_eq!(
        (added["name"].as_str(), added["latency"].as_u64()),
        (Some("Phaser"), Some(0))
    );
    let insert = added["insert"].clone();
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )
    .unwrap();
    assert_eq!(params["total"], 12);
    let shown: Vec<&str> = params["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["display"].as_str().unwrap())
        .collect();
    assert_eq!(
        shown,
        [
            "3", "95.0 %", "70.0 %", "0.25 Hz", "0 °", "14000 Hz", "0.0 %", "50.0 %", "0.0 dB",
            "Triangle", "Linear", "Off"
        ],
        "{params}"
    );
    let kind = crate::native::find("Phaser").unwrap();
    let offline = |values: &[(&str, f64)]| {
        let settings = values.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        crate::native::render(kind, 48_000.0, &settings, &dry, 512).unwrap()
    };
    let close = |a: &[Vec<f32>], b: &[Vec<f32>], what: &str| {
        for (c, (x, y)) in a.iter().zip(b).enumerate() {
            for (n, (p, q)) in x.iter().zip(y).enumerate() {
                assert!(
                    (p - q).abs() <= 1e-5,
                    "{what}: channel {c} sample {n}: {p} vs {q}"
                );
            }
        }
    };
    let out = mixdown_pair(&r, &mut s, &dir, "default.wav");
    close(&out, &offline(&[]), "the defaults");
    assert_ne!(out, dry);
    let set = |s: &mut Session, parameter: &str, value: f64| {
        call(
            &r,
            s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": parameter, "value": value}),
        )
        .unwrap()["display"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(set(&mut s, "Mix", 0.0), "0.0 %");
    assert_eq!(mixdown_pair(&r, &mut s, &dir, "mix-0.wav"), dry);
    // Six stages, deeper and faster, with feedback, on a sine.
    let mut want: Vec<(&str, f64)> = kind.params.iter().map(|p| (p.name, p.default)).collect();
    for (name, value) in [
        ("Stages", 6.0),
        ("Depth", 90.0),
        ("Mod Rate", 0.8),
        ("Feedback", 40.0),
        ("Mix", 50.0),
    ] {
        want.iter_mut().find(|w| w.0 == name).unwrap().1 = value;
    }
    for (p, (_, v)) in kind.params.iter().zip(&want) {
        set(&mut s, p.name, p.normalized(*v));
    }
    assert_eq!(set(&mut s, "Shape", 1.0), "Sine");
    want.push(("Shape", 1.0));
    let out = mixdown_pair(&r, &mut s, &dir, "six-stages.wav");
    close(&out, &offline(&want), "six stages");
    call(
        &r,
        &mut s,
        "add_automation_lane",
        json!({"track": track,
               "target": {"type": "parameter", "insert": insert, "parameter": "Mod Rate"},
               "points": [{"tick": 0, "value": 0.2}]}),
    )
    .unwrap();
}

#[test]
fn a_native_effect_in_the_waveform_rack_applies_what_the_preview_plays() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("native-rack");
    let file = dir.join("stereo.wav");
    let left: Vec<f32> = (0..30_000)
        .map(|i| (i as f32 * 0.021).sin() * 0.5)
        .collect();
    let right: Vec<f32> = (0..30_000)
        .map(|i| (i as f32 * 0.047).cos() * 0.3)
        .collect();
    wav::write_f32(
        &file,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![left.clone(), right.clone()],
        },
    )
    .unwrap();
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let amp = call(
        &r,
        &mut s,
        "add_insert",
        json!({"editor": true, "plugin": "Amplify"}),
    )
    .unwrap()["insert"]
        .clone();
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": amp, "parameter": "Right Gain", "value": (-9.0 + 96.0) / 144.0}),
    )
    .unwrap();
    s.update();
    let (start, end) = (1_234u64, 25_000u64);
    let heard = s.capture_preview(doc as u32, start, end).unwrap();
    call(
        &r,
        &mut s,
        "apply_audio_effects",
        json!({"document": doc, "start": start, "end": end}),
    )
    .unwrap();
    s.wait_for_apply(Duration::from_secs(20)).unwrap();
    let applied = s.document(doc as u32).unwrap().audio.clone();
    for (c, heard) in heard.iter().enumerate() {
        assert_eq!(
            applied.channels[c][start as usize..end as usize],
            heard[..],
            "channel {c}"
        );
    }
    // And it did something: the right channel is 9 dB down, the left untouched.
    assert_eq!(applied.channels[0], left);
    assert_ne!(applied.channels[1], right);
}

#[test]
fn a_native_effects_look_ahead_is_compensated_as_it_changes() {
    let r = Registry::new();
    let mut s = headless();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let added = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Dynamics"}),
    )
    .unwrap();
    // 3 ms at 48 kHz.
    assert_eq!(added["latency"], 144);
    let insert = added["insert"].as_u64().unwrap() as u32;
    // 10 ms of 0..500: the latency follows at the next update, before any audio has run.
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert, "parameter": "Lookahead Time", "value": 0.02}),
    )
    .unwrap();
    s.update();
    assert_eq!(s.insert_info(insert).map(|i| i.1), Some(480));
    // A Multiband Compressor's delay follows its crossovers: lower bass, more delay.
    let multi = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Multiband Compressor"}),
    )
    .unwrap();
    assert_eq!(multi["latency"], 693);
    let multi = multi["insert"].as_u64().unwrap() as u32;
    // Crossover 1 to 60 Hz: (60 − 20) / 19980 of its range.
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": multi, "parameter": "Crossover 1", "value": 40.0 / 19_980.0}),
    )
    .unwrap();
    s.update();
    let deeper = s.insert_info(multi).map(|i| i.1).unwrap();
    assert!(deeper > 693, "{deeper}");
}

/// A stereo file: steady white noise at about -35 dBFS RMS, alone for `quiet` samples, then
/// a 1 kHz sine at -12 dBFS over it.
fn noisy_tone_file(dir: &std::path::Path, n: usize, quiet: usize) -> PathBuf {
    let path = dir.join("noisy.wav");
    let channel = |seed: u64| -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let hiss = ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 0.031;
                let tone = if i >= quiet {
                    0.25 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin()
                } else {
                    0.0
                };
                (hiss + tone) as f32
            })
            .collect()
    };
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: 48_000,
            channels: vec![channel(11), channel(12)],
        },
    )
    .unwrap();
    path
}

#[test]
fn a_noise_print_captured_from_a_selection_reduces_the_noise_and_keeps_the_tone() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("noise-print");
    let file = noisy_tone_file(&dir, 4 * 48_000, 2 * 48_000);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    // No insert named: the rack gets a Noise Reduction, which learns the selection.
    let captured = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 72_000}),
    )
    .unwrap();
    assert_eq!(captured["effect"], "Noise Reduction");
    let nr = id(&captured, "insert");
    assert_eq!(s.editor_rack().len(), 1);
    // Capturing again reuses it, named or not, and is no step of the project's history.
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    let again = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 72_000}),
    )
    .unwrap();
    assert_eq!(id(&again, "insert"), nr);
    let named = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 72_000, "insert": nr}),
    )
    .unwrap();
    assert_eq!(id(&named, "insert"), nr);
    assert_eq!(s.editor_rack().len(), 1);
    assert_eq!(
        call(&r, &mut s, "get_history", Value::Null).unwrap(),
        history
    );
    assert!(s.native(nr as u32).unwrap().print().is_some());
    s.update();
    call(&r, &mut s, "apply_audio_effects", json!({"document": doc})).unwrap();
    s.wait_for_apply(Duration::from_secs(30)).unwrap();
    let before = wav::read(&file).unwrap();
    let after = s.document(doc as u32).unwrap().audio.clone();
    let rms_db = |x: &[f32]| {
        let p = x.iter().map(|v| f64::from(*v).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * p.max(1e-30).log10()
    };
    let tone_db = |x: &[f32]| {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, v) in x.iter().enumerate() {
            let p = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0;
            re += f64::from(*v) * p.cos();
            im += f64::from(*v) * p.sin();
        }
        20.0 * ((re * re + im * im).sqrt() * 2.0 / x.len() as f64).log10()
    };
    for c in 0..2 {
        // The noise alone: Reduce By's 40 dB, a little deeper for steady noise.
        let noise = 10_000..80_000;
        let cut = rms_db(&before.channels[c][noise.clone()]) - rms_db(&after.channels[c][noise]);
        assert!(cut > 38.0, "channel {c}: noise down {cut:.1} dB");
        let tone = 110_000..180_000;
        let kept = tone_db(&after.channels[c][tone.clone()]) - tone_db(&before.channels[c][tone]);
        assert!(kept.abs() < 0.5, "channel {c}: tone moved {kept:.2} dB");
    }

    // Into a track's insert by id; it is saved with the project.
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let on_track = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Noise Reduction"}),
    )
    .unwrap()["insert"]
        .clone();
    assert_eq!(
        call(
            &r,
            &mut s,
            "capture_noise_print",
            json!({"document": doc, "start": 0, "end": 72_000, "insert": on_track}),
        )
        .unwrap()["insert"],
        on_track
    );
    let print = s
        .native(on_track.as_u64().unwrap() as u32)
        .unwrap()
        .print()
        .map(<[f32]>::to_vec);
    let path = dir.join("print.dawproj");
    call(&r, &mut s, "save_project", json!({"path": path})).unwrap();
    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": path})).unwrap();
    let reopened = s
        .native(on_track.as_u64().unwrap() as u32)
        .unwrap()
        .print()
        .map(<[f32]>::to_vec);
    assert!(print.is_some());
    assert_eq!(print, reopened);

    // What can't learn one, and ranges outside the file, are refused by name.
    let amp = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": "Amplify"}),
    )
    .unwrap()["insert"]
        .clone();
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let err = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 72_000, "insert": amp}),
    )
    .unwrap_err();
    assert!(err.contains("doesn't learn from audio"), "{err}");
    let err = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 10_000_000}),
    )
    .unwrap_err();
    assert!(err.contains("isn't a range"), "{err}");
    let err = call(
        &r,
        &mut s,
        "capture_noise_print",
        json!({"document": doc, "start": 0, "end": 1000}),
    )
    .unwrap_err();
    assert!(err.contains("select more noise"), "{err}");
}

#[test]
fn trimming_a_clip_hides_and_restores_its_audio_in_place() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("trim");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap(),
        "clip",
    );
    let full = mixdown(&r, &mut s, &dir, "full.wav");
    // Trim both ends: the audio between stays exactly where it was, the rest is silent.
    let trimmed = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "trim_start": 20_000, "trim_end": 30_000}),
    )
    .unwrap();
    assert_eq!(
        (trimmed["start"].as_u64(), trimmed["length"].as_u64()),
        (Some(20_000), Some(10_000))
    );
    let cut = mixdown(&r, &mut s, &dir, "cut.wav");
    assert_eq!(cut[20_000..30_000], full[20_000..30_000]);
    assert!(cut[..20_000]
        .iter()
        .chain(&cut[30_000..])
        .all(|v| *v == 0.0));
    // Out again to the file's ends: the whole thing back, bit for bit.
    call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "trim_start": 0, "trim_end": 48_000}),
    )
    .unwrap();
    assert_eq!(mixdown(&r, &mut s, &dir, "back.wav"), full);
    // Not past the audio, either way.
    let err = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "trim_end": 48_001}),
    )
    .unwrap_err();
    assert!(err.contains("ends at sample 48000"), "{err}");
    call(
        &r,
        &mut s,
        "move_clip",
        json!({"clip": clip, "start": 1000}),
    )
    .unwrap();
    let err = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": clip, "trim_start": 999}),
    )
    .unwrap_err();
    assert!(err.contains("starts at sample 1000"), "{err}");
    // A split piece reaches back into the audio its neighbour plays.
    let split = call(
        &r,
        &mut s,
        "split_clip",
        json!({"clip": clip, "position": 25_000}),
    )
    .unwrap();
    let second = split["second"]["clip"].as_u64().unwrap();
    let pulled = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": second, "trim_start": 15_000}),
    )
    .unwrap();
    assert_eq!(pulled["offset"], 14_000);
    // A stretched clip trims and restores within its render (twice as long: 96000 samples,
    // its first at start − offset on the timeline): its end reaches the render's end.
    let stretched = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": second, "stretch": 2.0}),
    )
    .unwrap();
    s.wait_for_stretches(Duration::from_secs(60)).unwrap();
    let (start, offset) = (
        stretched["start"].as_u64().unwrap(),
        stretched["offset"].as_u64().unwrap(),
    );
    let audio_end = start + 96_000 - offset;
    let view = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": second, "trim_end": audio_end}),
    )
    .unwrap();
    assert_eq!(view["length"].as_u64(), Some(audio_end - start));
    let err = call(
        &r,
        &mut s,
        "set_clip",
        json!({"clip": second, "trim_end": audio_end + 1}),
    )
    .unwrap_err();
    assert!(
        err.contains(&format!("ends at sample {audio_end}")),
        "{err}"
    );
    // Undo takes a trim back.
    call(&r, &mut s, "undo", Value::Null).unwrap();
    let back = &call(&r, &mut s, "get_project", Value::Null).unwrap()["tracks"][0]["clips"];
    assert!(back
        .as_array()
        .unwrap()
        .iter()
        .any(|c| c["id"] == second && c["length"] != 96_000));
}

/// Opening behind the loading bar (17.1): a staged open returns before any of the project's
/// files is decoded, the files are read on workers (each once, none on the opening thread),
/// the bar counts the files and then the plug-ins, a file that won't read is said once at the
/// end, and a cancel goes back to the project open before (then, cancelled again, to none).
#[test]
fn opening_reads_files_on_workers_and_cancels_back() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("worker-open");
    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Workers", "folder": dir}),
    )
    .unwrap();
    let path = PathBuf::from(made["path"].as_str().unwrap());
    let project_dir = path.parent().unwrap().to_path_buf();
    let mut clips = Vec::new();
    for k in 0..5 {
        let file = dir.join(format!("take {k}.wav"));
        let samples = (0..24_000)
            .map(|i| 0.1 * (i as f32 * 0.01 * (k + 1) as f32).sin())
            .collect();
        let audio = wav::Audio {
            sample_rate: 48_000,
            channels: vec![samples],
        };
        wav::write_f32(&file, &audio).unwrap();
        let track = id(
            &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        let import = json!({"track": track, "path": file, "position": 0});
        clips.push(id(
            &call(&r, &mut s, "import_audio", import).unwrap(),
            "clip",
        ));
        if k < 2 {
            let insert = json!({"track": track, "plugin": "amplify"});
            call(&r, &mut s, "add_insert", insert).unwrap();
        }
    }
    call(&r, &mut s, "save_project", Value::Null).unwrap();
    let ours =
        |loads: &[(std::thread::ThreadId, PathBuf)]| -> Vec<(std::thread::ThreadId, PathBuf)> {
            loads
                .iter()
                .filter(|(_, p)| p.starts_with(&project_dir))
                .cloned()
                .collect()
        };
    let before = ours(&crate::media::LOADS.lock().unwrap()).len();

    let mut o = headless();
    o.stage_loads = true;
    o.hold_file_reads(Some(0));
    o.hold_loading(true);
    o.open(&path).unwrap();
    let progress = o.loading().expect("opening");
    assert_eq!(progress.stage, LoadStage::Reading);
    assert_eq!((progress.done, progress.total), (0, 5 + 2));
    assert_eq!((progress.stage_done, progress.stage_total), (0, 5));
    assert_eq!(progress.next.as_deref(), Some("take 0.wav"));
    assert_eq!(
        ours(&crate::media::LOADS.lock().unwrap()).len(),
        before,
        "the open decoded a file before returning"
    );
    let busy = call(&r, &mut o, "get_busy", Value::Null).unwrap();
    assert_eq!(busy["loading"]["stage"], "reading");
    assert_eq!(busy["loading"]["stage_total"], 5);

    // Let the files go: they arrive while the plug-ins wait, then the plug-ins.
    o.hold_file_reads(None);
    let started = Instant::now();
    let mut stages = vec![LoadStage::Reading];
    while let Some(progress) = o.loading() {
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "stuck: {progress:?}"
        );
        if stages.last() != Some(&progress.stage) {
            stages.push(progress.stage);
        }
        if progress.stage == LoadStage::Loading {
            assert_eq!((progress.done, progress.stage_total), (5, 2));
            assert_eq!(progress.next.as_deref(), Some("Amplify"));
            o.hold_loading(false);
        }
        o.update();
    }
    assert_eq!(stages, [LoadStage::Reading, LoadStage::Loading]);
    let loads = ours(&crate::media::LOADS.lock().unwrap());
    let here = std::thread::current().id();
    assert!(
        loads.iter().all(|(thread, _)| *thread != here),
        "a file was decoded on the opening thread: {loads:?}"
    );
    for k in 0..5 {
        let name = format!("take {k}.wav");
        let reads = loads.iter().filter(|(_, p)| p.ends_with(&name)).count();
        assert_eq!(reads, 1, "{name} read {reads} times");
    }
    for &clip in &clips {
        let clip = o.project.find_clip(clip as u32).expect("clip").1.clone();
        assert!(
            o.clip_audio(&clip).is_some(),
            "clip {} has no audio",
            clip.id
        );
    }
    assert!(o.notices.is_empty(), "{:?}", o.notices);

    // A file gone: said once, at the end.
    let gone = project_dir.join("Imported").join("take 3.wav");
    std::fs::remove_file(&gone).unwrap();
    o.stage_loads = false;
    o.open(&path).unwrap();
    let said: Vec<_> = o.notices.iter().filter(|n| n.contains("take 3")).collect();
    assert_eq!(said.len(), 1, "{:?}", o.notices);
    o.notices.clear();

    // Cancel: back to the project open before, then (cancelled again) to none.
    let other = dir.join("Other.oxide");
    call(&r, &mut s, "save_project", json!({"path": other})).unwrap();
    o.stage_loads = true;
    o.hold_file_reads(Some(0));
    o.open(&other).unwrap();
    assert_eq!(o.path.as_deref(), Some(other.as_path()));
    assert_eq!(o.cancel_loading().unwrap().as_deref(), Some(path.as_path()));
    assert_eq!(o.path.as_deref(), Some(path.as_path()));
    assert!(o.loading().is_some(), "the reopen loads behind the bar too");
    assert_eq!(o.cancel_loading().unwrap(), None);
    assert!(o.path.is_none() && o.project.tracks.is_empty());
    assert!(o.loading().is_none());
    let err = call(&r, &mut o, "cancel_open", Value::Null).unwrap_err();
    assert!(err.contains("no project is being opened"), "{err}");

    // Through the action: the project before back whole, its plug-ins in.
    o.stage_loads = false;
    o.open(&path).unwrap();
    o.stage_loads = true;
    o.open(&other).unwrap();
    let back = call(&r, &mut o, "cancel_open", Value::Null).unwrap();
    assert_eq!(
        back["reopened"].as_str().map(PathBuf::from),
        Some(path.clone())
    );
    assert!(o.loading().is_none());
    assert_eq!(o.project.tracks.len(), 5);
    let amp = o.project.tracks[0].inserts[0].id;
    assert!(
        o.insert_info(amp).is_some(),
        "the reopened project's plug-in"
    );
}

/// The Waveform view's files reopen with the project: a save writes the open files (inside the
/// project's folder relative, the clip one was opened from, the one showing, the view in
/// front), opening or closing a file is neither an undo step nor an unsaved change, an open
/// reads them on the workers (none before it returns, none on the opening thread) and brings
/// them back in order with the same one showing; a file gone is skipped with one notice; a
/// mixdown (in `Rendered/`) comes back too; a Save As into another folder keeps them as
/// absolute paths; a backup holds them, a template doesn't; a cancel goes back to the project
/// before with its files; and a file without the list opens as before.
#[test]
fn waveform_view_files_reopen_with_the_project() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("reopen-docs");
    s.templates_dir = Some(dir.join("Templates"));
    let made = call(
        &r,
        &mut s,
        "new_project",
        json!({"name": "Docs", "folder": dir}),
    )
    .unwrap();
    let path = PathBuf::from(made["path"].as_str().unwrap());
    let project_dir = path.parent().unwrap().to_path_buf();
    let write = |file: &std::path::Path, rate: u32, scale: f32| {
        let samples = (0..12_000)
            .map(|i| scale * (i as f32 * 0.01).sin())
            .collect();
        let audio = wav::Audio {
            sample_rate: rate,
            channels: vec![samples],
        };
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        wav::write_f32(file, &audio).unwrap();
    };
    // One in the project's folder, one from outside it (opened as its copy in Imported), at
    // its own rate, and one opened from a clip.
    let inside = project_dir.join("Rendered").join("inside.wav");
    write(&inside, 48_000, 0.2);
    let outside = dir.join("outside.wav");
    write(&outside, 44_100, 0.3);
    let take = dir.join("take.wav");
    write(&take, 48_000, 0.4);
    let extra_file = dir.join("extra.wav");
    write(&extra_file, 48_000, 0.5);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let import = json!({"track": track, "path": take, "position": 0});
    let clip = id(&call(&r, &mut s, "import_audio", import).unwrap(), "clip");
    call(&r, &mut s, "save_project", Value::Null).unwrap();
    let undo_before = call(&r, &mut s, "get_history", Value::Null).unwrap();
    let open = |s: &mut Session, args: Value| {
        id(&call(&r, s, "open_audio_editor", args).unwrap(), "document") as u32
    };
    let a = open(&mut s, json!({"path": inside}));
    let b = open(&mut s, json!({"path": outside}));
    let c = open(&mut s, json!({"clip": clip}));
    let extra = open(&mut s, json!({"path": extra_file}));
    call(&r, &mut s, "close_audio_editor", json!({"document": extra})).unwrap();
    // Opening and closing files: no undo step, nothing unsaved.
    assert_eq!(
        call(&r, &mut s, "get_history", Value::Null).unwrap(),
        undo_before
    );
    assert!(
        !s.has_unsaved_changes(),
        "opening files is no change to save"
    );
    let ids: Vec<u32> = s.documents().iter().map(|d| d.id).collect();
    assert_eq!(ids, [a, b, c]);
    let paths: Vec<PathBuf> = s.documents().iter().map(|d| d.path.clone()).collect();
    assert_eq!(paths[0], inside);
    assert!(
        paths[1].starts_with(project_dir.join("Imported")),
        "{paths:?}"
    );
    // The app tells the session what the Waveform view shows (here the outside file's copy).
    s.set_document_view(Some(b), true);
    call(&r, &mut s, "save_project", Value::Null).unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let files = &saved["open_files"];
    let relative = |p: &std::path::Path| {
        p.strip_prefix(&project_dir)
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    assert_eq!(files["files"][0]["path"], relative(&paths[0]));
    assert_eq!(files["files"][1]["path"], relative(&paths[1]));
    assert_eq!(files["files"][2]["path"], relative(&paths[2]));
    assert_eq!(files["files"][2]["clip"], clip);
    assert!(files["files"][0].get("clip").is_none());
    assert_eq!(files["shown"], 1);
    assert_eq!(files["in_front"], true);
    assert!(
        !s.has_unsaved_changes(),
        "the save's list is no change either"
    );
    assert!(
        s.project.open_files.is_empty(),
        "the list stays out of the project"
    );
    // A backup holds the list too; a template doesn't.
    let backup = call(&r, &mut s, "save_backup", Value::Null).unwrap();
    let backup = PathBuf::from(backup["path"].as_str().unwrap());
    let backed: Value = serde_json::from_str(&std::fs::read_to_string(&backup).unwrap()).unwrap();
    assert_eq!(backed["open_files"], *files);
    let template = call(&r, &mut s, "save_template", json!({"name": "Docs"})).unwrap();
    let template = std::fs::read_to_string(template["path"].as_str().unwrap()).unwrap();
    assert!(!template.contains("open_files"), "{template}");

    // Reopened behind the bar: nothing read before `open` returns, the files on workers.
    let here = std::thread::current().id();
    let ours = || -> Vec<(std::thread::ThreadId, PathBuf)> {
        crate::media::LOADS
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, p)| paths.contains(p))
            .cloned()
            .collect()
    };
    let before = ours().len();
    let mut o = headless();
    o.stage_loads = true;
    o.hold_file_reads(Some(0));
    o.open(&path).unwrap();
    let progress = o.loading().expect("opening");
    assert_eq!(progress.stage, LoadStage::Reading);
    assert_eq!(progress.stage_total, 1 + 3, "the take and the three files");
    assert!(o.documents().is_empty(), "the files come back at the end");
    assert_eq!(ours().len(), before, "a file was read before open returned");
    o.hold_file_reads(None);
    let started = Instant::now();
    while o.loading().is_some() {
        assert!(started.elapsed() < Duration::from_secs(30), "stuck");
        o.update();
    }
    // The take once for its clip (at the engine's rate), and the three files.
    let reads: Vec<_> = ours().into_iter().skip(before).collect();
    assert_eq!(reads.len(), 1 + 3, "{reads:?}");
    assert!(
        reads.iter().all(|(thread, _)| *thread != here),
        "a file was read on the opening thread: {reads:?}"
    );
    assert!(o.notices.is_empty(), "{:?}", o.notices);
    let back: Vec<PathBuf> = o.documents().iter().map(|d| d.path.clone()).collect();
    assert_eq!(back, paths, "in their order");
    let docs = o.documents();
    assert_eq!(docs[1].audio.sample_rate, 44_100, "at its own rate");
    assert_eq!(docs[2].source, Some((track as u32, clip as u32)));
    assert_eq!(docs[0].source, None);
    let shown = docs[1].id;
    let view = o.take_reopened_view().expect("the view, once");
    assert_eq!(view.shown, Some(shown));
    assert!(view.in_front);
    assert!(o.take_reopened_view().is_none());
    assert!(!o.has_unsaved_changes());

    // A file gone: skipped, said once, the rest back; nothing unsaved.
    std::fs::remove_file(&paths[1]).unwrap();
    o.stage_loads = false;
    o.open(&path).unwrap();
    let said: Vec<_> = o
        .notices
        .iter()
        .filter(|n| n.contains("wasn't reopened in the Waveform view"))
        .collect();
    assert_eq!(said.len(), 1, "{:?}", o.notices);
    o.notices.clear();
    let back: Vec<PathBuf> = o.documents().iter().map(|d| d.path.clone()).collect();
    assert_eq!(back, [paths[0].clone(), paths[2].clone()]);
    let view = o.take_reopened_view().expect("the view");
    assert_eq!(view.shown, Some(o.documents()[0].id), "the first instead");
    assert!(!o.has_unsaved_changes());

    // A mixdown: in Rendered, open, saved with the project and back with it.
    let mixed = call(
        &r,
        &mut o,
        "mixdown_to_new_file",
        json!({"tail_seconds": 0}),
    )
    .unwrap();
    let mixed = PathBuf::from(mixed["path"].as_str().unwrap());
    assert!(mixed.starts_with(project_dir.join("Rendered")), "{mixed:?}");
    o.wait_for_export(Duration::from_secs(30)).unwrap();
    o.update();
    assert!(o.documents().iter().any(|d| d.path == mixed));
    call(&r, &mut o, "save_project", Value::Null).unwrap();
    o.open(&path).unwrap();
    let back: Vec<PathBuf> = o.documents().iter().map(|d| d.path.clone()).collect();
    assert_eq!(back, [paths[0].clone(), paths[2].clone(), mixed.clone()]);

    // Another project closes them; a cancel back reopens the project before with its files.
    let other_dir = dir.join("Other");
    std::fs::create_dir_all(&other_dir).unwrap();
    // A copy of the project: its files missing there, held unread so the open waits.
    let other = other_dir.join("Other.oxide");
    std::fs::copy(&path, &other).unwrap();
    o.stage_loads = true;
    o.hold_file_reads(Some(0));
    o.open(&other).unwrap();
    assert!(o.documents().is_empty(), "the last project's files close");
    assert_eq!(o.cancel_loading().unwrap().as_deref(), Some(path.as_path()));
    o.finish_loading();
    let back: Vec<PathBuf> = o.documents().iter().map(|d| d.path.clone()).collect();
    assert_eq!(back, [paths[0].clone(), paths[2].clone(), mixed.clone()]);
    o.stage_loads = false;
    o.open(&other).unwrap();
    assert!(o.documents().is_empty(), "none of its files are there");
    assert_eq!(o.take_reopened_view().map(|v| v.in_front), Some(false));
    o.notices.clear();
    o.open(&path).unwrap();

    // Saved as into another folder: the files stay where they are, as absolute paths.
    let moved = dir.join("Moved").join("Docs.oxide");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    call(&r, &mut o, "save_project", json!({"path": moved})).unwrap();
    let saved: Value = serde_json::from_str(&std::fs::read_to_string(&moved).unwrap()).unwrap();
    assert_eq!(
        saved["open_files"]["files"][0]["path"]
            .as_str()
            .map(PathBuf::from),
        Some(paths[0].clone())
    );
    let mut m = headless();
    m.open(&moved).unwrap();
    assert_eq!(m.documents().len(), 3);
    assert_eq!(m.documents()[0].path, paths[0]);

    // A project file from before the list opens as before, with no files.
    let mut old: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("open_files");
    let old_path = project_dir.join("Old.oxide");
    std::fs::write(&old_path, serde_json::to_string_pretty(&old).unwrap()).unwrap();
    m.open(&old_path).unwrap();
    assert!(m.documents().is_empty());
    assert_eq!(m.project.tracks.len(), 1);
}

#[test]
fn the_songs_grid_is_set_read_and_undone() {
    let r = Registry::new();
    let mut s = headless();
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(project["grid"], "1/8");
    for grid in ["1/16", "auto", "1/64t", "1/4"] {
        let set = call(&r, &mut s, "set_grid", json!({ "grid": grid })).unwrap();
        assert_eq!(set["grid"], grid);
        let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
        assert_eq!(project["grid"], grid);
    }
    for bad in ["1/2", "beat", "2 bars", "sixteenth"] {
        let e = call(&r, &mut s, "set_grid", json!({ "grid": bad })).unwrap_err();
        assert!(e.contains(bad) && e.contains("auto"), "{e}");
    }
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    // One step: changes of one setting in a row merge, as a slider's do.
    assert_eq!(history["undo"], json!(["Set grid"]));
    call(&r, &mut s, "undo", Value::Null).unwrap();
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    assert_eq!(project["grid"], "1/8");
    call(&r, &mut s, "redo", Value::Null).unwrap();
    call(&r, &mut s, "set_grid", json!({ "grid": "1/64t" })).unwrap();
    // Snapping follows it: a fixed grid needs no zoom, so the session's map snaps to it.
    let map = s.tempo_map();
    let at = |t: f64| t;
    let near = |p| map.snap_grid(p, s.project.grid, &at, crate::grid::Rounding::Nearest);
    // 120 BPM at 48 kHz: a sixty-fourth triplet is 1000 samples.
    assert_eq!(near(1_400), 1_000);
}

/// Dev plan 17.6: an agent reads a MIDI clip as music (bars and beats, names, key, chords)
/// and writes notes back in the same terms; the tick and number forms stay.
#[test]
fn agents_read_midi_as_music_and_write_it_back_in_bars_and_names() {
    let r = Registry::new();
    let mut s = headless();
    let keys = id(
        &call(&r, &mut s, "add_track", json!({"kind": "instrument"})).unwrap(),
        "track",
    );
    // Bars 2 and 3 (4/4 at 120): song ticks 3840 to 11520.
    let made = call(
        &r,
        &mut s,
        "add_midi_clip",
        json!({"track": keys, "bar": 2, "bars": 2, "name": "Chords"}),
    )
    .unwrap();
    let clip = id(&made, "clip");
    // C, Am, F, G, two to a bar, written with positions and names (and one by number and
    // tick, the stable form).
    let chords: [(&str, [&str; 3]); 4] = [
        ("2:1:000", ["C4", "E4", "G4"]),
        ("2:3", ["A3", "C4", "E4"]),
        ("3:1:000", ["F3", "A3", "C4"]),
        ("3.3.000", ["G3", "B3", "D4"]),
    ];
    let mut notes: Vec<Value> = chords
        .iter()
        .flat_map(|(at, pitches)| {
            pitches
                .iter()
                .map(move |p| json!({"at": at, "length": 1800, "pitch": p, "velocity": 96}))
        })
        .collect();
    notes[0] = json!({"tick": 0, "length": 1800, "pitch": 60, "velocity": 96});
    let undo_count = |s: &mut Session| {
        call(&r, s, "get_history", Value::Null).unwrap()["undo"]
            .as_array()
            .unwrap()
            .len()
    };
    let undo_before = undo_count(&mut s);
    let added = call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": notes}),
    )
    .unwrap();
    assert_eq!(added["notes"].as_array().unwrap().len(), 12);
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"][0], "Add notes");
    let read = call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap();
    let ticks: Vec<(u64, u64)> = read["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| (n["tick"].as_u64().unwrap(), n["pitch"].as_u64().unwrap()))
        .collect();
    assert_eq!(
        ticks,
        [
            (0, 60),
            (0, 64),
            (0, 67),
            (1920, 57),
            (1920, 60),
            (1920, 64),
            (3840, 53),
            (3840, 57),
            (3840, 60),
            (5760, 55),
            (5760, 59),
            (5760, 62)
        ]
    );

    // The musical read: a read, so no undo step and no change.
    let d = call(&r, &mut s, "describe_midi_clip", json!({"clip": clip})).unwrap();
    assert_eq!(
        undo_count(&mut s),
        undo_before + 1,
        "only add_notes made a step"
    );
    assert_eq!(
        call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap(),
        read
    );
    assert_eq!(
        (d["clip"].as_u64(), d["track"].as_u64()),
        (Some(clip), Some(keys))
    );
    assert_eq!(
        (&d["start"], &d["end"]),
        (&json!("2:1:000"), &json!("4:1:000"))
    );
    assert_eq!(
        (&d["start_tick"], &d["length"]),
        (&json!(3840), &json!(7680))
    );
    assert_eq!(d["bars"], json!({"first": 2, "last": 3, "length": 2.0}));
    assert_eq!(
        d["meters"],
        json!([{"at": "2:1:000", "tick": 3840, "tempo": 120.0, "time_signature": [4, 4]}])
    );
    assert_eq!(d["key"]["name"], "C major");
    assert_eq!(d["key"]["signature"], "no sharps or flats");
    assert_eq!(
        d["key"]["scale"],
        json!(["C", "D", "E", "F", "G", "A", "B"])
    );
    assert_eq!(d["runner_up"]["name"], "A minor");
    assert!(d["key"]["score"].as_f64() > d["runner_up"]["score"].as_f64());
    assert_eq!(d["progression"], "C Am | F G");
    assert_eq!(d["chords"][1]["at"], "2:3:000");
    assert_eq!(d["chords"][1]["tick"], 1920);
    assert_eq!(d["chords"][1]["tones"], json!(["A", "C", "E"]));
    assert_eq!(d["chords"][3]["quality"], "major");
    assert_eq!(d["range"]["lowest"], "F3");
    assert_eq!(d["range"]["highest"], "G4");
    let first = &d["notes"][0];
    assert_eq!(
        (
            &first["at"],
            &first["name"],
            &first["tick"],
            &first["pitch"]
        ),
        (&json!("2:1:000"), &json!("C4"), &json!(0), &json!(60))
    );
    assert_eq!(d["notes"][11]["at"], "3:3:000");

    // Asked for flats, the names follow; a spelling that isn't one is refused.
    let flat = call(
        &r,
        &mut s,
        "describe_midi_clip",
        json!({"clip": clip, "spelling": "flats"}),
    )
    .unwrap();
    assert_eq!(flat["key"]["name"], "C major");
    assert!(call(
        &r,
        &mut s,
        "describe_midi_clip",
        json!({"clip": clip, "spelling": "both"})
    )
    .is_err());

    // set_notes moves a note by position and renames its pitch.
    let g = d["notes"][11]["id"].as_u64().unwrap();
    let changed = call(
        &r,
        &mut s,
        "set_notes",
        json!({"clip": clip, "notes": [{"note": g, "at": "3:4:240", "pitch": "Bb3"}]}),
    )
    .unwrap();
    assert_eq!(
        (&changed["notes"][0]["tick"], &changed["notes"][0]["pitch"]),
        (&json!(4 * 960 + 3 * 960 + 240), &json!(58))
    );

    // Bad values are named, and nothing is added.
    let refused = |s: &mut Session, note: Value, says: &str| {
        let args = json!({"clip": clip, "notes": [{"tick": 0, "length": 10, "pitch": 60}, note]});
        let err = call(&r, s, "add_notes", args).unwrap_err();
        assert!(err.contains(says), "{err}");
    };
    for (note, says) in [
        (
            json!({"at": "1:4:000", "length": 10, "pitch": 60}),
            "note 1: \"1:4:000\" is before the clip, which starts at 2:1:000",
        ),
        (
            json!({"at": "2:5", "length": 10, "pitch": 60}),
            "note 1: \"2:5\": bar 2 is in 4/4, with beats 1 to 4",
        ),
        (
            json!({"at": "2:1:000", "tick": 0, "length": 10, "pitch": 60}),
            "note 1: give `tick`",
        ),
        (
            json!({"length": 10, "pitch": 60}),
            "note 1: give `tick` (from the clip's start) or `at`",
        ),
        (
            json!({"tick": 0, "length": 10, "pitch": "H2"}),
            "note 1: \"H2\" is not a note name",
        ),
        (
            json!({"tick": 0, "length": 10, "pitch": "E"}),
            "\"E\" has no octave",
        ),
        (
            json!({"tick": 0, "length": 10, "pitch": 200}),
            "note 1: pitch 200 is outside 0..127",
        ),
        (
            json!({"tick": 0, "length": 10, "pitch": 128}),
            "note 1: pitch 128 is outside 0..127",
        ),
    ] {
        refused(&mut s, note, says);
    }
    let count = |s: &mut Session| {
        call(&r, s, "get_midi_clip", json!({"clip": clip})).unwrap()["notes"]
            .as_array()
            .unwrap()
            .len()
    };
    assert_eq!(count(&mut s), 12);

    // A meter change inside the clip: 3/4 from bar 3. The clip keeps its ticks; its second
    // bar's notes now read in 3/4, and positions written there resolve through it.
    call(
        &r,
        &mut s,
        "add_tempo_change",
        json!({"bar": 3, "bpm": 90, "numerator": 3, "denominator": 4}),
    )
    .unwrap();
    let map = call(&r, &mut s, "get_tempo_map", Value::Null).unwrap();
    assert_eq!(map["changes"][0]["tick"], 7680);
    let d = call(&r, &mut s, "describe_midi_clip", json!({"clip": clip})).unwrap();
    assert_eq!(
        d["meters"],
        json!([
            {"at": "2:1:000", "tick": 3840, "tempo": 120.0, "time_signature": [4, 4]},
            {"at": "3:1:000", "tick": 7680, "tempo": 90.0, "time_signature": [3, 4]}
        ])
    );
    // Song tick 9600 is two beats into bar 3; the clip's end (11520) is a beat into bar 4.
    assert_eq!(d["notes"][9]["at"], "3:3:000");
    assert_eq!(d["end"], "4:2:000");
    assert_eq!(
        (d["bars"]["first"].as_i64(), d["bars"]["last"].as_i64()),
        (Some(2), Some(4))
    );
    let late = call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [{"at": "4:1:000", "length": 240, "pitch": "C5"}]}),
    )
    .unwrap();
    let late = late["notes"][0].as_u64().unwrap();
    let read = call(&r, &mut s, "get_midi_clip", json!({"clip": clip})).unwrap();
    let tick = read["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["id"] == late)
        .unwrap()["tick"]
        .clone();
    assert_eq!(
        tick,
        json!(3840 + 3 * 960),
        "bar 4 starts 3 quarters after bar 3 in 3/4"
    );
    let err = call(
        &r,
        &mut s,
        "add_notes",
        json!({"clip": clip, "notes": [{"at": "3:4", "length": 1, "pitch": 60}]}),
    )
    .unwrap_err();
    assert!(err.contains("bar 3 is in 3/4"), "{err}");

    // insert_midi_track by bars: a clip from bar 5 for 4 bars (3/4 bars from bar 3).
    let inserted = call(
        &r,
        &mut s,
        "insert_midi_track",
        json!({"bar": 5, "bars": 4, "after": keys}),
    )
    .unwrap();
    let bar5 = 7680 + 2 * 2880;
    assert_eq!(
        (&inserted["clip"]["start"], &inserted["clip"]["length"]),
        (&json!(bar5), &json!(4 * 2880))
    );
    let two = call(&r, &mut s, "insert_midi_track", json!({"bar": 2})).unwrap();
    assert_eq!(
        two["clip"]["length"],
        json!(3840 + 2880),
        "two bars by default: one of 4/4, one of 3/4"
    );
    for (args, says) in [
        (json!({"bar": 5, "start": 0}), "not both"),
        (json!({}), "give `start` (samples) or `bar`"),
        (json!({"bar": 0}), "bars count from 1"),
        (json!({"bar": 2, "end": 96000}), "`end` goes with `start`"),
        (json!({"start": 0, "bars": 2}), "`bars` goes with `bar`"),
    ] {
        let err = call(&r, &mut s, "insert_midi_track", args.clone()).unwrap_err();
        assert!(err.contains(says), "{args}: {err}");
    }
    // The sample form stays: bar 2 starts at 96 000 samples (two seconds at 120).
    let by_samples = call(&r, &mut s, "insert_midi_track", json!({"start": 96000})).unwrap();
    assert_eq!(by_samples["clip"]["start"], 3840);
}

// ------------------------------------------------------------------- the rack kept (18.2)

/// A stereo file of a tone over a little noise, `n` samples at 48 kHz.
fn noisy(dir: &std::path::Path, name: &str, n: usize) -> PathBuf {
    let path = dir.join(name);
    let mut seed = 12_345u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5
    };
    let channels: Vec<Vec<f32>> = (0..2)
        .map(|c| {
            (0..n)
                .map(|i| {
                    let t = i as f32 / 48_000.0;
                    0.05 * noise()
                        + 0.4 * (2.0 * std::f32::consts::PI * (330.0 + 110.0 * c as f32) * t).sin()
                })
                .collect()
        })
        .collect();
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: 48_000,
            channels,
        },
    )
    .unwrap();
    path
}

/// Amplify in the Waveform view's rack with its gains (dB) set. Returns its insert id.
fn amplify(r: &Registry, s: &mut Session, left: f64, right: f64) -> u32 {
    let insert = id(
        &call(
            r,
            s,
            "add_insert",
            json!({"editor": true, "plugin": "Amplify"}),
        )
        .unwrap(),
        "insert",
    );
    for (name, db) in [("Left Gain", left), ("Right Gain", right)] {
        call(
            r,
            s,
            "set_plugin_parameter",
            json!({"insert": insert, "parameter": name, "value": (db + 96.0) / 144.0}),
        )
        .unwrap();
    }
    insert as u32
}

/// Apply the rack to the whole of `doc` and return the file as it is then.
fn apply_whole(s: &mut Session, doc: u64) -> Vec<Vec<f32>> {
    s.update();
    s.apply_effects(doc as u32, None).unwrap();
    s.wait_for_apply(Duration::from_secs(60)).unwrap();
    s.document(doc as u32).unwrap().audio.channels.clone()
}

#[test]
fn the_effects_rack_comes_back_with_its_settings_in_a_new_session() {
    let r = Registry::new();
    let dir = temp_dir("rack-kept");
    let file = noisy(&dir, "noisy.wav", 30_000);
    let kept = dir.join("rack.json");

    let mut s = headless();
    s.rack_file = Some(kept.clone());
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    // Amplify off its defaults, a Parametric EQ bypassed with a band moved, and a
    // Noise Reduction that learned a print.
    amplify(&r, &mut s, 4.0, -7.5);
    let eq = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"editor": true, "plugin": "Parametric EQ"}),
        )
        .unwrap(),
        "insert",
    ) as u32;
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": eq, "parameter": "Band 3 Gain", "value": 0.8}),
    )
    .unwrap();
    s.insert_mut(eq).unwrap().bypass = true;
    s.capture_noise_print(doc as u32, (0, 9_000), None).unwrap();
    s.update();
    let before = s.rack_as_file();
    assert_eq!(before.slots.len(), 3);
    assert!(
        String::from_utf8_lossy(before.slots[2].state.as_deref().unwrap()).contains("print"),
        "the noise print is in its state"
    );
    s.save_rack_now().unwrap();
    assert!(kept.is_file());
    let first = apply_whole(&mut s, doc);

    // A new session, the same file: the same rack, the same settings, the same sound.
    let mut t = headless();
    t.rack_file = Some(kept.clone());
    assert_eq!(t.restore_rack().unwrap(), 3);
    let state = call(&r, &mut t, "get_audio_editor", json!({})).unwrap();
    let rack = state["rack"].as_array().unwrap();
    let names: Vec<&str> = rack.iter().map(|s| s["name"].as_str().unwrap()).collect();
    assert_eq!(names[..2], ["Amplify", "Parametric EQ"]);
    assert_eq!(names[2], before.slots[2].name);
    assert_eq!(
        rack.iter().map(|s| s["bypass"] == true).collect::<Vec<_>>(),
        [false, true, false]
    );
    assert!(rack
        .iter()
        .all(|s| s["loaded"] == true && s["error"].is_null()));
    assert_eq!(t.rack_as_file(), before, "every slot's settings came back");
    let amp_after = t.editor_rack()[0].id;
    let page = call(
        &r,
        &mut t,
        "list_plugin_parameters",
        json!({"insert": amp_after}),
    )
    .unwrap();
    let right = page["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Right Gain")
        .cloned()
        .unwrap();
    assert!(
        right["display"].as_str().unwrap().contains("-7.5"),
        "{right}"
    );
    let doc_t = id(
        &call(&r, &mut t, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    assert_eq!(apply_whole(&mut t, doc_t), first, "it applies as it did");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_kept_rack_slot_that_fails_to_load_keeps_its_settings_and_says_why() {
    let r = Registry::new();
    let dir = temp_dir("rack-missing");
    let kept = dir.join("rack.json");
    let gone = crate::rack::RackSlot {
        name: "Gone Plug-in".into(),
        path: dir.join("Gone Plug-in.vst3"),
        bypass: false,
        state: Some(std::sync::Arc::new(vec![1, 2, 3, 250])),
    };
    let amp = crate::rack::RackSlot {
        name: "Amplify".into(),
        path: crate::native::path_of(&crate::native::amplify::KIND),
        bypass: false,
        state: None,
    };
    std::fs::write(
        &kept,
        serde_json::to_string(&crate::rack::RackFile::new(vec![amp, gone.clone()])).unwrap(),
    )
    .unwrap();

    // Staged, as the app loads it: the slots at once, the plug-ins over the next updates, the
    // project untouched.
    let mut s = headless();
    s.rack_file = Some(kept.clone());
    s.stage_loads = true;
    s.save(&dir.join("p.oxide")).unwrap();
    s.add_track(crate::project::TrackKind::Audio, None);
    assert!(s.has_unsaved_changes());
    assert_eq!(s.restore_rack().unwrap(), 2);
    assert_eq!(s.editor_rack().len(), 2);
    assert!(s
        .loading()
        .is_some_and(|l| l.kind == crate::session::LoadKind::Rack));
    let busy = call(&r, &mut s, "get_busy", Value::Null).unwrap();
    assert_eq!(busy["loading"]["rack"], true, "{busy}");
    assert!(!s.is_loaded(s.editor_rack()[0].id));
    let err = s.cancel_loading().unwrap_err().to_string();
    assert!(err.contains("can't be cancelled"), "{err}");
    let deadline = Instant::now() + Duration::from_secs(20);
    while s.loading().is_some() {
        assert!(Instant::now() < deadline);
        s.update();
    }
    assert!(s.has_unsaved_changes(), "loading the rack saved nothing");
    assert_eq!(s.project.tracks.len(), 1, "nor touched the project");
    let state = call(&r, &mut s, "get_audio_editor", json!({})).unwrap();
    assert_eq!(state["rack"][0]["loaded"], true);
    assert_eq!(state["rack"][1]["loaded"], false);
    assert!(
        state["rack"][1]["error"]
            .as_str()
            .is_some_and(|e| !e.is_empty()),
        "{state}"
    );

    // Kept again (a change elsewhere in the rack): the missing plug-in's settings with it.
    let first = s.editor_rack()[0].id;
    s.insert_mut(first).unwrap().bypass = true;
    s.save_rack_now().unwrap();
    let back = crate::rack::read(&kept).unwrap();
    assert!(back.slots[0].bypass);
    assert_eq!(back.slots[1], gone);

    // A file that doesn't read is put aside, not lost.
    std::fs::write(&kept, "not a rack").unwrap();
    let mut t = headless();
    t.rack_file = Some(kept.clone());
    assert!(t.restore_rack().is_err());
    assert!(dir.join("rack.json.unreadable").is_file());
    assert!(t.editor_rack().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// 21.2: the rack synced with a clip's track over MCP: the sync and the source reported, a
/// parameter and an effect added in the rack reaching the track, Apply's question as a
/// required flag, and off again.
#[test]
fn the_rack_syncs_with_a_clips_track_over_actions() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("rack-sync");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let clip = id(
        &call(
            &r,
            &mut s,
            "import_audio",
            json!({"track": track, "path": file, "position": 0}),
        )
        .unwrap(),
        "clip",
    );
    let amp = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": track, "plugin": "Amplify"}),
        )
        .unwrap(),
        "insert",
    );
    // A file opened by its path has no track to sync with.
    let loose = call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap();
    assert!(loose["source"].is_null());
    let refused = call(
        &r,
        &mut s,
        "set_rack_sync",
        json!({"document": loose["document"], "sync": true}),
    )
    .unwrap_err();
    assert!(refused.contains("wasn't opened from a clip"), "{refused}");
    // Opened from its clip, it knows the clip and its track.
    let doc = call(&r, &mut s, "open_audio_editor", json!({"clip": clip})).unwrap();
    assert_eq!(doc["source"], json!({"track": track, "clip": clip}));
    let doc = doc["document"].clone();
    let synced = call(
        &r,
        &mut s,
        "set_rack_sync",
        json!({"document": doc, "sync": true}),
    )
    .unwrap();
    assert_eq!(synced["slots"].as_array().unwrap().len(), 1);
    assert_eq!(synced["slots"][0]["name"], "Amplify");
    assert_eq!(synced["sync"]["track"], track);
    assert_eq!(synced["sync"]["document"], doc);
    let editor = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    assert_eq!(editor["sync"]["clip"], clip);
    assert_eq!(editor["rack"][0]["track_insert"], amp);
    let copy = editor["rack"][0]["insert"].clone();
    // A parameter set in the rack is the track's too.
    let left = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp})).unwrap()
        ["parameters"][0]["id"]
        .clone();
    call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": copy, "parameter": left, "value": 0.7}),
    )
    .unwrap();
    let shown = call(&r, &mut s, "list_plugin_parameters", json!({"insert": amp})).unwrap();
    assert_eq!(shown["parameters"][0]["value"], 0.7);
    // An effect added to the rack lands on the track as one undo step.
    let steps = call(&r, &mut s, "get_history", Value::Null).unwrap()["undo"]
        .as_array()
        .unwrap()
        .len();
    call(
        &r,
        &mut s,
        "add_insert",
        json!({"editor": true, "plugin": "Limiter"}),
    )
    .unwrap();
    let project = call(&r, &mut s, "get_project", Value::Null).unwrap();
    let on_track = project["tracks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["id"] == track)
        .unwrap()["inserts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["name"].as_str().unwrap().to_string())
        .collect::<Vec<_>>();
    assert_eq!(on_track, ["Amplify", "Limiter"]);
    let history = call(&r, &mut s, "get_history", Value::Null).unwrap();
    assert_eq!(history["undo"].as_array().unwrap().len(), steps + 1);
    // Apply asks first: the flag is required while synced.
    let asked = call(&r, &mut s, "apply_audio_effects", json!({"document": doc})).unwrap_err();
    assert!(asked.contains("turn_off_track"), "{asked}");
    call(
        &r,
        &mut s,
        "apply_audio_effects",
        json!({"document": doc, "turn_off_track": true}),
    )
    .unwrap();
    s.wait_for_apply(std::time::Duration::from_secs(60))
        .unwrap();
    s.update();
    let editor = call(&r, &mut s, "get_audio_editor", Value::Null).unwrap();
    assert!(editor["rack"]
        .as_array()
        .unwrap()
        .iter()
        .all(|slot| slot["bypass"] == true));
    // Off: the rack set aside (empty here) is back.
    let off = call(
        &r,
        &mut s,
        "set_rack_sync",
        json!({"document": doc, "sync": false}),
    )
    .unwrap();
    assert!(off["slots"].as_array().unwrap().is_empty());
    assert!(off["sync"].is_null());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn rack_presets_round_trip_and_bad_names_are_refused() {
    let r = Registry::new();
    let dir = temp_dir("rack-presets");
    let mut s = headless();
    s.rack_presets_dir = Some(dir.join("RackPresets"));
    let listed = call(&r, &mut s, "list_rack_presets", Value::Null).unwrap();
    assert_eq!(listed["presets"], json!([]));
    let err = call(&r, &mut s, "save_rack_preset", json!({"name": "Empty"})).unwrap_err();
    assert!(err.contains("empty"), "{err}");

    amplify(&r, &mut s, -3.0, 2.0);
    let eq = id(
        &call(
            &r,
            &mut s,
            "add_insert",
            json!({"editor": true, "plugin": "Parametric EQ"}),
        )
        .unwrap(),
        "insert",
    ) as u32;
    s.insert_mut(eq).unwrap().bypass = true;
    s.update();
    for (name, says) in [
        ("", "needs a name"),
        ("   ", "needs a name"),
        ("Bass/DI", "'/'"),
        ("What?", "'?'"),
        ("Clean.", "dot"),
        ("con", "device"),
        ("LPT1.mastering", "device"),
    ] {
        let err = call(&r, &mut s, "save_rack_preset", json!({ "name": name })).unwrap_err();
        assert!(err.contains(says), "{name:?}: {err}");
    }
    let saved = call(&r, &mut s, "save_rack_preset", json!({"name": "Clean DI"})).unwrap();
    let path = PathBuf::from(saved["path"].as_str().unwrap());
    assert_eq!(path, dir.join("RackPresets").join("Clean DI.json"));
    let err = call(&r, &mut s, "save_rack_preset", json!({"name": "Clean DI"})).unwrap_err();
    assert!(err.contains("exists already"), "{err}");
    call(
        &r,
        &mut s,
        "save_rack_preset",
        json!({"name": "Clean DI", "replace": true}),
    )
    .unwrap();
    let listed = call(&r, &mut s, "list_rack_presets", Value::Null).unwrap();
    assert_eq!(listed["presets"][0]["name"], "Clean DI");
    assert_eq!(listed["presets"].as_array().unwrap().len(), 1);
    let as_saved = s.rack_as_file();

    // Changed, then loaded back: the preset's slots replace the rack's, settings and all.
    let old: Vec<u32> = s.editor_rack().iter().map(|s| s.id).collect();
    amplify(&r, &mut s, 9.0, 9.0);
    let loaded = call(&r, &mut s, "load_rack_preset", json!({"name": "Clean DI"})).unwrap();
    let ids: Vec<u32> = loaded["slots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["insert"].as_u64().unwrap() as u32)
        .collect();
    assert_eq!(ids.len(), 2);
    assert!(ids.iter().all(|id| !old.contains(id)), "new slots");
    assert!(
        old.iter().all(|id| !s.is_loaded(*id)),
        "the old plug-ins went"
    );
    assert_eq!(loaded["slots"][1]["name"], "Parametric EQ");
    assert_eq!(loaded["slots"][1]["loaded"], true);
    assert_eq!(s.rack_as_file(), as_saved);

    let err = call(&r, &mut s, "load_rack_preset", json!({"name": "Nope"})).unwrap_err();
    assert!(err.contains("no rack preset called \"Nope\""), "{err}");
    call(
        &r,
        &mut s,
        "delete_rack_preset",
        json!({"name": "Clean DI"}),
    )
    .unwrap();
    let listed = call(&r, &mut s, "list_rack_presets", Value::Null).unwrap();
    assert_eq!(listed["presets"], json!([]));
    let err = call(
        &r,
        &mut s,
        "delete_rack_preset",
        json!({"name": "Clean DI"}),
    )
    .unwrap_err();
    assert!(err.contains("no rack preset"), "{err}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_apply_ignores_a_bypass_and_a_removal_made_while_it_runs() {
    let r = Registry::new();
    let dir = temp_dir("rack-apply-snapshot");
    let file = noisy(&dir, "a.wav", 48_000);
    let copy = dir.join("b.wav");
    std::fs::copy(&file, &copy).unwrap();
    // Three Amplifies: what all three make, and what the third alone makes.
    let rack = |r: &Registry, s: &mut Session| {
        [
            amplify(r, s, 6.0, 0.0),
            amplify(r, s, 0.0, -9.0),
            amplify(r, s, -2.5, 1.5),
        ]
    };
    let mut all = headless();
    rack(&r, &mut all);
    let doc = id(
        &call(&r, &mut all, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let expected_all = apply_whole(&mut all, doc);
    let mut third = headless();
    let [a, b, _] = rack(&r, &mut third);
    third.insert_mut(a).unwrap().bypass = true;
    third.remove_insert(b);
    let doc = id(
        &call(&r, &mut third, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    let expected_third = apply_whole(&mut third, doc);
    assert_ne!(expected_all, expected_third);

    // The Apply held before it renders; meanwhile the first slot is bypassed and the second
    // removed, as clicks would.
    let mut s = headless();
    let [a, b, _] = rack(&r, &mut s);
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": file})).unwrap(),
        "document",
    );
    s.update();
    s.hold_apply(true);
    s.apply_effects(doc as u32, None).unwrap();
    s.insert_mut(a).unwrap().bypass = true;
    s.update();
    call(&r, &mut s, "remove_insert", json!({"insert": b})).unwrap();
    assert_eq!(s.editor_rack().len(), 2, "removed from the rack at once");
    assert!(!s.is_loaded(b));
    s.hold_apply(false);
    s.wait_for_apply(Duration::from_secs(60)).unwrap();
    assert!(
        s.notices.iter().all(|n| !n.contains("Apply")),
        "{:?}",
        s.notices
    );
    assert_eq!(
        s.document(doc as u32).unwrap().audio.channels,
        expected_all,
        "that Apply rendered the rack as it was when it started"
    );
    // The next one takes the changes.
    let doc = id(
        &call(&r, &mut s, "open_audio_editor", json!({"path": copy})).unwrap(),
        "document",
    );
    assert_eq!(apply_whole(&mut s, doc), expected_third);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn opening_a_project_keeps_the_effects_racks_settings() {
    let r = Registry::new();
    let dir = temp_dir("rack-over-open");
    let mut s = headless();
    s.save(&dir.join("p.oxide")).unwrap();
    amplify(&r, &mut s, 5.0, -4.0);
    s.update();
    // An open lets go of every plug-in, the rack's too, and loads them back.
    call(
        &r,
        &mut s,
        "open_project",
        json!({"path": dir.join("p.oxide")}),
    )
    .unwrap();
    // (Its settings read for the first time after the open: nothing read them before.)
    let insert = s.editor_rack()[0].id;
    let page = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({ "insert": insert }),
    )
    .unwrap();
    let right = page["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["name"] == "Right Gain")
        .cloned()
        .unwrap();
    assert!(right["display"].as_str().unwrap().contains("-4"), "{right}");
    let _ = std::fs::remove_dir_all(dir);
}

/// A track with the one-second tone at 0 and Amplify on it at +6 dB (left gain): its id and
/// Amplify's.
fn amplified_track(r: &Registry, s: &mut Session, dir: &std::path::Path) -> (u64, u64) {
    let file = tone(dir);
    let track = id(
        &call(r, s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    let import = json!({"track": track, "path": file, "position": 0});
    call(r, s, "import_audio", import).unwrap();
    let args = json!({"track": track, "plugin": "amplify"});
    let amp = id(&call(r, s, "add_insert", args).unwrap(), "insert");
    let params = call(r, s, "list_plugin_parameters", json!({"insert": amp})).unwrap();
    let left = params["parameters"][0]["id"].clone();
    let plus_6 = (6.0 + 96.0) / 144.0;
    let set = json!({"insert": amp, "parameter": left, "value": plus_6});
    call(r, s, "set_plugin_parameter", set).unwrap();
    (track, amp)
}

/// Updates (a frame each, a few milliseconds apart for the workers' reads) until nothing
/// loads; how many it took.
fn until_loaded(s: &mut Session) -> usize {
    let mut frames = 0;
    while s.loading().is_some() {
        assert!(frames < 10_000, "never loaded: {:?}", s.loading());
        std::thread::sleep(Duration::from_millis(2));
        s.update();
        frames += 1;
    }
    frames
}

/// 18.3: an undo that brings a plug-in back (Amplify, replaced by the Limiter in one step)
/// makes the project the step's at once and returns before the plug-in is in: it loads over
/// the next frames (an `undo` loading, held here as the app holds it until its bar is painted),
/// the plug-in it drops is out of the session at once, and the mix is then Amplify's again,
/// bit for bit. A redo while it loads takes it over: the Limiter is live again at once (no
/// reload) and Amplify never loads; undoing again loads it staged once more.
#[test]
fn an_undo_that_brings_plug_ins_back_loads_them_staged() {
    use crate::session::{InsertTarget, LoadKind};
    let r = Registry::new();
    let mut s = headless();
    s.stage_loads = true;
    let dir = temp_dir("undo-staged");
    let (track, amp) = amplified_track(&r, &mut s, &dir);
    let amplified = mixdown(&r, &mut s, &dir, "amplified.wav");
    let limiter_path = crate::native::path_of(&crate::native::hard_limiter::KIND);
    s.begin_edit("Replace Amplify", None);
    s.remove_insert(amp as u32);
    let limiter = s
        .add_insert(InsertTarget::Track(track as u32), &limiter_path)
        .unwrap();
    s.end_edit();
    s.update();
    assert_ne!(mixdown(&r, &mut s, &dir, "limited.wav"), amplified);

    s.hold_loading(true);
    assert_eq!(s.undo().unwrap(), "Replace Amplify");
    let slots: Vec<u32> = s
        .project
        .track(track as u32)
        .unwrap()
        .inserts
        .iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(slots, [amp as u32], "the step is the project at once");
    let loading = s.loading().expect("Amplify loads staged");
    assert_eq!(loading.kind, LoadKind::Undo);
    assert_eq!(loading.label.as_deref(), Some("Replace Amplify"));
    assert_eq!(
        (loading.stage, loading.stage_total),
        (LoadStage::Loading, 1)
    );
    assert!(s.insert_info(amp as u32).is_none(), "not in yet");
    assert!(
        s.insert_info(limiter).is_none(),
        "the dropped one is out at once"
    );
    assert_eq!(s.history().1, ["Replace Amplify"], "the history is whole");
    // Held (the bar not painted yet): frames go by, nothing loads, and bouncing waits.
    for _ in 0..3 {
        s.update();
    }
    assert!(s.insert_info(amp as u32).is_none());
    let export = json!({"path": dir.join("x.wav")});
    let err = call(&r, &mut s, "export_mixdown", export).unwrap_err();
    assert!(err.contains("still loading"), "{err}");
    let err = s.cancel_loading().unwrap_err().to_string();
    assert!(err.contains("can't be cancelled"), "{err}");

    // A redo while it loads: the Limiter comes straight back, Amplify never loads.
    assert_eq!(s.redo().unwrap(), "Replace Amplify");
    assert!(s.loading().is_none(), "nothing to load: {:?}", s.loading());
    assert!(s.insert_info(limiter).is_some(), "live again, not reloaded");
    assert!(s.insert_info(amp as u32).is_none());
    s.hold_loading(false);
    s.update();
    assert_ne!(mixdown(&r, &mut s, &dir, "limited-again.wav"), amplified);

    // Undo again: staged, then the mix is Amplify's, bit for bit (its +6 dB kept for undo).
    s.hold_loading(true);
    s.undo().unwrap();
    assert!(s.loading().is_some_and(|l| l.kind == LoadKind::Undo));
    s.hold_loading(false);
    let frames = until_loaded(&mut s);
    assert!(frames >= 1);
    assert!(s.insert_info(amp as u32).is_some());
    assert!(s.insert_info(limiter).is_none());
    assert_eq!(mixdown(&r, &mut s, &dir, "amplified-again.wav"), amplified);
    let _ = std::fs::remove_dir_all(dir);
}

/// 18.3: a new sample rate reloads every plug-in and file staged (a `sample_rate` loading):
/// `apply_audio_settings` returns before Amplify is back, positions are at the new rate at
/// once, and once it is in, Amplify has its settings and the file plays at 44.1 kHz.
#[test]
fn a_new_sample_rate_reloads_the_plug_ins_staged() {
    use crate::session::LoadKind;
    let r = Registry::new();
    let mut s = headless();
    s.stage_loads = true;
    let dir = temp_dir("rate-staged");
    let (track, amp) = amplified_track(&r, &mut s, &dir);
    let clip = s.project.track(track as u32).unwrap().clips[0].id;
    let moved = json!({"clip": clip, "start": 48_000});
    call(&r, &mut s, "move_clip", moved).unwrap();
    let shown = |s: &mut Session| {
        call(&r, s, "list_plugin_parameters", json!({"insert": amp})).unwrap()["parameters"][0]
            ["display"]
            .clone()
    };
    let gain = shown(&mut s);
    s.hold_loading(true);
    let mut settings = s.settings.clone();
    settings.sample_rate = Some(44_100);
    s.apply_audio_settings(settings);
    assert_eq!(s.sample_rate(), 44_100);
    let loading = s.loading().expect("the plug-ins reload staged");
    assert_eq!(loading.kind, LoadKind::SampleRate);
    assert!(s.insert_info(amp as u32).is_none(), "not back yet");
    assert_eq!(
        s.project.track(track as u32).unwrap().clips[0].start,
        44_100
    );
    for _ in 0..3 {
        s.update();
    }
    assert!(s.insert_info(amp as u32).is_none(), "held");
    let err = s.undo().unwrap_err().to_string();
    assert!(err.contains("sample rate"), "{err}");
    s.hold_loading(false);
    until_loaded(&mut s);
    assert!(s.insert_info(amp as u32).is_some());
    assert_eq!(shown(&mut s), gain, "its settings came back");
    let clip = s.project.track(track as u32).unwrap().clips[0].clone();
    let (audio, _) = s.clip_audio(&clip).expect("read again");
    assert_eq!(audio.sample_rate, 44_100);
    let _ = std::fs::remove_dir_all(dir);
}

/// 18.3: a track's render runs on export's thread: `render_track` answers at once (as
/// export_mixdown does) while the UI thread goes on, undo waits for it, Cancel ends it with no
/// new track, and a render let finish puts its file on a new track (named in
/// get_export_status) whose clip plays the file's samples exactly, read on the render's thread.
#[test]
fn a_track_render_runs_on_a_worker_and_can_be_cancelled() {
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("render-worker");
    let (track, _) = amplified_track(&r, &mut s, &dir);
    let tracks = s.project.tracks.len();
    s.hold_export(true);
    let args = json!({"track": track, "tail_seconds": 0});
    call(&r, &mut s, "render_track", args.clone()).unwrap();
    let status = call(&r, &mut s, "get_export_status", Value::Null).unwrap();
    assert_eq!(status["state"], "running", "{status}");
    assert_eq!(status["kind"], "render", "{status}");
    let started = Instant::now();
    for _ in 0..20 {
        s.update();
    }
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "the UI thread waited"
    );
    let err = call(&r, &mut s, "undo", Value::Null).unwrap_err();
    assert!(err.contains("render"), "{err}");
    let rate = json!({"sample_rate": 44_100});
    let err = call(&r, &mut s, "set_audio_settings", rate).unwrap_err();
    assert!(err.contains("render"), "{err}");
    call(&r, &mut s, "cancel_export", Value::Null).unwrap();
    let err = s
        .wait_for_export(Duration::from_secs(20))
        .unwrap_err()
        .to_string();
    assert!(err.contains("cancelled"), "{err}");
    assert_eq!(s.project.tracks.len(), tracks, "no new track");
    assert!(s.project.track(track as u32).unwrap().enabled);

    s.hold_export(false);
    let rendered = call(&r, &mut s, "render_track", args).unwrap();
    let path = PathBuf::from(rendered["path"].as_str().unwrap());
    s.wait_for_export(Duration::from_secs(20)).unwrap();
    let status = call(&r, &mut s, "get_export_status", Value::Null).unwrap();
    assert_eq!(status["state"], "finished", "{status}");
    let new = status["track"].as_u64().expect("the new track") as u32;
    let clip = s.project.track(new).unwrap().clips[0].clone();
    let (audio, _) = s.clip_audio(&clip).unwrap();
    let file = wav::read(&path).unwrap();
    assert_eq!(audio.channels, file.channels, "the file's samples exactly");
    assert!(!s.project.track(track as u32).unwrap().enabled);
    let _ = std::fs::remove_dir_all(dir);
}

/// A take draws while it records (21.1). With a virtual device playing a signal into two
/// inputs, `record` starts a mono and a stereo take; between updates their peaks reach the
/// main thread and `get_transport` reports them growing from where recording began. On
/// `stop`, each clip lands where its take was last drawn, as long, on the same peaks bit for
/// bit, so the arrangement swaps one for the other without a jump.
#[test]
fn a_take_draws_while_it_records_and_lands_where_it_was_drawn() {
    let r = Registry::new();
    let mut s = Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        sample_rate: Some(48_000),
        threads: Some(0),
        ..Default::default()
    });
    let dir = temp_dir("recording-takes");
    s.scratch = Some(dir.clone());
    let mut tracks = Vec::new();
    for (channel, stereo) in [(1, false), (0, true)] {
        let track = id(
            &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
            "track",
        );
        call(
            &r,
            &mut s,
            "set_track_input",
            json!({"track": track, "input": {"type": "audio", "channel": channel, "stereo": stereo}}),
        )
        .unwrap();
        call(
            &r,
            &mut s,
            "set_track_mixer",
            json!({"track": track, "arm": true}),
        )
        .unwrap();
        tracks.push(track as u32);
    }
    assert!(
        call(&r, &mut s, "record", Value::Null).is_err(),
        "no inputs yet"
    );
    let mut n = 0u32;
    let signal = Box::new(move |frames: &mut [f32]| {
        for frame in frames.chunks_exact_mut(2) {
            frame[0] = ((n * 37) % 1000) as f32 / 1000.0 - 0.5;
            frame[1] = ((n / 64) % 50) as f32 / 50.0 * if n.is_multiple_of(2) { 1.0 } else { -1.0 };
            n += 1;
        }
    });
    s.start_virtual_input(256, 2, signal).unwrap();
    assert_eq!(s.input_channels(), 2);
    s.locate(4_800).unwrap();
    s.update();
    call(&r, &mut s, "record", Value::Null).unwrap();

    type Drawn = (u32, u64, u64, u64, Vec<Vec<(f32, f32)>>);
    let snapshot = |s: &Session| -> Vec<Drawn> {
        s.recording_takes()
            .iter()
            .map(|t| (t.track, t.start, t.offset, t.length, t.peaks.to_vec()))
            .collect()
    };
    let started = Instant::now();
    let mut lengths = Vec::new();
    let mut reported = None;
    while lengths.last().is_none_or(|&l| l < 19_200) {
        assert!(started.elapsed() < Duration::from_secs(10), "{lengths:?}");
        s.update();
        let drawn = snapshot(&s);
        if let Some(take) = drawn.first() {
            assert_eq!(drawn.len(), 2);
            assert_eq!((take.1, take.2), (4_800, 0), "from where recording began");
            assert_eq!(take.4.len(), 1, "mono");
            assert_eq!(drawn[1].4.len(), 2, "stereo");
            lengths.push(take.3);
            if reported.is_none() {
                reported = Some(call(&r, &mut s, "get_transport", Value::Null).unwrap());
            }
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(
        lengths.windows(2).filter(|w| w[1] > w[0]).count() >= 3,
        "it grew as it recorded: {lengths:?}"
    );
    let reported = reported.unwrap();
    assert_eq!(reported["recording"], true);
    let takes = reported["recording_takes"].as_array().unwrap();
    assert_eq!(takes.len(), 2);
    assert_eq!(takes[0]["track"], tracks[0]);
    assert_eq!(takes[0]["start"], 4_800);
    assert!(takes[0]["length"].as_u64().unwrap() > 0);

    call(&r, &mut s, "stop", Value::Null).unwrap();
    // The last picture of the takes before they become clips.
    let mut last = Vec::new();
    let landed = loop {
        assert!(started.elapsed() < Duration::from_secs(20), "never landed");
        s.update();
        let drawn = snapshot(&s);
        if !drawn.is_empty() {
            last = drawn;
        } else if !s.is_recording() {
            break s.project.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(last.len(), 2);
    for (track, start, offset, length, peaks) in last {
        let clip = &landed.track(track).unwrap().clips[0];
        assert_eq!(
            (clip.start, clip.offset(), clip.length),
            (start, offset, length),
            "track {track}"
        );
        let (data, _) = s.clip_audio(clip).unwrap();
        assert!(
            data.peaks == peaks,
            "track {track}: the file's peaks exactly"
        );
    }
    let transport = call(&r, &mut s, "get_transport", Value::Null).unwrap();
    assert!(transport.get("recording_takes").is_none());
    s.stop_virtual_device();
    let _ = std::fs::remove_dir_all(dir);
}

/// The Record button's count-in (22.3) is the metronome's setting: `Session::set_count_in`,
/// which its menu calls, and the `set_metronome` action leave the same settings and the same
/// undo step.
#[test]
fn the_record_menus_count_in_is_set_metronomes() {
    let r = Registry::new();
    let mut by_menu = headless();
    let mut by_action = headless();
    for bars in [1, 2, 0] {
        // The app's frame is one edit, as `Registry::call` makes the action one.
        by_menu.begin_edit("", None);
        by_menu.set_count_in(bars).unwrap();
        by_menu.end_edit();
        let t = call(
            &r,
            &mut by_action,
            "set_metronome",
            json!({"count_in_bars": bars}),
        )
        .unwrap();
        assert_eq!(t["metronome"]["count_in_bars"], bars);
        assert_eq!(by_menu.project.metronome, by_action.project.metronome);
        assert_eq!(by_menu.project.metronome.count_in_bars, bars);
        assert_eq!(
            by_menu.history().0.first().map(String::as_str),
            Some("Metronome")
        );
    }
    // A metronome change merges with the one before it within the merge window, as the
    // metronome menu's volume slider does; one step at least, never none.
    assert!(!by_menu.history().0.is_empty());
    assert!(by_menu.set_count_in(3).is_err());
    assert_eq!(
        by_menu.project.metronome.count_in_bars, 0,
        "refused, unchanged"
    );
    // Nothing else of the metronome moves.
    by_menu.project.metronome.volume_db = -12.0;
    by_menu.set_count_in(2).unwrap();
    assert_eq!(by_menu.project.metronome.volume_db, -12.0);
}

/// A count-in reported while it plays (22.3): recording from a stop with one bar set, the
/// record point inside bar 1 so the count-in starts before the timeline. `get_transport`'s
/// `count_in` counts the beats down 4, 3, 2, 1 (the display shows the same numbers), the
/// position stays before the record point meanwhile, then it is gone and the take starts at the record point, and
/// lands there on stop.
#[test]
fn a_count_in_counts_down_its_beats_then_the_take_starts_at_the_playhead() {
    count_in_run(false);
    // Behind the Flanger's 5 082 samples of delay compensation: the click is heard that much
    // later, and the lead-in before it still shows 4, never 5.
    count_in_run(true);
}

fn count_in_run(latent: bool) {
    let r = Registry::new();
    let mut s = Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        sample_rate: Some(48_000),
        threads: Some(0),
        ..Default::default()
    });
    let dir = temp_dir(if latent {
        "count-in-latent"
    } else {
        "count-in"
    });
    s.scratch = Some(dir.clone());
    // 240 quarter notes a minute in 4/4: a beat is 12 000 samples, the bar a second.
    call(&r, &mut s, "set_tempo", json!({"bpm": 240})).unwrap();
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    ) as u32;
    call(
        &r,
        &mut s,
        "set_track_input",
        json!({"track": track, "input": {"type": "audio", "channel": 0, "stereo": false}}),
    )
    .unwrap();
    call(
        &r,
        &mut s,
        "set_track_mixer",
        json!({"track": track, "arm": true}),
    )
    .unwrap();
    if latent {
        call(
            &r,
            &mut s,
            "add_insert",
            json!({"track": track, "plugin": "flanger"}),
        )
        .unwrap();
    }
    call(&r, &mut s, "set_metronome", json!({"count_in_bars": 1})).unwrap();
    let signal = Box::new(|frames: &mut [f32]| frames.fill(0.25));
    s.start_virtual_input(256, 1, signal).unwrap();
    s.locate(4_800).unwrap();
    // The engine's thread takes the locate and the graph at its next block: under load, not
    // at once.
    let started = Instant::now();
    let latency = if latent {
        crate::native::flanger::LATENCY as u64
    } else {
        0
    };
    while s.position() != 4_800 || s.record_shift() < latency {
        assert!(started.elapsed() < Duration::from_secs(10), "never located");
        s.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    s.update();
    let t = call(&r, &mut s, "record", Value::Null).unwrap();
    assert_eq!(
        t["count_in"],
        json!({"bars": 1, "beats_left": 4, "record_point": 4_800}),
        "the whole count-in ahead as Record is pressed: {t}"
    );

    let mut beats = Vec::new();
    let take = loop {
        assert!(started.elapsed() < Duration::from_secs(10), "{beats:?}");
        s.update();
        let t = call(&r, &mut s, "get_transport", Value::Null).unwrap();
        if let Some(count_in) = t.get("count_in") {
            assert!(t.get("recording_takes").is_none(), "{t}");
            if t["playing"] == true {
                // Held at 0 before the timeline, then on towards the record point.
                assert!(t["position"].as_u64().unwrap() < 4_800, "{t}");
            }
            let left = count_in["beats_left"].as_u64().unwrap();
            if beats.last() != Some(&left) {
                beats.push(left);
            }
        } else if let Some(take) = t["recording_takes"].get(0) {
            break take.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    // Polled every 5 ms against 250 ms beats; a stall under load could skip one.
    assert!(
        beats.first() == Some(&4)
            && beats.last() == Some(&1)
            && beats.len() >= 3
            && beats.windows(2).all(|w| w[1] < w[0]),
        "counted down from 4 to 1, a beat at a time: {beats:?}"
    );
    assert_eq!(take["start"], 4_800, "the take starts at the playhead");
    assert!(s.count_in_progress().is_none());

    call(&r, &mut s, "stop", Value::Null).unwrap();
    let landed = loop {
        assert!(started.elapsed() < Duration::from_secs(20), "never landed");
        s.update();
        if !s.is_recording() {
            break s.project.clone();
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(landed.track(track).unwrap().clips[0].start, 4_800);
    s.stop_virtual_device();
    let _ = std::fs::remove_dir_all(dir);
}

/// The real manual (docs/manual) built in memory, its search index written as the generator
/// writes it into a folder of its own, for search_manual and open_manual (27.3).
fn manual_dir(name: &str) -> PathBuf {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
    let source = manual::Source::read(&src, &src.join("shots")).unwrap();
    let built = manual::build(
        &source,
        &manual::Options::default(),
        &manual::mcp::Reference::load(),
    );
    assert_eq!(built.errors(), 0, "{:#?}", built.problems);
    let dir = temp_dir(name);
    std::fs::write(
        dir.join(manual::search::JSON_FILE),
        &built.files[manual::search::JSON_FILE],
    )
    .unwrap();
    dir
}

#[test]
fn search_manual_answers_from_the_manuals_pages() {
    let r = Registry::new();
    let mut s = headless();
    // No manual beside the program: says so, and how to build one.
    let e = call(&r, &mut s, "search_manual", json!({ "question": "x" })).unwrap_err();
    assert!(
        e.contains("isn't installed") && e.contains("cargo run -p manual"),
        "{e}"
    );

    let dir = manual_dir("manual-search");
    s.manual_dir = Some(dir.clone());
    let found = call(
        &r,
        &mut s,
        "search_manual",
        json!({ "question": "choppy audio processing block" }),
    )
    .unwrap();
    let results = found["results"].as_array().unwrap();
    assert_eq!(results.len(), 5, "{found:#}");
    // The page about it first (its title has the words), then its section on the block.
    assert_eq!(results[0]["page"], "choppy-audio", "{found:#}");
    let section = results
        .iter()
        .find(|p| p["anchor"].is_string())
        .expect("a section");
    assert_eq!(section["page"], "choppy-audio", "{found:#}");
    assert_eq!(section["anchor"], "raise-the-processing-block", "{found:#}");
    assert_eq!(section["heading"], "Raise the processing block");
    let text = section["text"].as_str().unwrap();
    assert!(text.to_lowercase().contains("processing block"), "{text}");
    assert!(section["snippet"].as_str().unwrap().len() < text.len());
    assert!(section["score"].as_u64().unwrap() >= 3000);
    // `limit`, and a question that matches nothing.
    let one = call(
        &r,
        &mut s,
        "search_manual",
        json!({ "question": "processing block", "limit": 1 }),
    )
    .unwrap();
    assert_eq!(one["results"].as_array().unwrap().len(), 1);
    let none = call(&r, &mut s, "search_manual", json!({ "question": "zzzqqq" })).unwrap();
    assert!(none["results"].as_array().unwrap().is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// A host that notes what open_manual asked to show, as the app's check runs do.
struct ManualHost<'a> {
    session: &'a mut Session,
    shown: Vec<crate::manual::Target>,
}

impl crate::api::Host for ManualHost<'_> {
    fn session(&mut self) -> &mut Session {
        self.session
    }

    fn open_manual(
        &mut self,
        _dir: &std::path::Path,
        target: &crate::manual::Target,
    ) -> crate::api::ApiResult<Value> {
        self.shown.push(target.clone());
        Ok(json!({ "opened": false }))
    }
}

#[test]
fn open_manual_shows_a_page_a_section_or_a_topic() {
    let r = Registry::new();
    let mut s = headless();
    let dir = manual_dir("manual-open");
    s.manual_dir = Some(dir.clone());
    // Headless: there is no window to show it in.
    let e = call(&r, &mut s, "open_manual", Value::Null).unwrap_err();
    assert!(e.contains("app window"), "{e}");

    let mut host = ManualHost {
        session: &mut s,
        shown: Vec::new(),
    };
    let open = |host: &mut ManualHost, args: Value| match r.call_now(host, "open_manual", args) {
        Ok(ActionOutput::Json(v)) => Ok(v),
        Ok(ActionOutput::Png(_)) => Err("an image".to_string()),
        Err(e) => Err(e.0),
    };
    // Nothing given: the first page.
    let first = open(&mut host, Value::Null).unwrap();
    assert_eq!(first["page"], "index");
    assert_eq!(first["title"], "Oxide manual");
    assert_eq!(first["opened"], false);
    assert_eq!(
        first["file"].as_str().map(PathBuf::from),
        Some(dir.join("index.html"))
    );
    // A page and a section, apart or as `page#anchor`.
    let section = open(
        &mut host,
        json!({ "page": "choppy-audio", "anchor": "raise-the-processing-block" }),
    )
    .unwrap();
    assert_eq!(section["heading"], "Raise the processing block");
    let same = open(
        &mut host,
        json!({ "page": "choppy-audio#raise-the-processing-block" }),
    )
    .unwrap();
    assert_eq!(same["anchor"], section["anchor"]);
    // A topic: the best passage.
    let topic = open(
        &mut host,
        json!({ "topic": "how do I raise the processing block" }),
    )
    .unwrap();
    assert_eq!(topic["page"], "choppy-audio", "{topic:#}");
    assert_eq!(topic["anchor"], "raise-the-processing-block", "{topic:#}");
    let shown: Vec<(String, Option<String>)> = host
        .shown
        .iter()
        .map(|t| (t.page.clone(), t.anchor.clone()))
        .collect();
    let block = Some("raise-the-processing-block".to_string());
    assert_eq!(
        shown,
        [
            ("index".to_string(), None),
            ("choppy-audio".to_string(), block.clone()),
            ("choppy-audio".to_string(), block.clone()),
            ("choppy-audio".to_string(), block),
        ]
    );
    // Bad ids and arguments are refused, naming what is there; nothing more is shown.
    let e = open(&mut host, json!({ "page": "no-such-page" })).unwrap_err();
    assert!(
        e.contains("no page 'no-such-page'") && e.contains("choppy-audio"),
        "{e}"
    );
    let e = open(
        &mut host,
        json!({ "page": "choppy-audio", "anchor": "nope" }),
    )
    .unwrap_err();
    assert!(e.contains("no section 'nope'"), "{e}");
    let e = open(&mut host, json!({ "page": "index", "topic": "x" })).unwrap_err();
    assert!(e.contains("not both"), "{e}");
    let e = open(&mut host, json!({ "topic": "zzzqqq" })).unwrap_err();
    assert!(e.contains("nothing in the manual"), "{e}");
    assert_eq!(host.shown.len(), 4);
    // Not an undo step, and no project change.
    assert!(host.session.history().0.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}

/// Built-in effects renamed in 28.5 (`native::RENAMED`): a project saved under the old ids and
/// names opens to today's, renders bit for bit as the same project made with today's ids, and
/// saves only today's. A rack file (a preset, `rack.json`) reads them alike, and `add_insert`
/// takes an old id or name and stores today's.
#[test]
fn projects_with_renamed_effects_open_and_render_as_before() {
    use crate::native::{self, RENAMED};
    let r = Registry::new();
    let mut s = headless();
    let dir = temp_dir("renamed-effects");
    let file = tone(&dir);
    let track = id(
        &call(&r, &mut s, "add_track", json!({"kind": "audio"})).unwrap(),
        "track",
    );
    call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": file, "position": 0}),
    )
    .unwrap();
    // Every renamed effect on the track, each with a setting off its default.
    for renamed in RENAMED {
        let insert = id(
            &call(
                &r,
                &mut s,
                "add_insert",
                json!({"track": track, "plugin": format!("native:{}", renamed.id)}),
            )
            .unwrap(),
            "insert",
        );
        let kind = native::find(renamed.id).unwrap();
        if let Some(i) = kind.params.iter().position(|p| p.automatable) {
            s.set_parameter(insert as u32, i as u32, 0.4).unwrap();
        }
    }
    let today = mixdown(&r, &mut s, &dir, "today.wav");
    let saved = dir.join("today.oxide");
    call(&r, &mut s, "save_project", json!({"path": saved})).unwrap();

    // The same file as an older version wrote it: the old ids and names.
    let mut old = std::fs::read_to_string(&saved).unwrap();
    for renamed in RENAMED {
        let kind = native::find(renamed.id).unwrap();
        if let Some(old_id) = renamed.old_id {
            let new_path = format!("\"native:{}\"", kind.id);
            assert!(old.contains(&new_path), "{new_path} isn't in the file");
            old = old.replace(&new_path, &format!("\"native:{old_id}\""));
        }
        let new_name = format!("\"name\": \"{}\"", kind.name);
        assert!(old.contains(&new_name), "{new_name} isn't in the file");
        old = old.replace(&new_name, &format!("\"name\": \"{}\"", renamed.old_name));
    }
    let old_file = dir.join("old.oxide");
    std::fs::write(&old_file, &old).unwrap();

    call(&r, &mut s, "new_project", Value::Null).unwrap();
    call(&r, &mut s, "open_project", json!({"path": old_file})).unwrap();
    let slots = &s.project.tracks[0].inserts;
    assert_eq!(slots.len(), RENAMED.len());
    for (slot, renamed) in slots.iter().zip(RENAMED) {
        let kind = native::find(renamed.id).unwrap();
        assert_eq!(slot.path, native::path_of(kind));
        assert_eq!(slot.name, kind.name);
    }
    let reopened = mixdown(&r, &mut s, &dir, "reopened.wav");
    assert!(
        today
            .iter()
            .zip(&reopened)
            .all(|(a, b)| a.to_bits() == b.to_bits())
            && today.len() == reopened.len(),
        "the old file plays differently"
    );
    assert!(today.iter().any(|x| *x != 0.0), "the render is silent");

    // Saved again: only today's ids and names.
    let resaved = dir.join("resaved.oxide");
    call(&r, &mut s, "save_project", json!({"path": resaved})).unwrap();
    let text = std::fs::read_to_string(&resaved).unwrap();
    for renamed in RENAMED {
        if let Some(old_id) = renamed.old_id {
            assert!(
                !text.contains(&format!("native:{old_id}\"")),
                "{old_id} saved"
            );
        }
        assert!(
            !text.contains(&format!("\"{}\"", renamed.old_name)),
            "{} saved",
            renamed.old_name
        );
    }

    // A rack file (a preset, rack.json) with the old ids reads as today's.
    let rack = crate::rack::RackFile::new(
        RENAMED
            .iter()
            .filter_map(|renamed| {
                Some(crate::rack::RackSlot {
                    name: renamed.old_name.into(),
                    path: format!("native:{}", renamed.old_id?).into(),
                    bypass: false,
                    state: None,
                })
            })
            .collect(),
    );
    let read: crate::rack::RackFile =
        serde_json::from_str(&serde_json::to_string(&rack).unwrap()).unwrap();
    for (slot, renamed) in read.slots.iter().zip(RENAMED) {
        let kind = native::find(renamed.id).unwrap();
        assert_eq!(
            (slot.path.clone(), slot.name.as_str()),
            (native::path_of(kind), kind.name)
        );
    }

    // add_insert by an old id, and by an old name.
    for plugin in ["native:dehummer", "dehummer", "DeHummer"] {
        let insert = id(
            &call(
                &r,
                &mut s,
                "add_insert",
                json!({"track": track, "plugin": plugin}),
            )
            .unwrap(),
            "insert",
        );
        let slot = s.project.tracks[0]
            .inserts
            .iter()
            .find(|i| i.id == insert as u32)
            .unwrap();
        assert_eq!(
            (slot.path.to_str(), slot.name.as_str()),
            (Some("native:hum-remover"), "Hum Remover"),
            "{plugin}"
        );
    }
}
