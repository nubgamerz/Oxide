//! One rack for a clip (dev plan 21.2): the Waveform view's effects rack showing the insert
//! chain of the track a file was opened from, both ways.
//!
//! Turning sync on sets the user's own rack aside (it is what `rack.json` keeps meanwhile, and
//! it comes back when sync is turned off) and makes the rack copies of the track's inserts,
//! each taking its original's settings as it loads (a plug-in a frame, behind the bar, as any
//! rack loads). Each rack slot is then *paired* with the track insert it copies, and:
//!
//! - **Structure** (adding, removing, reordering, bypassing) is reconciled by comparing each
//!   side with how it was when last reconciled ([`Session::sync_rack`], from `end_edit` and
//!   every `update`): whichever side moved is copied onto the other, a new slot loading a new
//!   instance there (staged, its settings read as it loads). When both moved, the track wins.
//!   Running in `end_edit` makes a rack edit mirrored onto the track part of the same undo
//!   step; an undo changes the track, and the next reconcile brings the rack along.
//! - **Parameters** set by hand (an editor's knob, a built-in effect's slider,
//!   `set_plugin_parameter`) are set on the counterpart through [`Session::set_parameter`]'s
//!   path (the parameter cache, no plug-in lock). A mirrored set never mirrors again, and the
//!   counterpart's own report of that value, should its editor send one, is dropped
//!   ([`ECHO`]). Values a lane or the processor moves are never mirrored: the rack has no lanes.
//! - **Whole states** go across where parameters can't show a change: a plug-in that says its
//!   values changed (`kParamValuesChanged`, a preset loaded inside it), a plug-in's editor
//!   closing, a built-in effect learning a print or taking a file. One copy an update, and
//!   only when the state differs from the one last copied (so the copy's own echo copies
//!   nothing back).

use super::{DocumentId, InsertSlot, Instance, LoadKind, Loading, Reads, Session};
use crate::project::{ClipId, InsertId, TrackId};
use crate::rack::{RackFile, RackSlot};
use anyhow::{anyhow, bail, Result};
use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long a value the mirror set is expected back from the plug-in it was set on (an
/// editor reporting a host's set as its own edit), and dropped if it comes.
const ECHO: Duration = Duration::from_secs(1);

/// The rack synced with a track.
pub(super) struct RackSync {
    pub(super) document: DocumentId,
    pub(super) track: TrackId,
    pub(super) clip: ClipId,
    /// The user's own rack, set aside while synced.
    pub(super) own: RackFile,
    /// Each rack slot and the track insert it copies.
    pairs: Vec<(InsertId, InsertId)>,
    /// Each side's slots in order, with their bypass, when last reconciled.
    rack_shape: Vec<(InsertId, bool)>,
    track_shape: Vec<(InsertId, bool)>,
    /// Inserts whose whole state goes across to their counterpart, one an update.
    states: VecDeque<InsertId>,
    /// Each paired insert's state as last copied (from it, or into it and read back).
    last_state: HashMap<InsertId, Arc<Vec<u8>>>,
    /// Values the mirror set (insert, parameter, value, when), whose echo is dropped.
    echoes: Vec<(InsertId, u32, f64, Instant)>,
}

impl RackSync {
    fn counterpart(&self, id: InsertId) -> Option<InsertId> {
        self.pairs.iter().find_map(|&(rack, track)| {
            if rack == id {
                Some(track)
            } else if track == id {
                Some(rack)
            } else {
                None
            }
        })
    }
}

/// What the rack is synced with ([`Session::rack_sync`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RackSyncInfo {
    /// The Waveform view's file it was turned on for.
    pub document: DocumentId,
    pub track: TrackId,
    /// The clip the file was opened from.
    pub clip: ClipId,
}

fn shape(slots: &[InsertSlot]) -> Vec<(InsertId, bool)> {
    slots.iter().map(|s| (s.id, s.bypass)).collect()
}

impl Session {
    /// What the effects rack is synced with, if it is.
    pub fn rack_sync(&self) -> Option<RackSyncInfo> {
        self.rack_sync.as_ref().map(|s| RackSyncInfo {
            document: s.document,
            track: s.track,
            clip: s.clip,
        })
    }

    /// While synced: the insert paired with `insert` (a rack slot's track insert, or a track
    /// insert's rack slot).
    pub fn rack_counterpart(&self, insert: InsertId) -> Option<InsertId> {
        self.rack_sync.as_ref()?.counterpart(insert)
    }

