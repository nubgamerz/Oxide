//! The synced rack's tests (dev plan 21.2), on built-in effects in a headless session.

use super::*;
use crate::device::AudioSettings;
use crate::project::TrackKind;
use crate::session::InsertTarget;
use std::path::PathBuf;

fn session() -> Session {
    Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        threads: Some(0),
        ..Default::default()
    })
}

fn effect(name: &str) -> PathBuf {
    crate::native::path_of(crate::native::find(name).expect("a built-in effect"))
}

/// Two seconds of a stereo signal at 48 kHz (different on each side), in a folder of the
/// test's own.
fn stereo_file(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-rack-sync-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(format!("{name}.wav"));
    let tau = 2.0 * std::f64::consts::PI;
    let side = |f: f64, beat: f64| -> Vec<f32> {
        (0..96_000)
            .map(|i| {
                let t = f64::from(i) / 48_000.0;
                let level = if (t * beat).fract() < 0.2 { 0.8 } else { 0.2 };
                (level * (tau * f * t).sin()) as f32
            })
            .collect()
    };
    crate::wav::write_f32(
        &path,
        &crate::wav::Audio {
            sample_rate: 48_000,
            channels: vec![side(220.0, 3.0), side(330.0, 5.0)],
        },
    )
    .unwrap();
    path
}

/// A session with the user's own rack (an Amplify at a setting), and a track holding a clip
/// with an Amplify (moved), a Parametric EQ (bypassed) and a Compressor
/// working: the file opened from the clip.
struct Setup {
    s: Session,
    track: TrackId,
    doc: DocumentId,
    own_amp: InsertId,
    chain: Vec<InsertId>,
}

fn setup(name: &str) -> Setup {
    let mut s = session();
    s.scratch = Some(std::env::temp_dir().join(format!(
        "daw-rack-sync-{name}-scratch-{}",
        std::process::id()
    )));
    let own_amp = s
        .add_insert(InsertTarget::Editor, &effect("Amplify"))
        .unwrap();
    s.set_parameter(own_amp, 0, 0.3).unwrap();
    let track = s.add_track(TrackKind::Audio, None);
    let clip = s.add_clip_from_file(track, &stereo_file(name), 0).unwrap();
    let mut chain = Vec::new();
    for name in ["Amplify", "Parametric EQ", "Compressor"] {
        chain.push(
            s.add_insert(InsertTarget::Track(track), &effect(name))
                .unwrap(),
        );
    }
    s.set_parameter(chain[0], 0, 0.62).unwrap();
    s.insert_mut(chain[1]).unwrap().bypass = true;
    s.set_parameter(chain[2], 0, 0.6).unwrap();
    s.set_parameter(chain[2], 1, 0.2).unwrap();
    s.update();
    let doc = s.open_clip_document(clip).unwrap();
    Setup {
        s,
        track,
        doc,
        own_amp,
        chain,
    }
}

fn state(s: &Session, id: InsertId) -> Vec<u8> {
    s.native(id).expect("a built-in effect").save_state()
}

fn names(slots: &[InsertSlot]) -> Vec<(String, bool)> {
    slots.iter().map(|s| (s.name.clone(), s.bypass)).collect()
}

fn track_slots(s: &Session, track: TrackId) -> Vec<InsertSlot> {
    s.insert_slots(InsertTarget::Track(track)).to_vec()
}

/// An edit as the app makes one (a frame) or an action does.
fn edit(s: &mut Session, change: impl FnOnce(&mut Session)) {
    s.begin_edit("", None);
    change(s);
    s.end_edit();
}

