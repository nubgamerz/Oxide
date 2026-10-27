//! Undo and redo, by snapshots of the project.
//!
//! An edit is a transaction: [`History::begin`] keeps a copy of the project, and
//! [`History::end`] files it under the edit's name only if the project actually changed, so a
//! failed or no-op edit leaves nothing behind. Undo swaps the project for the copy (keeping the
//! current one for redo); the session then brings the live state in line (plug-ins, media, the
//! graph). Plug-in state blobs are shared between copies (`Arc`), so a snapshot costs the size
//! of the project's structure, which is what the cap counts.
//!
//! Repeated changes to one thing in quick succession (a fader nudged over MCP, the same value
//! set several times) merge into one entry when they carry the same merge key; the entry keeps
//! the state from before the first of them.

use crate::project::Project;
use std::time::{Duration, Instant};

/// Changes with the same merge key closer together than this become one entry.
const MERGE_WINDOW: Duration = Duration::from_millis(1500);
const MAX_ENTRIES: usize = 300;
/// Estimated bytes of snapshots kept, beyond which the oldest go.
const MAX_BYTES: usize = 64 << 20;

struct Entry {
    name: String,
    /// The project before the edit (on the undo stack) or before the undo (on the redo stack).
    project: Project,
    key: Option<String>,
    at: Instant,
    bytes: usize,
}

struct Pending {
    name: String,
    key: Option<String>,
    before: Project,
}

#[derive(Default)]
pub struct History {
    undo: Vec<Entry>,
    redo: Vec<Entry>,
    pending: Option<Pending>,
    /// Edits begun and not yet ended: an edit inside an edit is part of the outer one.
    depth: usize,
}

impl History {
    /// Start an edit. A nested begin (an edit inside an edit) is part of the outer one, and
    /// names it if the outer one has no name yet.
    pub fn begin(&mut self, name: &str, key: Option<String>, project: &Project) {
        if self.depth == 0 {
            self.pending = Some(Pending {
                name: name.to_string(),
                key,
                before: project.clone(),
            });
        } else {
            self.label(name, key);
        }
        self.depth += 1;
    }

    /// Name the edit in progress, unless something already did. The app runs every frame as
    /// one unnamed edit, and whatever changes the project in it says what it is; the first
    /// to say wins. `key` merges a gesture that spans frames (a drag) into one step.
    pub fn label(&mut self, name: &str, key: Option<String>) {
        if let Some(pending) = &mut self.pending {
            if pending.name.is_empty() && !name.is_empty() {
                pending.name = name.to_string();
                pending.key = key;
            }
        }
    }

    /// The project was just replaced (undo, redo) in the middle of an edit: compare against
    /// the new one, so the replacement is not taken for an edit.
    pub fn rebase(&mut self, project: &Project) {
        if let Some(pending) = &mut self.pending {
            pending.before = project.clone();
        }
    }

    /// Finish the edit begun last. Only the outermost end records anything. Returns whether
    /// it changed something (and was recorded).
    pub fn end(&mut self, project: &Project) -> bool {
        if self.depth == 0 {
            return false;
        }
        self.depth -= 1;
        if self.depth > 0 {
            return false;
        }
        let Some(mut pending) = self.pending.take() else {
            return false;
        };
        // View state (track heights) is not an edit: compare, and keep, the snapshot with the
        // current view.
        pending.before.copy_view_from(project);
        if pending.before == *project {
            return false;
        }
        if pending.name.is_empty() {
            // Nothing said what it was (a UI gesture): name it by what changed, with a key
            // that merges the gesture's frames.
            let (name, key) = describe(&pending.before, project);
            pending.name = name;
            pending.key = key;
        }
        self.redo.clear();
        if let (Some(key), Some(top)) = (&pending.key, self.undo.last_mut()) {
            if top.key.as_ref() == Some(key) && top.at.elapsed() < MERGE_WINDOW {
                top.at = Instant::now();
                return true;
            }
        }
        let bytes = estimate(&pending.before);
        self.undo.push(Entry {
            name: pending.name,
            project: pending.before,
            key: pending.key,
            at: Instant::now(),
            bytes,
        });
        self.trim();
        true
    }

    /// Change every snapshot the same way the project just changed outside an edit (saving
    /// to another folder rewrites relative paths and the name), so undo never brings back
    /// stale versions of them.
    pub fn for_each_snapshot(&mut self, mut change: impl FnMut(&mut Project)) {
        for entry in self.undo.iter_mut().chain(&mut self.redo) {
            change(&mut entry.project);
        }
        if let Some(pending) = &mut self.pending {
            change(&mut pending.before);
        }
    }

