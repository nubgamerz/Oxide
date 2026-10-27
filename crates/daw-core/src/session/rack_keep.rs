//! The Waveform view's effects rack, kept (dev plan 18.2): between runs in one file beside the
//! program ([`Session::rack_file`], `rack.json`), and as named presets
//! ([`Session::rack_presets_dir`], `RackPresets/<name>.json`).
//!
//! What is kept is each slot's plug-in, bypass and settings, read from the live plug-in
//! ([`Session::rack_as_file`]). A plug-in's knobs turned in its own window send no word, so the
//! rack is looked at a moment after a slot is added, moved, switched or removed, and otherwise
//! every few seconds while nothing plays through it, and written (on a worker) only when it
//! differs from what was last written; once more on exit ([`Session::save_rack_now`]).
//!
//! A rack coming back (the last run's, or a preset) has its slots at once; its plug-ins load as
//! an open's do, one a frame behind the app's busy overlay (a `Loading` with `rack` set, which
//! leaves the project as it is), restored from their settings after activation. A plug-in that
//! fails to load keeps its slot and its settings (greyed, saying why), so the next write keeps
//! them too.

use super::{InsertId, InsertSlot, LoadKind, Loading, Reads, Session};
use crate::rack::{self, presets, RackFile, RackSlot};
use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// How long after a slot changes the rack is looked at (several changes in a row write once).
const AFTER_CHANGE: Duration = Duration::from_millis(500);
/// How often it is looked at otherwise, for settings changed in a plug-in's own window.
const LOOK_EVERY: Duration = Duration::from_secs(5);
/// What the queue calls the rack's slots while they load (the busy overlay's "on …").
pub(super) const RACK_LABEL: &str = "the effects rack";

/// What keeping the rack remembers.
pub(super) struct Keeper {
    /// The rack as last written (or read): nothing is written while it is the same.
    kept: Option<RackFile>,
    /// The slots and their bypass when last looked at, to notice a change cheaply.
    shape: Vec<(InsertId, bool)>,
    /// When to look next.
    due: Instant,
    /// The write in flight.
    writer: Option<JoinHandle<Result<()>>>,
}

impl Default for Keeper {
    fn default() -> Self {
        Self {
            kept: None,
            shape: Vec::new(),
            due: Instant::now() + LOOK_EVERY,
            writer: None,
        }
    }
}

/// Write `rack` to `path` (its folder made), whole or not at all.
fn write(path: &Path, rack: &RackFile) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    crate::session::write_whole(path, &serde_json::to_string(rack)?)
}

impl Session {
    /// Read each loaded rack plug-in's settings into its slot (a slot whose plug-in didn't load
    /// keeps the settings it came with).
    pub(super) fn capture_rack_states(&mut self) {
        let mut failures = Vec::new();
        for slot in &mut self.editor_rack {
            if let Some(live) = self.plugins.get(&slot.id) {
                match live.save_state() {
                    Ok(state) => slot.state = Some(std::sync::Arc::new(state)),
                    Err(e) => failures.push(format!("{}: saving state: {e:#}", slot.name)),
                }
            }
        }
        self.notices.extend(failures);
    }

    /// The rack as it would be kept now: its slots in order, each plug-in's settings read now.
    pub fn rack_as_file(&mut self) -> RackFile {
        self.capture_rack_states();
        RackFile::new(
            self.editor_rack
                .iter()
                .map(|s| RackSlot {
                    name: s.name.clone(),
                    path: s.path.clone(),
                    bypass: s.bypass,
                    state: s.state.clone(),
                })
                .collect(),
        )
    }

    /// The rack to keep between runs: the user's own, which a synced rack (dev plan 21.2) has
    /// set aside, never the track's chain it shows meanwhile.
    fn kept_rack(&mut self) -> RackFile {
        match &self.rack_sync {
            Some(sync) => sync.own.clone(),
            None => self.rack_as_file(),
        }
    }

    /// Why insert `id` didn't load, if it failed to (its slot stays, greyed, with its settings).
    pub fn load_failure(&self, id: InsertId) -> Option<&str> {
        self.load_failures.get(&id).map(String::as_str)
    }

    /// Bring back the rack kept in [`Self::rack_file`] (the last run's), replacing the rack:
    /// its slots at once, its plug-ins loaded as an open's are (a plug-in a frame with
    /// [`Self::stage_loads`], else before this returns). Returns how many slots it has (0 with
    /// no file kept yet). A file that can't be read is put aside (`rack.json.unreadable`), so
    /// writing the empty rack over it loses nothing.
    pub fn restore_rack(&mut self) -> Result<usize> {
        let Some(path) = self.rack_file.clone() else {
            return Ok(0);
        };
        if !path.is_file() {
            return Ok(0);
        }
        let file = match rack::read(&path) {
            Ok(file) => file,
            Err(e) => {
                let mut aside = path.clone().into_os_string();
                aside.push(".unreadable");
                let aside = PathBuf::from(aside);
                let _ = std::fs::rename(&path, &aside);
                bail!("{e:#} (put aside as {})", aside.display());
            }
        };
        let slots = file.slots.len();
        self.rack_keep.kept = Some(file.clone());
        self.set_rack(file)?;
        Ok(slots)
    }