    /// Sync the Waveform view's effects rack with the track that `document` was opened from
    /// (`on`), or stop (off). On: the user's own rack is set aside and the rack becomes copies
    /// of the track's inserts with their settings now (loaded a plug-in a frame with
    /// [`Self::stage_loads`]); from then on changes on either side reach the other. Turning it
    /// on for another file moves the sync there, the user's rack still aside. Off: the user's
    /// own rack comes back (loaded the same way). Refused for a file not opened from a clip,
    /// while an Apply renders, and while other plug-ins load. Returns the rack's slots.
    pub fn set_rack_sync(&mut self, document: DocumentId, on: bool) -> Result<Vec<InsertId>> {
        let doc = self
            .document(document)
            .ok_or_else(|| anyhow!("no open audio file with id {document}"))?;
        let ids = |s: &Self| s.editor_rack.iter().map(|s| s.id).collect::<Vec<_>>();
        if !on {
            return match &self.rack_sync {
                Some(sync) if sync.document == document => self.end_rack_sync(),
                Some(sync) => bail!(
                    "the effects rack is synced with another file (document {}), not {document}",
                    sync.document
                ),
                None => Ok(ids(self)),
            };
        }
        let Some((_, clip)) = doc.source else {
            bail!(
                "{} wasn't opened from a clip, so it has no track to sync the effects rack with \
                 (open it from its clip: open_audio_editor with `clip`, or a double-click in the \
                 Multitrack)",
                doc.path.display()
            );
        };
        let file = doc.path.display().to_string();
        let track = self
            .project
            .find_clip(clip)
            .map(|(track, _)| track)
            .ok_or_else(|| anyhow!("clip {clip}, which {file} was opened from, is gone"))?;
        // Another file of the same track (or this one again): the rack is right as it is.
        if let Some(sync) = self.rack_sync.as_mut().filter(|s| s.track == track) {
            sync.document = document;
            sync.clip = clip;
            return Ok(ids(self));
        }
        self.check_rack_replaceable()?;
        let slots = self
            .project
            .track(track)
            .map(|t| t.inserts.clone())
            .unwrap_or_default();
        let rack = RackFile::new(
            slots
                .iter()
                .map(|s| RackSlot {
                    name: s.name.clone(),
                    path: s.path.clone(),
                    bypass: s.bypass,
                    state: s.state.clone(),
                })
                .collect(),
        );
        let copies: Vec<Option<InsertId>> = slots.iter().map(|s| Some(s.id)).collect();
        // The user's own rack, read now (the copies come in its place), or still the one set
        // aside when the sync moves from another file.
        let own = match self.rack_sync.take() {
            Some(sync) => sync.own,
            None => self.rack_as_file(),
        };
        let made = match self.set_rack_copying(rack, &copies) {
            Ok(made) => made,
            Err(e) => {
                // Checked above, so not expected; the user's rack is put back as it was.
                let _ = self.set_rack(own);
                return Err(e);
            }
        };
        let pairs: Vec<(InsertId, InsertId)> = made
            .iter()
            .copied()
            .zip(slots.iter().map(|s| s.id))
            .collect();
        self.rack_sync = Some(RackSync {
            document,
            track,
            clip,
            own,
            pairs,
            rack_shape: shape(&self.editor_rack),
            track_shape: shape(&slots),
            states: VecDeque::new(),
            last_state: HashMap::new(),
            echoes: Vec::new(),
        });
        Ok(made)
    }

    /// Stop syncing and bring the user's own rack back (staged). Returns its slots.
    fn end_rack_sync(&mut self) -> Result<Vec<InsertId>> {
        self.check_rack_replaceable()?;
        let sync = self.rack_sync.take().expect("synced");
        self.set_rack(sync.own)
    }