    /// Change only the snapshot of the edit in progress, for a change that is not an edit
    /// (saving plug-in states during a frame), so the edit does not record it.
    pub fn update_pending(&mut self, change: impl FnOnce(&mut Project)) {
        if let Some(pending) = &mut self.pending {
            change(&mut pending.before);
        }
    }

    /// Forget everything: a new or opened project, or one whose positions were converted.
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Step back: returns the edit's name and the project to restore, keeping `current` for
    /// redo.
    pub fn undo(&mut self, current: &Project) -> Option<(String, Project)> {
        let entry = self.undo.pop()?;
        self.redo.push(Entry {
            name: entry.name.clone(),
            project: current.clone(),
            key: None,
            at: Instant::now(),
            bytes: estimate(current),
        });
        Some((entry.name, entry.project))
    }

    /// Step forward again after an undo.
    pub fn redo(&mut self, current: &Project) -> Option<(String, Project)> {
        let entry = self.redo.pop()?;
        self.undo.push(Entry {
            name: entry.name.clone(),
            project: current.clone(),
            key: None,
            at: Instant::now(),
            bytes: estimate(current),
        });
        Some((entry.name, entry.project))
    }

    /// Names of what undo and redo would do, most recent first.
    pub fn names(&self) -> (Vec<String>, Vec<String>) {
        let names = |v: &Vec<Entry>| v.iter().rev().map(|e| e.name.clone()).collect();
        (names(&self.undo), names(&self.redo))
    }

    /// Estimated bytes the history holds.
    pub fn bytes(&self) -> usize {
        self.undo.iter().chain(&self.redo).map(|e| e.bytes).sum()
    }

    fn trim(&mut self) {
        while self.undo.len() > MAX_ENTRIES || (self.bytes() > MAX_BYTES && self.undo.len() > 1) {
            self.undo.remove(0);
        }
    }
}

/// Changes made by dragging or nudging over several frames: these merge into one step. Every
/// other change (a split, a delete, a mute) is a step of its own, however quickly the next
/// one follows.
const CONTINUOUS: &[&str] = &[
    "Volume",
    "Pan",
    "Send level",
    "Automation",
    "Move clip",
    "Clip gain",
    "Stretch clip",
    "Transpose clip",
    "Follow tempo",
    "Fade in",
    "Fade out",
    "Move marker",
    "Master volume",
    "Master mixer",
    "Tempo",
    "Tempo change",
    "Loop",
    "Punch",
    "Metronome",
    "Loudness target",
];

