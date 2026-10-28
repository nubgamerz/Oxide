//! Saving without a hang (dev plan 20.1). A save's slow part is reading each plug-in's state:
//! a VST3 makes it in `IComponent::getState`, on the main thread (the spec's thread for it)
//! and under the plug-in's lock (TD148), and some take long (a piano sampler 1.5-1.7 s, a
//! sampler with a kit about 40 ms; mixing, mastering and amp-sim plug-ins 0.1-2 ms). Serialising and writing even a
//! 3 MB project takes milliseconds, on a worker since 15.2.
//!
//! So the app's save ([`Session::save_in_background`]) only begins: the states are read over
//! the next updates, a VST3 an update (built-in effects, microseconds each, together), each
//! into its slot exactly as [`Session::save`] reads them all at once; then the project is named
//! after the file, cloned and written on a worker. The bytes are the plain save's. The app holds
//! the first read until its busy bar has been painted ([`Session::hold_saving`]), so the window
//! says "Saving…" before any frame waits on a plug-in.
//!
//! The same reads serve autosave (silently: a backup written on a worker only if it differs from
//! the last) and the "Save the changes?" question's look for changes
//! ([`Session::check_unsaved`]), which read every state too.

use super::{write_whole, InsertId, InsertTarget, Instance, LoadKind, SaveJob, Session};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

/// What the plug-ins' states are being read for.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Purpose {
    /// Saving to this file (behind the busy bar).
    Save(PathBuf),
    /// An autosave's backup (silently).
    Backup,
    /// Whether anything is unsaved (the "Save the changes?" question, behind the bar).
    Check,
}

/// Plug-in states being read, a VST3 an update.
pub(super) struct Capture {
    purpose: Purpose,
    /// Still to read, in the project's order: the insert, its name and where it is.
    queue: VecDeque<(InsertId, String, String)>,
    total: usize,
}

/// How far a save, or the look for unsaved changes, has come ([`Session::save_progress`]).
#[derive(Debug, Clone, PartialEq)]
pub struct SaveProgress {
    /// The file being saved; `None` while looking for unsaved changes.
    pub path: Option<PathBuf>,
    /// Plug-in states read, of `total`.
    pub done: usize,
    pub total: usize,
    /// The plug-in read next (the one a frame may wait on), and its track ("Master" for the
    /// master's).
    pub next: Option<String>,
    pub track: Option<String>,
    /// Every state is in: the file is being written (or the project compared) on a worker.
    pub writing: bool,
}

impl SaveProgress {
    /// How far, 0 to 1 (the reads; the write is the last step).
    pub fn fraction(&self) -> f32 {
        if self.writing {
            1.0
        } else {
            self.done as f32 / (self.total + 1) as f32
        }
    }
}

/// An autosave's backup being written: its JSON, or `None` when nothing changed.
pub(super) type BackupJob = JoinHandle<Result<Option<String>>>;

impl Session {
    /// Save to `path` without holding the window: this only begins it. The plug-ins' states are
    /// read over the next [`Self::update`]s (a VST3 each, after [`Self::hold_saving`] lets go),
    /// then the file is written on a worker; [`Self::saving`] names it meanwhile and
    /// [`Self::take_saved`] says how it ended. The file is byte for byte what [`Self::save`]
    /// writes. One at a time; refused while plug-ins load.
    pub fn save_in_background(&mut self, path: &Path) -> Result<()> {
        if self.saving().is_some() {
            bail!("a save is already being written; wait for it to finish");
        }
        if self.loading.is_some() {
            bail!("plug-ins are loading; save once they are in");
        }
        let path = super::with_project_extension(path);
        // An autosave's or a look for changes' reads give way (the save reads every state).
        self.drop_capture();
        self.capture = Some(self.capture_for(Purpose::Save(path)));
        Ok(())
    }

    /// The file a save is writing (its states being read, or the file written), while it is.
    pub fn saving(&self) -> Option<&Path> {
        match self.capture.as_ref().map(|c| &c.purpose) {
            Some(Purpose::Save(path)) => Some(path),
            _ => self.saving.as_ref().map(|s| s.path.as_path()),
        }
    }