#[test]
fn turning_sync_on_copies_the_tracks_chain_and_off_brings_the_users_rack_back() {
    let Setup {
        mut s,
        track,
        doc,
        own_amp,
        chain,
    } = setup("on-off");
    let own_state = state(&s, own_amp);
    // A file opened on its own has no track to sync with.
    let loose = s.open_document(&stereo_file("on-off-loose")).unwrap();
    let refused = s.set_rack_sync(loose, true).unwrap_err().to_string();
    assert!(refused.contains("wasn't opened from a clip"), "{refused}");
    assert_eq!(s.sync_track_of(loose), None);
    assert_eq!(s.sync_track_of(doc), Some((track, false)));
    // On: the rack is the track's chain, each copy with its original's settings and bypass.
    let rack = s.set_rack_sync(doc, true).unwrap();
    assert_eq!(rack.len(), 3);
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    for (&copy, &original) in rack.iter().zip(&chain) {
        assert!(s.is_loaded(copy));
        assert_eq!(state(&s, copy), state(&s, original), "insert {original}");
        assert_eq!(s.rack_counterpart(copy), Some(original));
        assert_eq!(s.rack_counterpart(original), Some(copy));
    }
    assert_eq!(s.sync_track_of(doc), Some((track, true)));
    assert_eq!(s.rack_sync().map(|i| i.track), Some(track));
    // Another file of the same track: the sync moves to it, the rack untouched.
    let other = s
        .add_clip_from_file(track, &stereo_file("on-off-other"), 96_000)
        .unwrap();
    let doc2 = s.open_clip_document(other).unwrap();
    assert_eq!(s.set_rack_sync(doc2, true).unwrap(), rack);
    assert_eq!(
        s.rack_sync().map(|i| (i.document, i.clip)),
        Some((doc2, other))
    );
    assert_eq!(s.set_rack_sync(doc, true).unwrap(), rack);
    // Kept between runs meanwhile: the user's own rack, never the track's chain.
    let kept = std::env::temp_dir().join(format!("daw-rack-sync-kept-{}.json", std::process::id()));
    let _ = std::fs::remove_file(&kept);
    s.rack_file = Some(kept.clone());
    s.save_rack_now().unwrap();
    let file = crate::rack::read(&kept).unwrap();
    assert_eq!(file.slots.len(), 1);
    assert_eq!(file.slots[0].state.as_deref(), Some(&own_state));
    s.rack_file = None;
    let _ = std::fs::remove_file(&kept);
    // A preset would replace the track's effects too: refused while synced.
    s.rack_presets_dir = Some(std::env::temp_dir());
    let refused = s.load_rack_preset("anything").unwrap_err().to_string();
    assert!(refused.contains("synced"), "{refused}");
    // Off: the user's own rack is back with its settings, the track untouched.
    let back = s.set_rack_sync(doc, false).unwrap();
    assert_eq!(back.len(), 1);
    // (Its settings went through their saved form: equal to the last bit of a plain value.)
    let gain = |s: &Session, id: InsertId| s.native(id).unwrap().value(0);
    assert!((gain(&s, back[0]) - 0.3).abs() < 1e-12);
    assert_eq!(s.rack_sync(), None);
    assert_eq!(
        track_slots(&s, track)
            .iter()
            .map(|s| s.id)
            .collect::<Vec<_>>(),
        chain
    );
    // Closing the file ends a sync the same way.
    s.set_rack_sync(doc, true).unwrap();
    s.close_document(doc, true).unwrap();
    assert_eq!(s.rack_sync(), None);
    assert_eq!(s.editor_rack().len(), 1);
    assert!((gain(&s, s.editor_rack()[0].id) - 0.3).abs() < 1e-12);
    // So does opening a project: the user's rack comes in with it, not the old track's chain.
    let doc = s
        .open_clip_document(s.project.tracks[0].clips[0].id)
        .unwrap();
    s.set_rack_sync(doc, true).unwrap();
    let saved = s.scratch.clone().unwrap().join("synced.oxide");
    s.save(&saved).unwrap();
    s.open(&saved).unwrap();
    assert_eq!(s.rack_sync(), None);
    assert_eq!(s.editor_rack().len(), 1);
    let id = s.editor_rack()[0].id;
    assert!(s.is_loaded(id));
    assert!((gain(&s, id) - 0.3).abs() < 1e-12);
}

/// Turning sync on with the plug-ins loading a frame at a time (the app): the copies' slots at
/// once, each loading with its original's settings as they are when it loads.
#[test]
fn a_synced_racks_copies_load_staged_with_their_originals_settings() {
    let Setup {
        mut s, doc, chain, ..
    } = setup("staged");
    s.stage_loads = true;
    s.hold_loading(true);
    let rack = s.set_rack_sync(doc, true).unwrap();
    assert_eq!(rack.len(), 3);
    assert!(rack.iter().all(|id| !s.is_loaded(*id)), "held");
    assert!(s
        .loading()
        .is_some_and(|l| l.kind == LoadKind::Rack && l.total == 3));
    // Turned while it waits: the copy takes the value it has when it loads.
    s.set_parameter(chain[0], 0, 0.9).unwrap();
    s.hold_loading(false);
    for _ in 0..10 {
        s.update();
    }
    assert!(s.loading().is_none());
    for (&copy, &original) in rack.iter().zip(&chain) {
        assert_eq!(state(&s, copy), state(&s, original));
    }
}