/// Name a change by what differs between two versions of the project, for the undo list:
/// "Volume", "Move clip", "Rename track". The first difference found names it. Continuous
/// changes also get a key (name and the object's id) that merges the frames of one gesture,
/// such as a fader drag, into one step.
pub fn describe(before: &Project, after: &Project) -> (String, Option<String>) {
    let named = |name: &str, id: u32| {
        let key = CONTINUOUS.contains(&name).then(|| format!("{name}:{id}"));
        (name.to_string(), key)
    };
    // Tempo first: clips that follow the tempo move with it, in the same edit.
    let tempo = if before.tempo != after.tempo {
        Some("Tempo")
    } else if before.time_signature != after.time_signature {
        Some("Time signature")
    } else if before.tempo_changes != after.tempo_changes {
        Some("Tempo change")
    } else {
        None
    };
    if let Some(change) = tempo {
        return named(change, 0);
    }
    if let Some((change, id)) = describe_markers(&before.markers, &after.markers) {
        return named(change, id);
    }
    if before.tracks.len() != after.tracks.len() {
        let name = if after.tracks.len() > before.tracks.len() {
            "Add track"
        } else {
            "Remove track"
        };
        return named(name, 0);
    }
    let order = |p: &Project| {
        p.tracks
            .iter()
            .map(|t| (t.id, t.parent))
            .collect::<Vec<_>>()
    };
    if order(before) != order(after) {
        return named("Move track", 0);
    }
    for (a, b) in before.tracks.iter().zip(&after.tracks) {
        let id = b.id;
        let change = if a.name != b.name {
            "Rename track"
        } else if a.color != b.color {
            "Track colour"
        } else if a.mixer.gain_db != b.mixer.gain_db {
            "Volume"
        } else if a.mixer.pan != b.mixer.pan {
            "Pan"
        } else if a.mixer.mute != b.mixer.mute {
            "Mute"
        } else if a.mixer.solo != b.mixer.solo {
            "Solo"
        } else if a.arm != b.arm {
            "Arm track"
        } else if a.monitor != b.monitor {
            "Monitoring"
        } else if a.input != b.input {
            "Track input"
        } else if a.output != b.output {
            "Track output"
        } else if a.collapsed != b.collapsed {
            "Fold folder"
        } else if a.enabled != b.enabled {
            if b.enabled {
                "Enable track"
            } else {
                "Disable track"
            }
        } else if a.automation_mode != b.automation_mode {
            "Automation mode"
        } else if let Some((change, send)) = describe_sends(&a.sends, &b.sends) {
            return named(change, send);
        } else if let Some(change) = describe_inserts(&a.inserts, &b.inserts) {
            change
        } else if let Some((change, clip)) = describe_clips(&a.clips, &b.clips) {
            return named(change, clip);
        } else if let Some((change, clip)) = describe_midi_clips(&a.midi_clips, &b.midi_clips) {
            return named(change, clip);
        } else if let Some((change, lane)) = describe_automation(&a.automation, &b.automation) {
            return named(change, lane);
        } else {
            continue;
        };
        return named(change, id);
    }
    let (a, b) = (&before.master, &after.master);
    let master = if a.mixer.gain_db != b.mixer.gain_db {
        Some("Master volume")
    } else if a.mixer != b.mixer {
        Some("Master mixer")
    } else if a.output_channel != b.output_channel {
        Some("Master output")
    } else if a.automation_mode != b.automation_mode {
        Some("Automation mode")
    } else {
        describe_inserts(&a.inserts, &b.inserts)
    };
    if let Some(change) = master {
        return named(change, 0);
    }
    if let Some((change, lane)) = describe_automation(&a.automation, &b.automation) {
        return named(change, lane);
    }
    let change = if before.loop_region != after.loop_region {
        "Loop"
    } else if before.punch != after.punch {
        "Punch"
    } else if before.metronome != after.metronome {
        "Metronome"
    } else if before.chase_notes != after.chase_notes {
        "Chase notes"
    } else if before.loudness_target != after.loudness_target {
        "Loudness target"
    } else if before.midi_record != after.midi_record {
        "MIDI record options"
    } else if before.grid != after.grid {
        "Grid"
    } else {
        "Edit"
    };
    named(change, 0)
}

fn describe_inserts(
    a: &[crate::project::InsertSlot],
    b: &[crate::project::InsertSlot],
) -> Option<&'static str> {
    use std::cmp::Ordering::*;
    match a.len().cmp(&b.len()) {
        Less => return Some("Add plug-in"),
        Greater => return Some("Remove plug-in"),
        Equal => {}
    }
    if a.iter().zip(b).any(|(x, y)| x.id != y.id) {
        Some("Move plug-in")
    } else if a.iter().zip(b).any(|(x, y)| x.bypass != y.bypass) {
        Some("Bypass plug-in")
    } else {
        None
    }
}

/// The send change and the send it happened to (0 for an added or removed send: those
/// steps never merge).
fn describe_sends(
    a: &[crate::project::TrackSend],
    b: &[crate::project::TrackSend],
) -> Option<(&'static str, u32)> {
    use std::cmp::Ordering::*;
    match a.len().cmp(&b.len()) {
        Less => return Some(("Add send", 0)),
        Greater => return Some(("Remove send", 0)),
        Equal => {}
    }
    a.iter().zip(b).find(|(x, y)| x != y).map(|(x, y)| {
        let change = if x.id != y.id {
            "Edit sends"
        } else if x.gain_db != y.gain_db {
            "Send level"
        } else if x.mute != y.mute {
            "Send mute"
        } else if x.pre_fader != y.pre_fader {
            "Send pre/post fader"
        } else {
            "Send target"
        };
        (change, y.id)
    })
}