    /// An open (or a cancel back to the project before) lets go of every plug-in: a synced
    /// rack ends first, and the user's own rack's slots take its place, to load with the next
    /// project's plug-ins (`let_go_of_plugins` then reads nothing from them: not loaded yet).
    pub(super) fn end_rack_sync_for_close(&mut self) {
        let Some(sync) = self.rack_sync.take() else {
            return;
        };
        // The preview plays the rack's plug-ins: off the audio thread before they go.
        self.release_preview();
        self.flush_pending();
        for slot in std::mem::take(&mut self.editor_rack) {
            self.load_failures.remove(&slot.id);
            // In no graph: destroyed here.
            drop(self.plugins.remove(&slot.id));
        }
        for kept in sync.own.slots {
            self.next_rack_id += 1;
            self.editor_rack.push(InsertSlot {
                id: self.next_rack_id,
                name: kept.name,
                path: kept.path,
                bypass: kept.bypass,
                state: kept.state,
            });
        }
    }

    /// Queue `slot` to load (a rack slot or a track insert), taking `from`'s settings as it
    /// loads: beside the rack's loading if one runs, else as one of its own (a rack loading,
    /// which leaves the project going).
    fn queue_copy(&mut self, slot: InsertSlot, label: String, from: InsertId) {
        let loading = self.loading.get_or_insert_with(|| Loading {
            kind: LoadKind::Rack,
            label: None,
            retiring: Vec::new(),
            closing_total: self.closing.len(),
            reads: Reads::start(Vec::new(), self.sample_rate, self.read_hold.clone()),
            total: 0,
            queue: VecDeque::new(),
            path: None,
            backup: false,
            previous: self.path.clone(),
            copies: HashMap::new(),
            reopen: None,
        });
        loading.copies.insert(slot.id, from);
        loading.queue.push_back((slot, label));
        loading.total += 1;
    }

    /// Bring the two sides in line (from `end_edit` and every `update`): whichever moved since
    /// the last time is copied onto the other; the track wins when both did. Ends the sync when
    /// its file was closed or its track removed. Waits while an Apply renders and while plug-ins
    /// other than the rack's load (an open, an undo bringing plug-ins back): it catches up after.
    pub(super) fn sync_rack(&mut self) {
        if self.rack_syncing || self.applying.is_some() {
            return;
        }
        let Some(sync) = &self.rack_sync else {
            return;
        };
        if self
            .loading
            .as_ref()
            .is_some_and(|l| l.kind != LoadKind::Rack)
        {
            return;
        }
        let open = self.documents.iter().any(|d| d.id == sync.document);
        let Some(track) = self.project.track(sync.track).filter(|_| open) else {
            let why = if open {
                "its track was removed"
            } else {
                "its file was closed"
            };
            match self.end_rack_sync() {
                Ok(_) => self
                    .notices
                    .push(format!("The effects rack is no longer synced: {why}")),
                Err(e) => self
                    .notices
                    .push(format!("Ending the effects rack's sync: {e:#}")),
            }
            return;
        };
        let track_now = shape(&track.inserts);
        let rack_now = shape(&self.editor_rack);
        let track_moved = track_now != sync.track_shape;
        let rack_moved = rack_now != sync.rack_shape;
        if !track_moved && !rack_moved {
            return;
        }
        self.rack_syncing = true;
        if track_moved {
            self.mirror_to_rack();
        } else {
            self.mirror_to_track();
        }
        self.rack_syncing = false;
        let track_shape = self
            .project
            .track(self.rack_sync.as_ref().map_or(0, |s| s.track))
            .map(|t| shape(&t.inserts))
            .unwrap_or_default();
        let rack_shape = shape(&self.editor_rack);
        if let Some(sync) = &mut self.rack_sync {
            sync.track_shape = track_shape;
            sync.rack_shape = rack_shape;
        }
        if !self.stage_loads && self.loading.is_some() {
            self.finish_loading();
        }
    }