#[test]
fn structure_mirrors_both_ways_and_undo_covers_both_views() {
    let Setup {
        mut s, track, doc, ..
    } = setup("structure");
    s.set_rack_sync(doc, true).unwrap();
    let steps = |s: &Session| s.history().0.len();
    let before = steps(&s);
    // In the rack: added, then switched off, then moved first: the track follows, each one
    // undo step.
    edit(&mut s, |s| {
        s.add_insert(InsertTarget::Editor, &effect("Limiter"))
            .unwrap();
    });
    assert_eq!(steps(&s), before + 1);
    let limiter = s.editor_rack()[3].id;
    let on_track = s.rack_counterpart(limiter).expect("copied to the track");
    assert!(s.is_loaded(on_track));
    assert_eq!(state(&s, on_track), state(&s, limiter));
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    edit(&mut s, |s| s.insert_mut(limiter).unwrap().bypass = true);
    edit(&mut s, |s| {
        s.move_insert_to(limiter, 0);
    });
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    assert_eq!(track_slots(&s, track)[0].id, on_track);
    assert!(track_slots(&s, track)[0].bypass);
    assert_eq!(steps(&s), before + 3);
    // Undone, in the project: the rack follows at the next update.
    s.undo().unwrap();
    s.update();
    assert_eq!(track_slots(&s, track)[0].name, "Amplify");
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    s.undo().unwrap();
    s.undo().unwrap();
    s.update();
    assert_eq!(
        track_slots(&s, track).len(),
        3,
        "the limiter's adding undone"
    );
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    s.redo().unwrap();
    s.update();
    let back = track_slots(&s, track)[3].id;
    assert_eq!(back, on_track, "the same insert back");
    let copy = s
        .rack_counterpart(back)
        .expect("its copy is back in the rack");
    assert!(s.is_loaded(copy));
    assert_eq!(state(&s, copy), state(&s, back));
    // On the track: added, switched, moved and removed: the rack follows.
    edit(&mut s, |s| {
        s.add_insert(InsertTarget::Track(track), &effect("Delay"))
            .unwrap();
    });
    let delay = track_slots(&s, track)[4].id;
    let delay_copy = s.rack_counterpart(delay).expect("copied to the rack");
    assert!(s.is_loaded(delay_copy));
    edit(&mut s, |s| s.insert_mut(delay).unwrap().bypass = true);
    edit(&mut s, |s| {
        s.move_insert_to(delay, 1);
    });
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    assert_eq!(s.editor_rack()[1].id, delay_copy);
    edit(&mut s, |s| s.remove_insert(delay));
    assert_eq!(s.rack_counterpart(delay_copy), None);
    assert!(!s.editor_rack().iter().any(|s| s.id == delay_copy));
    assert!(!s.is_loaded(delay_copy));
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    // And removed from the rack: gone from the track, one step, undone together.
    let first = s.editor_rack()[0].id;
    let first_on_track = s.rack_counterpart(first).unwrap();
    let before = steps(&s);
    edit(&mut s, |s| s.remove_insert(first));
    assert_eq!(steps(&s), before + 1);
    assert!(s.project.find_insert(first_on_track).is_none());
    s.undo().unwrap();
    s.update();
    assert!(s.project.find_insert(first_on_track).is_some());
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    // The track removed: the sync ends, the user's rack back.
    edit(&mut s, |s| s.remove_track(track));
    s.update();
    assert_eq!(s.rack_sync(), None);
    assert_eq!(s.editor_rack().len(), 1);
    assert_eq!(s.editor_rack()[0].name, "Amplify");
}