/// The MIDI clip change and the clip it happened to.
fn describe_midi_clips(
    a: &[crate::midi_clip::MidiClip],
    b: &[crate::midi_clip::MidiClip],
) -> Option<(&'static str, u32)> {
    use std::cmp::Ordering::*;
    match a.len().cmp(&b.len()) {
        // A split adds a clip and shortens one, as for audio clips.
        Less => {
            let shortened = a
                .iter()
                .find(|x| b.iter().any(|y| y.id == x.id && y.length < x.length));
            return Some(match shortened {
                Some(clip) => ("Split clip", clip.id),
                None => ("Add MIDI clip", 0),
            });
        }
        Greater => return Some(("Remove MIDI clip", 0)),
        Equal => {}
    }
    a.iter().zip(b).find(|(x, y)| x != y).map(|(x, y)| {
        let change = if x.id != y.id {
            "Edit MIDI clips"
        } else if x.notes != y.notes {
            "Edit notes"
        } else if x.controllers != y.controllers {
            "Edit controllers"
        } else if x.start != y.start {
            "Move MIDI clip"
        } else if x.length != y.length {
            "Resize MIDI clip"
        } else {
            "Rename clip"
        };
        (change, y.id)
    })
}

/// The automation change and the lane it happened to.
fn describe_automation(
    a: &[crate::automation::AutomationLane],
    b: &[crate::automation::AutomationLane],
) -> Option<(&'static str, u32)> {
    use std::cmp::Ordering::*;
    match a.len().cmp(&b.len()) {
        Less => return Some(("Add automation lane", 0)),
        Greater => return Some(("Remove automation lane", 0)),
        Equal => {}
    }
    a.iter().zip(b).find(|(x, y)| x != y).map(|(x, y)| {
        let change = if x.id != y.id {
            "Edit automation lanes"
        } else if x.enabled != y.enabled {
            "Automation on/off"
        } else {
            "Automation"
        };
        (change, y.id)
    })
}

/// The marker change and the marker it happened to.
fn describe_markers(
    a: &[crate::project::Marker],
    b: &[crate::project::Marker],
) -> Option<(&'static str, u32)> {
    if a.len() != b.len() {
        return Some(if b.len() > a.len() {
            ("Add marker", 0)
        } else {
            ("Remove marker", 0)
        });
    }
    // Moves reorder them, so compare by id.
    b.iter().find_map(|y| {
        let x = a.iter().find(|x| x.id == y.id)?;
        if x.tick != y.tick {
            Some(("Move marker", y.id))
        } else if x.name != y.name {
            Some(("Rename marker", y.id))
        } else {
            None
        }
    })
}

/// The clip change and the clip it happened to.
fn describe_clips(
    a: &[crate::project::Clip],
    b: &[crate::project::Clip],
) -> Option<(&'static str, u32)> {
    use std::cmp::Ordering::*;
    match a.len().cmp(&b.len()) {
        // A split adds a clip and shortens one; anything else adding one is an import.
        Less => {
            let shortened = a
                .iter()
                .find(|x| b.iter().any(|y| y.id == x.id && y.length < x.length));
            return Some(match shortened {
                Some(clip) => ("Split clip", clip.id),
                None => ("Add clip", 0),
            });
        }
        Greater => return Some(("Delete clip", 0)),
        Equal => {}
    }
    for (x, y) in a.iter().zip(b) {
        let change = if x.start != y.start {
            "Move clip"
        } else if x.active != y.active {
            "Choose take"
        } else if x.takes.len() != y.takes.len() {
            "Delete take"
        } else if x.gain_db != y.gain_db {
            "Clip gain"
        } else if x.follow.as_ref().map(|f| f.tempo) != y.follow.as_ref().map(|f| f.tempo) {
            // Before the stretch: following resets it.
            "Follow tempo"
        } else if x.stretch != y.stretch {
            "Stretch clip"
        } else if x.pitch != y.pitch {
            "Transpose clip"
        } else if x.transients != y.transients {
            "Keep attacks"
        } else if x.fade_in != y.fade_in && x.length == y.length {
            "Fade in"
        } else if x.fade_out != y.fade_out && x.length == y.length {
            "Fade out"
        } else if x != y {
            "Edit clip"
        } else {
            continue;
        };
        return Some((change, y.id));
    }
    None
}