    /// The track moved: the rack follows it (slots gone, new ones copied, order and bypass).
    fn mirror_to_rack(&mut self) {
        let Some(sync) = self.rack_sync.as_mut() else {
            return;
        };
        let Some(track) = self.project.track(sync.track) else {
            return;
        };
        let name = track.name.clone();
        let slots = track.inserts.clone();
        // Rack slots whose track insert went, and rack slots of its own (the track wins).
        let kept: Vec<InsertId> = slots.iter().map(|s| s.id).collect();
        sync.pairs.retain(|(_, t)| kept.contains(t));
        let gone: Vec<InsertId> = self
            .editor_rack
            .iter()
            .map(|s| s.id)
            .filter(|r| !sync.pairs.iter().any(|(p, _)| p == r))
            .collect();
        for id in gone {
            self.remove_rack_slot(id);
        }
        // New track inserts: a copy each in the rack.
        for slot in &slots {
            if self.rack_counterpart(slot.id).is_some() {
                continue;
            }
            self.next_rack_id += 1;
            let copy = InsertSlot {
                id: self.next_rack_id,
                ..slot.clone()
            };
            self.editor_rack.push(copy.clone());
            if let Some(sync) = &mut self.rack_sync {
                sync.pairs.push((copy.id, slot.id));
            }
            self.queue_copy(copy, name.clone(), slot.id);
        }
        // Their order and bypass.
        let mut rack = std::mem::take(&mut self.editor_rack);
        let mut ordered = Vec::with_capacity(rack.len());
        for slot in &slots {
            if let Some(r) = self.rack_counterpart(slot.id) {
                if let Some(k) = rack.iter().position(|s| s.id == r) {
                    let mut copy = rack.remove(k);
                    copy.bypass = slot.bypass;
                    ordered.push(copy);
                }
            }
        }
        ordered.extend(rack);
        self.editor_rack = ordered;
    }

    /// The rack moved: the track follows it, as one undo step (or part of the edit under way).
    fn mirror_to_track(&mut self) {
        let Some(sync) = self.rack_sync.as_ref() else {
            return;
        };
        let track_id = sync.track;
        let Some(track) = self.project.track(track_id) else {
            return;
        };
        let label = format!("Sync {}'s effects with the Waveform view", track.name);
        let name = track.name.clone();
        self.history.begin(&label, None, &self.project);
        // Track inserts whose rack slot went.
        let rack_ids: Vec<InsertId> = self.editor_rack.iter().map(|s| s.id).collect();
        let gone: Vec<InsertId> = self
            .rack_sync
            .as_ref()
            .map(|s| {
                s.pairs
                    .iter()
                    .filter(|(r, _)| !rack_ids.contains(r))
                    .map(|&(_, t)| t)
                    .collect()
            })
            .unwrap_or_default();
        if let Some(sync) = &mut self.rack_sync {
            sync.pairs.retain(|(r, _)| rack_ids.contains(r));
        }
        for id in gone {
            self.drop_project_insert(id);
        }
        // New rack slots: a copy each on the track.
        for slot in self.editor_rack.clone() {
            if self.rack_counterpart(slot.id).is_some() {
                continue;
            }
            let copy = InsertSlot {
                id: self.project.next_id(),
                state: self
                    .plugins
                    .get(&slot.id)
                    .and_then(|p| p.save_state().ok())
                    .map(Arc::new)
                    .or(slot.state.clone()),
                ..slot.clone()
            };
            if let Some(track) = self.project.track_mut(track_id) {
                track.inserts.push(copy.clone());
            }
            if let Some(sync) = &mut self.rack_sync {
                sync.pairs.push((slot.id, copy.id));
            }
            self.queue_copy(copy, name.clone(), slot.id);
        }
        // Their order and bypass.
        let order: Vec<(InsertId, bool)> = self
            .editor_rack
            .iter()
            .filter_map(|s| Some((self.rack_counterpart(s.id)?, s.bypass)))
            .collect();
        if let Some(track) = self.project.track_mut(track_id) {
            let mut slots = std::mem::take(&mut track.inserts);
            let mut ordered = Vec::with_capacity(slots.len());
            for (id, bypass) in order {
                if let Some(k) = slots.iter().position(|s| s.id == id) {
                    let mut slot = slots.remove(k);
                    slot.bypass = bypass;
                    ordered.push(slot);
                }
            }
            ordered.extend(slots);
            track.inserts = ordered;
        }
        self.graph_dirty = true;
        self.history.end(&self.project);
    }

    /// Take a rack slot out and destroy its plug-in (it is in no graph): the editor's half of
    /// [`Self::remove_insert`], which needs no `update`.
    pub(super) fn remove_rack_slot(&mut self, id: InsertId) {
        self.load_failures.remove(&id);
        // The preview plays a snapshot of the rack: stop it before the plug-in goes.
        self.release_preview();
        self.flush_pending();
        self.editor_rack.retain(|s| s.id != id);
        if let Some(loading) = &mut self.loading {
            let before = loading.queue.len();
            loading.queue.retain(|(s, _)| s.id != id);
            loading.total -= before - loading.queue.len();
        }
        if let Some(mut live) = self.plugins.remove(&id) {
            if self.applying.is_some() {
                // An Apply renders through it: its window closes now, and it is destroyed
                // once the Apply hands its chain back, so that render goes on unchanged.
                if let Some(mut window) = live.window.take() {
                    window.close();
                }
                live.native_open = false;
                self.retired.push(live);
            } else {
                drop(live);
            }
        }
    }