    /// How far a save, or the look for unsaved changes ([`Self::check_unsaved`]), has come:
    /// what the busy bar shows. `None` otherwise (an autosave reads silently).
    pub fn save_progress(&self) -> Option<SaveProgress> {
        if let Some(capture) = &self.capture {
            let path = match &capture.purpose {
                Purpose::Save(path) => Some(path.clone()),
                Purpose::Check => None,
                Purpose::Backup => return None,
            };
            let front = capture.queue.front();
            return Some(SaveProgress {
                path,
                done: capture.total - capture.queue.len(),
                total: capture.total,
                next: front.map(|(_, name, _)| name.clone()),
                track: front.map(|(_, _, track)| track.clone()),
                writing: false,
            });
        }
        let path = match (&self.saving, &self.checking) {
            (Some(job), _) => Some(job.path.clone()),
            (None, Some(_)) => None,
            (None, None) => return None,
        };
        Some(SaveProgress {
            path,
            done: 0,
            total: 0,
            next: None,
            track: None,
            writing: true,
        })
    }

    /// Hold a save's (or a look for changes') plug-in reads back while set: the app holds them
    /// until its busy bar has been painted, so no frame waits on a plug-in before "Saving…"
    /// is on screen. [`Self::finish_saving`] doesn't wait for it.
    pub fn hold_saving(&mut self, hold: bool) {
        self.saves_held = hold;
    }

    /// Wait for a save to be written (its states read now, the worker joined): the app's exit,
    /// tools and tests. Nothing to do without one.
    pub fn finish_saving(&mut self) -> Result<()> {
        if matches!(
            self.capture.as_ref().map(|c| &c.purpose),
            Some(Purpose::Save(_))
        ) {
            // How an earlier save ended, if no one has asked yet, stays as it was.
            let earlier = self.saved.take();
            while self.capture.is_some() {
                self.capture_step(true);
            }
            let refused = std::mem::replace(&mut self.saved, earlier);
            // Refused before the write began (preparing it failed).
            if let Some(Err(e)) = refused {
                return Err(anyhow!(e));
            }
        }
        let Some(job) = self.saving.take() else {
            return Ok(());
        };
        let written = job
            .handle
            .join()
            .map_err(|_| anyhow!("the save stopped unexpectedly"))?;
        let json = written?;
        self.finish_save(job.path, json);
        Ok(())
    }

    /// Whether the project differs from its file (as [`Self::has_unsaved_changes`]), without
    /// holding the window: the first call begins reading the plug-ins' states (a VST3 an
    /// update, behind the busy bar: [`Self::save_progress`] with no path), and the project is
    /// compared on a worker; `None` until the answer is in, then the answer, once.
    pub fn check_unsaved(&mut self) -> Option<bool> {
        self.check_unsaved_with(false)
    }

    /// [`Self::check_unsaved`]; `staged` reads behind the bar even with no VST3 (tests).
    pub(crate) fn check_unsaved_with(&mut self, staged: bool) -> Option<bool> {
        if let Some(answer) = self.unsaved_answer.take() {
            return Some(answer);
        }
        // An open's project isn't whole yet: nothing to lose.
        if self
            .loading
            .as_ref()
            .is_some_and(|l| l.kind == LoadKind::Open)
        {
            return Some(false);
        }
        let looking = self.checking.is_some()
            || matches!(
                self.capture.as_ref().map(|c| &c.purpose),
                Some(Purpose::Check)
            );
        if !looking {
            if self.saving().is_some() {
                // A save first; asked again after it.
                return None;
            }
            // No VST3 to wait on (only built-in effects, microseconds each): answered now, with
            // no bar to flash before the question or the close.
            let vst3 = self.project.all_inserts().any(|s| {
                self.plugins
                    .get(&s.id)
                    .is_some_and(|live| matches!(live.instance, Instance::Vst3 { .. }))
            });
            if !vst3 && !staged {
                self.drop_capture();
                return Some(self.has_unsaved_changes());
            }
            self.drop_capture();
            self.capture = Some(self.capture_for(Purpose::Check));
        }
        None
    }