/// A rough size of a project snapshot, leaving out plug-in states (shared, not copied).
fn estimate(p: &Project) -> usize {
    let text = |s: &str| s.len() + 24;
    let path = |p: &std::path::Path| p.as_os_str().len() + 24;
    1024 + p.tempo_changes.len() * 48
        + p.master
            .inserts
            .iter()
            .map(|s| 96 + text(&s.name) + path(&s.path))
            .sum::<usize>()
        + p.tracks
            .iter()
            .map(|t| {
                256 + text(&t.name)
                    + t.inserts
                        .iter()
                        .map(|s| 96 + text(&s.name) + path(&s.path))
                        .sum::<usize>()
                    + t.clips
                        .iter()
                        .map(|c| {
                            96 + text(&c.name)
                                + c.takes
                                    .iter()
                                    .map(|k| 48 + text(&k.name) + path(&k.file))
                                    .sum::<usize>()
                        })
                        .sum::<usize>()
            })
            .sum::<usize>()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TrackKind;

    #[test]
    fn records_only_real_changes_and_merges_by_key() {
        let mut h = History::default();
        let mut p = Project::default();
        h.begin("Nothing", None, &p);
        assert!(!h.end(&p), "no change, no entry");

        h.begin("Add track", None, &p);
        p.add_track(TrackKind::Audio, None);
        assert!(h.end(&p));

        let id = p.tracks[0].id;
        for gain in [-1.0, -2.0, -3.0] {
            h.begin("Set track mixer", Some(format!("mixer:{id}")), &p);
            p.track_mut(id).unwrap().mixer.gain_db = gain;
            h.end(&p);
        }
        assert_eq!(h.names().0, vec!["Set track mixer", "Add track"]);

        // Undo the merged fader moves in one step, back to before the first.
        let (name, before) = h.undo(&p).unwrap();
        assert_eq!(name, "Set track mixer");
        assert_eq!(before.tracks[0].mixer.gain_db, 0.0);
        let (_, after) = h.redo(&before).unwrap();
        assert_eq!(after.tracks[0].mixer.gain_db, -3.0);

        // A new edit after an undo drops the redo.
        let (_, before) = h.undo(&after).unwrap();
        h.begin("Rename", None, &before);
        let mut renamed = before.clone();
        renamed.tracks[0].name = "DI".into();
        h.end(&renamed);
        assert!(h.names().1.is_empty());
    }

    #[test]
    fn an_edit_inside_an_edit_is_part_of_it() {
        let mut h = History::default();
        let mut p = Project::default();
        h.begin("Remove insert", None, &p);
        // Something inside it (a recording landing during an update) begins and ends its own.
        h.begin("Record", None, &p);
        p.add_track(TrackKind::Audio, None);
        assert!(!h.end(&p), "the inner end records nothing");
        p.name = "changed".into();
        assert!(h.end(&p));
        assert_eq!(h.names().0, vec!["Remove insert"]);
        assert!(!h.end(&p), "an unmatched end does nothing");
    }

    #[test]
    fn frames_are_named_by_what_changed_them_and_drags_merge() {
        let mut h = History::default();
        let mut p = Project::default();
        let id = p.add_track(TrackKind::Audio, None);
        // A fader dragged over three frames: each frame is an edit the fader names.
        for gain in [-1.0, -2.0, -3.0] {
            h.begin("", None, &p);
            h.label("Volume", Some(format!("volume:{id}")));
            p.track_mut(id).unwrap().mixer.gain_db = gain;
            h.end(&p);
        }
        // A frame that changes nothing records nothing; one nobody names is named by what
        // changed.
        h.begin("", None, &p);
        h.end(&p);
        h.begin("", None, &p);
        p.track_mut(id).unwrap().name = "DI".into();
        h.end(&p);
        assert_eq!(h.names().0, vec!["Rename track", "Volume"]);

        // Undo in the middle of a frame is not itself taken for an edit.
        h.begin("", None, &p);
        let (_, restored) = h.undo(&p).unwrap();
        h.rebase(&restored);
        assert!(!h.end(&restored));
        assert_eq!(
            h.names(),
            (vec!["Volume".to_string()], vec!["Rename track".to_string()])
        );
    }

    #[test]
    fn quick_discrete_edits_stay_separate_steps() {
        let mut h = History::default();
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        let clip = p.next_id();
        p.track_mut(t)
            .unwrap()
            .clips
            .push(crate::project::Clip::new(
                clip,
                "c".into(),
                "c.wav".into(),
                0,
                0,
                1000,
            ));
        // S pressed twice in quick succession: two splits, two steps.
        for at in [300, 600] {
            h.begin("", None, &p);
            let last = p.track(t).unwrap().clips.last().unwrap().id;
            p.split_clip(last, at);
            h.end(&p);
        }
        assert_eq!(h.names().0, vec!["Split clip", "Split clip"]);
        // Mute toggled twice: two steps as well.
        for _ in 0..2 {
            h.begin("", None, &p);
            let mute = &mut p.track_mut(t).unwrap().mixer.mute;
            *mute = !*mute;
            h.end(&p);
        }
        assert_eq!(h.names().0.len(), 4);
    }

    #[test]
    fn a_midi_clip_split_is_named_like_an_audio_one() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Instrument, None);
        let clip = p.add_midi_clip(t, 0, 3840).unwrap();
        let mut q = p.clone();
        q.split_midi_clip(clip, 960).unwrap();
        assert_eq!(describe(&p, &q).0, "Split clip");
        let mut q = p.clone();
        q.add_midi_clip(t, 3840, 960).unwrap();
        assert_eq!(describe(&p, &q).0, "Add MIDI clip");
    }

    #[test]
    fn changes_are_named_by_what_differs() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        let named = |a: &Project, b: &Project| describe(a, b).0;
        let mut q = p.clone();
        q.add_track(TrackKind::Audio, None);
        assert_eq!(named(&p, &q), "Add track");
        assert_eq!(named(&q, &p), "Remove track");

        let mut q = p.clone();
        q.track_mut(t).unwrap().mixer.pan = 0.5;
        assert_eq!(describe(&p, &q), ("Pan".into(), Some(format!("Pan:{t}"))));

        let clip = p.next_id();
        p.track_mut(t)
            .unwrap()
            .clips
            .push(crate::project::Clip::new(
                clip,
                "c".into(),
                "c.wav".into(),
                0,
                0,
                1000,
            ));
        let mut q = p.clone();
        q.split_clip(clip, 400);
        assert_eq!(named(&p, &q), "Split clip");
        let mut q = p.clone();
        q.move_clip(clip, 500, None).unwrap();
        assert_eq!(
            describe(&p, &q),
            ("Move clip".into(), Some(format!("Move clip:{clip}")))
        );
        // The clip editor's fields are dragged, so each one's frames merge into one step.
        let mut q = p.clone();
        q.clip_mut(clip).unwrap().set_stretch(2.0);
        assert_eq!(
            describe(&p, &q),
            ("Stretch clip".into(), Some(format!("Stretch clip:{clip}")))
        );
        let mut q = p.clone();
        q.clip_mut(clip).unwrap().pitch = 3.0;
        assert_eq!(
            describe(&p, &q),
            (
                "Transpose clip".into(),
                Some(format!("Transpose clip:{clip}"))
            )
        );
        // Markers: added, dragged (merged per marker), renamed, removed.
        let mut q = p.clone();
        let marker = q.add_marker(960, None);
        assert_eq!(named(&p, &q), "Add marker");
        assert_eq!(named(&q, &p), "Remove marker");
        let mut r = q.clone();
        r.set_marker(marker, None, Some(1920)).unwrap();
        assert_eq!(
            describe(&q, &r),
            ("Move marker".into(), Some(format!("Move marker:{marker}")))
        );
        let mut r = q.clone();
        r.set_marker(marker, Some("Verse".into()), None).unwrap();
        assert_eq!(named(&q, &r), "Rename marker");
        // Fade handles are dragged too.
        let mut q = p.clone();
        let fade = crate::project::Fade {
            length: 300,
            shape: crate::project::FadeShape::Linear,
        };
        q.clip_mut(clip)
            .unwrap()
            .set_fades(fade, Default::default());
        assert_eq!(
            describe(&p, &q),
            ("Fade in".into(), Some(format!("Fade in:{clip}")))
        );
        // Following resets the stretch; it's named for the following.
        let mut stretched = p.clone();
        stretched.clip_mut(clip).unwrap().set_stretch(1.5);
        let mut q = stretched.clone();
        let map = crate::tempo::TempoMap::of(&q, 48_000);
        q.clip_mut(clip).unwrap().set_follow(Some(120.0), &map);
        assert_eq!(
            describe(&stretched, &q),
            ("Follow tempo".into(), Some(format!("Follow tempo:{clip}")))
        );

        let mut q = p.clone();
        q.tempo = 90.0;
        assert_eq!(named(&p, &q), "Tempo");
        let mut q = p.clone();
        q.loop_region.enabled = true;
        assert_eq!(named(&p, &q), "Loop");
        let mut q = p.clone();
        q.master.mixer.gain_db = -3.0;
        assert_eq!(named(&p, &q), "Master volume");
    }

    #[test]
    fn the_oldest_entries_go_first() {
        let mut h = History::default();
        let mut p = Project::default();
        for k in 0..MAX_ENTRIES + 5 {
            h.begin(&format!("Edit {k}"), None, &p);
            p.name = format!("{k}");
            h.end(&p);
        }
        let (undo, _) = h.names();
        assert_eq!(undo.len(), MAX_ENTRIES);
        assert_eq!(undo.last().unwrap(), "Edit 5");
    }
}