    /// Take a track's insert out of the project and let its plug-in go once the engine runs a
    /// graph without it ([`Self::remove_insert`] without its `update`).
    fn drop_project_insert(&mut self, id: InsertId) {
        self.load_failures.remove(&id);
        if let Some(loading) = &mut self.loading {
            let before = loading.queue.len();
            loading.queue.retain(|(s, _)| s.id != id);
            loading.total -= before - loading.queue.len();
        }
        self.project.remove_insert(id);
        self.graph_dirty = true;
        let restoring = self.loading.as_ref().is_some_and(|l| l.kind.restores());
        if self.rendering.is_none() && !restoring {
            self.send_project_graph();
        }
        self.flush_pending();
        self.release_plugin(id);
    }

    /// A parameter set by hand on `insert` (now holding `value`) reaches its counterpart.
    pub(super) fn mirror_parameter(&mut self, insert: InsertId, id: u32, value: f64) {
        if self.rack_syncing {
            return;
        }
        let Some(other) = self.rack_counterpart(insert) else {
            return;
        };
        self.rack_syncing = true;
        if self.set_parameter_here(other, id, value).is_ok() {
            if let Some(sync) = &mut self.rack_sync {
                sync.echoes.push((other, id, value, Instant::now()));
            }
        }
        self.rack_syncing = false;
    }

    /// Editor knob gestures (from [`Self::take_parameter_changes`]) reach their counterparts;
    /// a gesture that is the echo of a mirrored value is dropped from the list.
    pub(super) fn mirror_gestures(
        &mut self,
        gestures: &mut Vec<(
            crate::automation::LaneOwner,
            InsertId,
            crate::param_cache::Gesture,
        )>,
    ) {
        use vst3_host::ParameterEditKind;
        let Some(sync) = &mut self.rack_sync else {
            return;
        };
        sync.echoes.retain(|e| e.3.elapsed() < ECHO);
        let mut mirrored = Vec::new();
        gestures.retain(|(_, insert, gesture)| {
            let (ParameterEditKind::ValueChange, Some(value)) =
                (&gesture.edit.kind, gesture.edit.value)
            else {
                return true;
            };
            let echo = sync.echoes.iter().position(|e| {
                e.0 == *insert && e.1 == gesture.edit.id && (e.2 - value).abs() < 1e-9
            });
            if let Some(k) = echo {
                sync.echoes.remove(k);
                return false;
            }
            if sync.counterpart(*insert).is_some() {
                mirrored.push((*insert, gesture.edit.id, value.clamp(0.0, 1.0)));
            }
            true
        });
        for (insert, id, value) in mirrored {
            self.mirror_parameter(insert, id, value);
        }
    }

    /// Copy `insert`'s whole state to its counterpart, if it has one (soon: one copy an
    /// update): for a change its parameters can't show, such as a preset loaded inside the
    /// plug-in or a noise print learned. Nothing is copied when the state is the one last
    /// copied.
    pub fn mirror_state(&mut self, insert: InsertId) {
        if let Some(sync) = &mut self.rack_sync {
            if sync.counterpart(insert).is_some() && !sync.states.contains(&insert) {
                sync.states.push_back(insert);
            }
        }
    }

    /// One whole-state copy waiting (from `update`), or all of them without staged loads.
    pub(super) fn state_step(&mut self) {
        loop {
            let Some(from) = self.rack_sync.as_mut().and_then(|s| s.states.pop_front()) else {
                return;
            };
            if let Err(e) = self.copy_state(from) {
                self.notices
                    .push(format!("Syncing a plug-in's settings: {e:#}"));
            }
            if self.stage_loads {
                return;
            }
        }
    }