    /// Make `file` the rack, in place of the one there: the old one's plug-ins are destroyed,
    /// the new slots appear at once and their plug-ins load (staged as an open's). Returns the
    /// new slots' ids.
    pub(super) fn set_rack(&mut self, file: RackFile) -> Result<Vec<InsertId>> {
        self.set_rack_copying(file, &[])
    }

    /// Why the rack can't be replaced now, if it can't: an Apply renders through it, or plug-ins
    /// other than the rack's are loading.
    pub(super) fn check_rack_replaceable(&self) -> Result<()> {
        if self.applying.is_some() {
            bail!("the effects rack is busy applying; wait for it to finish");
        }
        match self.loading.as_ref().map(|l| l.kind) {
            None | Some(LoadKind::Rack) => Ok(()),
            Some(LoadKind::Open) => {
                bail!("a project is being opened; change the effects rack once it is in")
            }
            Some(_) => {
                bail!("plug-ins are still loading; change the effects rack once they are in")
            }
        }
    }

    /// [`Self::set_rack`], each slot `k` with `copies[k]` taking that insert's settings as it
    /// loads (a synced rack, dev plan 21.2) rather than the file's.
    pub(super) fn set_rack_copying(
        &mut self,
        file: RackFile,
        copies: &[Option<InsertId>],
    ) -> Result<Vec<InsertId>> {
        self.check_rack_replaceable()?;
        // The preview plays the rack's plug-ins: off the audio thread before they go.
        self.release_preview();
        self.flush_pending();
        // A rack still loading gives way to this one; a track's insert queued beside it (a
        // synced rack's copy) still loads.
        let mut queue = std::collections::VecDeque::new();
        let mut copying = std::collections::HashMap::new();
        if let Some(loading) = self.loading.take() {
            for (slot, label) in loading.queue {
                if self.project.find_insert(slot.id).is_some() {
                    if let Some(&from) = loading.copies.get(&slot.id) {
                        copying.insert(slot.id, from);
                    }
                    queue.push_back((slot, label));
                }
            }
        }
        for slot in std::mem::take(&mut self.editor_rack) {
            self.load_failures.remove(&slot.id);
            // Not in any graph, and the preview let go of it: destroyed here.
            drop(self.plugins.remove(&slot.id));
        }
        let mut ids = Vec::new();
        for (k, kept) in file.slots.into_iter().enumerate() {
            self.next_rack_id += 1;
            let slot = InsertSlot {
                id: self.next_rack_id,
                name: kept.name,
                path: kept.path,
                bypass: kept.bypass,
                state: kept.state,
            };
            if let Some(Some(from)) = copies.get(k) {
                copying.insert(slot.id, *from);
            }
            ids.push(slot.id);
            queue.push_back((slot.clone(), RACK_LABEL.to_string()));
            self.editor_rack.push(slot);
        }
        if !queue.is_empty() {
            self.loading = Some(Loading {
                kind: LoadKind::Rack,
                label: None,
                retiring: Vec::new(),
                closing_total: self.closing.len(),
                reads: Reads::start(Vec::new(), self.sample_rate, self.read_hold.clone()),
                total: queue.len(),
                queue,
                path: None,
                backup: false,
                previous: self.path.clone(),
                copies: copying,
            });
            if !self.stage_loads {
                self.finish_loading();
            }
        }
        Ok(ids)
    }

    fn rack_presets_folder(&self) -> Result<&Path> {
        self.rack_presets_dir
            .as_deref()
            .ok_or_else(|| anyhow!("no rack presets folder is set"))
    }

    /// The saved rack presets, by name.
    pub fn rack_presets(&self) -> Vec<presets::Preset> {
        self.rack_presets_dir
            .as_deref()
            .map(presets::list)
            .unwrap_or_default()
    }

    /// Save the rack as it is now (each plug-in's settings read now) as the preset `name`.
    /// Refused for an empty rack, a name that can't be a file's, or a preset of that name
    /// unless `replace`. Returns its path.
    pub fn save_rack_preset(&mut self, name: &str, replace: bool) -> Result<PathBuf> {
        let dir = self.rack_presets_folder()?.to_path_buf();
        presets::check_name(name)?;
        if self.editor_rack.is_empty() {
            bail!("the effects rack is empty: add plug-ins to it before saving it as a preset");
        }
        let file = self.rack_as_file();
        presets::save(&dir, name, &file, replace)
    }