#[test]
fn parameters_and_whole_states_mirror_both_ways() {
    let Setup {
        mut s,
        track,
        doc,
        chain,
        ..
    } = setup("parameters");
    // A Noise Reduction on the track too, to learn a print.
    let nr = s
        .add_insert(InsertTarget::Track(track), &effect("Noise Reduction"))
        .unwrap();
    s.update();
    s.set_rack_sync(doc, true).unwrap();
    let amp_copy = s.rack_counterpart(chain[0]).unwrap();
    let value = |s: &Session, id: InsertId| s.native(id).unwrap().value(0);
    // A parameter set on either side reaches the other.
    s.set_parameter(chain[0], 0, 0.71).unwrap();
    assert_eq!(value(&s, amp_copy), 0.71);
    s.set_parameter(amp_copy, 0, 0.44).unwrap();
    assert_eq!(value(&s, chain[0]), 0.44);
    assert_eq!(state(&s, amp_copy), state(&s, chain[0]));
    // A print learned on the track is no parameter: the whole state goes across.
    let nr_copy = s.rack_counterpart(nr).unwrap();
    assert!(s.native(nr_copy).unwrap().print().is_none());
    s.learn_from(doc, (0, 24_000), Some(nr), None).unwrap();
    assert!(s.native(nr).unwrap().print().is_some());
    s.update();
    assert_eq!(state(&s, nr_copy), state(&s, nr));
    // Learned in the rack, it goes back the other way.
    s.learn_from(doc, (48_000, 72_000), Some(nr_copy), None)
        .unwrap();
    s.update();
    assert_eq!(state(&s, nr), state(&s, nr_copy));
    // Nothing new: nothing copied (the state as last copied).
    let before = state(&s, nr);
    s.mirror_state(nr);
    s.update();
    assert_eq!(state(&s, nr), before);
}

#[test]
fn apply_while_synced_turns_off_the_tracks_copies_as_one_step() {
    let Setup {
        mut s,
        track,
        doc,
        chain,
        ..
    } = setup("apply");
    s.set_rack_sync(doc, true).unwrap();
    // What an Apply would print: the rack's switched-on slots' originals.
    let applied = s.applied_track_inserts(doc);
    assert_eq!(applied, vec![chain[0], chain[2]]);
    let steps = s.history().0.len();
    s.apply_effects_turning_off(doc, Some((0, 48_000))).unwrap();
    s.wait_for_apply(Duration::from_secs(60)).unwrap();
    let (file_steps, _, _) = s.document(doc).unwrap().history();
    assert!(
        file_steps.last().unwrap().starts_with("Apply"),
        "{file_steps:?}"
    );
    // The track's printed effects are off (the bypassed one stays so), one step of the
    // project's own; the rack follows.
    assert!(track_slots(&s, track).iter().all(|s| s.bypass));
    let (undo, _, _) = s.history();
    assert_eq!(undo.len(), steps + 1);
    assert!(undo[0].starts_with("Turn off"), "{undo:?}");
    s.update();
    assert!(s.editor_rack().iter().all(|s| s.bypass));
    s.undo().unwrap();
    s.update();
    assert_eq!(
        names(&track_slots(&s, track)),
        vec![
            ("Amplify".to_string(), false),
            ("Parametric EQ".to_string(), true),
            ("Compressor".to_string(), false),
        ]
    );
    assert_eq!(names(s.editor_rack()), names(&track_slots(&s, track)));
    // Without sync there is nothing to turn off.
    s.set_rack_sync(doc, false).unwrap();
    assert!(s.applied_track_inserts(doc).is_empty());
    assert!(s.apply_effects_turning_off(doc, None).is_err());
}

/// Render = preview: with sync on, the clip's file previewed through the rack is, sample for
/// sample, the track's own output for that clip (the same effects with equal settings, the
/// same block size and pre-roll).
#[test]
fn a_synced_preview_nulls_against_the_tracks_output() {
    let Setup {
        mut s, doc, chain, ..
    } = setup("null");
    s.set_rack_sync(doc, true).unwrap();
    // A latent effect too (added in the rack, so mirrored), so compensation is in it.
    edit(&mut s, |s| {
        let limiter = s
            .add_insert(InsertTarget::Editor, &effect("Limiter"))
            .unwrap();
        s.set_parameter(limiter, 1, 0.8).unwrap();
    });
    let limiter = s.editor_rack()[3].id;
    let on_track = s.rack_counterpart(limiter).unwrap();
    assert_eq!(state(&s, on_track), state(&s, limiter));
    assert!(s.insert_info(on_track).unwrap().1 > 0, "latent");
    assert!(s.rack_counterpart(chain[0]).is_some());
    s.update();
    let length = 96_000u64;
    let preview = s.capture_preview(doc, 0, length).unwrap();
    let playback = s.capture_playback(0, length, 512).unwrap();
    assert_eq!(preview[0].len(), length as usize);
    assert_eq!(playback[0].len(), length as usize);
    let loud = preview[0].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(loud > 0.05, "the effects let the file through ({loud})");
    for c in 0..2 {
        for (n, (p, t)) in preview[c].iter().zip(&playback[c]).enumerate() {
            assert_eq!(
                p.to_bits(),
                t.to_bits(),
                "channel {c}, sample {n}: {p} vs {t}"
            );
        }
    }
}