    /// The reads' share of [`Self::update`]: the next plug-in's state (a VST3, or every
    /// built-in effect up to the next one), and the work they were for once all are in.
    pub(super) fn saving_step(&mut self) {
        self.capture_step(false);
        self.poll_save();
        self.poll_backup();
        self.poll_check();
    }

    /// Read states: all (`wait`), or up to and including the next VST3's.
    pub(super) fn capture_step(&mut self, wait: bool) {
        let Some(purpose) = self.capture.as_ref().map(|c| c.purpose.clone()) else {
            return;
        };
        // An autosave gives way to playing, recording or an export (reading a state takes the
        // plug-in's lock, which can cost its track a block); it comes back when due.
        if purpose == Purpose::Backup && (self.autosave_busy() || wait) {
            self.capture = None;
            return;
        }
        if self.saves_held && !wait && purpose != Purpose::Backup {
            return;
        }
        while let Some((id, _, _)) = self.capture.as_mut().and_then(|c| c.queue.pop_front()) {
            if self.capture_state(id) && !wait {
                break;
            }
        }
        if self.capture.as_ref().is_some_and(|c| c.queue.is_empty()) {
            let capture = self.capture.take().expect("checked");
            self.captured(capture.purpose);
        }
    }

    /// Every state is in: what they were read for.
    fn captured(&mut self, purpose: Purpose) {
        match purpose {
            Purpose::Save(path) => match self.prepare_save(&path) {
                Ok(work) => {
                    let spawned = std::thread::Builder::new()
                        .name("project save".into())
                        .spawn(move || work.run())
                        .context("starting the save");
                    match spawned {
                        Ok(handle) => self.saving = Some(SaveJob { path, handle }),
                        Err(e) => self.saved = Some(Err(format!("{e:#}"))),
                    }
                }
                Err(e) => self.saved = Some(Err(format!("{e:#}"))),
            },
            Purpose::Backup => {
                if let Err(e) = self.write_backup_in_background() {
                    self.notices.push(format!("Autosave: {e:#}"));
                }
            }
            Purpose::Check => {
                let project = self.project.clone();
                let saved = self.saved_json.clone();
                let spawned = std::thread::Builder::new()
                    .name("unsaved check".into())
                    .spawn(move || {
                        serde_json::to_string_pretty(&project)
                            .map_or(true, |json| super::differs(&json, saved.as_deref()))
                    });
                match spawned {
                    Ok(handle) => self.checking = Some(handle),
                    // Can't tell: ask.
                    Err(_) => self.unsaved_answer = Some(true),
                }
            }
        }
    }

    /// Read insert `id`'s state into its slot now (and into the edit in progress's snapshot,
    /// so the edit doesn't record it), as [`Self::capture_plugin_states`] does for each.
    /// Returns whether it was a VST3's (the slow kind).
    fn capture_state(&mut self, id: InsertId) -> bool {
        let Some(live) = self.plugins.get(&id) else {
            return false;
        };
        let vst3 = matches!(live.instance, Instance::Vst3 { .. });
        let read = live.save_state();
        let Some(slot) = self.project.all_inserts_mut().find(|s| s.id == id) else {
            return vst3;
        };
        match read {
            Ok(state) => {
                let state = Arc::new(state);
                slot.state = Some(state.clone());
                self.history.update_pending(|project| {
                    if let Some(slot) = project.all_inserts_mut().find(|s| s.id == id) {
                        slot.state = Some(state);
                    }
                });
            }
            Err(e) => {
                let name = slot.name.clone();
                self.notices.push(format!("{name}: saving state: {e:#}"));
            }
        }
        vst3
    }

    /// Reads for `purpose`: every insert of the project with a live plug-in, in order.
    fn capture_for(&self, purpose: Purpose) -> Capture {
        let queue: VecDeque<(InsertId, String, String)> = self
            .project
            .all_inserts()
            .filter(|s| self.plugins.contains_key(&s.id))
            .map(|s| {
                let place = match self.insert_owner(s.id) {
                    Some(InsertTarget::Track(t)) => self
                        .project
                        .track(t)
                        .map(|t| t.name.clone())
                        .unwrap_or_default(),
                    Some(InsertTarget::Master) => "Master".to_string(),
                    _ => String::new(),
                };
                (s.id, s.name.clone(), place)
            })
            .collect();
        Capture {
            total: queue.len(),
            purpose,
            queue,
        }
    }