    fn copy_state(&mut self, from: InsertId) -> Result<()> {
        if self.applying.is_some() {
            // Its plug-ins are the Apply's until it ends: again then.
            if let Some(sync) = &mut self.rack_sync {
                sync.states.push_back(from);
            }
            return Ok(());
        }
        let Some(to) = self.rack_counterpart(from) else {
            return Ok(());
        };
        let (Some(source), true) = (self.plugins.get(&from), self.plugins.contains_key(&to)) else {
            return Ok(());
        };
        let state = Arc::new(source.save_state()?);
        let last = self
            .rack_sync
            .as_ref()
            .and_then(|s| s.last_state.get(&from));
        if last == Some(&state) {
            return Ok(());
        }
        // Changed unseen: the source's own cached values may be stale too.
        if let Some(cache) = self.plugins.get_mut(&from).and_then(|p| p.params.as_mut()) {
            cache.refresh()?;
        }
        // The preview's settle and the engine's blocks take the plug-in's lock too: the state
        // goes in between two of them.
        let live = self.plugins.get_mut(&to).expect("checked");
        let mut latency = None;
        match &mut live.instance {
            Instance::Vst3 { plugin, .. } => {
                let mut plugin = plugin.lock().map_err(|_| anyhow!("lock poisoned"))?;
                plugin
                    .load_state(&state)
                    .map_err(|e| anyhow!("setting the state: {e}"))?;
                latency = Some(plugin.latency_samples() as usize);
            }
            Instance::Native(native) => native.restore(&state)?,
        }
        if let Some(cache) = &mut live.params {
            cache.refresh()?;
        }
        if let Some(latency) = latency.filter(|l| *l != live.latency) {
            live.latency = latency;
            self.graph_dirty = true;
        }
        let after = Arc::new(live.save_state()?);
        if let Some(sync) = &mut self.rack_sync {
            sync.last_state.insert(from, state);
            sync.last_state.insert(to, after);
        }
        Ok(())
    }

    /// The track the rack would sync with for `document` and whether it is synced with it now:
    /// `None` for a file not opened from a clip (or whose clip is gone).
    pub fn sync_track_of(&self, document: DocumentId) -> Option<(TrackId, bool)> {
        let (_, clip) = self.document(document)?.source?;
        let (track, _) = self.project.find_clip(clip)?;
        let synced = self
            .rack_sync
            .as_ref()
            .is_some_and(|s| s.document == document && s.track == track);
        Some((track, synced))
    }

    /// While synced with `document`'s track: the track inserts an Apply of the rack now would
    /// print into the file (their rack copies switched on and loaded), which the track would
    /// then play again. Empty when not synced with it.
    pub fn applied_track_inserts(&self, document: DocumentId) -> Vec<InsertId> {
        let Some(sync) = self.rack_sync.as_ref().filter(|s| s.document == document) else {
            return Vec::new();
        };
        self.editor_rack
            .iter()
            .filter(|s| !s.bypass && self.plugins.contains_key(&s.id))
            .filter_map(|s| sync.counterpart(s.id))
            .collect()
    }

    /// Apply the rack into `document` ([`Self::apply_effects`]) while it is synced with that
    /// file's track, and once the Apply lands, switch off the track's inserts it printed, so
    /// the clip isn't processed twice: an undo step of the project's own ("Turn off …"), the
    /// Apply being a step of the file's history. A cancelled or failed Apply switches nothing.
    pub fn apply_effects_turning_off(
        &mut self,
        document: DocumentId,
        range: Option<(u64, u64)>,
    ) -> Result<()> {
        let inserts = self.applied_track_inserts(document);
        let track = match &self.rack_sync {
            Some(s) if s.document == document => s.track,
            _ => bail!(
                "the effects rack isn't synced with document {document}'s track: there is \
                 nothing to turn off (apply without it)"
            ),
        };
        self.apply_effects(document, range)?;
        if let Some(job) = &mut self.applying {
            job.turn_off = Some((track, inserts));
        }
        Ok(())
    }

    /// An Apply that landed and was asked to: the track's printed inserts switched off.
    pub(super) fn turn_off_applied(&mut self, track: TrackId, inserts: &[InsertId]) {
        let Some(name) = self.project.track(track).map(|t| t.name.clone()) else {
            return;
        };
        self.begin_edit(
            &format!("Turn off {name}'s effects (applied into the file)"),
            None,
        );
        if let Some(t) = self.project.track_mut(track) {
            for slot in t.inserts.iter_mut().filter(|s| inserts.contains(&s.id)) {
                slot.bypass = true;
            }
        }
        self.end_edit();
    }
}

#[cfg(test)]
#[path = "rack_sync_tests.rs"]
mod tests;