    /// Replace the rack with the preset `name`: its slots at once, its plug-ins loaded with
    /// their settings (staged as an open's). Returns the new slots' ids.
    pub fn load_rack_preset(&mut self, name: &str) -> Result<Vec<InsertId>> {
        if let Some(sync) = &self.rack_sync {
            let track = self
                .project
                .track(sync.track)
                .map_or_else(|| format!("track {}", sync.track), |t| t.name.clone());
            bail!(
                "the effects rack is synced with {track}: turn sync off before loading a preset                  (it would replace {track}'s effects too)"
            );
        }
        let file = presets::load(self.rack_presets_folder()?, name)?;
        self.set_rack(file)
    }

    pub fn delete_rack_preset(&self, name: &str) -> Result<()> {
        presets::delete(self.rack_presets_folder()?, name)
    }

    /// Keep the rack (from `update`): a moment after its slots change, and every few seconds
    /// otherwise, while nothing plays through it, written on a worker if it differs from what
    /// was last written.
    pub(super) fn keep_rack_tick(&mut self) {
        let Some(path) = self.rack_file.clone() else {
            return;
        };
        if self
            .rack_keep
            .writer
            .as_ref()
            .is_some_and(JoinHandle::is_finished)
        {
            let writer = self.rack_keep.writer.take().expect("checked");
            if let Ok(Err(e)) = writer.join() {
                self.notices
                    .push(format!("Keeping the effects rack: {e:#}"));
            }
        }
        let now = Instant::now();
        let shape: Vec<(InsertId, bool)> =
            self.editor_rack.iter().map(|s| (s.id, s.bypass)).collect();
        if shape != self.rack_keep.shape {
            self.rack_keep.shape = shape;
            self.rack_keep.due = now + AFTER_CHANGE;
        }
        // Reading a plug-in's settings takes its lock, which a preview or an Apply would miss;
        // and nothing is new while plug-ins are still coming in.
        let busy = self.previewing.is_some() || self.applying.is_some() || self.loading.is_some();
        if now < self.rack_keep.due || busy || self.rack_keep.writer.is_some() {
            return;
        }
        self.rack_keep.due = now + LOOK_EVERY;
        let file = self.kept_rack();
        if self.rack_keep.kept.as_ref() == Some(&file) {
            return;
        }
        self.rack_keep.kept = Some(file.clone());
        let spawned = std::thread::Builder::new()
            .name("keep rack".into())
            .spawn(move || write(&path, &file));
        match spawned {
            Ok(handle) => self.rack_keep.writer = Some(handle),
            Err(e) => self
                .notices
                .push(format!("Keeping the effects rack: {e:#}")),
        }
    }

    /// Write the rack now if it changed since it was last written, after any write in flight
    /// (the app's exit; checks). Nothing without [`Self::rack_file`]. Slots whose plug-ins
    /// aren't in (still loading, or failed) keep the settings they came with.
    pub fn save_rack_now(&mut self) -> Result<()> {
        let Some(path) = self.rack_file.clone() else {
            return Ok(());
        };
        if let Some(writer) = self.rack_keep.writer.take() {
            writer
                .join()
                .map_err(|_| anyhow!("the rack's writer panicked"))??;
        }
        let file = self.kept_rack();
        if self.rack_keep.kept.as_ref() == Some(&file) {
            return Ok(());
        }
        write(&path, &file)?;
        self.rack_keep.kept = Some(file);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::AudioSettings;
    use crate::project::TrackKind;
    use crate::session::InsertTarget;

    /// A preset loading (staged, held here) leaves the project's housekeeping running: a track
    /// added meanwhile reaches the engine's graph, as it would while playing or recording.
    #[test]
    fn the_project_goes_on_while_the_rack_loads() {
        let mut s = Session::new(AudioSettings {
            output_device: Some("<none>".into()),
            threads: Some(0),
            ..Default::default()
        });
        s.stage_loads = true;
        let amplify = crate::native::path_of(&crate::native::amplify::KIND);
        s.add_insert(InsertTarget::Editor, &amplify).unwrap();
        let rack = s.rack_as_file();
        s.update();
        s.hold_loading(true);
        s.set_rack(rack).unwrap();
        assert!(s.loading.as_ref().is_some_and(|l| l.kind == LoadKind::Rack));
        let before = s.sent_keys.clone();
        s.add_track(TrackKind::Audio, None);
        s.update();
        assert!(s.loading.is_some(), "still held");
        assert_ne!(s.sent_keys, before, "the graph was rebuilt meanwhile");
        s.hold_loading(false);
        s.update();
        assert!(s.loading.is_none());
        assert!(s.is_loaded(s.editor_rack()[0].id));
    }
}