    /// An autosave's or a look for changes' reads given up (a save, an open); a save's are
    /// never dropped here.
    pub(super) fn drop_capture(&mut self) {
        if !matches!(
            self.capture.as_ref().map(|c| &c.purpose),
            Some(Purpose::Save(_))
        ) {
            self.capture = None;
        }
    }

    /// The project is about to be replaced (an open, a new project): a save under way is
    /// finished first (it is this project's; how it ended stays for [`Self::take_saved`]), and
    /// an autosave's or a look for changes' reads are given up.
    pub(super) fn settle_saving(&mut self) {
        if let Some(path) = self.saving().map(Path::to_path_buf) {
            self.saved = Some(
                self.finish_saving()
                    .map(|()| path)
                    .map_err(|e| format!("{e:#}")),
            );
        }
        self.capture = None;
        self.checking = None;
        self.unsaved_answer = None;
    }

    /// Autosave's reads begin: every state over the next updates, then the backup written on
    /// a worker if it differs from the last.
    pub(super) fn begin_autosave(&mut self) {
        if self.capture.is_none() && self.backing_up.is_none() {
            self.capture = Some(self.capture_for(Purpose::Backup));
        }
    }

    /// Whether an autosave's reads, or its backup's write, are under way.
    pub fn autosaving(&self) -> bool {
        self.backing_up.is_some()
            || matches!(
                self.capture.as_ref().map(|c| &c.purpose),
                Some(Purpose::Backup)
            )
    }

    /// The project (its states read) written as a backup on a worker, unless it is the last
    /// one backed up or saved.
    fn write_backup_in_background(&mut self) -> Result<()> {
        let folder = self.project_folder(crate::folder::BACKUPS)?;
        let stem = crate::folder::stamped(&self.project.name);
        let keep = self.keep_backups.max(1);
        let baseline = self.backup_baseline.clone();
        // With the Waveform view's open files, as a save writes them.
        let project = self.project_for_file(self.project_dir().as_deref());
        let handle = std::thread::Builder::new()
            .name("autosave".into())
            .spawn(move || {
                let json = serde_json::to_string_pretty(&project)?;
                if baseline.as_ref() == Some(&json) {
                    return Ok(None);
                }
                let path = crate::folder::free_name(&folder, &stem, crate::folder::EXTENSION);
                write_whole(&path, &json)?;
                super::prune_backups(&folder, keep);
                Ok(Some(json))
            })
            .context("starting the autosave")?;
        self.backing_up = Some(handle);
        Ok(())
    }

    fn poll_backup(&mut self) {
        if !self.backing_up.as_ref().is_some_and(|h| h.is_finished()) {
            return;
        }
        let Some(handle) = self.backing_up.take() else {
            return;
        };
        match handle.join() {
            Ok(Ok(Some(json))) => self.backup_baseline = Some(json),
            Ok(Ok(None)) => {}
            Ok(Err(e)) => self.notices.push(format!("Autosave: {e:#}")),
            Err(_) => self
                .notices
                .push("Autosave: the backup stopped unexpectedly".into()),
        }
    }

    fn poll_check(&mut self) {
        if !self.checking.as_ref().is_some_and(|h| h.is_finished()) {
            return;
        }
        let Some(handle) = self.checking.take() else {
            return;
        };
        // Can't tell: ask.
        self.unsaved_answer = Some(handle.join().unwrap_or(true));
    }

    /// A background save that has been written: finished (the project's path, the backup
    /// baseline), and how it ended kept for [`Self::take_saved`].
    pub(super) fn poll_save(&mut self) {
        if !self.saving.as_ref().is_some_and(|s| s.handle.is_finished()) {
            return;
        }
        let path = self
            .saving
            .as_ref()
            .map(|s| s.path.clone())
            .unwrap_or_default();
        self.saved = Some(
            self.finish_saving()
                .map(|()| path)
                .map_err(|e| format!("{e:#}")),
        );
    }
}
