//! The running session: the project plus everything live behind it (plugins, audio device,
//! MIDI ports, media, the engine). The UI owns one `Session` on its main thread and edits
//! `session.project` directly; [`Session::update`] (once per frame) pushes those edits to the
//! engine.
//!
//! Plugins are created, edited and destroyed on this (main) thread only, as VST3 requires.

use crate::audio_edit;
use crate::automation::{
    AutomationLane, AutomationMode, AutomationTarget, LaneId, LaneOwner, NewPoint,
};
use crate::automation_write::{WriteKey, Writer};
use crate::device::{self, AudioSettings, RunningDevice};
use crate::diagnostics;
use crate::engine::{self, Command, EngineCore, EngineHandle, Garbage, Preview, RecordedMidi};
use crate::export::{self, ExportJob, ExportSpec};
use crate::graph::midi_channel;
use crate::graph::{
    ClipSource, Graph, GraphBuilder, Meter, NodeAutomation, NodeInput, NodeParams, NodeSpec,
    ParameterLane, Processor, SendParams, SendSpec, SpecTarget, Tap, Transport, WRITE_MUTE,
    WRITE_PAN, WRITE_VOLUME,
};
use crate::grid::{Division, Grid, Rounding};
use crate::history::History;
use crate::media::{self, AudioData, MediaPool};
use crate::metronome::ClickSettings;
use crate::midi::MidiInputs;
use crate::midi_clip::{Note, NoteId};
use crate::midi_record::{self, MidiHit};
use crate::param_cache::{Gesture, ParameterCache};
use crate::plugins::{self, InsertFlags, PluginInsert};
use crate::pool::WorkerPool;
use crate::project::{
    AnyClip, Clip, ClipId, Destination, InsertId, InsertSlot, MetronomeSettings, Monitor, Project,
    Region, SendId, Take, TempoChange, Track, TrackId, TrackInput, TrackKind,
};
use crate::rack;
use crate::record::{self, RecordedFile, TakeSpec};
use crate::sequencer::Sequence;
use crate::smf::{self, MidiFileTrack, TempoSettings};
use crate::stretch;
use crate::tempo::TempoMap;
use crate::util::{db_to_gain, AtomicF32};
use crate::wav;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use vst3_host::parameters::Parameter;
use vst3_host::{AudioBusLayout, Plugin, PluginWindow, Vst3Host};

mod rack_keep;
mod rack_sync;
mod saving;
pub use rack_sync::RackSyncInfo;
pub use saving::SaveProgress;

/// Largest block the engine and plugins process at once.
pub const MAX_BLOCK: usize = 2048;

/// Where a touch lets go (or a write pass ends), the lane glides back to its old curve over
/// this many ticks: a 32nd note.
pub const WRITE_GLIDE: u64 = crate::tempo::TICKS_PER_QUARTER as u64 / 8;
/// Graph node key of the master bus (tracks use their id).
const MASTER_KEY: u64 = u64::MAX;
/// Graph keys of plug-in output edges (`Tap::Output`): the listening track's id above every
/// send id (a u32), so state handover never mixes the two up.
const OUTPUT_EDGE_KEY: u64 = 1 << 40;

/// Plug-ins being loaded a few a frame ([`Session::loading`]), in three stages so the window
/// keeps painting (dev plan 17.1): plug-ins let go of destroyed a few a frame
/// (`Session::closing`), the audio files read on workers ([`Reads`]), then the plug-ins loaded
/// one or a few a frame (VST3 plug-ins must be created and destroyed on the main thread). What
/// it is for, and so what else runs meanwhile, is its [`LoadKind`].
struct Loading {
    kind: LoadKind,
    /// The undo step's name (an undo or redo's).
    label: Option<String>,
    /// Plug-ins an undo or redo dropped (with their names), alive until the graph without them
    /// is sent, once the plug-ins it brings back are in: the engine plays the graph before until
    /// then ([`Session::restore`]).
    retiring: Vec<(InsertId, String, LivePlugin)>,
    /// The last project's plug-ins to destroy when the open began (for the bar).
    closing_total: usize,
    reads: Reads,
    /// The plug-ins to load, with the name of the track each is on.
    queue: std::collections::VecDeque<(InsertSlot, String)>,
    total: usize,
    /// The newest backup after the project file, found once loading ends.
    path: Option<PathBuf>,
    /// A backup opened in the project's place: never what the file holds.
    backup: bool,
    /// The project open before, reopened if this open is cancelled ([`Session::cancel_loading`]).
    previous: Option<PathBuf>,
    /// Slots that copy another insert (a synced rack, dev plan 21.2): each slot's id and the
    /// insert whose settings it takes, read as it loads (a plug-in a frame, not all at once).
    copies: HashMap<InsertId, InsertId>,
}

/// What plug-ins are loading for ([`Session::loading`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadKind {
    /// A project being opened (or a backup, or a project from a template): nothing of it runs
    /// until it is all in; Cancel goes back to the project before.
    Open,
    /// Only the Waveform view's effects rack (kept from the last run, or a preset): the project
    /// stays as it is and goes on (playing, recording).
    Rack,
    /// An undo that brought plug-ins back: the project is as the step left it at once, and the
    /// engine plays the graph before until they are in; the rest goes on. Not cancellable.
    Undo,
    /// A redo that brought plug-ins back, as [`Self::Undo`].
    Redo,
    /// Every plug-in reloaded (and every file read again) at a new sample rate: nothing of the
    /// project plays until it is all in. Not cancellable.
    SampleRate,
}

impl LoadKind {
    /// Whether the project waits for it: no graph and none of `update`'s housekeeping until
    /// it is in.
    fn holds_project(self) -> bool {
        matches!(self, Self::Open | Self::SampleRate)
    }

    /// An undo or redo bringing plug-ins back.
    fn restores(self) -> bool {
        matches!(self, Self::Undo | Self::Redo)
    }
}

/// An open's audio files, read and decoded on workers (a few at once, each file once), handed
/// back to the main thread, which owns the media pool. Dropping it stops the workers after the
/// file each is on, and its results go nowhere (the channel is this open's alone).
struct Reads {
    files: Vec<PathBuf>,
    arrived: Vec<bool>,
    count: usize,
    results: Receiver<(usize, Result<AudioData>)>,
    stop: Arc<AtomicBool>,
    /// Files that didn't read, said once the open ends.
    failures: Vec<String>,
}

/// Workers at most reading an open's files.
const READERS: usize = 4;

impl Reads {
    /// Start reading `files` (each with whether its onsets are wanted: a clip that keeps its
    /// attacks while stretched) at `sample_rate`, held while `hold` says so.
    fn start(files: Vec<(PathBuf, bool)>, sample_rate: u32, hold: Arc<ReadHold>) -> Self {
        let (tx, results) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let paths: Vec<PathBuf> = files.iter().map(|(p, _)| p.clone()).collect();
        let files = Arc::new(files);
        let next = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let workers = std::thread::available_parallelism()
            .map_or(2, |n| n.get() / 2)
            .clamp(1, READERS)
            .min(files.len());
        for k in 0..workers {
            let (tx, stop, files, next, hold) = (
                tx.clone(),
                stop.clone(),
                files.clone(),
                next.clone(),
                hold.clone(),
            );
            // A worker that can't start leaves its share to the others; if none can, the
            // channel closes and every file is reported unread.
            let _ = std::thread::Builder::new()
                .name(format!("project read {k}"))
                .spawn(move || {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some((path, onsets)) = files.get(i) else {
                            break;
                        };
                        if !hold.wait(i, &stop) {
                            break;
                        }
                        // Onsets too, when a clip will ask for them at the graph build: they
                        // take a while on a long file.
                        let read = media::load(path, sample_rate).inspect(|data| {
                            if *onsets {
                                data.onsets();
                            }
                        });
                        if stop.load(Ordering::Relaxed) || tx.send((i, read)).is_err() {
                            break;
                        }
                    }
                });
        }
        Self {
            arrived: vec![false; paths.len()],
            files: paths,
            count: 0,
            results,
            stop,
            failures: Vec::new(),
        }
    }

    fn done(&self) -> bool {
        self.count == self.files.len()
    }
}

impl Drop for Reads {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Holds an open's file reads back while set ([`Session::hold_file_reads`]): the check runs'
/// way to catch the bar on its files, and the tests' to see that opening returns before any
/// file is read. `Some(n)`: the first `n` files read, the rest wait.
#[derive(Default)]
struct ReadHold {
    held: Mutex<Option<usize>>,
    changed: std::sync::Condvar,
}

impl ReadHold {
    fn set(&self, hold: Option<usize>) {
        if let Ok(mut held) = self.held.lock() {
            *held = hold;
        }
        self.changed.notify_all();
    }

    /// Wait while file `i` is held; false once `stop` is set.
    fn wait(&self, i: usize, stop: &AtomicBool) -> bool {
        let Ok(mut held) = self.held.lock() else {
            return !stop.load(Ordering::Relaxed);
        };
        while held.is_some_and(|n| i >= n) && !stop.load(Ordering::Relaxed) {
            match self.changed.wait_timeout(held, Duration::from_millis(50)) {
                Ok((h, _)) => held = h,
                Err(_) => break,
            }
        }
        !stop.load(Ordering::Relaxed)
    }
}

/// What an open is doing ([`Session::loading`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoadStage {
    /// The last project's plug-ins being destroyed (main thread, a few a frame).
    Closing,
    /// The audio files being read and decoded (on workers).
    Reading,
    /// The plug-ins being loaded (main thread, one or a few a frame).
    Loading,
}

/// How far an open has come ([`Session::loading`]).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadProgress {
    pub stage: LoadStage,
    /// Over the whole open: plug-ins closed, files read and plug-ins loaded, of all of them.
    pub done: usize,
    pub total: usize,
    /// Within the stage.
    pub stage_done: usize,
    pub stage_total: usize,
    /// What the stage is on: the plug-in closing or loading next (the one a frame may wait
    /// on), or the file being read (its name).
    pub next: Option<String>,
    /// The track of the plug-in loading next ("Master" for the master's).
    pub track: Option<String>,
    /// What it is for: an open, the Waveform view's effects rack ([`Session::restore_rack`],
    /// [`Session::load_rack_preset`]), an undo or redo, a new sample rate.
    pub kind: LoadKind,
    /// An undo or redo's step name.
    pub label: Option<String>,
}

impl LoadProgress {
    /// How far, 0 to 1, over the whole open.
    pub fn fraction(&self) -> f32 {
        self.done as f32 / self.total.max(1) as f32
    }

    /// A project is being opened (the window shows only the bar).
    pub fn opens(&self) -> bool {
        self.kind == LoadKind::Open
    }
}

/// A save in the background ([`Session::save_in_background`]): the file written on a worker.
struct SaveJob {
    path: PathBuf,
    handle: std::thread::JoinHandle<Result<String>>,
}

/// What a save writes, gathered on the main thread ([`Session::prepare_save`]).
struct SaveWork {
    path: PathBuf,
    project: Project,
    /// Files to copy along (Save As into another folder): from, to.
    copies: Vec<(PathBuf, PathBuf)>,
}

impl SaveWork {
    /// Copy the files and write the project: the part that can run on a worker. Returns the
    /// JSON written.
    fn run(self) -> Result<String> {
        for (from, to) in &self.copies {
            if from.is_file() && !to.exists() {
                if let Some(parent) = to.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(from, to)
                    .with_context(|| format!("copying {} to {}", from.display(), to.display()))?;
            }
        }
        let json = serde_json::to_string_pretty(&self.project)?;
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        write_whole(&self.path, &json)?;
        Ok(json)
    }
}

/// A plug-in on the clipboard ([`Session::copy_plugin`]): what makes another like it, kept
/// apart from the project, so it pastes after the original is gone and survives undo.
#[derive(Debug, Clone)]
pub struct PluginClip {
    pub name: String,
    pub path: PathBuf,
    pub bypass: bool,
    /// Its settings when copied.
    pub state: Arc<Vec<u8>>,
}

/// One of a plug-in's audio outputs ([`Session::plugin_outputs`]).
#[derive(Debug, Clone, PartialEq)]
pub struct PluginOutput {
    /// Its bus index: 0 is the main pair.
    pub bus: usize,
    pub name: String,
    pub channels: usize,
    /// On in the plug-in (a track listens to it, or it is the main pair).
    pub active: bool,
    /// The track listening to it, if any.
    pub track: Option<TrackId>,
}

/// Silence run through each plugin after activation, in seconds.
const PREROLL_SECONDS: f64 = 1.0;
/// MIDI events the engine can hand over between two UI frames while recording.
const MIDI_RECORD_RING: usize = 16_384;
/// How long an export (or a track's render, or an Apply) waits for impulses being prepared.
const IMPULSE_WAIT: Duration = Duration::from_secs(60);

/// Previewed notes ringing at once, at most.
const MAX_NOTE_PREVIEWS: usize = 32;

/// Where an insert lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InsertTarget {
    Track(TrackId),
    Master,
    /// The Waveform view's effects rack (dev plan 10.4): not in the project or its graph; the
    /// preview plays through it and Apply renders it.
    Editor,
}

/// What a loaded insert runs: a VST3 plug-in, or one of the built-in effects.
enum Instance {
    Vst3 {
        plugin: Arc<Mutex<Plugin>>,
        layout: AudioBusLayout,
    },
    Native(crate::native::Native),
}

struct LivePlugin {
    instance: Instance,
    bypass: Arc<AtomicBool>,
    latency: usize,
    is_instrument: bool,
    has_editor: bool,
    /// A VST3 plug-in's own editor window.
    window: Option<PluginWindow>,
    /// A native effect's editor, which the app draws.
    native_open: bool,
    /// Its processing time, for the CPU panel (the graph's insert is [`crate::usage::Timed`]).
    usage: Arc<crate::usage::Usage>,
    /// Blocks it missed live (its lock held elsewhere): [`InsertFlags::counting`].
    misses: Arc<AtomicU64>,
    /// A VST3 plug-in's parameters, read and set without its lock (`param_cache`); `None` for
    /// a native effect (its parameters are atomics already) or a plug-in without a controller.
    params: Option<ParameterCache>,
    /// Its audio outputs' names, main pair first ("KT Out 1", "KT Out 2"...): read once at
    /// load, since reading them takes the plug-in's lock.
    output_names: Vec<String>,
}

impl LivePlugin {
    fn vst3(&self) -> Option<&Arc<Mutex<Plugin>>> {
        match &self.instance {
            Instance::Vst3 { plugin, .. } => Some(plugin),
            Instance::Native(_) => None,
        }
    }

    fn editor_open(&self) -> bool {
        self.native_open || self.window.as_ref().is_some_and(PluginWindow::is_open)
    }

    /// The insert's settings, for saving and for undo.
    fn save_state(&self) -> Result<Vec<u8>> {
        match &self.instance {
            Instance::Vst3 { plugin, .. } => plugin
                .lock()
                .map_err(|_| anyhow!("lock poisoned"))
                .and_then(|mut p| {
                    plugins::flush_parameters(&mut p);
                    p.save_state().map_err(|e| anyhow!("{e}"))
                }),
            Instance::Native(native) => Ok(native.save_state()),
        }
    }
}

impl Drop for LivePlugin {
    fn drop(&mut self) {
        // The cache's link holds the plug-in's controller: released while its module is
        // still loaded, before the plug-in itself can go.
        self.params = None;
        if let Some(mut window) = self.window.take() {
            window.close();
        }
        if let Some(plugin) = self.vst3() {
            if let Ok(mut plugin) = plugin.lock() {
                let _ = plugin.stop_processing();
            }
        }
    }
}

struct ActiveRecording {
    files: Receiver<RecordedFile>,
    expected: usize,
    received: Vec<RecordedFile>,
    start: Option<u64>,
    /// [`Session::record_shift`] when recording began: takes are placed this much earlier
    /// than where they were tapped.
    latency: u64,
    /// Where a count-in leads to; stopping before it returns here.
    record_point: Option<u64>,
    /// The count-in's length in bars and where it starts (heard position, negative when it
    /// starts before the timeline), with `record_point` (22.3): what `count_in_progress` counts.
    count_in: Option<(u32, i64)>,
    /// The loop when recording started: each pass becomes a take.
    looped: Option<(u64, u64)>,
    /// The punch range when recording started; with a loop, each pass is trimmed to it.
    punch: Option<(u64, u64)>,
    /// Stop was pressed; the files are being finished.
    stopping: bool,
    /// The engine has given the record job back: nothing more will be recorded.
    job_returned: bool,
    /// MIDI that arrived while recording, stamped by the engine, and what's been read so far.
    midi: Option<rtrb::Consumer<RecordedMidi>>,
    midi_events: Vec<RecordedMidi>,
    midi_dropped: u64,
    /// Armed instrument tracks and the input each listens to: port index (`None` = all
    /// ports) and MIDI channel.
    midi_tracks: Vec<(TrackId, Option<usize>, Option<u8>)>,
    /// The device's output latency when recording began: MIDI is placed this much before the
    /// heard position it was stamped with, where the player heard it.
    output_latency: u64,
    /// The recorded window on the timeline, as audio takes are placed (`RecordJob::from` and
    /// `to`, less `latency`).
    window: (u64, u64),
    /// Heard position (less output latency) when stop was pressed: the end of the window.
    stopped_at: Option<u64>,
    /// Each audio take's waveform so far, drained from the audio thread every update (21.1).
    live: Vec<record::LivePeaks>,
    /// The job was back at the last update: the takes' peaks were whole then, and have been
    /// drawn so for a frame before they become clips.
    drawn_whole: bool,
}

/// The count-in's clicks left to hear, counting the one playing: the click falls on each whole
/// beat, so with the count-in running from beat `start` to the take's `point` and `heard` where
/// the output is now, that is its beats after `heard` and before `point`, plus the last one
/// heard if the count-in's first has sounded. The delay compensation's lead-in before the
/// first click (`heard` before `start`) shows the whole count, never one more (TD164: the
/// metronome can click on a beat in that lead-in); a record point off the beat counts down on
/// the clicks, not on its own phase. At least 1.
fn clicks_left(start: f64, heard: f64, point: f64) -> u32 {
    // Beat positions from the tempo map are a little off whole numbers at times.
    const NEAR: f64 = 1e-6;
    let first = (start - NEAR).ceil();
    let heard_beat = (heard + NEAR).floor();
    let last = (point - NEAR).ceil() - 1.0;
    let after = (last - (heard_beat + 1.0).max(first) + 1.0).max(0.0);
    let sounded = heard_beat >= first;
    (after as u32 + u32::from(sounded)).max(1)
}

/// A count-in in progress ([`Session::count_in_progress`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CountInProgress {
    /// Its length in bars (the setting when recording began).
    pub bars: u32,
    /// Beats left before the take starts (of the time signature's lower number, as the click
    /// plays them), counting the one playing: 4, 3, 2, 1 through a bar of 4/4.
    pub beats_left: u32,
    /// Where the take starts (the playhead when Record was pressed), in samples.
    pub record_point: u64,
}

/// A take being recorded, as the arrangement draws it (21.1): the clip it becomes on stop, as
/// far as it has come, and its peaks so far (one `(min, max)` per [`crate::media::PEAK_BLOCK`]
/// frames per channel, as the finished file's are).
pub struct RecordingTake<'a> {
    pub track: TrackId,
    /// The clip's name to be (the file's).
    pub name: &'a str,
    /// Where it sits on the timeline, in samples.
    pub start: u64,
    /// Where in the file it begins (a loop recording's pass), in samples.
    pub offset: u64,
    /// How much of it there is so far, in samples.
    pub length: u64,
    /// One list per channel of the file (1 or 2), from the file's first frame.
    pub peaks: &'a [Vec<(f32, f32)>],
}

/// What a running export job is making.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExportKind {
    /// The mix to a file (`export_mixdown`).
    Export,
    /// A track's render (`render_track`).
    Render,
    /// A mixdown to a new file, opened in the Waveform view.
    Mixdown,
    /// A Waveform view file in another format (`export_audio`).
    File,
}

/// Where mixdown export stands.
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ExportStatus {
    Idle,
    /// `progress` 0 to 1; `what` says what it is making, as the progress dialog does.
    Running {
        progress: f32,
        kind: ExportKind,
        what: String,
    },
    /// The file, its loudness, and how it was normalised (if it was asked to be); a track's
    /// render also says the track its file went on.
    Finished {
        path: PathBuf,
        loudness: crate::loudness::Loudness,
        normalized: Option<export::Normalized>,
        #[serde(skip_serializing_if = "Option::is_none")]
        track: Option<TrackId>,
    },
    Failed {
        error: String,
    },
}

/// Status the UI shows.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub dsp_load: f32,
    pub latency: usize,
    pub plugin_misses: u64,
    pub skipped_blocks: u64,
    pub input_underruns: u64,
    /// The block the engine processes (and exports) in: the device's buffer, or the larger
    /// block it gathers buffers into (`AudioSettings::processing_block`).
    pub block_size: usize,
    /// The device's buffer, in frames.
    pub device_block: usize,
    /// Frames the driver asks for between waits: more than `device_block` for a driver that
    /// calls back in bursts (FlexASIO on shared WASAPI), `device_block` otherwise.
    pub cadence: usize,
    /// Worker threads helping the audio thread.
    pub workers: usize,
}

/// A crossfade's length when the clips only touch (and none is asked for): 20 ms.
pub const DEFAULT_CROSSFADE_SECONDS: f64 = 0.02;

pub struct Session {
    pub project: Project,
    pub path: Option<PathBuf>,
    pub settings: AudioSettings,
    pub device: Option<RunningDevice>,
    /// A clock running the engine in place of a device, for checks
    /// ([`Self::start_virtual_device`]).
    virtual_device: Option<device::VirtualDevice>,
    pub device_error: Option<String>,
    pub midi: MidiInputs,
    /// Messages for the UI to show and clear.
    pub notices: Vec<String>,
    pub media: MediaPool,
    /// Stretched and transposed renders of clips' files, made in the background.
    stretches: stretch::Cache,
    /// Clips copied or cut, with the tracks they came from (not saved, not undone).
    clipboard: Vec<(TrackId, AnyClip)>,
    /// The tempo map clips that follow the tempo were last laid out under.
    laid_out: Option<TempoMap>,
    host: Vst3Host,
    sample_rate: u32,
    installed: Vec<PathBuf>,
    plugins: HashMap<InsertId, LivePlugin>,
    /// A project's plug-ins let go of by an open (named), destroyed a few a frame once the
    /// engine runs a graph without them ([`Self::close_step`]).
    closing: std::collections::VecDeque<(String, LivePlugin)>,
    /// When the plug-ins in `closing` were let go of.
    closing_since: Instant,
    params: HashMap<TrackId, Arc<NodeParams>>,
    /// Each send's level and mute, shared with the graph so moving them needs no rebuild.
    send_params: HashMap<SendId, Arc<SendParams>>,
    /// Notes previewed from the editor (or an agent), waiting for their release: when, on
    /// which track, which pitch.
    note_previews: Vec<(Instant, TrackId, u8)>,
    master_params: Arc<NodeParams>,
    /// Where an unsaved session keeps the files it makes (recordings, imported copies). The
    /// app sets None: it always has a project folder (its start screen), so nothing lands
    /// outside one, and recording or importing without one is refused. A temporary folder by
    /// default, for tests and tools.
    pub scratch: Option<PathBuf>,
    /// How often the project is backed up while it has unsaved changes (None: never). The app
    /// sets it from its settings.
    pub autosave: Option<Duration>,
    /// How many backups are kept, the newest.
    pub keep_backups: usize,
    /// Where project templates are kept (`crate::template`): the app's `Templates` folder
    /// beside the program, app-wide. None (the default): templates can't be saved or used.
    pub templates_dir: Option<PathBuf>,
    /// The manual's folder (`manual` beside the program, built by `crates/manual`; 27.3): where
    /// `search_manual` and `open_manual` read its search index ([`crate::manual`]). The app
    /// sets it; None (the default, headless sessions): there is no manual to read.
    pub manual_dir: Option<PathBuf>,
    /// The Waveform view's effects rack is kept in this file between runs (`rack.json` beside
    /// the program, [`Self::restore_rack`]): written a moment after it changes and on exit
    /// ([`Self::save_rack_now`]). None (the default, and check runs): it isn't kept.
    pub rack_file: Option<PathBuf>,
    /// Where rack presets are kept (`RackPresets` beside the program, `crate::rack::presets`).
    /// None (the default): presets can't be saved or loaded.
    pub rack_presets_dir: Option<PathBuf>,
    /// Keeping the rack: what was last written, the slots and bypasses last looked at, when to
    /// look next, and the write in flight (`session::rack_keep`).
    rack_keep: rack_keep::Keeper,
    /// Why each insert that failed to load didn't (a slot's hover says it).
    load_failures: HashMap<InsertId, String>,
    /// An Apply waits before rendering while set ([`Self::hold_apply`]: tests and checks).
    apply_hold: Arc<AtomicBool>,
    /// Rack plug-ins removed while an Apply rendered through them: destroyed here, on the main
    /// thread, once it has handed its chain back ([`Self::collect_apply`]).
    retired: Vec<LivePlugin>,
    /// A plug-in's outputs need switching but an export was running ([`Self::update`]).
    outputs_waiting: bool,
    /// The plug-in last copied ([`Self::copy_plugin`]).
    plugin_clipboard: Option<PluginClip>,
    /// Opens leave their plug-ins to load a few a frame (`update`), so a window can paint
    /// meanwhile: the app sets it. Off (tests, tools): opening loads everything before it
    /// returns.
    pub stage_loads: bool,
    loading: Option<Loading>,
    /// Counts opens begun and cancelled: a caller waiting for an open to end
    /// ([`Self::open_serial`]) knows it ended as asked only if this hasn't moved.
    open_serial: u64,
    /// The main thread's share of an open (destroying and loading plug-ins) waits while set
    /// ([`Self::hold_loading`]); files still read.
    loads_held: bool,
    /// A staged load's plug-ins after the first `n` wait ([`Self::hold_plugin_loads`]).
    plugins_held_after: Option<usize>,
    read_hold: Arc<ReadHold>,
    /// Plug-in states being read, a VST3 an update, for a save, an autosave or the look for
    /// unsaved changes (`saving.rs`).
    capture: Option<saving::Capture>,
    /// A save's (or a look for changes') reads wait while set ([`Self::hold_saving`]).
    saves_held: bool,
    /// A background save's file being written, once its states are read.
    saving: Option<SaveJob>,
    /// How the last background save ended, until [`Self::take_saved`].
    saved: Option<std::result::Result<PathBuf, String>>,
    /// An autosave's backup being written on a worker.
    backing_up: Option<saving::BackupJob>,
    /// The look for unsaved changes comparing on a worker, and its answer until
    /// [`Self::check_unsaved`] takes it.
    checking: Option<std::thread::JoinHandle<bool>>,
    unsaved_answer: Option<bool>,
    /// The project as last saved, backed up or opened (with the plug-ins' live states), to
    /// tell whether there is anything to back up.
    backup_baseline: Option<String>,
    /// The project as last saved or opened (as [`Self::backup_baseline`]), for
    /// [`Self::has_unsaved_changes`]; None for a backup opened in the project's place.
    /// Comparing the project itself, not the undo history, means an edit undone again is no
    /// change, and a plug-in turned by hand in its editor (no undo step) is one.
    saved_json: Option<String>,
    last_autosave: Instant,
    /// A backup newer than the project file when it was opened: a crash, or a close without
    /// saving.
    newer_backup: Option<PathBuf>,
    /// The last CPU reading ([`Self::cpu_usage`]) and when it was taken.
    cpu: crate::usage::CpuUsage,
    cpu_at: Option<Instant>,
    /// The process's CPU time at `cpu_at` (`cpu::process_time`), for the next reading.
    cpu_time: f64,
    core: Arc<Mutex<EngineCore>>,
    engine: EngineHandle,
    graph_dirty: bool,
    /// Graphs sent to the engine that it has not swapped out yet.
    graphs_in_flight: usize,
    /// Audio files open in the Waveform view (dev plan 10).
    documents: Vec<AudioDocument>,
    next_document: DocumentId,
    /// The document the preview plays, while it does.
    previewing: Option<DocumentId>,
    /// Audio copied in the Waveform view, for pasting.
    audio_clipboard: Option<audio_edit::Clipboard>,
    /// The Waveform view's effects rack. Its ids come from their own range
    /// ([`RACK_IDS`]), so adding one isn't an edit of the project.
    editor_rack: Vec<InsertSlot>,
    next_rack_id: InsertId,
    /// An Apply rendering on its own thread.
    applying: Option<ApplyJob>,
    /// The rack synced with a track's insert chain (dev plan 21.2, `session::rack_sync`).
    rack_sync: Option<rack_sync::RackSync>,
    /// The sync is changing one side: what it changes is not mirrored back.
    rack_syncing: bool,
    /// Node keys of the last graph sent, in its processing order.
    sent_keys: Vec<u64>,
    /// Every automation lane as the last graph built has it (tracks in order, then the
    /// master), so any edit of a lane rebuilds the graph without each edit asking for it.
    built_lanes: Vec<AutomationLane>,
    /// Per plug-in parameter, whether it is being written: shared with its lane in the graph,
    /// which then sends nothing and lets the editor's knob play.
    param_writing: HashMap<(InsertId, u32), Arc<AtomicBool>>,
    /// The automation write pass under way (write modes), and where it was last frame.
    pub(crate) writer: Writer,
    was_playing: bool,
    last_write_position: u64,
    /// Each plug-in knob's value as its editor gesture began.
    gesture_before: HashMap<(InsertId, u32), f64>,
    /// The tempo and meter the engine has, so changes are sent once.
    sent_tempo: Option<TempoMap>,
    /// The loop the engine has.
    sent_loop: Option<Option<(u64, u64)>>,
    sent_click: Option<ClickSettings>,
    /// Undo and redo.
    history: History,
    recording: Option<ActiveRecording>,
    export: Option<ExportJob>,
    /// Holds a running export halfway ([`Self::hold_export`]).
    export_hold: Arc<AtomicBool>,
    export_outcome: ExportStatus,
    /// What the running (or last) export job makes, as the progress dialog says it.
    export_kind: ExportKind,
    export_what: String,
    /// The running export is a track's render ([`Self::render_track`]): the engine plays its
    /// own graph until it ends, and then its file goes on a new track.
    rendering: Option<TrackRender>,
    /// The track the last render made, for whoever waits on it (`take_rendered_track`).
    rendered_track: Option<TrackId>,
    /// The running export is a mixdown to a new file, to open when it ends.
    mixdown_pending: bool,
    /// The document the last mixdown opened (`take_opened_mixdown`).
    opened_mixdown: Option<DocumentId>,
    last_service: Instant,
}

impl Session {
    pub fn new(settings: AudioSettings) -> Self {
        let sample_rate = settings.sample_rate.unwrap_or(48_000);
        let (mut core, engine) = engine::new_engine(MAX_BLOCK);
        core.configure_io(0, 2);
        let host = plugins::host(f64::from(sample_rate), MAX_BLOCK)
            .expect("the VST3 host has no failure modes for valid settings");
        let installed = plugins::installed(&host);
        let mut session = Self {
            project: Project::default(),
            path: None,
            settings,
            device: None,
            virtual_device: None,
            device_error: None,
            midi: MidiInputs::default(),
            notices: Vec::new(),
            media: MediaPool::default(),
            stretches: stretch::Cache::default(),
            clipboard: Vec::new(),
            laid_out: None,
            host,
            sample_rate,
            installed,
            plugins: HashMap::new(),
            closing: std::collections::VecDeque::new(),
            closing_since: Instant::now(),
            params: HashMap::new(),
            send_params: HashMap::new(),
            note_previews: Vec::new(),
            master_params: Arc::new(NodeParams::default()),
            autosave: None,
            keep_backups: 20,
            templates_dir: None,
            manual_dir: None,
            rack_file: None,
            rack_presets_dir: None,
            rack_keep: Default::default(),
            load_failures: HashMap::new(),
            apply_hold: Arc::default(),
            retired: Vec::new(),
            outputs_waiting: false,
            plugin_clipboard: None,
            stage_loads: false,
            loading: None,
            loads_held: false,
            plugins_held_after: None,
            read_hold: Arc::default(),
            capture: None,
            saves_held: false,
            saving: None,
            saved: None,
            backing_up: None,
            checking: None,
            unsaved_answer: None,
            backup_baseline: None,
            saved_json: None,
            last_autosave: Instant::now(),
            newer_backup: None,
            open_serial: 0,
            scratch: Some({
                // One per session, so sessions side by side (tests) never share files.
                static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let n = NEXT.fetch_add(1, Ordering::Relaxed);
                std::env::temp_dir().join(format!("oxide-unsaved-{}-{n}", std::process::id()))
            }),
            cpu: Default::default(),
            cpu_at: None,
            cpu_time: 0.0,
            core: Arc::new(Mutex::new(core)),
            engine,
            graph_dirty: true,
            graphs_in_flight: 0,
            documents: Vec::new(),
            next_document: 1,
            previewing: None,
            audio_clipboard: None,
            editor_rack: Vec::new(),
            next_rack_id: RACK_IDS,
            applying: None,
            rack_sync: None,
            rack_syncing: false,
            sent_keys: Vec::new(),
            built_lanes: Vec::new(),
            param_writing: HashMap::new(),
            writer: Writer::default(),
            was_playing: false,
            last_write_position: 0,
            gesture_before: HashMap::new(),
            sent_tempo: None,
            sent_loop: None,
            sent_click: None,
            history: History::default(),
            recording: None,
            export: None,
            export_hold: Arc::default(),
            export_outcome: ExportStatus::Idle,
            export_kind: ExportKind::Export,
            export_what: String::new(),
            rendering: None,
            rendered_track: None,
            mixdown_pending: false,
            opened_mixdown: None,
            last_service: Instant::now(),
        };
        session.project.sample_rate = sample_rate;
        session.saved_json = serde_json::to_string_pretty(&session.project).ok();
        session.open_midi();
        session.apply_threads();
        session.apply_processing_block();
        session.start_device();
        session
    }

    /// Start (or replace) the worker threads the settings ask for.
    /// Hand the engine the block to gather device buffers into (23.1); it picks it up at its
    /// next callback, no restart needed.
    fn apply_processing_block(&self) {
        let block = self.settings.processing_block.unwrap_or(0) as usize;
        self.engine
            .shared
            .processing_block
            .store(block.min(MAX_BLOCK) as u64, Ordering::Relaxed);
    }

    /// What gathering device buffers into blocks adds between input and output, in samples (0
    /// without it): a take is moved earlier by it too.
    pub fn stage_latency(&self) -> u64 {
        self.engine.shared.stage_latency.load(Ordering::Relaxed)
    }

    fn apply_threads(&mut self) {
        let workers = self
            .settings
            .threads
            .unwrap_or_else(crate::cpu::default_workers);
        let pool = (workers > 0).then(|| Box::new(WorkerPool::new(workers)));
        self.send(Command::SetPool(pool));
    }

    /// Change how many worker threads help the audio thread (`Some(0)` = none).
    pub fn set_threads(&mut self, threads: Option<usize>) {
        self.settings.threads = threads;
        self.apply_threads();
        self.flush_pending();
    }

    // ---------------------------------------------------------------- devices

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Whether something runs the engine on a thread of its own: a device, or a check's
    /// [`Self::start_virtual_device`].
    fn engine_running(&self) -> bool {
        self.device.is_some() || self.virtual_device.is_some()
    }

    /// For checks: run the engine every `block` samples at the session's rate on a thread of
    /// its own, as a device would, with the audio going nowhere, so what the main thread does
    /// meets the audio thread as it does live. Refused while a device runs.
    pub fn start_virtual_device(&mut self, block: usize) -> Result<()> {
        if self.device.is_some() {
            bail!("a device is running");
        }
        if block == 0 || block > MAX_BLOCK {
            bail!("block size {block} is outside 1..={MAX_BLOCK}");
        }
        self.virtual_device = None;
        self.virtual_device = Some(device::VirtualDevice::start(
            self.core.clone(),
            block,
            self.sample_rate,
        )?);
        Ok(())
    }

    /// For checks and the app's `--show recording`: replace the audio device, if one runs, with
    /// a virtual one ([`Self::start_virtual_device`]) whose `inputs` input channels play
    /// `signal`, so recording runs as it does live without a device (21.1). Nothing is heard:
    /// the virtual device's output goes nowhere.
    pub fn start_virtual_input(
        &mut self,
        block: usize,
        inputs: usize,
        signal: device::InputSignal,
    ) -> Result<()> {
        if block == 0 || block > MAX_BLOCK {
            bail!("block size {block} is outside 1..={MAX_BLOCK}");
        }
        if inputs == 0 || inputs > crate::engine::MAX_IO_CHANNELS {
            bail!(
                "{inputs} inputs is outside 1..={}",
                crate::engine::MAX_IO_CHANNELS
            );
        }
        if let Some(device) = self.device.take() {
            device.pause();
        }
        self.virtual_device = None;
        let outputs = self.output_channels();
        if let Ok(mut core) = self.core.lock() {
            core.configure_io(inputs, outputs);
        }
        self.virtual_device = Some(device::VirtualDevice::start_with_input(
            self.core.clone(),
            block,
            self.sample_rate,
            inputs,
            Some(signal),
        )?);
        self.graph_dirty = true;
        Ok(())
    }

    /// The input channels of a check's virtual device ([`Self::start_virtual_device`],
    /// [`Self::start_virtual_input`]; 0 without a signal), `None` when none runs.
    pub fn virtual_inputs(&self) -> Option<usize> {
        self.virtual_device.as_ref().map(|v| v.inputs)
    }

    /// Stop [`Self::start_virtual_device`]'s clock.
    pub fn stop_virtual_device(&mut self) {
        self.virtual_device = None;
    }

    fn start_device(&mut self) {
        self.device = None;
        self.virtual_device = None;
        match device::start(&self.settings, self.core.clone()) {
            Ok(device) => {
                let rate = device.info.sample_rate;
                self.device_error = None;
                self.device = Some(device);
                if rate != self.sample_rate {
                    self.change_sample_rate(rate);
                }
            }
            Err(e) => {
                self.device_error = Some(format!("{e:#}"));
                // Keep a stereo output so export still works without a device.
                if let Ok(mut core) = self.core.lock() {
                    core.configure_io(0, 2);
                }
            }
        }
        self.graph_dirty = true;
    }

    /// Switch to new audio settings, restarting the device unless only the record offset
    /// changed.
    pub fn apply_audio_settings(&mut self, settings: AudioSettings) {
        if !self.settings.needs_restart(&settings) {
            self.settings = settings;
            self.apply_processing_block();
            return;
        }
        if let Some(device) = self.device.take() {
            device.pause();
        }
        let threads_changed = settings.threads != self.settings.threads;
        self.settings = settings;
        self.apply_processing_block();
        if threads_changed {
            self.apply_threads();
        }
        if let Some(rate) = self.settings.sample_rate {
            if rate != self.sample_rate {
                self.change_sample_rate(rate);
            }
        }
        self.start_device();
    }

    /// Reload every plug-in at a new sample rate (keeping its state), convert clip positions and
    /// read the files again at it. Staged as an open is (a [`LoadKind::SampleRate`] loading:
    /// the old plug-ins destroyed and the new ones loaded a few a frame, the files read on
    /// workers, nothing of the project playing meanwhile) with [`Self::stage_loads`]; otherwise
    /// all in before this returns. Playback stops.
    fn change_sample_rate(&mut self, rate: u32) {
        // An Apply renders through the rack's plug-ins, which this lets go of.
        if self.applying.is_some() {
            self.cancel_apply();
            if let Err(e) = self.wait_for_apply(IMPULSE_WAIT) {
                self.notices.push(format!("{e:#}"));
            }
        }
        self.capture_plugin_states();
        // A load under way starts again at the new rate; an open stays an open (its ending,
        // its Cancel), anything else becomes this.
        let before = self.loading.take();
        let old = self.sample_rate;
        let position = self.position();
        self.stop();
        self.let_go_of_plugins();
        // Slots copying another insert (a synced rack still loading) still copy it.
        let mut copies = HashMap::new();
        let (kind, path, backup, previous) = match before {
            Some(mut l) => {
                self.retire(std::mem::take(&mut l.retiring));
                copies = std::mem::take(&mut l.copies);
                match l.kind {
                    LoadKind::Open => (LoadKind::Open, l.path, l.backup, l.previous),
                    _ => (LoadKind::SampleRate, None, false, None),
                }
            }
            None => (LoadKind::SampleRate, None, false, None),
        };
        self.sample_rate = rate;
        match plugins::host(f64::from(rate), MAX_BLOCK) {
            Ok(host) => self.host = host,
            Err(e) => self.notices.push(format!("VST3 host: {e:#}")),
        }
        self.project.convert_sample_rate(rate);
        let scaled = (position as f64 * f64::from(rate) / f64::from(old.max(1))).round() as u64;
        self.send(Command::Locate(scaled));
        // Older snapshots hold positions at the old rate.
        self.history.reset();
        self.forget_media();
        self.stretches.clear();
        let queue = self.insert_queue();
        let reads = Reads::start(self.files_to_read(), rate, self.read_hold.clone());
        self.loading = Some(Loading {
            kind,
            label: None,
            retiring: Vec::new(),
            closing_total: self.closing.len(),
            reads,
            total: queue.len(),
            queue,
            path,
            backup,
            previous,
            copies,
        });
        self.graph_dirty = true;
        if !self.stage_loads {
            self.finish_loading();
        }
    }

    /// Every insert of the project (tracks in order, then the master) and of the effects rack,
    /// with the name of where each is, as loading queues them.
    fn insert_queue(&self) -> std::collections::VecDeque<(InsertSlot, String)> {
        let mut queue = std::collections::VecDeque::new();
        for track in &self.project.tracks {
            queue.extend(
                track
                    .inserts
                    .iter()
                    .map(|s| (s.clone(), track.name.clone())),
            );
        }
        let master = self.project.master.inserts.iter();
        queue.extend(master.map(|s| (s.clone(), "Master".to_string())));
        let rack = self.editor_rack.iter();
        queue.extend(rack.map(|s| (s.clone(), rack_keep::RACK_LABEL.to_string())));
        queue
    }

    /// Queue plug-ins to be destroyed a few a frame ([`Self::close_step`]) once the engine runs
    /// a graph without them: call only after sending one.
    fn retire(&mut self, plugins: Vec<(InsertId, String, LivePlugin)>) {
        if plugins.is_empty() {
            return;
        }
        for (_, name, live) in plugins {
            self.closing.push_back((name, live));
        }
        self.closing_since = Instant::now();
    }

    /// Build the project's graph and send it now (what `update` does when it is dirty).
    fn send_project_graph(&mut self) {
        self.graph_dirty = false;
        let graph = self.build_graph();
        // New tracks get their parameter slots while building; fill them before the graph
        // can run, or a muted track would play its first block.
        self.sync_params();
        self.send_graph(graph);
    }

    pub fn restart_midi(&mut self) {
        self.open_midi();
        self.graph_dirty = true;
    }

    fn open_midi(&mut self) {
        // Close the old ports first so they can be reopened.
        self.midi = MidiInputs::default();
        let (midi, consumers) = MidiInputs::open_all();
        for error in &midi.errors {
            self.notices.push(format!("MIDI: {error}"));
        }
        self.midi = midi;
        self.send(Command::SetMidiPorts(consumers));
    }

    // ---------------------------------------------------------------- engine plumbing

    /// Send a new graph, linked to the one it replaces so the engine can hand running state
    /// (fader ramps, delay lines, queued MIDI) over and the swap stays inaudible.
    fn send_graph(&mut self, mut graph: Graph) {
        graph.link_previous(&self.sent_keys);
        self.sent_keys = graph.keys();
        self.send(Command::SetGraph(Box::new(graph)));
    }

    /// Queue a command. Without a running device nothing drains the queue, so apply it here.
    fn send(&mut self, command: Command) {
        if matches!(command, Command::SetGraph(_)) {
            self.graphs_in_flight += 1;
        }
        let pushed = self.engine.commands.push(command);
        let running = self.engine_running();
        if let Err(rtrb::PushError::Full(command)) = pushed {
            // The device stalled; apply directly so the UI never blocks forever.
            if let Ok(mut core) = self.core.lock() {
                core.handle_commands();
            }
            let _ = self.engine.commands.push(command);
        }
        if !running {
            if let Ok(mut core) = self.core.lock() {
                core.handle_commands();
            }
        }
        self.collect_garbage();
    }

    /// Drop everything the audio thread handed back. Recording jobs carry the take start.
    fn collect_garbage(&mut self) {
        while let Ok(garbage) = self.engine.garbage.pop() {
            match garbage {
                Garbage::Graph(graph) => {
                    self.graphs_in_flight = self.graphs_in_flight.saturating_sub(1);
                    drop(graph);
                }
                Garbage::MidiPorts(ports) => drop(ports),
                Garbage::Pool(pool) => drop(pool),
                Garbage::TempoMap(map) => drop(map),
                Garbage::Preview(preview) => drop(preview),
                Garbage::Recording(job) => {
                    let lost: u64 = job
                        .takes
                        .iter()
                        .filter_map(|t| t.peaks.as_ref())
                        .map(|p| p.dropped)
                        .sum();
                    if lost > 0 {
                        // Only the drawing while it recorded: the clip's peaks are the file's.
                        log::warn!("recording: {lost} waveform bins dropped (the UI stalled)");
                    }
                    if let Some(rec) = &mut self.recording {
                        rec.start = job.start;
                        rec.job_returned = true;
                        rec.midi_dropped = job.midi_dropped;
                    }
                    if job.dropped > 0 {
                        self.notices.push(format!(
                            "Recording lost {} samples (disk too slow)",
                            job.dropped
                        ));
                    }
                    // Dropping the producers tells the disk writer to finish the files.
                    drop(job);
                }
            }
        }
    }

    /// Make the engine let go of every plugin (an empty graph) and wait until it has.
    fn flush_engine(&mut self) {
        self.send_graph(Graph::empty(MAX_BLOCK));
        self.flush_pending();
        self.graph_dirty = true;
    }

    /// Destroy every plugin, after the engine has let go of them.
    fn release_plugins(&mut self) {
        self.flush_engine();
        self.plugins.clear();
    }

    /// Wait until the engine runs the latest graph sent, so plugins only the older graphs
    /// used can be destroyed.
    fn flush_pending(&mut self) {
        // With a device running, let the callback pick the graph up rather than taking the
        // engine lock (which would cost a silent block).
        let deadline = Instant::now() + std::time::Duration::from_millis(500);
        while self.engine_running() && Instant::now() < deadline {
            self.collect_garbage();
            if self.graphs_in_flight == 0 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // No device, or it stalled: apply under the lock, which also guarantees the swap.
        if let Ok(mut core) = self.core.lock() {
            core.handle_commands();
        }
        self.collect_garbage();
    }

    fn build_graph(&mut self) -> Graph {
        self.build_graph_of(None)
    }

    /// The graph: the whole project, without disabled tracks; or, with `only`, that track as
    /// it sounds on its own (with whatever reaches the mix through it: a folder's tracks), its
    /// main output straight to the master's outputs through a master with no effects, level
    /// or automation (a track's render, [`Self::render_track`]). Its sends are left out: the
    /// buses play those.
    fn build_graph_of(&mut self, only: Option<TrackId>) -> Graph {
        self.request_stretches();
        self.sync_plugin_outputs();
        self.built_lanes = self
            .project
            .all_lanes()
            .map(|(_, lane)| lane.clone())
            .collect();
        for lane in &self.built_lanes {
            if let AutomationTarget::Parameter { insert, parameter } = lane.target {
                self.param_writing.entry((insert, parameter)).or_default();
            }
        }
        let mut b = GraphBuilder::new();
        let (master_inserts, master_automation, master_params) = match only {
            None => (
                self.processors(&self.project.master.inserts),
                self.node_automation(
                    &self.project.master.automation,
                    &self.project.master.inserts,
                ),
                self.master_params.clone(),
            ),
            Some(_) => (Vec::new(), NodeAutomation::default(), Arc::default()),
        };
        let master = b.add(NodeSpec {
            key: MASTER_KEY,
            label: "Master".into(),
            params: master_params,
            clips: Vec::new(),
            input: NodeInput::None,
            inserts: master_inserts,
            target: SpecTarget::Hardware(usize::from(self.project.master.output_channel)),
        });
        b.set_automation(master, master_automation);
        let included: HashSet<TrackId> = self
            .project
            .tracks
            .iter()
            .map(|t| t.id)
            .filter(|&id| match only {
                // The track itself even when disabled; what feeds it only if it plays.
                Some(only) => {
                    self.project.main_feeds(id, only)
                        && (id == only || self.project.track(id).is_some_and(|t| t.enabled))
                }
                None => self.project.plays(id),
            })
            .collect();
        // A render of a plug-in's output tracks (or their folder) needs the instrument that
        // plays into them, but not that instrument's own main output or sends.
        let hosts: HashSet<TrackId> = match only {
            Some(_) => self
                .project
                .tracks
                .iter()
                .filter(|t| included.contains(&t.id))
                .filter_map(|t| match t.input {
                    TrackInput::Plugin { insert, .. } => self.project.insert_track(insert),
                    _ => None,
                })
                .filter(|h| {
                    !included.contains(h) && self.project.track(*h).is_some_and(|t| t.enabled)
                })
                .collect(),
            None => HashSet::new(),
        };
        let included: HashSet<TrackId> = included.union(&hosts).copied().collect();
        let mut index = HashMap::new();
        for track in self
            .project
            .tracks
            .iter()
            .filter(|t| included.contains(&t.id))
        {
            let params = self
                .params
                .entry(track.id)
                .or_insert_with(|| Arc::new(NodeParams::default()))
                .clone();
            let clips = track
                .clips
                .iter()
                .filter_map(|clip| {
                    let (data, offset) = self.clip_audio(clip)?;
                    Some(ClipSource {
                        data,
                        start: clip.start,
                        offset,
                        length: clip.length,
                        gain: db_to_gain(clip.gain_db),
                        fades: (clip.fade_in, clip.fade_out),
                    })
                })
                .collect();
            let input = match &track.input {
                TrackInput::None => NodeInput::None,
                TrackInput::Audio { channel, stereo } => NodeInput::Audio {
                    channel: usize::from(*channel),
                    stereo: *stereo,
                },
                TrackInput::Midi { port, channel } => NodeInput::Midi {
                    port: port
                        .as_deref()
                        .map(|name| self.midi.port_index(name).unwrap_or(usize::MAX)),
                    channel: *channel,
                },
                // Fed by the plug-in's node, like a bus (below).
                TrackInput::Plugin { .. } => NodeInput::None,
            };
            let inserts = self.processors(&track.inserts);
            let sequence = (track.kind == TrackKind::Instrument)
                .then(|| Sequence::new(&track.midi_clips, self.project.chase_notes));
            let i = b.add(NodeSpec {
                key: u64::from(track.id),
                label: track.name.clone(),
                params,
                clips,
                input,
                inserts,
                target: SpecTarget::Spec(master),
            });
            if let Some(sequence) = sequence {
                b.set_sequence(i, sequence);
            }
            b.set_automation(i, self.node_automation(&track.automation, &track.inserts));
            index.insert(track.id, i);
        }
        for track in self
            .project
            .tracks
            .iter()
            .filter(|t| included.contains(&t.id))
        {
            let target = match self.project.destination(track) {
                // The rendered track goes to the plain master, wherever it is routed.
                _ if Some(track.id) == only => SpecTarget::Spec(master),
                _ if hosts.contains(&track.id) => SpecTarget::Nowhere,
                Destination::Master => SpecTarget::Spec(master),
                Destination::Bus(id) => match index.get(&id) {
                    Some(&i) => SpecTarget::Spec(i),
                    None => SpecTarget::Spec(master),
                },
                Destination::Hardware(ch) => SpecTarget::Hardware(usize::from(ch)),
            };
            b.set_target(index[&track.id], target);
            if Some(track.id) == only || hosts.contains(&track.id) {
                continue;
            }
            for send in &track.sends {
                // A send whose target is gone or can't take one (a damaged file) is skipped;
                // a loop is cut by the builder.
                let takes_sends = self
                    .project
                    .track(send.target)
                    .is_some_and(|t| t.kind.mixes_tracks());
                // Sends to a track left out (disabled, or outside a render) go nowhere.
                if send.target == track.id || !takes_sends || !index.contains_key(&send.target) {
                    continue;
                }
                // New sends get their slot here, filled by `sync_params` before the graph runs.
                let params = self
                    .send_params
                    .entry(send.id)
                    .or_insert_with(|| Arc::new(SendParams::default()))
                    .clone();
                b.add_send(
                    index[&track.id],
                    SendSpec {
                        key: u64::from(send.id),
                        target: SpecTarget::Spec(index[&send.target]),
                        tap: if send.pre_fader {
                            Tap::PreFader
                        } else {
                            Tap::PostFader
                        },
                        params,
                        lane: crate::automation::playing(
                            &track.automation,
                            AutomationTarget::SendLevel { send: send.id },
                        )
                        .cloned(),
                    },
                );
            }
        }
        // Plug-ins' extra outputs: each an edge from the plug-in's track to the track that
        // listens, taken right after the plug-in (counted among its chain's loaded inserts,
        // as the graph's are). A listener whose plug-in is gone or left out hears nothing.
        for track in self
            .project
            .tracks
            .iter()
            .filter(|t| included.contains(&t.id))
        {
            let TrackInput::Plugin { insert, output } = track.input else {
                continue;
            };
            let Some(host) = self.project.insert_track(insert).filter(|h| *h != track.id) else {
                continue;
            };
            let (Some(&from), Some(host_track)) = (index.get(&host), self.project.track(host))
            else {
                continue;
            };
            let Some(position) = host_track
                .inserts
                .iter()
                .filter(|s| self.plugins.contains_key(&s.id))
                .position(|s| s.id == insert)
            else {
                continue;
            };
            b.add_send(
                from,
                SendSpec {
                    key: OUTPUT_EDGE_KEY | u64::from(track.id),
                    target: SpecTarget::Spec(index[&track.id]),
                    tap: Tap::Output {
                        insert: position,
                        bus: usize::from(output),
                    },
                    params: Arc::default(),
                    lane: None,
                },
            );
        }
        b.build(MAX_BLOCK)
    }

    /// Turn on exactly the extra outputs some track listens to, on every VST3 plug-in, and
    /// read back its layout and latency for the inserts the next graph builds. Switching a
    /// bus means stopping the plug-in for a moment (the audio thread skips it meanwhile),
    /// so it happens only when the listeners change; never during an export, which waits for
    /// plug-ins on the audio thread.
    fn sync_plugin_outputs(&mut self) {
        let exporting = self.export.as_ref().is_some_and(|e| !e.is_finished());
        let mut wanted: HashMap<InsertId, Vec<usize>> = HashMap::new();
        for track in &self.project.tracks {
            if let TrackInput::Plugin { insert, output } = track.input {
                wanted.entry(insert).or_default().push(usize::from(output));
            }
        }
        let mut failures = Vec::new();
        for (id, live) in &mut self.plugins {
            let Instance::Vst3 { plugin, layout } = &mut live.instance else {
                continue;
            };
            let want = wanted.get(id).map_or(&[][..], Vec::as_slice);
            let change: Vec<(usize, bool)> = layout
                .outputs
                .iter()
                .enumerate()
                .skip(1)
                .filter_map(|(k, bus)| {
                    let on = want.contains(&k);
                    (bus.active != on).then_some((k, on))
                })
                .collect();
            if change.is_empty() {
                continue;
            }
            if exporting {
                // After the export (`update` rebuilds then).
                self.outputs_waiting = true;
                continue;
            }
            let Ok(mut p) = plugin.lock() else {
                continue;
            };
            let result = (|| -> Result<()> {
                p.stop_processing()?;
                for &(k, on) in &change {
                    p.set_bus_active(
                        vst3_host::audio::MediaType::Audio,
                        vst3_host::audio::BusDirection::Output,
                        k as i32,
                        on,
                    )?;
                }
                p.start_processing()?;
                p.service_host_requests()?;
                *layout = p.audio_bus_layout()?;
                live.latency = p.latency_samples() as usize;
                Ok(())
            })();
            if let Err(e) = result {
                // Back to processing whatever the buses are, so the plug-in keeps playing.
                let _ = p.start_processing();
                if let Ok(l) = p.audio_bus_layout() {
                    *layout = l;
                }
                failures.push(format!("{}: switching its outputs: {e:#}", p.info().name));
            }
        }
        self.notices.extend(failures);
    }

    /// Insert `insert`'s audio outputs: each one's name, channels, whether it is on, and the
    /// track listening to it. The first is the main pair, which plays on the plug-in's own
    /// track; the rest are what a multi-out rack routes to tracks of their own.
    pub fn plugin_outputs(&self, insert: InsertId) -> Result<Vec<PluginOutput>> {
        let live = self
            .plugins
            .get(&insert)
            .ok_or_else(|| anyhow!("no loaded insert with id {insert}"))?;
        let Instance::Vst3 { layout, .. } = &live.instance else {
            return Ok(Vec::new());
        };
        let listeners = self.project.listeners(insert);
        Ok(layout
            .outputs
            .iter()
            .enumerate()
            .map(|(k, bus)| PluginOutput {
                bus: k,
                name: live
                    .output_names
                    .get(k)
                    .cloned()
                    .unwrap_or_else(|| format!("Output {}", k + 1)),
                channels: bus.channel_count,
                active: bus.active,
                track: listeners
                    .iter()
                    .find(|(_, o)| usize::from(*o) == k)
                    .map(|(t, _)| *t),
            })
            .collect())
    }

    /// For a rack slot's menu, without locking anything: insert `insert`'s extra outputs (0
    /// for a native effect or a plain plug-in) and how many have a track.
    pub fn multi_out(&self, insert: InsertId) -> (usize, usize) {
        let extra = match self.plugins.get(&insert).map(|l| &l.instance) {
            Some(Instance::Vst3 { layout, .. }) => layout.outputs.len().saturating_sub(1),
            _ => 0,
        };
        (extra, self.project.listeners(insert).len())
    }

    /// What a track listening to a plug-in's output shows as its input: "Sampler: Out 2".
    pub fn plugin_output_label(&self, insert: InsertId, output: u16) -> String {
        let name = self
            .project
            .find_insert(insert)
            .map_or_else(|| "A removed plug-in".to_string(), |s| s.name.clone());
        let bus = self
            .plugins
            .get(&insert)
            .and_then(|l| l.output_names.get(usize::from(output)).cloned())
            .unwrap_or_else(|| format!("Output {}", output + 1));
        format!("{name}: {bus}")
    }

    /// A multi-out rack: give insert `insert`'s extra outputs (`buses`,
    /// or every one not already listened to) a track each, in a folder of their own right
    /// after the plug-in's track. Each is an audio track named after its output, hearing it
    /// at unity, with its own fader, inserts and sends, summed in the folder. The main pair
    /// stays on the plug-in's track. Returns the folder and the new tracks.
    pub fn create_multi_out_rack(
        &mut self,
        insert: InsertId,
        buses: Option<&[usize]>,
    ) -> Result<(TrackId, Vec<TrackId>)> {
        let host = self
            .project
            .insert_track(insert)
            .ok_or_else(|| anyhow!("insert {insert} is not on a track"))?;
        if !self
            .project
            .track(host)
            .is_some_and(|t| t.kind.holds_clips())
        {
            bail!("a multi-out rack needs the plug-in on an audio or instrument track");
        }
        let outputs = self.plugin_outputs(insert)?;
        if outputs.len() < 2 {
            bail!(
                "{} has no outputs besides its main one",
                self.project
                    .find_insert(insert)
                    .map_or("the plug-in", |s| s.name.as_str())
            );
        }
        let chosen: Vec<&PluginOutput> = match buses {
            Some(buses) => {
                if let Some(bad) = buses.iter().find(|&&b| b == 0 || b >= outputs.len()) {
                    bail!(
                        "output {bad} isn't an extra output of insert {insert} (1 to {})",
                        outputs.len() - 1
                    );
                }
                outputs.iter().filter(|o| buses.contains(&o.bus)).collect()
            }
            None => outputs
                .iter()
                .skip(1)
                .filter(|o| o.track.is_none())
                .collect(),
        };
        if chosen.is_empty() {
            bail!("every extra output of insert {insert} already has a track");
        }
        let name = self
            .project
            .find_insert(insert)
            .map_or_else(String::new, |s| s.name.clone());
        let folder = self
            .project
            .insert_track_after(TrackKind::Folder, Some(host));
        if let Some(t) = self.project.track_mut(folder) {
            t.name = format!("{name} outputs");
        }
        let mut made = Vec::new();
        for output in chosen {
            // Each at the folder's end: in bus order.
            let track = self.project.add_track(TrackKind::Audio, Some(folder));
            if let Some(t) = self.project.track_mut(track) {
                t.name = output.name.clone();
                t.input = TrackInput::Plugin {
                    insert,
                    output: output.bus as u16,
                };
                t.arm = false;
            }
            made.push(track);
        }
        self.graph_dirty = true;
        Ok((folder, made))
    }

    /// Undo a multi-out rack: the tracks listening to insert `insert`'s outputs go (and the
    /// folders they leave empty), and the plug-in turns those outputs off. Returns the
    /// tracks removed.
    pub fn remove_multi_out_rack(&mut self, insert: InsertId) -> Result<Vec<TrackId>> {
        let listeners: Vec<TrackId> = self
            .project
            .listeners(insert)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        if listeners.is_empty() {
            bail!("insert {insert} has no tracks on its outputs");
        }
        let folders: Vec<TrackId> = listeners
            .iter()
            .filter_map(|t| self.project.track(*t).and_then(|t| t.parent))
            .collect();
        let mut removed = Vec::new();
        for track in listeners {
            self.remove_track(track);
            removed.push(track);
        }
        for folder in folders {
            let empty = !self.project.tracks.iter().any(|t| t.parent == Some(folder));
            if empty && self.project.track(folder).is_some() {
                self.remove_track(folder);
                removed.push(folder);
            }
        }
        self.graph_dirty = true;
        Ok(removed)
    }

    /// The lanes of a track (or the master) that play, as its graph node runs them. Send-level
    /// lanes go on the sends instead.
    fn node_automation(&self, lanes: &[AutomationLane], slots: &[InsertSlot]) -> NodeAutomation {
        let playing = |target| crate::automation::playing(lanes, target).cloned();
        // A plug-in that failed to load is left out of the chain (see `processors`), so a
        // lane's insert is counted among the loaded ones.
        let loaded: Vec<InsertId> = slots
            .iter()
            .filter(|s| self.plugins.contains_key(&s.id))
            .map(|s| s.id)
            .collect();
        let parameters = lanes
            .iter()
            .filter(|l| l.plays())
            .filter_map(|l| match l.target {
                AutomationTarget::Parameter { insert, parameter } => Some(ParameterLane {
                    insert: loaded.iter().position(|id| *id == insert)?,
                    parameter,
                    lane: l.clone(),
                    // Made in uild_graph for every parameter lane.
                    writing: self.param_writing.get(&(insert, parameter))?.clone(),
                }),
                _ => None,
            })
            .collect();
        NodeAutomation {
            volume: playing(AutomationTarget::Volume),
            pan: playing(AutomationTarget::Pan),
            mute: playing(AutomationTarget::Mute),
            parameters,
        }
    }

    fn processors(&self, slots: &[InsertSlot]) -> Vec<Box<dyn Processor>> {
        self.processors_with(slots, &self.engine.shared.inserts, false)
    }

    /// Inserts for `slots` that report to `flags` (the rack's own: see [`crate::rack`]). Each
    /// follows its slot's bypass as it changes (`sync_params`), or, with `snapshot`, keeps the
    /// bypass it has now (an Apply's render: a slot switched while it runs changes only the
    /// preview and the next Apply).
    fn processors_with(
        &self,
        slots: &[InsertSlot],
        flags: &InsertFlags,
        snapshot: bool,
    ) -> Vec<Box<dyn Processor>> {
        slots
            .iter()
            .filter_map(|slot| {
                let live = self.plugins.get(&slot.id)?;
                let bypass = if snapshot {
                    Arc::new(AtomicBool::new(slot.bypass))
                } else {
                    live.bypass.clone()
                };
                let insert = match &live.instance {
                    Instance::Vst3 { plugin, layout } => Box::new(PluginInsert::new(
                        plugin.clone(),
                        layout,
                        f64::from(self.sample_rate),
                        bypass,
                        flags.counting(&live.misses),
                        live.latency,
                        MAX_BLOCK,
                    ))
                        as Box<dyn Processor>,
                    Instance::Native(native) => {
                        Box::new(native.insert(bypass, flags.counting(&live.misses)))
                    }
                };
                Some(
                    Box::new(crate::usage::Timed::new(insert, live.usage.clone()))
                        as Box<dyn Processor>,
                )
            })
            .collect()
    }

    /// Call when tracks, routing, inputs, inserts or clips changed.
    pub fn mark_dirty(&mut self) {
        self.graph_dirty = true;
    }

    /// The CPU use of each track (and the master) and insert, most costly first: a reading
    /// over the last [`crate::usage::WINDOW`], taken again once that has passed (here, or in
    /// [`Self::update`] every frame), so the panel and `get_cpu_usage` show the same numbers.
    pub fn cpu_usage(&mut self) -> &crate::usage::CpuUsage {
        use crate::usage::{CpuUsage, InsertUsage, TrackUsage};
        let due = self
            .cpu_at
            .is_none_or(|at| at.elapsed() >= crate::usage::WINDOW);
        if !due {
            return &self.cpu;
        }
        let window = self.cpu_at.map_or(0.0, |at| at.elapsed().as_secs_f64());
        self.cpu_at = Some(Instant::now());
        let time = crate::cpu::process_time();
        let process = if window > 0.0 {
            ((time - self.cpu_time) / window).max(0.0)
        } else {
            0.0
        };
        self.cpu_time = time;
        let rate = self.sample_rate;
        let inserts = |slots: &[InsertSlot]| -> Vec<InsertUsage> {
            slots
                .iter()
                .filter_map(|slot| {
                    let live = self.plugins.get(&slot.id)?;
                    Some(InsertUsage {
                        insert: slot.id,
                        name: slot.name.clone(),
                        load: live.usage.take().load(rate),
                        bypass: slot.bypass,
                        misses: live.misses.load(Ordering::Relaxed),
                    })
                })
                .collect()
        };
        let mut tracks = vec![TrackUsage {
            track: None,
            name: "Master".into(),
            load: self.master_params.usage.take().load(rate),
            inserts: inserts(&self.project.master.inserts),
        }];
        for track in &self.project.tracks {
            let load = self
                .params
                .get(&track.id)
                .map(|p| p.usage.take().load(rate))
                .unwrap_or_default();
            tracks.push(TrackUsage {
                track: Some(track.id),
                name: track.name.clone(),
                load,
                inserts: inserts(&track.inserts),
            });
        }
        self.cpu = CpuUsage {
            window,
            tracks,
            process,
        };
        self.cpu.sort();
        &self.cpu
    }

    /// Per-frame housekeeping. Cheap when nothing changed.
    pub fn update(&mut self) {
        // A save's, an autosave's or a look for changes' next plug-in state, and their workers.
        self.saving_step();
        // Plug-ins still loading: a few more now. An open or a new sample rate holds the project
        // until they are in (no graph, latency or autosave); the effects rack loading, or an
        // undo bringing plug-ins back, leaves the rest going (playing, recording).
        if let Some(kind) = self.loading.as_ref().map(|l| l.kind) {
            self.load_step(Duration::from_millis(20));
            self.collect_garbage();
            if kind.holds_project() {
                return;
            }
        }
        // Plug-ins an open let go of and then gave up on (a cancel to no project): destroyed
        // a few a frame here too.
        self.close_step(Duration::from_millis(20), false);
        // A synced rack and its track in line: after an undo, and anything outside an edit.
        self.sync_rack();
        // A multi-out rack changed during an export: its outputs switch now.
        if self.outputs_waiting && self.export.as_ref().is_none_or(|e| e.is_finished()) {
            self.outputs_waiting = false;
            self.graph_dirty = true;
        }
        self.cpu_usage();
        self.autosave_tick();
        self.keep_rack_tick();
        self.advance_writer();
        self.collect_apply();
        // The preview plays outside the graph, so it follows the master's mute here.
        self.engine
            .shared
            .preview_muted
            .store(self.project.master.mixer.mute, Ordering::Relaxed);
        if self.previewing.is_some() && !self.engine.shared.preview_playing.load(Ordering::Relaxed)
        {
            self.previewing = None;
        }
        // A safety net: edits sync in `end_edit`, so this should find nothing to do.
        self.sync_following();
        let (arrived, failures) = self.stretches.poll();
        self.notices.extend(failures);
        // A finished render replaces the original in the clips that wait for it.
        self.graph_dirty |= arrived;
        self.service_native_files();
        self.follow_native_latency();
        let lanes_changed = !self
            .project
            .all_lanes()
            .map(|(_, lane)| lane)
            .eq(self.built_lanes.iter());
        // A track's render has the engine on a graph of its own until it ends (`poll_export`);
        // an undo bringing plug-ins back keeps the graph before until they are in (`load_step`
        // sends the new one then).
        let restoring = self.loading.as_ref().is_some_and(|l| l.kind.restores());
        if (self.graph_dirty || lanes_changed) && self.rendering.is_none() && !restoring {
            self.send_project_graph();
        } else {
            self.sync_params();
        }
        self.sync_transport();
        self.collect_garbage();
        self.poll_recording();
        self.release_note_previews(false);
        self.poll_export();
        self.service_plugins();
        self.state_step();
        if let Some(device) = &self.device {
            if let Ok(mut errors) = device.errors.lock() {
                self.notices.extend(errors.drain(..));
            }
        }
    }

    /// Send tempo, meter and loop changes to the engine. Runs every frame, and before playback
    /// starts so a change made just before pressing play is already in effect.
    fn sync_transport(&mut self) {
        let map = self.tempo_map();
        if self.sent_tempo.as_ref() != Some(&map) {
            self.sent_tempo = Some(map.clone());
            self.send(Command::SetTempoMap(Box::new(map)));
        }
        let looping = self.project.loop_region.active();
        if self.sent_loop != Some(looping) {
            self.sent_loop = Some(looping);
            self.send(Command::SetLoop(looping));
        }
        let m = self.project.metronome;
        let click = ClickSettings {
            enabled: m.enabled,
            gain: db_to_gain(m.volume_db),
            only_while_recording: m.only_while_recording,
            channel: usize::from(self.project.master.output_channel),
            muted: self.project.master.mixer.mute,
        };
        if self.sent_click != Some(click) {
            self.sent_click = Some(click);
            self.send(Command::SetMetronome(click));
        }
    }

    /// Change the metronome. The volume must be between −60 and +6 dB, and the count-in 0 to
    /// 2 bars.
    pub fn set_metronome(&mut self, settings: MetronomeSettings) -> Result<()> {
        if !(-60.0..=6.0).contains(&settings.volume_db) {
            bail!(
                "metronome volume {} dB is outside -60..6 dB",
                settings.volume_db
            );
        }
        if settings.count_in_bars > 2 {
            bail!(
                "a count-in of {} bars is not supported (0 to 2)",
                settings.count_in_bars
            );
        }
        self.project.metronome = settings;
        Ok(())
    }

    /// Set the count-in (0 = off, 1 or 2 bars) and nothing else of the metronome: the Record
    /// button's menu (22.3). The same setting, check and undo step ("Metronome") as
    /// [`Self::set_metronome`], which the metronome's menu and the `set_metronome` action use.
    pub fn set_count_in(&mut self, bars: u32) -> Result<()> {
        let mut settings = self.project.metronome;
        settings.count_in_bars = bars;
        self.set_metronome(settings)
    }

    // ---------------------------------------------------------------- undo

    /// Start an undoable edit called `name`. Changes with the same `key` in quick succession
    /// merge into one undo step (a fader nudged repeatedly).
    pub fn begin_edit(&mut self, name: &str, key: Option<String>) {
        self.history.begin(name, key, &self.project);
    }

    /// Finish the edit; it becomes an undo step if the project changed.
    pub fn end_edit(&mut self) -> bool {
        self.sync_following();
        // A synced rack and its track in line (21.2): an edit of either is one step.
        self.sync_rack();
        self.history.end(&self.project)
    }

    /// Say what the edit in progress is, unless something already did (see
    /// [`History::label`]). The app calls this where it changes the project.
    pub fn label_edit(&mut self, name: &str, key: Option<String>) {
        self.history.label(name, key);
    }

    /// Rename a track, folder or bus (dev plan 22.1): the name is trimmed, and an empty one is
    /// refused with the old name kept ([`crate::project::track_name`]). The header's and the
    /// inspector's name fields and `set_track` all rename through here; inside an edit it is
    /// one undo step, "Rename track".
    pub fn rename_track(&mut self, id: crate::project::TrackId, name: &str) -> Result<()> {
        let name = crate::project::track_name(name).map_err(|e| anyhow!(e))?;
        let track = self
            .project
            .track_mut(id)
            .ok_or_else(|| anyhow!("no track {id}"))?;
        if track.name != name {
            track.name = name.to_string();
            self.history.label("Rename track", None);
        }
        Ok(())
    }

    /// A volume (dB) or pan (-1 left to 1 right) typed in for a track, or the master when
    /// `owner` is `None` (dev plan 22.2): clamped to the fader's range, a value that isn't a
    /// number refused. Labelled without a gesture key, so each typed value is an undo step of
    /// its own rather than merging with the last drag of the same control.
    pub fn set_mix_level(
        &mut self,
        owner: Option<crate::project::TrackId>,
        gain_db: Option<f32>,
        pan: Option<f32>,
    ) -> Result<crate::project::Mixer> {
        if gain_db.is_some_and(f32::is_nan) || pan.is_some_and(|p| !p.is_finite()) {
            bail!("a volume or pan must be a number");
        }
        let mixer = match owner {
            Some(id) => {
                &mut self
                    .project
                    .track_mut(id)
                    .ok_or_else(|| anyhow!("no track {id}"))?
                    .mixer
            }
            None => &mut self.project.master.mixer,
        };
        let before = *mixer;
        if let Some(db) = gain_db {
            mixer.set_gain_db(db);
        }
        if let Some(pan) = pan {
            mixer.set_pan(pan);
        }
        let after = *mixer;
        let name = match (owner, after.gain_db != before.gain_db) {
            (None, true) => "Master volume",
            (Some(_), true) => "Volume",
            (None, false) => "Master mixer",
            (Some(_), false) => "Pan",
        };
        if after != before {
            self.history.label(name, None);
        }
        Ok(after)
    }

    /// What undo and redo would do, most recent first, and the history's estimated size.
    pub fn history(&self) -> (Vec<String>, Vec<String>, usize) {
        let (undo, redo) = self.history.names();
        (undo, redo, self.history.bytes())
    }

    /// Undo the last edit; returns its name. Plug-ins it brings back load staged with
    /// [`Self::stage_loads`] (a [`LoadKind::Undo`] loading); see [`Self::restore`].
    pub fn undo(&mut self) -> Result<String> {
        self.check_can_restore()?;
        let Some((name, project)) = self.history.undo(&self.project) else {
            bail!("nothing to undo");
        };
        self.restore(project, LoadKind::Undo, &name);
        self.history.rebase(&self.project);
        Ok(name)
    }

    /// Redo the last undone edit; returns its name. Staged as [`Self::undo`] is.
    pub fn redo(&mut self) -> Result<String> {
        self.check_can_restore()?;
        let Some((name, project)) = self.history.redo(&self.project) else {
            bail!("nothing to redo");
        };
        self.restore(project, LoadKind::Redo, &name);
        self.history.rebase(&self.project);
        Ok(name)
    }

    /// Undo and redo wait for a project opening or reloading at a new rate, and for an export
    /// or a track's render: those have the engine on a graph (and the plug-ins in it) that a
    /// step's plug-ins can't leave until they end.
    fn check_can_restore(&self) -> Result<()> {
        self.check_not_recording("the project")?;
        match self.loading.as_ref().map(|l| l.kind) {
            Some(LoadKind::Open) => bail!("a project is being opened; undo once it is in"),
            Some(LoadKind::SampleRate) => {
                bail!("the plug-ins are reloading at the new sample rate; wait for them")
            }
            _ => {}
        }
        let rendering = self.export.as_ref().is_some_and(|e| !e.is_finished())
            && self.export_kind != ExportKind::File;
        if rendering {
            bail!(
                "an export or a track's render is running; undo or redo once it is done (or \
                 cancel it)"
            );
        }
        Ok(())
    }

    /// Make `project` the project at once (so the history stays whole) and bring the live state
    /// in line. Plug-ins whose insert is gone are taken out (their live states saved into the
    /// snapshots); missing ones load from their saved state and missing files are read, staged
    /// with [`Self::stage_loads`] (a `kind` loading, one plug-in a frame): until they are in,
    /// the engine plays the graph before (gone plug-ins included), and then the new graph is
    /// sent once and the gone ones are destroyed a few a frame after it runs. With nothing to
    /// load, the new graph goes now. An undo over one still loading takes it over: plug-ins it
    /// dropped that this step wants are live again, and the queue is made afresh. Tempo, loop,
    /// metronome and mixer settings follow through `update` as usual.
    fn restore(&mut self, mut project: Project, kind: LoadKind, label: &str) {
        project.keep_ids_beyond(&self.project);
        // Undo changes the music, not the view: track heights stay as they are.
        project.copy_view_from(&self.project);
        self.project = project;
        let wanted: HashSet<InsertId> = self
            .project
            .all_inserts()
            .chain(&self.editor_rack)
            .map(|s| s.id)
            .collect();
        // A step still loading (or the rack's) gives way: its queue is made afresh below, its
        // slots that copy another insert (a synced rack) still copying it.
        let mut copies = HashMap::new();
        let mut retiring = match self.loading.take() {
            Some(l) if l.kind.restores() => {
                copies = l.copies;
                l.retiring
            }
            Some(l) if l.kind == LoadKind::Rack => {
                copies = l.copies;
                Vec::new()
            }
            other => {
                // Refused before (`check_can_restore`): an open or a rate change goes on.
                self.loading = other;
                Vec::new()
            }
        };
        // Plug-ins the step before dropped and this one wants back: live again, no reload.
        let mut still = Vec::new();
        for (id, name, live) in retiring.drain(..) {
            if wanted.contains(&id) {
                self.plugins.insert(id, live);
            } else {
                still.push((id, name, live));
            }
        }
        retiring = still;
        // Plug-ins this step drops: their states saved for undo, their windows closed.
        let gone: Vec<InsertId> = self
            .plugins
            .keys()
            .filter(|id| !wanted.contains(id))
            .copied()
            .collect();
        for id in gone {
            if let Some((name, live)) = self.take_plugin(id) {
                retiring.push((id, name, live));
            }
        }
        // Records the restored tempo map; the restored clips already fit it.
        self.sync_following();
        self.graph_dirty = true;
        let queue: std::collections::VecDeque<_> = self
            .insert_queue()
            .into_iter()
            .filter(|(s, _)| !self.plugins.contains_key(&s.id))
            .collect();
        let files: Vec<(PathBuf, bool)> = self
            .files_to_read()
            .into_iter()
            .filter(|(path, _)| self.media.get(path).is_none())
            .collect();
        if queue.is_empty() && files.is_empty() {
            // Nothing to load: the graph without the dropped plug-ins now (refused during a
            // render, whose graph this would replace), and they go once it runs.
            if !retiring.is_empty() {
                self.send_project_graph();
                self.retire(retiring);
            }
            return;
        }
        self.loading = Some(Loading {
            kind,
            label: Some(label.to_string()),
            retiring,
            closing_total: self.closing.len(),
            reads: Reads::start(files, self.sample_rate, self.read_hold.clone()),
            total: queue.len(),
            queue,
            path: None,
            backup: false,
            previous: None,
            copies,
        });
        if !self.stage_loads {
            self.finish_loading();
        }
    }

    /// Take a plug-in out of the session (no graph rebuilt, nothing destroyed), saving its live
    /// state into every undo snapshot that still has its insert first (undoing its removal then
    /// brings it back with its knobs where they were, not as last saved), its window closed.
    /// Returns it with its name, for [`Self::retire`].
    fn take_plugin(&mut self, id: InsertId) -> Option<(String, LivePlugin)> {
        let mut live = self.plugins.remove(&id)?;
        match live.save_state() {
            Ok(state) => {
                let state = Arc::new(state);
                self.history.for_each_snapshot(|project| {
                    for slot in project.all_inserts_mut().filter(|s| s.id == id) {
                        slot.state = Some(state.clone());
                    }
                });
            }
            Err(e) => self.notices.push(format!(
                "saving a removed plug-in's settings for undo: {e:#}"
            )),
        }
        if let Some(mut window) = live.window.take() {
            window.close();
        }
        live.native_open = false;
        let name = self
            .history_name(id)
            .unwrap_or_else(|| format!("insert {id}"));
        Some((name, live))
    }

    /// An insert's name in the project or any undo snapshot (a removed one's).
    fn history_name(&mut self, id: InsertId) -> Option<String> {
        if let Some(slot) = self.find_insert(id) {
            return Some(slot.name.clone());
        }
        let mut name = None;
        self.history.for_each_snapshot(|project| {
            if name.is_none() {
                name = project
                    .all_inserts()
                    .find(|s| s.id == id)
                    .map(|s| s.name.clone());
            }
        });
        name
    }

    /// Destroy a plug-in the engine no longer runs (rebuild and `flush_pending` first), after
    /// saving its live state into every undo snapshot that still has its insert. While an undo
    /// brings plug-ins back the engine still plays the graph before, which may run it: it waits
    /// with the plug-ins that undo dropped.
    fn release_plugin(&mut self, id: InsertId) {
        if let Some(l) = self.loading.as_mut().filter(|l| l.kind.restores()) {
            let before = l.queue.len();
            l.queue.retain(|(s, _)| s.id != id);
            l.total -= before - l.queue.len();
            if let Some((name, live)) = self.take_plugin(id) {
                if let Some(l) = self.loading.as_mut() {
                    l.retiring.push((id, name, live));
                }
            }
            return;
        }
        drop(self.take_plugin(id));
    }

    /// Add a tempo and/or time-signature change, or update the one already at that place
    /// (fields left `None` keep theirs). Bars and beats count from 0; bar 0 is the opening,
    /// which `project.tempo` and `project.time_signature` hold.
    /// Take a MIDI file's tempo map as the project's: its opening tempo and meter from bar 1,
    /// and its later changes `from_bar` bars on (where the file was dropped), replacing the
    /// project's changes. Checked first; nothing changes if any value is out of range.
    pub fn apply_tempo_settings(&mut self, settings: &TempoSettings, from_bar: u32) -> Result<()> {
        self.check_not_recording("the tempo")?;
        check_tempo(settings.tempo)?;
        check_time_signature(settings.time_signature.0, settings.time_signature.1)?;
        for change in &settings.changes {
            if let Some(tempo) = change.tempo {
                check_tempo(tempo)?;
            }
            if let Some((n, d)) = change.time_signature {
                check_time_signature(n, d)?;
            }
        }
        self.project.tempo = settings.tempo;
        self.project.time_signature = settings.time_signature;
        self.project.tempo_changes = settings
            .changes
            .iter()
            .map(|c| TempoChange {
                bar: c.bar + from_bar,
                ..*c
            })
            .collect();
        Ok(())
    }

    /// Put one track of a MIDI file on `track` as a clip from `start` (ticks), as long as its
    /// content rounded up to the next bar line.
    pub fn import_midi_track(
        &mut self,
        track: TrackId,
        file_track: &MidiFileTrack,
        start: u64,
        name: &str,
    ) -> Result<ClipId> {
        let map = self.tempo_map();
        let content = file_track
            .notes
            .iter()
            .map(|n| n.tick + n.length)
            .chain(file_track.controllers.iter().map(|e| e.tick + 1))
            .chain(std::iter::once(file_track.end))
            .max()
            .unwrap_or(1)
            .max(1);
        let last = map.bar_of_tick(start + content - 1);
        let length = map
            .tick_of_bar(last + 1, 0)
            .saturating_sub(start)
            .max(content);
        let clip = self
            .project
            .add_midi_clip(track, start, length)
            .map_err(|e| anyhow!(e))?;
        if let Some(c) = self.project.find_midi_clip_mut(clip) {
            c.name = name.to_string();
        }
        self.project
            .add_notes(clip, &file_track.notes)
            .map_err(|e| anyhow!(e))?;
        self.project
            .add_controller_events(clip, &file_track.controllers)
            .map_err(|e| anyhow!(e))?;
        self.graph_dirty = true;
        Ok(clip)
    }

    /// The instrument a MIDI file's extra tracks would share with `track`: the first insert
    /// of an instrument track (where its MIDI goes), when it is loaded.
    pub fn instrument_of(&self, track: TrackId) -> Option<InsertId> {
        let t = self
            .project
            .track(track)
            .filter(|t| t.kind == TrackKind::Instrument)?;
        t.inserts
            .first()
            .map(|s| s.id)
            .filter(|id| self.plugins.contains_key(id))
    }

    /// Import a MIDI file from `start` (ticks): its first track onto `track` if that's an
    /// instrument track, every other one (and the first, otherwise) onto new instrument tracks
    /// after it, named by `MidiFile::track_names`. With `same_instrument`, each new track gets
    /// its own copy of `track`'s instrument (`instrument_of`), so a piano's two staves both
    /// play the piano. With `set_tempo`, the file's tempo map becomes the project's first, and
    /// the clips stay on the bar and beat `start` was on. Returns the tracks and clips made.
    pub fn import_midi_file(
        &mut self,
        path: &Path,
        track: Option<TrackId>,
        start: u64,
        set_tempo: bool,
        same_instrument: bool,
    ) -> Result<Vec<(TrackId, ClipId)>> {
        let file = smf::read_path(path)?;
        if file.tracks.is_empty() {
            bail!("{} has no notes or controller events", path.display());
        }
        let start = if set_tempo {
            // Where `start` was in bars, kept through the tempo change.
            let before = self.tempo_map();
            let bar = before.bar_of_tick(start);
            let into = start - before.tick_of_bar(bar, 0);
            if let Some(settings) = file.tempo_settings() {
                self.apply_tempo_settings(&settings, bar.max(0) as u32)?;
                if settings.moved > 0 {
                    self.notices.push(format!(
                        "{} tempo or meter changes were moved onto a beat or bar line",
                        settings.moved
                    ));
                }
            }
            self.tempo_map().tick_of_bar(bar, 0) + into
        } else {
            start
        };
        let stem = path
            .file_stem()
            .map_or_else(|| "MIDI".into(), |s| s.to_string_lossy().into_owned());
        let names = file.track_names(&stem);
        let instrument = track
            .filter(|_| same_instrument)
            .and_then(|t| self.instrument_of(t));
        let mut made = Vec::new();
        let mut after = track;
        for (k, (file_track, name)) in file.tracks.iter().zip(&names).enumerate() {
            let onto = match track.filter(|_| k == 0) {
                Some(t)
                    if self
                        .project
                        .track(t)
                        .is_some_and(|t| t.kind == TrackKind::Instrument) =>
                {
                    t
                }
                _ => {
                    let new = self
                        .project
                        .insert_track_after(TrackKind::Instrument, after);
                    if let Some(t) = self.project.track_mut(new) {
                        t.arm = true;
                        t.name = name.clone();
                    }
                    if let Some(id) = instrument {
                        self.copy_insert(id, InsertTarget::Track(new), 0)?;
                    }
                    new
                }
            };
            let clip = self.import_midi_track(onto, file_track, start, name)?;
            made.push((onto, clip));
            after = Some(onto);
        }
        Ok(made)
    }

    /// Write a MIDI clip to `path` as a type 0 MIDI file: what plays, with the tempo and time
    /// signature at the clip's start. Overwrites `path`.
    pub fn export_midi_clip(&self, clip: ClipId, path: &Path) -> Result<()> {
        let (track, c) = self
            .project
            .find_midi_clip(clip)
            .ok_or_else(|| anyhow!("no MIDI clip with id {clip}"))?;
        let map = self.tempo_map();
        let meter = map.meter_at(map.sample_at_tick(c.start as f64).round() as i64);
        let name = self
            .project
            .track(track)
            .map_or_else(|| c.name.clone(), |t| t.name.clone());
        let bytes = smf::write_clip(c, &name, meter.tempo, (meter.numerator, meter.denominator))?;
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    /// Save every MIDI clip of instrument track `track` as one Standard MIDI file (type 0, at
    /// their places in the song) with the project's tempo and time signature changes: the
    /// notes and controller events as programmed, nothing from plug-ins.
    pub fn export_midi_track(&self, track: TrackId, path: &Path) -> Result<()> {
        let t = self
            .project
            .track(track)
            .ok_or_else(|| anyhow!("no track with id {track}"))?;
        if t.midi_clips.is_empty() {
            bail!("track {track} ({}) has no MIDI clips", t.name);
        }
        let tempo: Vec<smf::TempoAt> = self
            .tempo_map()
            .segments()
            .iter()
            .map(|s| {
                let tick =
                    (s.quarter_notes * f64::from(crate::tempo::TICKS_PER_QUARTER)).round() as u64;
                (tick, s.tempo, (s.numerator, s.denominator))
            })
            .collect();
        let bytes = smf::write_track(&t.midi_clips, &t.name, &tempo)?;
        std::fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
    }

    pub fn add_tempo_change(&mut self, change: TempoChange) -> Result<()> {
        if change.bar == 0 {
            bail!("the first bar is the opening tempo and meter; change those instead");
        }
        if change.tempo.is_none() && change.time_signature.is_none() {
            bail!("give a tempo, a time signature, or both");
        }
        if let Some(tempo) = change.tempo {
            check_tempo(tempo)?;
        }
        if let Some((n, d)) = change.time_signature {
            check_time_signature(n, d)?;
            if change.beat != 0 {
                bail!("a time signature can only change on the first beat of a bar");
            }
        }
        let map = self.tempo_map();
        let beats = map
            .meter_at(map.bar_start(i64::from(change.bar)).round() as i64)
            .numerator;
        if change.beat >= beats {
            bail!(
                "bar {} has {beats} beats, so there is no beat {}",
                change.bar + 1,
                change.beat + 1
            );
        }
        let changes = &mut self.project.tempo_changes;
        match changes
            .iter_mut()
            .find(|c| c.bar == change.bar && c.beat == change.beat)
        {
            Some(existing) => {
                existing.tempo = change.tempo.or(existing.tempo);
                existing.time_signature = change.time_signature.or(existing.time_signature);
            }
            None => changes.push(change),
        }
        changes.sort_by_key(|c| (c.bar, c.beat));
        Ok(())
    }

    /// Remove the change at a bar and beat (from 0).
    /// Set the song's grid (17.4): the Multitrack's lines and snapping, and the piano roll's
    /// and the Waveform view's bars and beats. A fixed one is a note from 1/4 to 1/64
    /// (straight or triplet); nothing is rebuilt, since it changes no audio.
    pub fn set_grid(&mut self, grid: Grid) -> Result<()> {
        if let Grid::Fixed(division) = grid {
            // Through the text form, which is what decides what a song grid may be.
            division
                .to_string()
                .parse::<Grid>()
                .map_err(anyhow::Error::msg)?;
        }
        self.project.grid = grid;
        Ok(())
    }

    pub fn remove_tempo_change(&mut self, bar: u32, beat: u32) -> Result<()> {
        let changes = &mut self.project.tempo_changes;
        let before = changes.len();
        changes.retain(|c| (c.bar, c.beat) != (bar, beat));
        if changes.len() == before {
            bail!(
                "there is no tempo change at bar {}, beat {}",
                bar + 1,
                beat + 1
            );
        }
        Ok(())
    }

    /// Change the tempo in force at `position`: the opening one, or the last change before
    /// it that set a tempo.
    pub fn set_tempo_at(&mut self, position: u64, tempo: f64) -> Result<()> {
        check_tempo(tempo)?;
        match self.change_in_force(position, |c| c.tempo.is_some()) {
            Some(i) => self.project.tempo_changes[i].tempo = Some(tempo),
            None => self.project.tempo = tempo,
        }
        Ok(())
    }

    /// Change the time signature in force at `position`, like [`Self::set_tempo_at`].
    pub fn set_time_signature_at(&mut self, position: u64, meter: (u32, u32)) -> Result<()> {
        check_time_signature(meter.0, meter.1)?;
        match self.change_in_force(position, |c| c.time_signature.is_some()) {
            Some(i) => self.project.tempo_changes[i].time_signature = Some(meter),
            None => self.project.time_signature = meter,
        }
        Ok(())
    }

    /// The last change at or before `position` that `sets` what is wanted.
    fn change_in_force(&self, position: u64, sets: impl Fn(&TempoChange) -> bool) -> Option<usize> {
        let map = self.tempo_map();
        let at = |c: &TempoChange| {
            let bar = map.bar_start(i64::from(c.bar));
            map.sample_of_beat(map.beats_at(bar) + f64::from(c.beat))
        };
        self.project
            .tempo_changes
            .iter()
            .enumerate()
            .filter(|(_, c)| sets(c) && at(c) <= position as f64 + 0.5)
            .max_by_key(|(_, c)| (c.bar, c.beat))
            .map(|(i, _)| i)
    }

    /// Where beats and bars fall, at the running sample rate.
    pub fn tempo_map(&self) -> TempoMap {
        TempoMap::of(&self.project, self.sample_rate)
    }

    /// Where the song ends, in samples: the end of the last audio or MIDI clip. Zoom to fit,
    /// the scroll bar and exports use it. MIDI clips sit in musical time, so their end
    /// depends on the tempo map.
    pub fn content_end(&self) -> u64 {
        let map = self.tempo_map();
        let midi = self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.midi_clips)
            .map(|c| map.sample_at_tick(c.end() as f64).ceil() as u64)
            .max()
            .unwrap_or(0);
        self.project.length().max(midi)
    }

    /// The tempo and meter at the playhead.
    pub fn meter(&self) -> Meter {
        self.tempo_map().meter_at(self.position() as i64)
    }

    /// Copy mixer settings into the atomics the graph reads. A control being written (see
    /// `advance_writer`) gives its live value instead, and its writing bit tells the graph to
    /// play that rather than the lane.
    fn sync_params(&mut self) {
        let writer = &self.writer;
        let live = |owner: LaneOwner, target| writer.live(WriteKey { owner, target });
        // A mute lane takes over from the mute button (the graph reads the lane), and a mute
        // being written from both; solo still silences the track.
        let mute_automated = |lanes: &[AutomationLane]| {
            crate::automation::playing(lanes, AutomationTarget::Mute).is_some()
        };
        let own_mute = |t: &Track| match live(Some(t.id), AutomationTarget::Mute) {
            Some(value) => value >= 0.5,
            None => t.mixer.mute && !mute_automated(&t.automation),
        };
        let audibility = self.project.audibility_with(own_mute);
        let bits = |owner: LaneOwner| {
            [
                (AutomationTarget::Volume, WRITE_VOLUME),
                (AutomationTarget::Pan, WRITE_PAN),
                (AutomationTarget::Mute, WRITE_MUTE),
            ]
            .into_iter()
            .filter(|(target, _)| live(owner, *target).is_some())
            .fold(0, |all, (_, bit)| all | bit)
        };
        let level =
            |owner: LaneOwner, target, fixed: f32| live(owner, target).map_or(fixed, |v| v as f32);
        // A track's render is its own sound: a solo elsewhere doesn't silence it or the tracks
        // it holds; their own mutes still do, as heard (the rendered track itself can't be
        // muted: `render_track` refuses).
        let rendered = self.rendering.as_ref().map(|r| r.track);
        for track in &self.project.tracks {
            let Some(params) = self.params.get(&track.id) else {
                continue;
            };
            let owner = Some(track.id);
            params
                .gain_db
                .store(level(owner, AutomationTarget::Volume, track.mixer.gain_db));
            params
                .pan
                .store(level(owner, AutomationTarget::Pan, track.mixer.pan));
            params.writing.store(bits(owner), Ordering::Relaxed);
            let in_render = rendered.is_some_and(|only| self.project.main_feeds(track.id, only));
            let (muted, main_muted) = if in_render {
                (own_mute(track), false)
            } else {
                (
                    audibility.muted.contains(&track.id),
                    audibility.main_muted.contains(&track.id),
                )
            };
            params.muted.store(muted, Ordering::Relaxed);
            params.main_muted.store(main_muted, Ordering::Relaxed);
            // Off, the track has no node to meter it: its meter falls silent rather than
            // holding its last level.
            if !self.project.plays(track.id) {
                for peak in &params.peak {
                    peak.store(0.0);
                }
            }
            let monitoring = match track.monitor {
                Monitor::On => true,
                Monitor::Off => false,
                Monitor::Auto => track.arm,
            };
            params.monitoring.store(monitoring, Ordering::Relaxed);
            for send in &track.sends {
                if let Some(params) = self.send_params.get(&send.id) {
                    let target = AutomationTarget::SendLevel { send: send.id };
                    params.gain_db.store(level(owner, target, send.gain_db));
                    params
                        .writing
                        .store(live(owner, target).is_some(), Ordering::Relaxed);
                    let muted = send.mute || audibility.muted_sends.contains(&send.id);
                    params.muted.store(muted, Ordering::Relaxed);
                }
            }
        }
        let master = &self.project.master.mixer;
        self.master_params
            .gain_db
            .store(level(None, AutomationTarget::Volume, master.gain_db));
        self.master_params
            .pan
            .store(level(None, AutomationTarget::Pan, master.pan));
        self.master_params
            .writing
            .store(bits(None), Ordering::Relaxed);
        let master_muted = match live(None, AutomationTarget::Mute) {
            Some(value) => value >= 0.5,
            None => master.mute && !mute_automated(&self.project.master.automation),
        };
        self.master_params
            .muted
            .store(master_muted, Ordering::Relaxed);
        for (&(insert, parameter), flag) in &self.param_writing {
            let written = writer.writes(AutomationTarget::Parameter { insert, parameter });
            flag.store(written, Ordering::Relaxed);
        }
        for (id, slot) in self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.inserts)
            .chain(&self.project.master.inserts)
            .chain(&self.editor_rack)
            .map(|s| (s.id, s.bypass))
        {
            if let Some(live) = self.plugins.get(&id) {
                live.bypass.store(slot, Ordering::Relaxed);
            }
        }
    }

    /// A built-in effect's delay follows its settings (a look-ahead): taken before the graph
    /// is built, so the same update compensates it and an export straight after is right.
    fn follow_native_latency(&mut self) {
        let rate = f64::from(self.sample_rate);
        for live in self.plugins.values_mut() {
            if let Instance::Native(native) = &mut live.instance {
                if let Some(latency) = native.latency_changed(rate) {
                    live.latency = latency;
                    self.graph_dirty = true;
                }
            }
        }
    }

    /// Built-in effects that play a file (the Convolution Reverb's impulse): what the DSP gave
    /// back is freed here, finished preparations go to it, and a new file, rate or shaping
    /// setting starts the next on the instance's worker. Failures become notices.
    fn service_native_files(&mut self) {
        let rate = f64::from(self.sample_rate);
        let dir = self.project_dir();
        let resolve = |file: &Path| match &dir {
            Some(dir) if file.is_relative() => dir.join(file),
            _ => file.to_path_buf(),
        };
        for live in self.plugins.values_mut() {
            if let Instance::Native(native) = &mut live.instance {
                if let Some(e) = native.service_file(rate, &resolve) {
                    self.notices.push(e);
                }
            }
        }
    }

    /// Inserts whose file (an impulse) is still being prepared for the settings now. Export
    /// waits for them ([`Self::wait_for_impulses`]), as it does for stretched clips.
    pub fn impulses_preparing(&self) -> Vec<InsertId> {
        let mut ids: Vec<InsertId> = self
            .plugins
            .iter()
            .filter(|(_, live)| matches!(&live.instance, Instance::Native(n) if n.file_pending()))
            .map(|(&id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }

    /// Block until every built-in effect's file (an impulse) is prepared for the settings now,
    /// so what renders is what plays: before an export, a track's render or an Apply, and for
    /// headless tools. A file that fails to load counts as done (its effect passes dry; the
    /// failure is a notice).
    pub fn wait_for_impulses(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.service_native_files();
            let waiting = self.impulses_preparing();
            if waiting.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("the impulses of inserts {waiting:?} are still being prepared");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Have a built-in effect that plays a file (the Convolution Reverb) play the audio file
    /// at `path` (WAV, AIFF or FLAC): read first to check it, copied into the project's
    /// `Imported` folder ([`Self::import_copy`]; a file already in the project is used in
    /// place) and saved by its project-relative path. It is prepared off the audio thread; the
    /// impulse before it plays until it is in. Returns the path as saved.
    pub fn set_impulse_file(&mut self, insert: InsertId, path: &Path) -> Result<PathBuf> {
        let reads = match self.plugins.get(&insert).map(|p| &p.instance) {
            Some(Instance::Native(n)) => match n.kind.reads {
                Some(reads) if !reads.makes() => reads,
                Some(reads) => bail!(
                    "insert {insert} ({}) makes its {} from its settings: it plays no file",
                    n.kind.name,
                    reads.what
                ),
                None => bail!("insert {insert} ({}) plays no impulse file", n.kind.name),
            },
            Some(Instance::Vst3 { .. }) => {
                bail!("insert {insert} is a VST3 plug-in: it chooses its own files")
            }
            None => bail!("no loaded insert with id {insert}"),
        };
        if !path.is_file() {
            bail!("{} is not a file", path.display());
        }
        // Read once here, so a file that isn't audio is refused before it is copied in.
        let audio = crate::media::read_audio(path)
            .with_context(|| format!("reading {} as an {}", path.display(), reads.what))?;
        if audio.is_empty() {
            bail!("{} holds no audio", path.display());
        }
        let copy = self.import_copy(path)?;
        let saved = self.project_relative(&copy);
        if let Some(Instance::Native(native)) =
            self.plugins.get_mut(&insert).map(|p| &mut p.instance)
        {
            native.set_file(saved.clone())?;
        }
        self.service_native_files();
        self.mirror_state(insert);
        Ok(saved)
    }

    fn service_plugins(&mut self) {
        // Plug-ins whose state may have changed unseen (a synced rack copies it across, 21.2).
        let mut unseen = Vec::new();
        for (&id, live) in self.plugins.iter_mut() {
            if let Some(window) = &live.window {
                if window.closed_by_user() {
                    if let Some(mut w) = live.window.take() {
                        w.close();
                    }
                    unseen.push(id);
                } else {
                    let _ = window.service_platform_events();
                }
            }
        }
        // Plugins may change latency at runtime (a lookahead or oversampling switch) or their
        // parameters (a preset from their own browser); pick it up a few times a second and
        // recompensate. Only a plug-in that asked (`restartComponent`) is locked to service it:
        // otherwise the lock would meet the audio thread now and then for nothing.
        if self.last_service.elapsed().as_millis() < 250 {
            for id in unseen {
                self.mirror_state(id);
            }
            return;
        }
        self.last_service = Instant::now();
        let mut changed = false;
        for (&id, live) in self.plugins.iter_mut() {
            let Some(plugin) = live.vst3().cloned() else {
                continue;
            };
            let mut reread = false;
            let pending = match &live.params {
                Some(cache) => {
                    // New values alone need nothing from the processor: read them from the
                    // controller, and leave the lock alone.
                    reread |= cache.link().take_values_changed();
                    cache.link().pending_restart()
                }
                None => vst3_host::RestartFlags::default(),
            };
            // Without a cache nothing tells whether it asked: service it as before.
            if live.params.is_none() || !pending.is_empty() {
                let Ok(mut plugin) = plugin.try_lock() else {
                    continue;
                };
                if let Ok(flags) = plugin.service_host_requests() {
                    if flags.latency_changed() {
                        live.latency = plugin.latency_samples() as usize;
                        changed = true;
                    }
                    reread |= flags.param_titles_changed() || flags.param_values_changed();
                }
            }
            if reread {
                // New values all at once (a preset loaded inside it): a synced copy takes its
                // whole state.
                unseen.push(id);
                if let Some(cache) = &mut live.params {
                    if let Err(e) = cache.refresh() {
                        self.notices.push(format!("{e:#}"));
                    }
                }
            }
        }
        if changed {
            self.graph_dirty = true;
        }
        for id in unseen {
            self.mirror_state(id);
        }
    }

    pub fn stats(&self) -> Stats {
        let s = &self.engine.shared;
        let block_size = s.block_size.load(Ordering::Relaxed) as usize;
        let device_block = s.device_block.load(Ordering::Relaxed) as usize;
        Stats {
            dsp_load: s.dsp_load.load(),
            latency: s.latency.load(Ordering::Relaxed) as usize,
            plugin_misses: s.inserts.misses.load(Ordering::Relaxed),
            skipped_blocks: s.skipped_blocks.load(Ordering::Relaxed),
            input_underruns: s.input_underruns.load(Ordering::Relaxed),
            block_size,
            device_block,
            cadence: match s.cadence.load(Ordering::Relaxed) as usize {
                0 => device_block,
                frames => frames,
            },
            workers: s.workers.load(Ordering::Relaxed) as usize,
        }
    }

    /// Worst callback's DSP load since the last call: its time over the time the driver allowed
    /// it (a period, or a whole burst for a driver that calls back in bursts); over 1.0 is late.
    pub fn take_dsp_peak(&self) -> f32 {
        self.engine.shared.dsp_peak.take()
    }

    /// Peak since the last call, per channel.
    pub fn track_peak(&self, id: TrackId) -> [f32; 2] {
        self.params
            .get(&id)
            .map_or([0.0; 2], |p| [p.peak[0].take(), p.peak[1].take()])
    }

    pub fn master_peak(&self) -> [f32; 2] {
        [
            self.master_params.peak[0].take(),
            self.master_params.peak[1].take(),
        ]
    }

    pub fn input_peak(&self, channel: usize) -> f32 {
        self.engine
            .shared
            .input_peaks
            .get(channel)
            .map_or(0.0, |p| p.take())
    }

    /// Peak since the last call on a hardware output: everything that leaves the device,
    /// including the metronome.
    pub fn output_peak(&self, channel: usize) -> f32 {
        self.engine
            .shared
            .output_peaks
            .get(channel)
            .map_or(0.0, |p| p.take())
    }

    pub fn input_channels(&self) -> usize {
        match (&self.device, &self.virtual_device) {
            (Some(device), _) => device.info.inputs,
            (None, Some(virtual_device)) => virtual_device.inputs,
            (None, None) => 0,
        }
    }

    pub fn output_channels(&self) -> usize {
        self.device.as_ref().map_or(2, |d| d.info.outputs)
    }

    // ---------------------------------------------------------------- transport

    /// Playhead position (what you hear now, i.e. minus delay compensation).
    pub fn position(&self) -> u64 {
        // The engine tracks this itself, through loop wraps (see `EngineCore::publish_position`).
        self.engine.shared.heard.load(Ordering::Relaxed)
    }

    pub fn is_playing(&self) -> bool {
        self.engine.shared.playing.load(Ordering::Relaxed)
    }

    pub fn is_recording(&self) -> bool {
        self.engine.shared.recording.load(Ordering::Relaxed) || self.recording.is_some()
    }

    /// The count-in playing before a take starts (22.3), or `None` (no count-in, the take
    /// already started, or Stop pressed). The transport's display and `get_transport` both read
    /// this, so they agree.
    pub fn count_in_progress(&self) -> Option<CountInProgress> {
        let rec = self.recording.as_ref().filter(|r| !r.stopping)?;
        let (point, (bars, start)) = (rec.record_point?, rec.count_in?);
        let heard = if self.is_playing() {
            self.engine.shared.heard_signed.load(Ordering::Relaxed)
        } else {
            // Record was pressed and the engine hasn't started yet: the whole count-in is ahead.
            start
        };
        if heard >= point as i64 {
            return None;
        }
        let map = self.tempo_map();
        Some(CountInProgress {
            bars,
            beats_left: clicks_left(
                map.beats_at(start as f64),
                map.beats_at(heard as f64),
                map.beats_at(point as f64),
            ),
            record_point: point,
        })
    }

    /// The master's loudness, as the live meter reads it (see `EngineCore::meter_loudness`).
    pub fn loudness(&self) -> crate::loudness::Loudness {
        self.engine.shared.loudness.read()
    }

    /// Send the device silence while everything else (meters included) runs as usual: for checks
    /// that need audio through the master but must not be heard.
    pub fn set_silent(&self, silent: bool) {
        self.engine.shared.silent.store(silent, Ordering::Relaxed);
    }

    /// Start a new loudness measurement: clear integrated, LRA, true peak and the maxima.
    pub fn reset_loudness(&mut self) {
        self.send(Command::ResetLoudness);
    }

    pub fn play(&mut self) {
        // One transport: the project and the Waveform view's preview never play together.
        self.stop_preview();
        self.sync_transport();
        self.send(Command::Play);
    }

    pub fn stop(&mut self) {
        let was_playing = self.is_playing();
        let mut at = self.position();
        // Stopping during a count-in goes back to where recording was to start.
        let heard = self.engine.shared.heard.load(Ordering::Relaxed);
        if let Some(rec) = &mut self.recording {
            // Stopping during a count-in goes back to where recording was to start.
            if let Some(point) = rec.record_point {
                at = at.max(point);
            }
            rec.stopping = true;
            rec.stopped_at
                .get_or_insert(heard.saturating_sub(rec.output_latency));
        }
        self.send(Command::Stop);
        // Stopping leaves the playhead where you heard it stop.
        if was_playing {
            self.send(Command::Locate(at));
        }
    }

    /// Move the playhead. Not while recording.
    pub fn locate(&mut self, position: u64) -> Result<()> {
        self.check_not_recording("the playhead")?;
        self.send(Command::Locate(position));
        Ok(())
    }

    /// Set the loop range, in samples. Keeps whether looping is on.
    pub fn set_loop_region(&mut self, start: u64, end: u64) -> Result<()> {
        self.check_not_recording("the loop")?;
        if end <= start {
            bail!("the loop must end after it starts ({start}..{end})");
        }
        let region = &mut self.project.loop_region;
        region.start = start;
        region.end = end;
        Ok(())
    }

    /// Turn looping on or off. Turning it on with no range set loops the four bars from the
    /// start of the bar the playhead is in.
    pub fn set_looping(&mut self, enabled: bool) -> Result<()> {
        self.check_not_recording("the loop")?;
        let mut region = self.project.loop_region;
        if enabled && region.end <= region.start {
            (region.start, region.end) = self.four_bars_at_playhead();
        }
        self.project.loop_region = Region { enabled, ..region };
        Ok(())
    }

    /// Set the punch range, in samples. Keeps whether punching is on.
    pub fn set_punch_range(&mut self, start: u64, end: u64) -> Result<()> {
        self.check_not_recording("the punch range")?;
        if end <= start {
            bail!("the punch range must end after it starts ({start}..{end})");
        }
        self.project.punch.start = start;
        self.project.punch.end = end;
        Ok(())
    }

    /// Turn punch recording on or off. Turning it on with no range set punches the loop
    /// region if there is one, or else the four bars from the playhead's bar.
    pub fn set_punching(&mut self, enabled: bool) -> Result<()> {
        self.check_not_recording("the punch range")?;
        let mut punch = self.project.punch;
        if enabled && punch.end <= punch.start {
            let looped = self.project.loop_region;
            (punch.start, punch.end) = if looped.end > looped.start {
                (looped.start, looped.end)
            } else {
                self.four_bars_at_playhead()
            };
        }
        self.project.punch = Region { enabled, ..punch };
        Ok(())
    }

    /// The four bars from the start of the bar the playhead is in.
    fn four_bars_at_playhead(&self) -> (u64, u64) {
        let map = self.tempo_map();
        let (bar, _) = map.bar_at(self.position() as f64);
        let at = |bar| map.bar_start(bar).round().max(0.0) as u64;
        (at(bar), at(bar + 4))
    }

    /// Start recording on every armed track with an audio input, and play. From a stop, with
    /// a count-in set, the transport starts that many bars early with the click, and the
    /// take starts at the playhead.
    pub fn record(&mut self) -> Result<()> {
        if self.recording.is_some() {
            bail!("already recording");
        }
        let dir = self.project_folder(crate::folder::RECORDED)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let takes: Vec<TakeSpec> = self
            .project
            .tracks
            .iter()
            .filter(|t| t.arm && t.kind == TrackKind::Audio)
            .filter_map(|t| match t.input {
                TrackInput::Audio { channel, stereo } => Some(TakeSpec {
                    track: t.id,
                    channel: usize::from(channel),
                    stereo,
                    path: dir.join(format!("{}-{stamp}.wav", sanitize(&t.name))),
                }),
                _ => None,
            })
            .collect();
        // Armed instrument tracks record their MIDI input.
        let midi_tracks: Vec<(TrackId, Option<usize>, Option<u8>)> = self
            .project
            .tracks
            .iter()
            .filter(|t| t.arm && t.kind == TrackKind::Instrument)
            .filter_map(|t| match &t.input {
                TrackInput::Midi { port, channel } => {
                    // A port that isn't connected matches nothing.
                    let port = port
                        .as_deref()
                        .map(|name| self.midi.port_index(name).unwrap_or(usize::MAX));
                    Some((t.id, port, *channel))
                }
                _ => None,
            })
            .collect();
        if takes.is_empty() && midi_tracks.is_empty() {
            bail!("arm an audio track with an audio input, or an instrument track, to record");
        }
        if !takes.is_empty() && self.input_channels() == 0 {
            bail!("the audio device has no inputs open");
        }
        let latency = self.record_shift();
        let playing = self.is_playing();
        let position = self.position();
        let punch = self.project.punch.active();
        // Loop recording: with the loop on (and the playhead not already playing past it),
        // every pass becomes a take.
        let looped = self
            .project
            .loop_region
            .active()
            .filter(|&(_, end)| !playing || position < end);
        match (looped, punch) {
            (Some((start, end)), _) if latency >= end - start => bail!(
                "the loop ({} samples) is shorter than the recording latency ({latency} \
                 samples: delay compensation, the device round trip and the record offset); \
                 make it longer to loop-record",
                end - start
            ),
            (Some((s, e)), Some((a, b))) if b <= s || a >= e => bail!(
                "the punch range ({a}..{b}) is outside the loop ({s}..{e}), so no pass would \
                 record anything"
            ),
            (None, Some((start, end))) if position >= end => bail!(
                "the playhead is past the punch range ({start}..{end}); move it before \
                 punch-out or turn punch off"
            ),
            _ => {}
        }
        let expected = takes.len();
        let record::Started {
            mut job,
            files,
            peaks: live,
        } = record::begin(takes, self.sample_rate)?;
        let midi = (!midi_tracks.is_empty()).then(|| {
            let (producer, consumer) = rtrb::RingBuffer::new(MIDI_RECORD_RING);
            job.midi = Some(producer);
            consumer
        });
        // What the player hears runs this late after the engine: the device's output latency
        // and what gathering buffers into blocks adds (23.1).
        let output_latency = self
            .engine
            .shared
            .output_latency
            .load(Ordering::Relaxed)
            .max(0) as u64
            + self.stage_latency();
        // Where playback starts. Loop recording records whole passes from the loop start: a
        // playhead inside or after the loop moves there; one before it is a run-up.
        let from_here = match looped {
            Some((start, _)) if playing || position < start => position,
            Some((start, _)) => start,
            None => position,
        };
        let bars = self.project.metronome.count_in_bars;
        let mut record_point = None;
        let mut count_in = None;
        if bars > 0 && !playing {
            record_point = Some(from_here);
            // As long as the bars before the record point's bar, along the tempo map.
            let map = self.tempo_map();
            let (bar, _) = map.bar_at(from_here as f64);
            let length = map.bar_start(bar) - map.bar_start(bar - i64::from(bars));
            let plan = record::count_in(from_here, length, latency);
            count_in = Some((bars, plan.locate as i64 - plan.pre_roll as i64));
            job.from = plan.from;
            job.count_in_to = Some(plan.click_until);
            self.send(Command::Locate(plan.locate));
            if plan.pre_roll > 0 {
                self.send(Command::PreRoll(plan.pre_roll));
            }
        } else if !playing && from_here != position {
            self.send(Command::Locate(from_here));
        }
        match looped {
            // The tap opens when the loop start is heard and stays open across the wraps;
            // a punch range is cut out of each pass afterwards.
            Some((start, _)) => {
                job.from = job.from.max(start + latency);
                // Already playing past the loop start: wait for the next pass.
                job.exact_from = true;
            }
            // Inside the punch range only. Checked above that some of it is still ahead.
            None => {
                if let Some(range) = punch {
                    (job.from, job.to) = record::punch(job.from, range, latency)
                        .context("the playhead is past the punch range")?;
                }
            }
        }
        self.recording = Some(ActiveRecording {
            files,
            expected,
            received: Vec::new(),
            start: None,
            latency,
            record_point,
            count_in,
            looped,
            punch,
            stopping: false,
            job_returned: false,
            midi,
            midi_events: Vec::new(),
            midi_dropped: 0,
            midi_tracks,
            output_latency,
            window: (
                job.from.saturating_sub(latency),
                job.to.saturating_sub(latency),
            ),
            stopped_at: None,
            live,
            drawn_whole: false,
        });
        // No start until the new job's tap opens: the last recording's must not place this one.
        self.engine
            .shared
            .record_start
            .store(u64::MAX, Ordering::Relaxed);
        self.send(Command::StartRecording(job));
        self.play();
        Ok(())
    }

    /// Whether the next `update` rebuilds the graph: for testing that an edit reaches the
    /// engine.
    #[cfg(test)]
    pub(crate) fn graph_dirty(&self) -> bool {
        self.graph_dirty
    }

    /// Act as if recording had started (no inputs exist headless), for testing the rules that
    /// apply while recording.
    #[cfg(test)]
    pub(crate) fn pretend_recording(&mut self) {
        let (_, files) = record::start(Vec::new(), self.sample_rate).expect("no files to create");
        self.recording = Some(ActiveRecording {
            files,
            expected: 1,
            received: Vec::new(),
            start: None,
            latency: 0,
            record_point: None,
            count_in: None,
            looped: self.project.loop_region.active(),
            punch: None,
            stopping: false,
            job_returned: false,
            midi: None,
            midi_events: Vec::new(),
            midi_dropped: 0,
            midi_tracks: Vec::new(),
            output_latency: 0,
            window: (0, u64::MAX),
            stopped_at: None,
            live: Vec::new(),
            drawn_whole: false,
        });
    }

    /// Finish a MIDI-only recording as if the engine had stamped `events` (heard positions)
    /// and stop had been pressed at `stopped_at`, for testing the placement without a device.
    #[cfg(test)]
    pub(crate) fn finish_midi_recording(
        &mut self,
        events: Vec<RecordedMidi>,
        window: (u64, u64),
        stopped_at: u64,
        output_latency: u64,
    ) {
        let (_, files) = record::start(Vec::new(), self.sample_rate).expect("no files to create");
        let midi_tracks = self
            .project
            .tracks
            .iter()
            .filter(|t| t.arm && t.kind == TrackKind::Instrument)
            .map(|t| match &t.input {
                TrackInput::Midi { channel, .. } => (t.id, None, *channel),
                _ => (t.id, None, None),
            })
            .collect();
        self.recording = Some(ActiveRecording {
            files,
            expected: 0,
            received: Vec::new(),
            start: None,
            latency: 0,
            record_point: None,
            count_in: None,
            looped: self.project.loop_region.active(),
            punch: self.project.punch.active(),
            stopping: true,
            job_returned: true,
            midi: None,
            midi_events: events,
            midi_dropped: 0,
            midi_tracks,
            output_latency,
            window,
            stopped_at: Some(stopped_at),
            live: Vec::new(),
            drawn_whole: false,
        });
        self.poll_recording();
    }

    /// How far a take is moved earlier than the engine position it was recorded at, so it
    /// lines up with what was heard while playing it: the delay compensation (the output runs
    /// that far behind the engine), the device round trip measured from its timestamps, what
    /// gathering buffers into blocks adds (the player hears the engine that much later, 23.1),
    /// and the manual record offset. One number, used for every placement: plain takes,
    /// count-in, punch and loop passes.
    pub fn record_shift(&self) -> u64 {
        let shared = &self.engine.shared;
        let compensation = shared.latency.load(Ordering::Relaxed) as i64;
        let round_trip = shared.round_trip.load(Ordering::Relaxed).max(0);
        let stage = self.stage_latency() as i64;
        let offset = i64::from(self.settings.record_offset);
        (compensation + round_trip + stage + offset).max(0) as u64
    }

    /// The device's measured output-to-input round trip in samples, once known.
    pub fn round_trip(&self) -> Option<u64> {
        let frames = self.engine.shared.round_trip.load(Ordering::Relaxed);
        (frames != i64::MIN).then(|| frames.max(0) as u64)
    }

    /// The audio takes being recorded, as far as each has come, where their clips will be when
    /// recording stops (21.1): the same start, offset into the file and peaks, so the clips
    /// take their place without a jump. A loop recording shows the pass being recorded. Empty
    /// before the tap opens (a count-in) and once the takes are clips.
    pub fn recording_takes(&self) -> Vec<RecordingTake<'_>> {
        let Some(rec) = &self.recording else {
            return Vec::new();
        };
        let engine_start = match rec.start {
            Some(start) => start,
            None => match self.engine.shared.record_start.load(Ordering::Relaxed) {
                u64::MAX => return Vec::new(),
                start => start,
            },
        };
        // As `poll_recording` places the takes.
        let heard_start = engine_start.saturating_sub(rec.latency);
        rec.live
            .iter()
            .filter_map(|take| {
                let (start, offset, length) =
                    record::growing(take.frames, heard_start, rec.looped, rec.punch);
                (length > 0).then_some(RecordingTake {
                    track: take.track,
                    name: &take.name,
                    start,
                    offset,
                    length,
                    peaks: &take.peaks,
                })
            })
            .collect()
    }

    /// The loop, the punch range and the playhead stay put while recording: takes are placed
    /// by them when recording stops.
    fn check_not_recording(&self, what: &str) -> Result<()> {
        if self.recording.as_ref().is_some_and(|r| !r.stopping) {
            bail!("stop recording first: {what} can't change while recording");
        }
        Ok(())
    }

    fn poll_recording(&mut self) {
        let Some(rec) = &mut self.recording else {
            return;
        };
        while let Ok(file) = rec.files.try_recv() {
            rec.received.push(file);
        }
        // Read MIDI as it comes, so the ring never fills; what's left after the engine let go
        // of the job is still there to read.
        if let Some(midi) = &mut rec.midi {
            while let Ok(event) = midi.pop() {
                rec.midi_events.push(event);
            }
        }
        // The takes' waveforms, likewise; the last bins arrive before the job comes back.
        for take in &mut rec.live {
            take.drain();
        }
        // Once the job is back the takes are whole: they are drawn so for a frame before they
        // become clips (the writer takes that long anyway), so their last picture is the
        // finished file's and the clips replace it without a jump.
        let drawn = rec.live.is_empty() || rec.drawn_whole;
        rec.drawn_whole = rec.job_returned;
        if !rec.job_returned || rec.received.len() < rec.expected || !drawn {
            return;
        }
        let mut rec = self.recording.take().expect("checked above");
        let received = std::mem::take(&mut rec.received);
        let start = rec.start.unwrap_or(0).saturating_sub(rec.latency);
        // The new takes are one undo step.
        self.begin_edit("Record", None);
        for file in received {
            if let Some(error) = file.error {
                self.notices
                    .push(format!("Recording {}: {error}", file.path.display()));
                continue;
            }
            if file.frames == 0 {
                let _ = std::fs::remove_file(&file.path);
                continue;
            }
            let passes = rec
                .looped
                .and_then(|range| record::slice_passes(file.frames, start, range, rec.punch));
            let placed = match passes {
                Some(passes) => self.add_loop_takes(file.track, &file.path, &passes),
                None => self
                    .add_clip_from_file(file.track, &file.path, start)
                    .map(|_| ()),
            };
            if let Err(e) = placed {
                self.notices.push(format!("{e:#}"));
            }
        }
        self.place_midi_recording(&rec);
        self.end_edit();
    }

    /// Turn the MIDI that came in while recording into notes on each armed instrument track
    /// (see `midi_record`), merged or replacing as the project's settings say.
    fn place_midi_recording(&mut self, rec: &ActiveRecording) {
        if rec.midi_dropped > 0 {
            self.notices.push(format!(
                "Recording lost {} MIDI events (too many at once)",
                rec.midi_dropped
            ));
        }
        if rec.midi_events.is_empty() {
            return;
        }
        let map = self.tempo_map();
        let settings = self.project.midi_record.clone();
        let quantize = settings.quantize.as_deref().and_then(|q| q.parse().ok());
        // Loop recording keeps the loop (and the punch range inside it); otherwise the window
        // up to where stop was pressed.
        let range = match rec.looped {
            Some((start, end)) => rec
                .punch
                .map_or((start, end), |(a, b)| (a.max(start), b.min(end))),
            None => (
                rec.window.0,
                rec.window.1.min(rec.stopped_at.unwrap_or(u64::MAX)),
            ),
        };
        for &(track, port, channel) in &rec.midi_tracks {
            let hits: Vec<MidiHit> = rec
                .midi_events
                .iter()
                .filter(|e| port.is_none_or(|p| p == e.port))
                .filter(|e| channel.is_none_or(|c| midi_channel(&e.event) == Some(c)))
                .map(|e| {
                    let mut at = e.heard - rec.output_latency as i64;
                    // Just after a wrap the player still hears the loop's end.
                    if let Some((start, end)) = rec.looped.filter(|_| e.wraps > 0) {
                        if at < start as i64 {
                            at += (end - start) as i64;
                        }
                    }
                    MidiHit { at, event: e.event }
                })
                .collect();
            let Some(take) = midi_record::take(&hits, range, &map, quantize) else {
                continue;
            };
            if let Err(e) = self.project.place_midi_take(track, take, settings.replace) {
                self.notices.push(e);
            }
        }
        self.graph_dirty = true;
    }

    /// Put a loop recording's passes on a track as takes. A clip that already spans exactly
    /// the same range (an earlier round of takes) gets them added; otherwise a new clip holds
    /// them. The newest complete pass plays.
    pub(crate) fn add_loop_takes(
        &mut self,
        track: TrackId,
        path: &Path,
        passes: &record::LoopTakes,
    ) -> Result<()> {
        self.media.get_or_load(path, self.sample_rate)?;
        let file = self.project_relative(path);
        let track_name = self
            .project
            .track(track)
            .map(|t| t.name.clone())
            .unwrap_or_default();
        let existing = self
            .project
            .track(track)
            .and_then(|t| {
                // Not a stretched or tempo-following clip: its takes' offsets count samples of
                // its render, and these passes are plain recordings.
                t.clips.iter().find(|c| {
                    c.start == passes.start
                        && c.length == passes.length
                        && !c.is_stretched()
                        && c.follow.is_none()
                })
            })
            .map(|c| (c.id, c.takes.len()));
        let first = existing.map_or(0, |(_, n)| n);
        let takes: Vec<Take> = passes
            .passes
            .iter()
            .enumerate()
            .map(|(k, pass)| Take {
                file: file.clone(),
                offset: pass.offset,
                name: format!("{track_name} take {}", first + k + 1),
            })
            .collect();
        match existing {
            Some((id, _)) => {
                let clip = self.project.clip_mut(id).expect("found above");
                clip.takes.extend(takes);
                clip.active = first + passes.active;
            }
            None => {
                let id = self.project.next_id();
                let name = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut clip = Clip::new(id, name, file, passes.start, 0, passes.length);
                clip.takes = takes;
                clip.active = passes.active;
                self.project
                    .track_mut(track)
                    .ok_or_else(|| anyhow!("track not found"))?
                    .clips
                    .push(clip);
            }
        }
        self.graph_dirty = true;
        Ok(())
    }

    /// `path` relative to the project folder when it lies inside it.
    fn project_relative(&self, path: &Path) -> PathBuf {
        self.project_dir()
            .and_then(|dir| path.strip_prefix(dir).ok().map(Path::to_path_buf))
            .unwrap_or_else(|| path.to_path_buf())
    }

    // ---------------------------------------------------------------- tracks

    pub fn add_track(&mut self, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
        let id = self.project.add_track(kind, parent);
        if kind == TrackKind::Instrument {
            if let Some(t) = self.project.track_mut(id) {
                t.arm = true;
            }
        }
        self.graph_dirty = true;
        id
    }

    /// Play `pitch` on an instrument track's plug-in now, armed or not: previewing a note
    /// from the editor. With a `length` it is released by itself after that long; without
    /// one, [`Self::preview_note_off`] releases it.
    pub fn preview_note(
        &mut self,
        track: TrackId,
        pitch: u8,
        velocity: u8,
        length: Option<std::time::Duration>,
    ) -> Result<()> {
        let t = self
            .project
            .track(track)
            .ok_or_else(|| anyhow!("no track with id {track}"))?;
        if t.kind != TrackKind::Instrument {
            bail!("track {track} is not an instrument track");
        }
        if t.inserts.is_empty() {
            bail!("track {track} has no plug-in to play");
        }
        if pitch > 127 {
            bail!("pitch {pitch} is outside 0..127");
        }
        if !(1..=127).contains(&velocity) {
            bail!("velocity {velocity} is outside 1..127");
        }
        // Never more than a handful ringing: the oldest goes first.
        if self.note_previews.len() >= MAX_NOTE_PREVIEWS {
            let (_, track, pitch) = self.note_previews.remove(0);
            self.preview_note_off(track, pitch);
        }
        let channel = vst3_host::MidiChannel::Ch1;
        self.send(Command::SendMidi {
            track,
            event: vst3_host::MidiEvent::NoteOn {
                channel,
                note: pitch,
                velocity,
            },
        });
        if let Some(length) = length {
            self.note_previews
                .push((Instant::now() + length, track, pitch));
        }
        Ok(())
    }

    /// Move notes of a MIDI clip (`notes`, or all of them) toward the grid: each start goes
    /// `strength` (0 to 1) of the way to its nearest line, in exact ticks; lengths are kept.
    pub fn quantize_notes(
        &mut self,
        clip: ClipId,
        notes: Option<&[NoteId]>,
        division: Division,
        strength: f64,
    ) -> Result<()> {
        let (_, c) = self
            .project
            .find_midi_clip(clip)
            .ok_or_else(|| anyhow!("no MIDI clip with id {clip}"))?;
        if let Some(missing) = notes
            .unwrap_or(&[])
            .iter()
            .find(|id| !c.notes.iter().any(|n| n.id == **id))
        {
            bail!("MIDI clip {clip} has no note with id {missing}");
        }
        let map = self.tempo_map();
        let strength = strength.clamp(0.0, 1.0);
        let start = c.start;
        let moved: Vec<Note> = c
            .notes
            .iter()
            .map(|n| {
                if notes.is_some_and(|ids| !ids.contains(&n.id)) {
                    return *n;
                }
                let at = start + n.tick;
                let line = map.snap_tick(at, division, Rounding::Nearest);
                let to = at as f64 + (line as f64 - at as f64) * strength;
                Note {
                    tick: (to.round() as u64).saturating_sub(start),
                    ..*n
                }
            })
            .collect();
        self.project
            .set_notes(clip, moved)
            .map_err(|e| anyhow!(e))?;
        self.graph_dirty = true;
        Ok(())
    }

    /// Lengthen a MIDI clip to the bar line after its last note's end, if a note runs past
    /// it: an edit that puts a note beyond the end makes it play rather than fall silent.
    pub fn fit_midi_clip(&mut self, clip: ClipId) {
        let map = self.tempo_map();
        let Some(c) = self.project.find_midi_clip_mut(clip) else {
            return;
        };
        let Some(end) = c.notes.iter().map(|n| n.tick + n.length).max() else {
            return;
        };
        if end > c.length {
            let last = map.bar_of_tick(c.start + end - 1);
            c.length = map
                .tick_of_bar(last + 1, 0)
                .saturating_sub(c.start)
                .max(end);
            self.graph_dirty = true;
        }
    }

    /// Release a previewed note.
    pub fn preview_note_off(&mut self, track: TrackId, pitch: u8) {
        self.note_previews
            .retain(|&(_, t, p)| (t, p) != (track, pitch));
        self.send(Command::SendMidi {
            track,
            event: vst3_host::MidiEvent::NoteOff {
                channel: vst3_host::MidiChannel::Ch1,
                note: pitch,
                velocity: 0,
            },
        });
    }

    /// Release previewed notes that are due (or all of them), forgetting those whose track
    /// has gone with its plug-in.
    fn release_note_previews(&mut self, all: bool) {
        let now = Instant::now();
        let due: Vec<(TrackId, u8)> = self
            .note_previews
            .iter()
            .filter(|(at, _, _)| all || *at <= now)
            .map(|&(_, t, p)| (t, p))
            .collect();
        for (track, pitch) in due {
            if self.project.track(track).is_some() {
                self.preview_note_off(track, pitch);
            } else {
                self.note_previews.retain(|&(_, t, _)| t != track);
            }
        }
    }

    /// How many previewed notes are waiting for their release (for tests).
    #[cfg(test)]
    pub(crate) fn note_previews_pending(&self) -> usize {
        self.note_previews.len()
    }

    /// Add a bus (a return track) at the end of the project and give `from` a post-fader send
    /// into it at 0 dB, as one undo step. Returns the bus and the send.
    pub fn add_send_to_new_bus(&mut self, from: TrackId) -> Result<(TrackId, SendId)> {
        if self.project.track(from).is_none() {
            bail!("no track with id {from}");
        }
        self.label_edit("Add send to new bus", None);
        let bus = self.add_track(TrackKind::Bus, None);
        // A brand-new bus feeds nothing, so this can't close a loop.
        let send = self
            .project
            .add_send(from, bus, false)
            .map_err(|e| anyhow!(e))?;
        Ok((bus, send))
    }

    pub fn remove_track(&mut self, id: TrackId) {
        let Some(track) = self.project.remove_track(id) else {
            return;
        };
        // Rebuild without the track so the audio thread lets go, then destroy its plugins.
        self.graph_dirty = true;
        self.update();
        self.flush_pending();
        for slot in &track.inserts {
            self.release_plugin(slot.id);
        }
        self.params.remove(&id);
        for send in &track.sends {
            self.send_params.remove(&send.id);
        }
    }

    // ---------------------------------------------------------------- plugins

    pub fn installed_plugins(&self) -> &[PathBuf] {
        &self.installed
    }

    pub fn rescan_plugins(&mut self) {
        self.installed = plugins::installed(&self.host);
    }

    fn slots_mut(&mut self, target: InsertTarget) -> Option<&mut Vec<InsertSlot>> {
        match target {
            InsertTarget::Master => Some(&mut self.project.master.inserts),
            InsertTarget::Editor => Some(&mut self.editor_rack),
            InsertTarget::Track(id) => self.project.track_mut(id).map(|t| &mut t.inserts),
        }
    }

    fn load_live(&mut self, path: &Path, state: Option<&[u8]>) -> Result<LivePlugin> {
        if let Some(kind) = crate::native::kind_of(path) {
            let native = crate::native::Native::new(kind, f64::from(self.sample_rate), state)?;
            return Ok(LivePlugin {
                latency: native.latency(),
                instance: Instance::Native(native),
                bypass: Arc::new(AtomicBool::new(false)),
                is_instrument: false,
                has_editor: true,
                window: None,
                native_open: false,
                usage: Arc::default(),
                misses: Arc::default(),
                params: None,
                output_names: Vec::new(),
            });
        }
        let mut plugin = plugins::load(&mut self.host, path, None)?;
        let preroll = (f64::from(self.sample_rate) * PREROLL_SECONDS) as usize;
        // Pre-roll with the project's tempo and a stopped transport, as export's pre-roll runs.
        plugins::send_transport(
            &mut plugin,
            &Transport {
                playing: false,
                position: 0,
                meter: self.meter(),
            },
            &mut None,
        );
        let activation = plugins::activate(&mut plugin, 512, preroll, state)?;
        if let Some(e) = &activation.state_error {
            self.notices.push(format!(
                "{}: could not restore state: {e}",
                plugins::display_name(path)
            ));
        }
        let is_instrument = plugins::is_instrument(&plugin);
        let has_editor = plugin.has_editor();
        let output_names = plugin.audio_output_names().unwrap_or_default();
        // Read once it is active with its state restored (some plug-ins' values settle then), so
        // nothing that reads parameters later needs the plug-in's lock.
        let params = plugin
            .controller_link()
            .and_then(|link| ParameterCache::new(link).ok());
        Ok(LivePlugin {
            instance: Instance::Vst3 {
                plugin: Arc::new(Mutex::new(plugin)),
                layout: activation.layout,
            },
            bypass: Arc::new(AtomicBool::new(false)),
            latency: activation.latency,
            is_instrument,
            has_editor,
            window: None,
            native_open: false,
            usage: Arc::default(),
            misses: Arc::default(),
            params,
            output_names,
        })
    }

    /// Load a plugin into a track's (or the master's) insert chain.
    pub fn add_insert(&mut self, target: InsertTarget, path: &Path) -> Result<InsertId> {
        self.add_insert_at(target, path, None)
    }

    /// Load a plug-in into `target`'s chain at `position` (0 is first; past the end appends).
    /// Without a position an instrument goes first, so MIDI reaches it, and an effect last.
    pub fn add_insert_at(
        &mut self,
        target: InsertTarget,
        path: &Path,
        position: Option<usize>,
    ) -> Result<InsertId> {
        if target == InsertTarget::Editor && self.applying.is_some() {
            bail!("the effects rack is busy applying; wait for it to finish");
        }
        // An old built-in id (`native::RENAMED`) is stored as today's.
        let path = &crate::native::current_path(path.to_path_buf());
        let live = self.load_live(path, None)?;
        let id = if target == InsertTarget::Editor {
            self.next_rack_id += 1;
            self.next_rack_id
        } else {
            self.project.next_id()
        };
        let slot = InsertSlot {
            id,
            name: plugins::display_name(path),
            path: path.to_path_buf(),
            bypass: false,
            state: None,
        };
        let is_instrument = live.is_instrument;
        let slots = self
            .slots_mut(target)
            .ok_or_else(|| anyhow!("track not found"))?;
        let at = insert_index(slots.len(), is_instrument, position);
        slots.insert(at, slot);
        self.plugins.insert(id, live);
        self.graph_dirty = true;
        Ok(id)
    }

    pub fn remove_insert(&mut self, id: InsertId) {
        self.load_failures.remove(&id);
        if self.editor_rack.iter().any(|s| s.id == id) {
            // In no graph: destroyed here (or after an Apply rendering through it).
            self.remove_rack_slot(id);
            return;
        }
        self.project.remove_insert(id);
        // Rebuild without it first so the audio thread lets go, then destroy it here.
        self.graph_dirty = true;
        self.update();
        self.flush_pending();
        self.release_plugin(id);
    }

    pub fn move_insert(&mut self, id: InsertId, up: bool) {
        for slots in self
            .project
            .tracks
            .iter_mut()
            .map(|t| &mut t.inserts)
            .chain(std::iter::once(&mut self.project.master.inserts))
            .chain(std::iter::once(&mut self.editor_rack))
        {
            if let Some(i) = slots.iter().position(|s| s.id == id) {
                let j = if up { i.checked_sub(1) } else { Some(i + 1) };
                if let Some(j) = j.filter(|j| *j < slots.len()) {
                    slots.swap(i, j);
                    self.graph_dirty = true;
                }
                return;
            }
        }
    }

    /// Move an insert to `position` in its chain (past the end: last). Returns whether it
    /// moved.
    pub fn move_insert_to(&mut self, id: InsertId, position: usize) -> bool {
        for slots in self
            .project
            .tracks
            .iter_mut()
            .map(|t| &mut t.inserts)
            .chain(std::iter::once(&mut self.project.master.inserts))
            .chain(std::iter::once(&mut self.editor_rack))
        {
            if let Some(i) = slots.iter().position(|s| s.id == id) {
                let to = position.min(slots.len() - 1);
                if to == i {
                    return false;
                }
                let slot = slots.remove(i);
                slots.insert(to, slot);
                self.graph_dirty = true;
                return true;
            }
        }
        false
    }

    /// Which track (or the master) holds insert `id` in its chain; None for the Waveform view's
    /// rack or an unknown id.
    pub fn insert_owner(&self, id: InsertId) -> Option<InsertTarget> {
        if self.project.master.inserts.iter().any(|s| s.id == id) {
            return Some(InsertTarget::Master);
        }
        self.project
            .tracks
            .iter()
            .find(|t| t.inserts.iter().any(|s| s.id == id))
            .map(|t| InsertTarget::Track(t.id))
    }

    /// The automation lanes of `owner` (a track or the master).
    fn owner_lanes(&mut self, owner: InsertTarget) -> Option<&mut Vec<AutomationLane>> {
        match owner {
            InsertTarget::Track(id) => self.project.track_mut(id).map(|t| &mut t.automation),
            InsertTarget::Master => Some(&mut self.project.master.automation),
            InsertTarget::Editor => None,
        }
    }

    /// Move insert `id` into `target`'s chain at `position` (0 is first; past the end makes it
    /// last): within its own chain a reorder, onto another track or the master a move, with
    /// the same plug-in instance (so its state) and its parameters' automation lanes. The
    /// Waveform view's rack keeps its own plug-ins. Returns whether anything moved.
    pub fn move_insert_into(
        &mut self,
        id: InsertId,
        target: InsertTarget,
        position: usize,
    ) -> Result<bool> {
        let from = self
            .insert_owner(id)
            .ok_or_else(|| anyhow!("no insert with id {id} on a track or the master"))?;
        if target == InsertTarget::Editor {
            bail!("the Waveform view's rack keeps its own plug-ins: copy settings there by hand");
        }
        if from == target {
            return Ok(self.move_insert_to(id, position));
        }
        if self.slots_mut(target).is_none() {
            bail!("no such track to move insert {id} to");
        }
        let source = self.slots_mut(from).expect("found its owner");
        let at = source
            .iter()
            .position(|s| s.id == id)
            .expect("found its owner");
        let slot = source.remove(at);
        let dest = self.slots_mut(target).expect("checked");
        let to = position.min(dest.len());
        dest.insert(to, slot);
        // Its parameters' lanes go with it.
        let moves = |l: &AutomationLane| matches!(l.target, AutomationTarget::Parameter { insert, .. } if insert == id);
        let lanes: Vec<AutomationLane> = match self.owner_lanes(from) {
            Some(lanes) => {
                let taken = lanes.iter().filter(|l| moves(l)).cloned().collect();
                lanes.retain(|l| !moves(l));
                taken
            }
            None => Vec::new(),
        };
        if let Some(dest) = self.owner_lanes(target) {
            dest.extend(lanes);
        }
        self.graph_dirty = true;
        Ok(true)
    }

    /// Copy insert `id` into `target`'s chain at `position`: a new instance of the same plug-in
    /// with the same settings (its state, read now) and bypass, without its automation. Returns
    /// the new insert's id.
    pub fn copy_insert(
        &mut self,
        id: InsertId,
        target: InsertTarget,
        position: usize,
    ) -> Result<InsertId> {
        let clip = self.plugin_clip(id)?;
        self.place_plugin(&clip, target, position)
    }

    /// Insert `id` as it is now: its plug-in, name, bypass and settings.
    fn plugin_clip(&self, id: InsertId) -> Result<PluginClip> {
        let (slot, live) = self
            .find_insert(id)
            .zip(self.plugins.get(&id))
            .ok_or_else(|| anyhow!("no loaded insert with id {id}"))?;
        Ok(PluginClip {
            name: slot.name.clone(),
            path: slot.path.clone(),
            bypass: slot.bypass,
            state: Arc::new(live.save_state()?),
        })
    }

    /// A new instance of `clip`'s plug-in with its settings, in `target`'s chain at `position`
    /// (past the end: last). Returns the new insert's id.
    fn place_plugin(
        &mut self,
        clip: &PluginClip,
        target: InsertTarget,
        position: usize,
    ) -> Result<InsertId> {
        if target == InsertTarget::Editor && self.applying.is_some() {
            bail!("the effects rack is busy applying; wait for it to finish");
        }
        if self.slots_mut(target).is_none() {
            bail!("no such track to put {} on", clip.name);
        }
        let live = self.load_live(&clip.path, Some(&clip.state))?;
        let new = if target == InsertTarget::Editor {
            self.next_rack_id += 1;
            self.next_rack_id
        } else {
            self.project.next_id()
        };
        let dest = self.slots_mut(target).expect("checked above");
        let to = position.min(dest.len());
        dest.insert(
            to,
            InsertSlot {
                id: new,
                name: clip.name.clone(),
                path: clip.path.clone(),
                bypass: clip.bypass,
                state: None,
            },
        );
        self.plugins.insert(new, live);
        self.graph_dirty = true;
        Ok(new)
    }

    /// Put insert `id` on the plug-in clipboard as it is now (its settings read now), for
    /// [`Self::paste_plugin`]. Replaces what was there; not an edit of the project.
    pub fn copy_plugin(&mut self, id: InsertId) -> Result<&PluginClip> {
        let clip = self.plugin_clip(id)?;
        Ok(self.plugin_clipboard.insert(clip))
    }

    /// The plug-in on the clipboard, if one was copied.
    pub fn plugin_clipboard(&self) -> Option<&PluginClip> {
        self.plugin_clipboard.as_ref()
    }

    /// A new instance of the copied plug-in, with the settings it had when copied, in
    /// `target`'s chain at `position` (past the end: last). Pastes as often as asked.
    pub fn paste_plugin(&mut self, target: InsertTarget, position: usize) -> Result<InsertId> {
        let clip = self
            .plugin_clipboard
            .clone()
            .ok_or_else(|| anyhow!("no plug-in has been copied"))?;
        self.place_plugin(&clip, target, position)
    }

    /// Bypass (or enable) every insert of `target` at once, as the fx rack's power button
    /// does. Returns how many changed.
    pub fn set_inserts_bypassed(&mut self, target: InsertTarget, bypass: bool) -> Result<usize> {
        let slots = self
            .slots_mut(target)
            .ok_or_else(|| anyhow!("track not found"))?;
        let mut changed = 0;
        for slot in slots.iter_mut().filter(|s| s.bypass != bypass) {
            slot.bypass = bypass;
            changed += 1;
        }
        Ok(changed)
    }

    /// An insert slot by id, in the project or the Waveform view's rack.
    pub fn find_insert(&self, id: InsertId) -> Option<&InsertSlot> {
        self.project
            .find_insert(id)
            .or_else(|| self.editor_rack.iter().find(|s| s.id == id))
    }

    pub fn insert_mut(&mut self, id: InsertId) -> Option<&mut InsertSlot> {
        if self.editor_rack.iter().any(|s| s.id == id) {
            return self.editor_rack.iter_mut().find(|s| s.id == id);
        }
        self.project.insert_mut(id)
    }

    /// The slots of an insert chain: a track's, the master's or the rack's.
    pub fn insert_slots(&self, target: InsertTarget) -> &[InsertSlot] {
        match target {
            InsertTarget::Track(id) => self.project.track(id).map_or(&[], |t| &t.inserts),
            InsertTarget::Master => &self.project.master.inserts,
            InsertTarget::Editor => &self.editor_rack,
        }
    }

    /// Blocks an insert has missed live since it loaded: blocks where its track went silent
    /// because something held the plug-in (or a built-in effect's DSP) when the audio thread
    /// came for it.
    pub fn insert_misses(&self, id: InsertId) -> Option<u64> {
        self.plugins
            .get(&id)
            .map(|p| p.misses.load(Ordering::Relaxed))
    }

    pub fn insert_info(&self, id: InsertId) -> Option<(bool, usize, bool)> {
        self.plugins
            .get(&id)
            .map(|p| (p.has_editor, p.latency, p.editor_open()))
    }

    /// The live plugin behind an insert, for reading and setting its parameters. Lock it only
    /// briefly: the audio thread skips the plugin while the lock is held.
    pub fn plugin(&self, id: InsertId) -> Option<Arc<Mutex<Plugin>>> {
        self.plugins.get(&id).and_then(|p| p.vst3().cloned())
    }

    /// The native effect behind an insert, if it holds one.
    pub fn native(&self, id: InsertId) -> Option<&crate::native::Native> {
        match &self.plugins.get(&id)?.instance {
            Instance::Native(native) => Some(native),
            Instance::Vst3 { .. } => None,
        }
    }

    /// The native effects whose editors are open, for the app to draw.
    pub fn open_native_editors(&self) -> Vec<InsertId> {
        let mut open: Vec<InsertId> = self
            .plugins
            .iter()
            .filter(|(_, p)| p.native_open)
            .map(|(&id, _)| id)
            .collect();
        open.sort_unstable();
        open
    }

    /// An insert's parameters, normalized to 0..1, whatever kind of insert it is. Every
    /// parameter caller (actions, automation, the app) comes here rather than to the plug-in.
    /// A VST3 plug-in's come from its cache (`param_cache`), never its lock, so reading them
    /// costs the track nothing.
    pub fn parameters(&self, insert: InsertId) -> Result<Vec<Parameter>> {
        let live = self
            .plugins
            .get(&insert)
            .ok_or_else(|| anyhow!("plug-in {insert} isn't loaded"))?;
        match &live.instance {
            Instance::Native(native) => Ok(native.parameters()),
            Instance::Vst3 { .. } => Ok(Self::cache(live, insert)?.parameters().to_vec()),
        }
    }

    /// A VST3 insert's edit controller and parameter queues (`vst3_host::ControllerLink`), for
    /// checks that act as its editor would or read it directly; the main thread only. Drop it
    /// before the plug-in goes (removed, or the session dropped): it holds the controller.
    pub fn controller_link(&self, insert: InsertId) -> Option<vst3_host::ControllerLink> {
        Some(self.plugins.get(&insert)?.params.as_ref()?.link().clone())
    }

    /// A VST3 insert's parameter cache.
    fn cache(live: &LivePlugin, insert: InsertId) -> Result<&ParameterCache> {
        live.params
            .as_ref()
            .ok_or_else(|| anyhow!("plug-in {insert} has no edit controller, so no parameters"))
    }

    /// How an insert shows each of `values` (a parameter id and a normalized value), e.g.
    /// "-6.0 dB". A VST3 plug-in's controller says, without the plug-in's lock.
    pub fn parameter_displays(
        &self,
        insert: InsertId,
        values: &[(u32, f64)],
    ) -> Result<Vec<String>> {
        let live = self
            .plugins
            .get(&insert)
            .ok_or_else(|| anyhow!("plug-in {insert} isn't loaded"))?;
        Ok(match &live.instance {
            Instance::Native(native) => values
                .iter()
                .map(|&(id, value)| native.format(id, value).unwrap_or_default())
                .collect(),
            Instance::Vst3 { .. } => {
                let cache = Self::cache(live, insert)?;
                values
                    .iter()
                    .map(|&(id, value)| cache.display(id, value))
                    .collect()
            }
        })
    }

    /// Set an insert's parameter `id` to a normalized `value` (0..1), and say how it shows it.
    /// A VST3 plug-in takes it without its lock: its controller and the cache now, its
    /// processor at the start of its next block (`ControllerLink::set`).
    pub fn set_parameter(&mut self, insert: InsertId, id: u32, value: f64) -> Result<String> {
        let shown = self.set_parameter_here(insert, id, value)?;
        // A synced rack's slot and its track insert are set alike (21.2): to what this one
        // holds now (a read-only one keeps its value).
        let now = match self.plugins.get(&insert).map(|p| (&p.instance, &p.params)) {
            Some((Instance::Native(native), _)) => native.value(id),
            Some((Instance::Vst3 { .. }, Some(cache))) => cache.get(id).map_or(value, |p| p.value),
            _ => value,
        };
        self.mirror_parameter(insert, id, now);
        Ok(shown)
    }

    /// [`Self::set_parameter`] on this insert alone.
    fn set_parameter_here(&mut self, insert: InsertId, id: u32, value: f64) -> Result<String> {
        let live = self
            .plugins
            .get_mut(&insert)
            .ok_or_else(|| anyhow!("plug-in {insert} isn't loaded"))?;
        match &mut live.instance {
            Instance::Native(native) => {
                native.set(id, value)?;
                // What it holds now: a read-only one keeps its value.
                native.format(id, native.value(id))
            }
            Instance::Vst3 { .. } => live
                .params
                .as_mut()
                .ok_or_else(|| anyhow!("plug-in {insert} has no edit controller"))?
                .set(id, value),
        }
    }

    /// A plug-in's automatable parameters (id and name), for choosing one to automate.
    pub fn automatable_parameters(&self, insert: InsertId) -> Result<Vec<(u32, String)>> {
        Ok(self
            .parameters(insert)?
            .into_iter()
            .filter(|p| p.can_automate && !p.is_read_only)
            .map(|p| (p.id, p.name))
            .collect())
    }

    // ------------------------------------------------------------------ automation

    /// What `target` of `owner` (a track, or the master for `None`) is set to by hand: the
    /// fader (dB), pan, mute (0 or 1), a send's level (dB) or a plug-in parameter (0..1).
    /// `None` when there is no such thing, or the plug-in isn't loaded.
    pub fn fixed_value(&self, owner: LaneOwner, target: AutomationTarget) -> Option<f64> {
        let mixer = match owner {
            Some(track) => &self.project.track(track)?.mixer,
            None => &self.project.master.mixer,
        };
        Some(match target {
            AutomationTarget::Volume => f64::from(mixer.gain_db),
            AutomationTarget::Pan => f64::from(mixer.pan),
            AutomationTarget::Mute => f64::from(u8::from(mixer.mute)),
            AutomationTarget::SendLevel { send } => {
                f64::from(self.project.find_send(send)?.1.gain_db)
            }
            AutomationTarget::Parameter { insert, parameter } => {
                self.parameters(insert)
                    .ok()?
                    .into_iter()
                    .find(|p| p.id == parameter)?
                    .value
            }
        })
    }

    /// The value the playing lane for `target` gives at the playhead, through `map`; `None`
    /// while no lane plays it. For showing what the fader is doing: the engine itself reads
    /// lanes at the position of the audio, after latency.
    pub fn playing_value(
        &self,
        owner: LaneOwner,
        target: AutomationTarget,
        map: &TempoMap,
    ) -> Option<f64> {
        let lanes = self.project.lanes(owner)?;
        crate::automation::playing(lanes, target)?.value_at(map.tick_at(self.position() as f64))
    }

    /// Give `owner` a lane for `target`, show its lanes in the arrangement, and return the
    /// lane's id. With no `points` the lane starts with one point at the song's start holding
    /// the current setting, so adding it changes nothing; it starts empty when that isn't known
    /// (a plug-in that didn't load). The app's menu and `add_automation_lane` both come here.
    pub fn add_automation_lane(
        &mut self,
        owner: LaneOwner,
        target: AutomationTarget,
        points: Option<&[NewPoint]>,
    ) -> Result<LaneId, String> {
        let seed: Vec<NewPoint> = match points {
            Some(points) => points.to_vec(),
            None => self
                .fixed_value(owner, target)
                .map(|value| {
                    let (low, high) = target.range();
                    let value = if target.steps() {
                        f64::from(u8::from(value >= 0.5))
                    } else {
                        value.clamp(low, high)
                    };
                    NewPoint {
                        tick: 0,
                        value,
                        curve: 0.0,
                    }
                })
                .into_iter()
                .collect(),
        };
        // Every check before the lane exists: a refused call leaves nothing behind.
        if let Some(error) = self.project.lane_target_error(owner, target) {
            return Err(error);
        }
        for p in &seed {
            p.check(&target)?;
        }
        let id = self.project.add_lane(owner, target)?;
        self.project.add_points(id, &seed)?;
        if let Some(track) = owner.and_then(|t| self.project.track_mut(t)) {
            track.show_automation = true;
        }
        Ok(id)
    }

    /// What `owner`'s controls do to its lanes while playing.
    pub fn automation_mode(&self, owner: LaneOwner) -> AutomationMode {
        match owner {
            Some(track) => self
                .project
                .track(track)
                .map_or(AutomationMode::Read, |t| t.automation_mode),
            None => self.project.master.automation_mode,
        }
    }

    /// The playhead in ticks, for writing.
    fn write_tick(&self) -> u64 {
        self.tempo_map()
            .tick_at(self.position() as f64)
            .round()
            .max(0.0) as u64
    }

    /// A control of `owner` was moved (or grabbed) to `value`. While playing in a write mode,
    /// this writes it: from now the control is what plays, and the pass puts it into the lane
    /// when playback stops. Otherwise nothing happens. Values are in the target's units.
    pub fn touch_control(&mut self, owner: LaneOwner, target: AutomationTarget, value: f64) {
        if self.is_playing() {
            let tick = self.write_tick();
            self.touch_control_at(owner, target, value, tick);
        }
    }

    /// [`Self::touch_control`] at `tick`, playing or not: for checks that drive a pass without a
    /// device. The value before the first touch (what a lane made for it starts at) is read
    /// here, so it is the setting before the control moved.
    pub fn touch_control_at(
        &mut self,
        owner: LaneOwner,
        target: AutomationTarget,
        value: f64,
        tick: u64,
    ) {
        let mode = self.automation_mode(owner);
        if !mode.writes() {
            return;
        }
        let key = WriteKey { owner, target };
        let before = if self.writer.has(key) {
            value
        } else {
            self.fixed_value(owner, target).unwrap_or(value)
        };
        self.writer.touch(key, mode, value, tick, before);
    }

    /// The control was let go: in touch mode the lane takes over again.
    pub fn release_control(&mut self, owner: LaneOwner, target: AutomationTarget) {
        let tick = self.write_tick();
        self.writer.release(WriteKey { owner, target }, tick);
    }

    /// The value a control is being written at, while it is: what it shows and what plays.
    pub fn written_value(&self, owner: LaneOwner, target: AutomationTarget) -> Option<f64> {
        self.writer.live(WriteKey { owner, target })
    }

    /// The write pass, once a frame: start it when playback starts (write mode arms every
    /// playing lane at its value there), record each written control at the playhead, notice
    /// jumps, take the knob gestures of plug-in editors, and when playback stops put
    /// everything written into the lanes as one undo step.
    fn advance_writer(&mut self) {
        // Every frame, writing or not, so the parameter caches stay current; gestures from
        // outside a pass are dropped.
        let mut gestures = self.take_parameter_changes();
        // A synced rack's knobs and its track's turn together (21.2).
        self.mirror_gestures(&mut gestures);
        // An export runs the transport too, but writes nothing.
        let exporting = self.export.as_ref().is_some_and(|e| !e.is_finished());
        let playing = self.is_playing() && !exporting;
        if !playing {
            if self.was_playing {
                self.finish_writing();
            }
            self.was_playing = false;
            return;
        }
        let position = self.position();
        let any_mode = self.project.master.automation_mode.writes()
            || self
                .project
                .tracks
                .iter()
                .any(|t| t.automation_mode.writes());
        if !any_mode && self.writer.is_empty() {
            self.was_playing = true;
            self.last_write_position = position;
            return;
        }
        let tick = self.write_tick();
        let starting = !self.was_playing;
        if starting {
            self.writer = Writer::default();
            self.arm_write_mode(tick);
        }
        // A loop wrap or a locate ends what was being written and starts again where it went.
        let jumped = self.was_playing
            && (position < self.last_write_position
                || position > self.last_write_position + u64::from(self.sample_rate) / 2);
        self.writer.advance(tick, jumped);
        // Gestures from before the pass are stale.
        if !starting {
            self.write_gestures(gestures, tick);
        }
        self.was_playing = true;
        self.last_write_position = position;
    }

    /// Write mode at the start of a pass: every playing lane of an owner in write mode is
    /// written from here, at its value here.
    fn arm_write_mode(&mut self, tick: u64) {
        let owners: Vec<(LaneOwner, &[AutomationLane])> = self
            .project
            .tracks
            .iter()
            .filter(|t| t.automation_mode == AutomationMode::Write)
            .map(|t| (Some(t.id), t.automation.as_slice()))
            .chain(
                (self.project.master.automation_mode == AutomationMode::Write)
                    .then_some((None, self.project.master.automation.as_slice())),
            )
            .collect();
        let mut arm = Vec::new();
        for (owner, lanes) in owners {
            for lane in lanes.iter().filter(|l| l.plays()) {
                if let Some(value) = lane.value_at(tick as f64) {
                    arm.push((
                        WriteKey {
                            owner,
                            target: lane.target,
                        },
                        value,
                    ));
                }
            }
        }
        for (key, value) in arm {
            self.writer.arm(key, value, tick);
        }
    }

    /// What moved in each VST3 plug-in since the last frame, without its lock (TD61): values
    /// from automation and the processor go into its parameter cache, and its editor's knob
    /// gestures come back, each with its insert and owner, for the write pass.
    fn take_parameter_changes(&mut self) -> Vec<(LaneOwner, InsertId, Gesture)> {
        let mut taken = Vec::new();
        for (&insert, live) in &mut self.plugins {
            let Some(cache) = live.params.as_mut() else {
                continue;
            };
            for gesture in cache.take_changes() {
                taken.push((insert, gesture));
            }
        }
        taken
            .into_iter()
            .map(|(insert, gesture)| {
                let owner = self
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.inserts.iter().any(|s| s.id == insert))
                    .map(|t| t.id);
                (owner, insert, gesture)
            })
            .collect()
    }

    /// Knob gestures from plug-in editors, for owners in a write mode: a value change touches
    /// the parameter, the end of the gesture lets it go.
    fn write_gestures(&mut self, gestures: Vec<(LaneOwner, InsertId, Gesture)>, tick: u64) {
        use vst3_host::ParameterEditKind;
        for (owner, insert, Gesture { edit, before }) in gestures {
            // The Waveform view's rack has no lanes (its owner would read as the master).
            if self.editor_rack.iter().any(|s| s.id == insert) {
                continue;
            }
            let mode = self.automation_mode(owner);
            if !mode.writes() {
                continue;
            }
            let key = WriteKey {
                owner,
                target: AutomationTarget::Parameter {
                    insert,
                    parameter: edit.id,
                },
            };
            match (edit.kind, edit.value) {
                (ParameterEditKind::BeginGesture, _) => {
                    if let Some(before) = before {
                        self.gesture_before.insert((insert, edit.id), before);
                    }
                }
                (ParameterEditKind::ValueChange, Some(value)) => {
                    let value = value.clamp(0.0, 1.0);
                    let before = self
                        .gesture_before
                        .get(&(insert, edit.id))
                        .copied()
                        .unwrap_or(value);
                    self.writer.touch(key, mode, value, tick, before);
                }
                (ParameterEditKind::EndGesture, _) => self.writer.release(key, tick),
                _ => {}
            }
        }
    }

    /// Playback stopped: put what the pass wrote into the lanes, as one undo step. A control
    /// without a lane gets one, starting at its fixed setting.
    pub(crate) fn finish_writing(&mut self) {
        let written = self.writer.finish(self.writer.tick());
        if written.iter().all(|p| p.segments.is_empty()) {
            return;
        }
        self.begin_edit("Write automation", None);
        self.gesture_before.clear();
        for pass in written {
            let (key, segments) = (pass.key, pass.segments);
            if segments.is_empty() {
                continue;
            }
            let existing = self
                .project
                .lanes(key.owner)
                .and_then(|lanes| lanes.iter().find(|l| l.target == key.target))
                .map(|l| l.id);
            let lane = match existing {
                Some(lane) => lane,
                // Seeded at the setting before the pass touched it, so the song before the
                // touch keeps it.
                None => match self.add_automation_lane(
                    key.owner,
                    key.target,
                    Some(&[NewPoint {
                        tick: 0,
                        value: pass.before,
                        curve: 0.0,
                    }]),
                ) {
                    Ok(lane) => lane,
                    Err(e) => {
                        self.notices.push(format!("Writing automation: {e}"));
                        continue;
                    }
                },
            };
            if let Err(e) = self.project.write_segments(lane, &segments, WRITE_GLIDE) {
                self.notices.push(format!("Writing automation: {e}"));
            }
        }
        self.end_edit();
    }

    /// Open or close a plugin's editor window.
    pub fn set_editor_open(&mut self, id: InsertId, open: bool) -> Result<()> {
        let is_open = self
            .plugins
            .get(&id)
            .is_some_and(|p| p.window.is_some() || p.native_open);
        if is_open != open {
            self.toggle_editor(id)?;
        }
        Ok(())
    }

    pub fn is_loaded(&self, id: InsertId) -> bool {
        self.plugins.contains_key(&id)
    }

    pub fn toggle_editor(&mut self, id: InsertId) -> Result<()> {
        let live = self
            .plugins
            .get_mut(&id)
            .ok_or_else(|| anyhow!("plugin is not loaded"))?;
        let plugin = match &live.instance {
            Instance::Native(_) => {
                live.native_open = !live.native_open;
                if !live.native_open {
                    self.mirror_state(id);
                }
                return Ok(());
            }
            Instance::Vst3 { plugin, .. } => plugin.clone(),
        };
        if let Some(mut window) = live.window.take() {
            window.close();
            // What its window did that no parameter showed reaches a synced copy (21.2).
            self.mirror_state(id);
            return Ok(());
        }
        let mut window = PluginWindow::new(plugin);
        window.open().context("opening editor")?;
        live.window = Some(window);
        Ok(())
    }

    pub fn any_editor_open(&self) -> bool {
        self.plugins
            .values()
            .any(|p| p.window.is_some() || p.native_open)
    }

    fn capture_plugin_states(&mut self) {
        let plugins = &self.plugins;
        let mut failures = Vec::new();
        for slot in self.project.all_inserts_mut() {
            if let Some(live) = plugins.get(&slot.id) {
                match live.save_state() {
                    Ok(state) => slot.state = Some(Arc::new(state)),
                    Err(e) => failures.push(format!("{}: saving state: {e:#}", slot.name)),
                }
            }
        }
        self.notices.extend(failures);
        // Not an edit: if one is in progress (the app's frame), it must not record this.
        let states: HashMap<InsertId, Option<Arc<Vec<u8>>>> = self
            .project
            .all_inserts()
            .map(|s| (s.id, s.state.clone()))
            .collect();
        self.history.update_pending(|project| {
            for slot in project.all_inserts_mut() {
                if let Some(state) = states.get(&slot.id) {
                    slot.state = state.clone();
                }
            }
        });
    }

    // ---------------------------------------------------------------- clips and media

    /// The project's folder: the project file's, or for an unsaved session its
    /// [`Self::scratch`] folder (none in the app until a project is created or opened).
    pub fn project_dir(&self) -> Option<PathBuf> {
        match &self.path {
            Some(p) => p.parent().map(Path::to_path_buf),
            None => self.scratch.clone(),
        }
    }

    /// The project's `sub` folder ([`crate::folder`]), made if it isn't there yet. Refused
    /// with no project open, so no file is ever written outside one.
    pub fn project_folder(&self, sub: &str) -> Result<PathBuf> {
        let dir = self.project_dir().ok_or_else(|| {
            anyhow!(
                "no project is open: create one (new_project with a name and folder) or open one"
            )
        })?;
        crate::folder::ensure(&dir, sub)
    }

    /// Absolute path of a clip's file.
    pub fn resolve(&self, file: &Path) -> PathBuf {
        match self.project_dir() {
            Some(dir) if file.is_relative() => dir.join(file),
            _ => file.to_path_buf(),
        }
    }

    /// The project's own copy of a file brought in from outside its folder, in `Imported/`
    /// (a copy already there with the same bytes is reused), so edits never touch the
    /// original and the folder holds everything. A file inside the folder is used in place.
    pub fn import_copy(&self, path: &Path) -> Result<PathBuf> {
        let path =
            std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))?;
        if self.project_dir().is_some_and(|dir| path.starts_with(dir)) {
            return Ok(path);
        }
        let folder = self.project_folder(crate::folder::IMPORTED)?;
        crate::folder::copy_into(&path, &folder)
    }

    fn load_media(&mut self) {
        let files: Vec<PathBuf> = self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .flat_map(|c| &c.takes)
            .map(|take| self.resolve(&take.file))
            .collect();
        for path in files {
            if self.media.get(&path).is_some() {
                continue;
            }
            if let Err(e) = self.media.get_or_load(&path, self.sample_rate) {
                self.notices.push(format!("{}: {e:#}", path.display()));
            }
        }
    }

    /// Play take `take` of a clip.
    pub fn set_active_take(&mut self, clip: ClipId, take: usize) -> Result<()> {
        self.project
            .set_active_take(clip, take)
            .map_err(|e| anyhow!(e))?;
        self.graph_dirty = true;
        Ok(())
    }

    /// Remove a take from a clip; its file stays on disk.
    pub fn delete_take(&mut self, clip: ClipId, take: usize) -> Result<()> {
        self.project
            .delete_take(clip, take)
            .map_err(|e| anyhow!(e))?;
        self.graph_dirty = true;
        Ok(())
    }

    /// Import an audio file onto a track at `start`.
    pub fn add_clip_from_file(
        &mut self,
        track: TrackId,
        path: &Path,
        start: u64,
    ) -> Result<ClipId> {
        // Loaded first, so a file that isn't audio is refused before it is copied in; the
        // copy holds the same bytes, so it takes the loaded audio (loaded once, cached once).
        let original =
            std::path::absolute(path).with_context(|| format!("resolving {}", path.display()))?;
        let cached = self.media.get(&original).is_some();
        let data = self.media.get_or_load(&original, self.sample_rate)?;
        let path = &self.import_copy(&original)?;
        if *path != original {
            self.media.insert(path.clone(), data.clone());
            if !cached {
                self.media.forget(|p| p == original);
            }
        }
        let id = self.project.next_id();
        let file = self.project_relative(path);
        let name = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let clip = Clip::new(id, name, file, start, 0, data.len() as u64);
        self.project
            .track_mut(track)
            .ok_or_else(|| anyhow!("track not found"))?
            .clips
            .push(clip);
        self.graph_dirty = true;
        Ok(id)
    }

    // ---------------------------------------------------------------- stretch

    /// The render a stretched or transposed clip plays: its take's whole file at its ratio and
    /// pitch, at the session's rate. `None` for a clip played as it is.
    fn stretch_key(&self, clip: &Clip, map: &TempoMap) -> Option<stretch::Key> {
        if !clip.is_stretched() && clip.follow.is_none() {
            return None;
        }
        let path = self.resolve(clip.file());
        let frames = self.media.get(&path)?.len();
        let mut warp = clip
            .follow_warp(map, frames)
            .unwrap_or_else(|| vec![(frames, stretch::stretched_length(frames, clip.stretch))]);
        if clip.transients && warp.iter().any(|(input, output)| input != output) {
            let data = self.media.get(&path)?;
            let ms = |v: f64| (v / 1000.0 * f64::from(self.sample_rate)) as usize;
            warp = stretch::protect_transients(
                &warp,
                data.onsets(),
                ms(stretch::ATTACK_BEFORE_MS),
                ms(stretch::ATTACK_AFTER_MS),
            );
        }
        // A clip following the tempo it was recorded at plays its file as it is.
        if clip.pitch == 0.0 && warp.iter().all(|(input, output)| input == output) {
            return None;
        }
        Some(stretch::Key::new(path, warp, clip.pitch, self.sample_rate))
    }

    /// The audio a clip plays and where in it the clip begins. A stretched clip plays its
    /// render; until that is ready (or if it failed) it plays its file as it is, from the same
    /// moment of it. `None` if the file isn't loaded.
    pub fn clip_audio(&self, clip: &Clip) -> Option<(Arc<AudioData>, u64)> {
        let source = self.media.get(&self.resolve(clip.file()))?;
        let key = self.stretch_key(clip, &self.tempo_map());
        Some(match key {
            None => (source.clone(), clip.offset()),
            Some(key) => match self.stretches.get(&key) {
                Some(data) => (data.clone(), clip.offset()),
                None => (source.clone(), clip.file_offset(self.sample_rate)),
            },
        })
    }

    /// Whether a stretched clip is still waiting for its render.
    pub fn clip_rendering(&self, clip: &Clip) -> bool {
        self.stretch_key(clip, &self.tempo_map())
            .is_some_and(|key| {
                self.media.get(&key.path).is_some()
                    && !matches!(
                        self.stretches.entry(&key),
                        Some(stretch::Entry::Ready(_) | stretch::Entry::Failed(_))
                    )
            })
    }

    /// Stretched clips still waiting for their render.
    pub fn clips_rendering(&self) -> Vec<ClipId> {
        self.project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| self.clip_rendering(c))
            .map(|c| c.id)
            .collect()
    }

    /// Start the renders stretched clips need, and drop the ones no clip uses any more.
    fn request_stretches(&mut self) {
        let map = self.tempo_map();
        let wanted: Vec<stretch::Key> = self
            .project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter_map(|c| self.stretch_key(c, &map))
            .collect();
        self.stretches.retain(|key| wanted.contains(key));
        for key in &wanted {
            if let Some(source) = self.media.get(&key.path).cloned() {
                self.stretches.request(key, &source);
            }
        }
    }

    /// Stretch a clip by `ratio` (2.0: twice as long, same pitch) and/or transpose it by
    /// `pitch` semitones (same length). Its start stays; its length scales with the ratio. The
    /// clip plays its file as it is until the render is ready ([`Self::clip_rendering`]).
    pub fn set_clip_stretch(
        &mut self,
        id: ClipId,
        ratio: Option<f64>,
        pitch: Option<f64>,
    ) -> Result<()> {
        let clip = self
            .project
            .clip_mut(id)
            .ok_or_else(|| anyhow!("no clip with id {id}"))?;
        if ratio.is_some() && clip.follow.is_some() {
            bail!("clip {id} follows the tempo, which sets its stretch; stop following first");
        }
        let (ratio, pitch) = (ratio.unwrap_or(clip.stretch), pitch.unwrap_or(clip.pitch));
        stretch::check(ratio, pitch)?;
        clip.set_stretch(ratio);
        clip.pitch = pitch;
        self.graph_dirty = true;
        Ok(())
    }

    // ---------------------------------------------------------------- audio quantize

    /// The transients (attacks) in a clip's audio as heard: timeline positions in samples
    /// and strengths (0 to 1). `sensitivity` 0 to 1 (higher finds softer attacks),
    /// `min_gap_ms` the shortest distance between two.
    pub fn detect_transients(
        &self,
        clip: ClipId,
        sensitivity: f32,
        min_gap_ms: f32,
    ) -> Result<Vec<(u64, f32)>> {
        let clip = self
            .project
            .find_clip(clip)
            .map(|(_, c)| c.clone())
            .ok_or_else(|| anyhow!("no clip with id {clip}"))?;
        // The audio as heard: a stretched clip's attacks are where its render has them.
        let (data, offset) = self
            .clip_audio(&clip)
            .ok_or_else(|| anyhow!("the audio for clip {} is not loaded", clip.id))?;
        let from = offset as usize;
        let samples = crate::analysis::mono(&data.channels, from, from + clip.length as usize);
        let settings =
            crate::analysis::OnsetSettings::new(sensitivity, min_gap_ms, self.sample_rate);
        Ok(crate::analysis::detect_onsets(&samples, settings)
            .into_iter()
            .map(|o| (clip.start + o.position as u64, o.strength))
            .collect())
    }

    /// Move clips' starts toward the grid (`division`, through the tempo map) by `strength`
    /// (0 to 1), shortening overlaps and, with `fill_gaps`, closing gaps from the audio
    /// before each hit, then joining neighbours with `crossfade_ms` linear crossfades (see
    /// [`Project::quantize_clips`]). Returns (clip, from, to) for each hit.
    pub fn quantize_clips(
        &mut self,
        ids: &[ClipId],
        division: crate::grid::Division,
        strength: f64,
        fill_gaps: bool,
        crossfade_ms: f64,
    ) -> Result<Vec<(ClipId, u64, u64)>> {
        if !(0.0..=100.0).contains(&crossfade_ms) {
            bail!("a crossfade of {crossfade_ms} ms is outside 0 to 100");
        }
        let crossfade = (crossfade_ms / 1000.0 * f64::from(self.sample_rate)).round() as u64;
        if let Some(bad) = ids.iter().find(|&&c| self.project.find_clip(c).is_none()) {
            bail!("no audio clip with id {bad}");
        }
        if !(0.0..=1.0).contains(&strength) {
            bail!("strength {strength} is outside 0 to 1");
        }
        let map = self.tempo_map();
        let moves = self.project.quantize_clips(
            ids,
            |start| map.snap(start, division, Rounding::Nearest),
            strength,
            fill_gaps,
            crossfade,
        );
        self.graph_dirty = true;
        Ok(moves)
    }

    // ---------------------------------------------------------------- markers

    /// Add a marker at `position` (samples; it's kept in ticks, so it stays on its bar when
    /// the tempo changes). Returns its id.
    pub fn add_marker(&mut self, position: u64, name: Option<String>) -> u32 {
        let tick = self.tempo_map().tick_at(position as f64).round().max(0.0) as u64;
        self.project.add_marker(tick, name)
    }

    /// Where a marker is now, in samples.
    pub fn marker_position(&self, id: u32) -> Option<u64> {
        let tick = self.project.marker(id)?.tick;
        Some(
            self.tempo_map()
                .sample_at_tick(tick as f64)
                .round()
                .max(0.0) as u64,
        )
    }

    /// Rename a marker and/or move it to `position` (samples).
    pub fn set_marker(
        &mut self,
        id: u32,
        name: Option<String>,
        position: Option<u64>,
    ) -> Result<()> {
        let tick = position.map(|p| self.tempo_map().tick_at(p as f64).round().max(0.0) as u64);
        self.project
            .set_marker(id, name, tick)
            .map_err(|e| anyhow!(e))
    }

    /// The id of marker number `number` (1 is the first in time).
    pub fn marker_number(&self, number: usize) -> Option<u32> {
        number
            .checked_sub(1)
            .and_then(|i| self.project.markers.get(i))
            .map(|m| m.id)
    }

    /// The first marker after `position` (`forward`) or the last one before it, strictly: from
    /// a marker, the next or previous one.
    pub fn adjacent_marker(&self, position: u64, forward: bool) -> Option<u32> {
        let at = |m: &&crate::project::Marker| self.marker_position(m.id).unwrap_or(0);
        if forward {
            self.project.markers.iter().find(|m| at(m) > position)
        } else {
            self.project.markers.iter().rev().find(|m| at(m) < position)
        }
        .map(|m| m.id)
    }

    /// Move the playhead to a marker; returns where.
    pub fn go_to_marker(&mut self, id: u32) -> Result<u64> {
        let at = self
            .marker_position(id)
            .ok_or_else(|| anyhow!("no marker with id {id}"))?;
        self.locate(at)?;
        Ok(at)
    }

    // ---------------------------------------------------------------- clips, several at once

    /// The clips with these ids (either kind) and their tracks, in the order given. Errors
    /// name the first id that isn't a clip.
    pub fn clips_by_id(&self, ids: &[ClipId]) -> Result<Vec<(TrackId, AnyClip)>> {
        ids.iter()
            .map(|&id| {
                let p = &self.project;
                p.find_clip(id)
                    .map(|(t, c)| (t, AnyClip::Audio(c.clone())))
                    .or_else(|| {
                        p.find_midi_clip(id)
                            .map(|(t, c)| (t, AnyClip::Midi(c.clone())))
                    })
                    .ok_or_else(|| anyhow!("no clip with id {id}"))
            })
            .collect()
    }

    /// Where a clip starts and ends on the timeline, in samples.
    fn clip_span(clip: &AnyClip, map: &TempoMap) -> (f64, f64) {
        match clip {
            AnyClip::Audio(c) => (c.start as f64, c.end() as f64),
            AnyClip::Midi(c) => (
                map.sample_at_tick(c.start as f64),
                map.sample_at_tick((c.start + c.length) as f64),
            ),
        }
    }

    /// Whether a clip of this kind can go on `track`: MIDI clips on instrument tracks, audio
    /// clips on tracks that hold clips (as `move_clip` allows).
    fn check_clip_track(&self, clip: &AnyClip, track: Option<&Track>) -> Result<()> {
        let id = clip.id();
        match (clip, track) {
            (_, None) => bail!("clip {id} would land below the last track"),
            (AnyClip::Midi(_), Some(t)) if t.kind != TrackKind::Instrument => {
                bail!(
                    "MIDI clip {id} would land on track {}, which isn't an instrument track",
                    t.id
                )
            }
            (AnyClip::Audio(_), Some(t)) if !t.kind.holds_clips() => {
                bail!("clip {id} would land on track {}, a folder or bus", t.id)
            }
            _ => Ok(()),
        }
    }

    /// Copy clips (either kind) to the clipboard, replacing what it held. Returns how many.
    pub fn copy_clips(&mut self, ids: &[ClipId]) -> Result<usize> {
        if ids.is_empty() {
            bail!("no clips given");
        }
        self.clipboard = self.clips_by_id(ids)?;
        Ok(self.clipboard.len())
    }

    /// Remove clips (either kind) from the timeline; their files stay on disk. All or none.
    pub fn delete_clips(&mut self, ids: &[ClipId]) -> Result<()> {
        self.clips_by_id(ids)?;
        for &id in ids {
            if self.project.remove_clip(id).is_none() {
                self.project.remove_midi_clip(id);
            }
        }
        self.graph_dirty = true;
        Ok(())
    }

    // ------------------------------------------------------------ the Waveform view's files

    /// Open an audio file for the Waveform view (any format `media::read_audio` reads), at its
    /// own sample rate; a file already open gives its existing id. Documents aren't part of
    /// the project: they aren't saved with it or undone.
    pub fn open_document(&mut self, path: &Path) -> Result<DocumentId> {
        // A file from outside the project opens as the project's copy (`Imported/`), so
        // saving never writes over the original.
        let read =
            media::read_audio(path).with_context(|| format!("opening {}", path.display()))?;
        let path = &self.import_copy(path)?;
        if let Some(doc) = self.documents.iter().find(|d| d.path == *path) {
            return Ok(doc.id);
        }
        let id = self.next_document;
        self.next_document += 1;
        let audio = Arc::new(AudioData::new(read.sample_rate, read.channels));
        let metadata = Arc::new(crate::metadata::read(path));
        self.documents.push(AudioDocument {
            id,
            path: path.to_path_buf(),
            audio: audio.clone(),
            metadata: metadata.clone(),
            playback: None,
            format: wav::read_format(path),
            history: vec![Step {
                name: "Open".into(),
                audio,
                metadata,
            }],
            cursor: 0,
            saved: Some(0),
            problems: None,
            next_problem: 1,
            source: None,
        });
        Ok(id)
    }

    /// Open an audio clip's playing take in the Waveform view, remembering the clip and its
    /// track (the rack can then sync with the track, [`Self::set_rack_sync`]). A file already
    /// open gives its existing document, which now remembers this clip.
    pub fn open_clip_document(&mut self, clip: ClipId) -> Result<DocumentId> {
        let (track, file) = match self.project.find_clip(clip) {
            Some((track, c)) => (track, self.resolve(c.file())),
            None if self.project.find_midi_clip(clip).is_some() => {
                bail!("clip {clip} is a MIDI clip, not audio")
            }
            None => bail!("no clip with id {clip}"),
        };
        let id = self.open_document(&file)?;
        self.document_mut(id)?.source = Some((track, clip));
        Ok(id)
    }

    /// The open documents, in the order they were opened.
    pub fn documents(&self) -> &[AudioDocument] {
        &self.documents
    }

    pub fn document(&self, id: DocumentId) -> Option<&AudioDocument> {
        self.documents.iter().find(|d| d.id == id)
    }

    /// Close a document, stopping its preview first so the audio thread lets go of it. One
    /// with unsaved edits refuses unless `discard`.
    pub fn close_document(&mut self, id: DocumentId, discard: bool) -> Result<()> {
        let index = self
            .documents
            .iter()
            .position(|d| d.id == id)
            .ok_or_else(|| anyhow!("no open audio file with id {id}"))?;
        if self.documents[index].is_dirty() && !discard {
            bail!(
                "{} has unsaved edits: save it, or close with discard",
                self.documents[index].path.display()
            );
        }
        if self.previewing == Some(id) {
            self.stop_preview();
            self.flush_pending();
        }
        self.documents.remove(index);
        // A rack synced through it goes back to the user's own.
        if self.rack_sync.as_ref().is_some_and(|s| s.document == id) {
            self.sync_rack();
        }
        Ok(())
    }

    /// The Waveform view's effects rack.
    pub fn editor_rack(&self) -> &[InsertSlot] {
        &self.editor_rack
    }

    /// The rack as a chain to run, with flags of its own; `None` when it is empty. With
    /// `snapshot` (an Apply), each slot keeps the bypass it has now.
    fn rack_chain(&self, snapshot: bool) -> Option<rack::RackChain> {
        if self.editor_rack.is_empty() {
            return None;
        }
        let flags = InsertFlags::default();
        let inserts = self.processors_with(&self.editor_rack, &flags, snapshot);
        Some(rack::RackChain::new(inserts, flags, MAX_BLOCK))
    }

    /// The block the rack runs in: the device's, as export uses.
    fn rack_block(&self) -> usize {
        match self.stats().block_size {
            0 => 512,
            n => n,
        }
    }

    /// The silence the rack settles on before it plays or renders: export's pre-roll.
    fn rack_preroll(&self) -> usize {
        (f64::from(self.sample_rate) * PREROLL_SECONDS) as usize
    }

    /// Render a document's `start`..`end` (the file's own samples; the whole file without a
    /// range) through the effects rack on a thread of its own, and put the result in the file
    /// as one step of its history when it is done ([`Self::update`] collects it). The same
    /// chain, settle, blocks and transport as the preview, so it writes what was heard. Stops
    /// the preview first: both use the rack's plug-ins.
    pub fn apply_effects(&mut self, id: DocumentId, range: Option<(u64, u64)>) -> Result<()> {
        if self.applying.is_some() {
            bail!("an Apply is already running");
        }
        if self.loading.is_some() {
            bail!("plug-ins are still loading; Apply once they are in");
        }
        let names: Vec<String> = self
            .editor_rack
            .iter()
            .filter(|s| !s.bypass && self.plugins.contains_key(&s.id))
            .map(|s| s.name.clone())
            .collect();
        if names.is_empty() {
            bail!("the effects rack has no plug-ins switched on");
        }
        // Apply writes what the preview plays: an impulse being prepared is waited for.
        self.wait_for_impulses(IMPULSE_WAIT)?;
        let rate = self.sample_rate;
        let block = self.rack_block();
        let preroll = self.rack_preroll();
        self.release_preview();
        self.flush_pending();
        // Its slots' bypass as it is now: switching one while it renders changes only the
        // preview. The chain holds its plug-ins until it is handed back (`collect_apply`).
        let mut chain = self
            .rack_chain(true)
            .ok_or_else(|| anyhow!("the effects rack is empty"))?;
        let doc = self.document_mut(id)?;
        if doc.audio.channels.len() > 2 {
            bail!(
                "the effects rack takes mono or stereo files, not {} channels",
                doc.audio.channels.len()
            );
        }
        let len = doc.audio.len() as u64;
        let (start, end) = range.unwrap_or((0, len));
        if end <= start || end > len {
            bail!("{start}..{end} isn't a range in the file ({len} samples)");
        }
        let source = doc.audio.clone();
        let playback = doc.playback_at(rate);
        let ratio = f64::from(rate) / f64::from(source.sample_rate);
        let at = |n: u64| (n as f64 * ratio).round() as u64;
        let (from, to) = (at(start), at(end));
        let progress = Arc::new(AtomicF32::new(0.0));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = std::sync::mpsc::channel();
        let (p, c, hold) = (progress.clone(), cancel.clone(), self.apply_hold.clone());
        std::thread::Builder::new()
            .name("apply-effects".into())
            .spawn(move || {
                while hold.load(Ordering::Relaxed) && !c.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(1));
                }
                chain.set_offline(true);
                chain.settle(preroll, block);
                let out = chain.render(
                    &playback,
                    from,
                    to,
                    block,
                    |f| p.store(f),
                    || c.load(Ordering::Relaxed),
                );
                let misses = chain.flags().misses.load(Ordering::Relaxed);
                // The chain goes back to be dropped on the main thread: it may hold the last
                // reference to a plug-in removed from the rack meanwhile.
                let _ = tx.send((out, misses, chain));
            })?;
        self.applying = Some(ApplyJob {
            document: id,
            range: (start, end),
            source,
            name: format!("Apply {}", names.join(", ")),
            progress,
            cancel,
            done: rx,
            turn_off: None,
        });
        Ok(())
    }

    /// Have a built-in effect learn from a document's `start`..`end` (the file's own samples)
    /// what it learns from audio: a noise print (Noise Reduction, Hiss Reduction, as
    /// its noise print) or the clip levels (Clip Repair's scan). Into
    /// `insert`, any such effect, anywhere; without one, into the Waveform rack's first such
    /// effect, adding a Noise Reduction to the rack when it has none. Measured on the audio the
    /// rack plays (the engine's rate), so a print's bins are the effect's. Returns the insert.
    /// The effect uses it from its next block, even playing.
    pub fn capture_noise_print(
        &mut self,
        id: DocumentId,
        range: (u64, u64),
        insert: Option<InsertId>,
    ) -> Result<InsertId> {
        self.learn_from(id, range, insert, None)
    }

    /// [`Session::capture_noise_print`], naming the scan for an effect whose scans set its
    /// parameters (the Click & Pop Repair's "Scan For Threshold Levels" or "Scan For All
    /// Levels", or a unique part of the name); without one, its first. Without `insert`, the
    /// Waveform rack's first effect that has that scan.
    pub fn learn_from(
        &mut self,
        id: DocumentId,
        range: (u64, u64),
        insert: Option<InsertId>,
        scan: Option<&str>,
    ) -> Result<InsertId> {
        let rate = self.sample_rate;
        let doc = self.document_mut(id)?;
        let len = doc.audio.len() as u64;
        let (start, end) = range;
        if end <= start || end > len {
            bail!("{start}..{end} isn't a range in the file ({len} samples)");
        }
        let playback = doc.playback_at(rate);
        let ratio = f64::from(rate) / f64::from(doc.audio.sample_rate);
        let at = |n: u64| ((n as f64 * ratio).round() as usize).min(playback.len());
        let audio: Vec<Vec<f32>> = playback
            .channels
            .iter()
            .map(|c| c[at(start)..at(end)].to_vec())
            .collect();
        let learns = |live: &LivePlugin| match &live.instance {
            Instance::Native(n) => {
                n.kind.capture.is_some() && scan.is_none_or(|name| n.scan_named(name).is_ok())
            }
            Instance::Vst3 { .. } => false,
        };
        let insert = match insert {
            Some(insert) => {
                let live = self
                    .plugins
                    .get(&insert)
                    .ok_or_else(|| anyhow!("no loaded insert with id {insert}"))?;
                if let (Some(name), Instance::Native(n)) = (scan, &live.instance) {
                    n.scan_named(name)?;
                }
                if !learns(live) {
                    bail!("insert {insert} doesn't learn from audio (no noise print or scan)");
                }
                insert
            }
            None => {
                let found = self
                    .editor_rack
                    .iter()
                    .map(|s| s.id)
                    .find(|id| self.plugins.get(id).is_some_and(learns));
                match (found, scan) {
                    (Some(insert), _) => insert,
                    (None, None) => self.add_insert(
                        InsertTarget::Editor,
                        &crate::native::path_of(&crate::native::noise_reduction::KIND),
                    )?,
                    (None, Some(name)) => bail!(
                        "no effect in the Waveform view's effects rack has a scan '{name}'                          (add a Click & Pop Repair, or name its insert)"
                    ),
                }
            }
        };
        let Some(Instance::Native(native)) = self.plugins.get_mut(&insert).map(|p| &mut p.instance)
        else {
            bail!("insert {insert} isn't loaded");
        };
        match scan {
            Some(name) => {
                let k = native.scan_named(name)?;
                native.scan(f64::from(rate), &audio, k)?;
            }
            None => native.capture(f64::from(rate), &audio)?,
        }
        // A print is no parameter: a synced copy takes the whole state.
        self.mirror_state(insert);
        Ok(insert)
    }

    /// The Apply running: its document and how far it is (0 to 1).
    pub fn apply_state(&self) -> Option<(DocumentId, f32)> {
        self.applying
            .as_ref()
            .map(|j| (j.document, j.progress.load()))
    }

    /// Hold an Apply back before it renders (from the next one started) while set: tests and
    /// checks change the rack while one is under way.
    pub fn hold_apply(&self, hold: bool) {
        self.apply_hold.store(hold, Ordering::Relaxed);
    }

    pub fn cancel_apply(&mut self) {
        if let Some(job) = &self.applying {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Wait for an Apply to land (for checks and tests; the app collects it in `update`).
    pub fn wait_for_apply(&mut self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while self.applying.is_some() {
            if Instant::now() > deadline {
                bail!("Apply didn't finish in {timeout:?}");
            }
            self.collect_apply();
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(())
    }

    /// An Apply that finished: its audio, back at the file's rate and length, into the file as a
    /// step of its history. Refused if the file changed meanwhile or a plug-in skipped a block.
    fn collect_apply(&mut self) {
        let Some(job) = &self.applying else {
            return;
        };
        let Ok((out, misses, chain)) = job.done.try_recv() else {
            return;
        };
        let job = self.applying.take().expect("checked");
        // Here, on the main thread: then any plug-in removed while it rendered.
        drop(chain);
        self.retired.clear();
        let Some(out) = out else {
            self.notices.push("Apply cancelled".into());
            return;
        };
        if misses > 0 {
            self.notices
                .push(format!("Apply failed: plug-ins skipped {misses} blocks"));
            return;
        }
        let Some(doc) = self.documents.iter().find(|d| d.id == job.document) else {
            return;
        };
        if !Arc::ptr_eq(&doc.audio, &job.source) {
            self.notices
                .push("Apply discarded: the file changed while it was rendering".into());
            return;
        }
        let file_rate = doc.audio.sample_rate;
        let length = (job.range.1 - job.range.0) as usize;
        // A mono file takes the average of the rack's two channels (equal for an effect that
        // treats them alike); a stereo file its two.
        let rendered: Vec<Vec<f32>> = if doc.audio.channels.len() == 1 {
            vec![out[0]
                .iter()
                .zip(&out[1])
                .map(|(l, r)| (l + r) * 0.5)
                .collect()]
        } else {
            out
        };
        let channels = rendered
            .into_iter()
            .map(|c| {
                let mut c = if file_rate == self.sample_rate {
                    c
                } else {
                    media::resample(&c, self.sample_rate, file_rate)
                };
                c.resize(length, 0.0);
                c
            })
            .collect();
        let edit = audio_edit::Edit::Replace {
            range: job.range,
            channels,
            name: job.name,
        };
        if let Err(e) = self.edit_document(job.document, &edit) {
            self.notices.push(format!("Apply: {e:#}"));
            return;
        }
        if let Some((track, inserts)) = &job.turn_off {
            self.turn_off_applied(*track, inserts);
        }
    }

    /// Play a document's `start`..`end` through the preview, as a device would, and return
    /// what reached the master's outputs with the rack's latency dropped: the preview as heard,
    /// for checks that compare it with Apply. Only without a device (it runs the engine here).
    pub fn capture_preview(
        &mut self,
        id: DocumentId,
        start: u64,
        end: u64,
    ) -> Result<Vec<Vec<f32>>> {
        if self.engine_running() {
            bail!("capturing the preview needs the device stopped");
        }
        let latency = self.rack_chain(false).map_or(0, |c| c.latency());
        let block = self.rack_block();
        self.preview_document(id, start, Some(end))?;
        let channel = usize::from(self.project.master.output_channel);
        let mut core = self
            .core
            .lock()
            .map_err(|_| anyhow!("engine lock poisoned"))?;
        let outputs = core.output_channels().max(channel + 2);
        let mut heard: Vec<Vec<f32>> = vec![Vec::new(); 2];
        let mut out = vec![0.0f32; block * outputs];
        loop {
            out.fill(0.0);
            core.process(&mut out, outputs, None, 0);
            for frame in out.chunks(outputs) {
                heard[0].push(frame[channel]);
                heard[1].push(frame[channel + 1]);
            }
            if !self.engine.shared.preview_playing.load(Ordering::Relaxed) {
                break;
            }
        }
        drop(core);
        self.previewing = None;
        self.collect_garbage();
        let doc = self
            .document(id)
            .ok_or_else(|| anyhow!("no open audio file with id {id}"))?;
        let ratio = f64::from(self.sample_rate) / f64::from(doc.audio.sample_rate);
        let length = ((end - start) as f64 * ratio).round() as usize;
        for c in &mut heard {
            c.drain(..latency.min(c.len()));
            c.truncate(length);
        }
        Ok(heard)
    }

    /// Play the project's `start`..`end` as a device would, in `block`-sample callbacks, and
    /// return what reached the master's outputs lined up with the timeline (the graph's
    /// latency dropped): playback as heard, for checks that compare it with an export. Only
    /// without a device (it runs the engine here).
    pub fn capture_playback(
        &mut self,
        start: u64,
        end: u64,
        block: usize,
    ) -> Result<Vec<Vec<f32>>> {
        if self.engine_running() {
            bail!("capturing playback needs the device stopped");
        }
        if end <= start || block == 0 {
            bail!("nothing to capture: {start}..{end} in blocks of {block}");
        }
        if self.is_playing() {
            self.stop();
        }
        self.update();
        self.locate(start)?;
        // Stopped for export's pre-roll first, as a device runs between takes: what played
        // before has died away when this starts, as it has in an export.
        let preroll = self.rack_preroll();
        let channel = usize::from(self.project.master.output_channel);
        {
            let mut core = self
                .core
                .lock()
                .map_err(|_| anyhow!("engine lock poisoned"))?;
            let outputs = core.output_channels().max(channel + 2);
            let mut out = vec![0.0f32; block * outputs];
            let mut done = 0;
            while done < preroll {
                core.process(&mut out, outputs, None, 0);
                done += block;
            }
        }
        self.play();
        let latency = self.stats().latency;
        let length = (end - start) as usize;
        let mut core = self
            .core
            .lock()
            .map_err(|_| anyhow!("engine lock poisoned"))?;
        let outputs = core.output_channels().max(channel + 2);
        let mut heard: Vec<Vec<f32>> = vec![Vec::new(); 2];
        let mut out = vec![0.0f32; block * outputs];
        while heard[0].len() < length + latency {
            out.fill(0.0);
            core.process(&mut out, outputs, None, 0);
            for frame in out.chunks(outputs) {
                heard[0].push(frame[channel]);
                heard[1].push(frame[channel + 1]);
            }
        }
        drop(core);
        self.stop();
        self.flush_pending();
        for c in &mut heard {
            c.drain(..latency);
            c.truncate(length);
        }
        Ok(heard)
    }

    fn document_mut(&mut self, id: DocumentId) -> Result<&mut AudioDocument> {
        self.documents
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| anyhow!("no open audio file with id {id}"))
    }

    /// Apply an edit to a document, as a step in its own history (not the project's). Stops
    /// its playback, which would still be playing the old audio.
    pub fn edit_document(&mut self, id: DocumentId, edit: &audio_edit::Edit) -> Result<()> {
        let doc = self.document_mut(id)?;
        let channels = audio_edit::apply(&doc.audio.channels, doc.audio.sample_rate, edit)?;
        let rate = doc.audio.sample_rate;
        doc.push(edit.name(), AudioData::new(rate, channels));
        if self.previewing == Some(id) {
            self.stop_preview();
        }
        Ok(())
    }

    /// Step a document's history back (`undo`) or on. Returns the step's name, `None` at an end.
    pub fn step_document(&mut self, id: DocumentId, undo: bool) -> Result<Option<String>> {
        let doc = self.document_mut(id)?;
        let to = if undo {
            doc.cursor.checked_sub(1)
        } else {
            Some(doc.cursor + 1).filter(|&c| c < doc.history.len())
        };
        let Some(to) = to else {
            return Ok(None);
        };
        // Undo names the step it takes back; redo the one it brings.
        let name = doc.history[if undo { doc.cursor } else { to }].name.clone();
        doc.show_step(to);
        if self.previewing == Some(id) {
            self.stop_preview();
        }
        Ok(Some(name))
    }

    /// Scan a document's `start`..`end` (the file's own samples; the whole file without a
    /// range) for `kind`'s problems: clicks by the
    /// Click Remover's detector, or clipped runs by Clip Repair's scan (its clip
    /// levels learned from the range), with `settings`. The list is kept on the file, replacing
    /// the last, until its audio changes (an edit, undo or redo; a repair keeps the rest of it).
    /// Returns it, in order of time.
    pub fn find_audio_problems(
        &mut self,
        id: DocumentId,
        kind: diagnostics::ProblemKind,
        settings: &diagnostics::Settings,
        range: Option<(u64, u64)>,
    ) -> Result<Vec<diagnostics::Problem>> {
        let doc = self.document_mut(id)?;
        let len = doc.audio.len() as u64;
        let (start, end) = range.unwrap_or((0, len));
        if end <= start || end > len {
            bail!("{start}..{end} isn't a range in the file ({len} samples)");
        }
        let found = diagnostics::find(
            &doc.audio.channels,
            kind,
            settings,
            (start as usize, end as usize),
        )?;
        let found = found
            .into_iter()
            .map(|f| {
                let n = doc.next_problem;
                doc.next_problem += 1;
                (n, f)
            })
            .collect();
        doc.problems = Some(ProblemList {
            kind,
            source: doc.audio.clone(),
            found,
        });
        Ok(self.audio_problems(id).map(|(_, l)| l).unwrap_or_default())
    }

    /// What the last scan of a document found that is still to repair, and its kind: `None`
    /// without a scan, or once the file's audio has changed since.
    pub fn audio_problems(
        &self,
        id: DocumentId,
    ) -> Option<(diagnostics::ProblemKind, Vec<diagnostics::Problem>)> {
        let doc = self.document(id)?;
        let list = doc
            .problems
            .as_ref()
            .filter(|l| Arc::ptr_eq(&l.source, &doc.audio))?;
        let problems = list
            .found
            .iter()
            .map(|(n, f)| diagnostics::Problem {
                id: *n,
                kind: list.kind,
                channel: f.channel,
                start: f.start as u64,
                length: f.repaired.len() as u64,
                level_db: f.level_db,
            })
            .collect();
        Some((list.kind, problems))
    }

    /// Repair problems the last scan found in a document: those with `ids`, or all of them
    /// (`None`), with the samples the scan rebuilt them with, as one step of the file's
    /// history ("Repair 12 clicks"). The repaired ones leave the list; the rest keep their ids.
    /// Returns how many were repaired. Stops the file's playback.
    pub fn repair_audio_problems(&mut self, id: DocumentId, ids: Option<&[u32]>) -> Result<usize> {
        let doc = self.document_mut(id)?;
        let Some(list) = doc
            .problems
            .as_mut()
            .filter(|l| Arc::ptr_eq(&l.source, &doc.audio))
        else {
            bail!("nothing found in the file as it is now: scan it first (find_audio_problems)");
        };
        // Sets, so a list of every clipped half-cycle of a song is repaired in one pass.
        let listed: std::collections::BTreeSet<u32> = list.found.iter().map(|(n, _)| *n).collect();
        let chosen: std::collections::BTreeSet<u32> = match ids {
            None => listed,
            Some(ids) => {
                if let Some(n) = ids.iter().find(|n| !listed.contains(n)) {
                    bail!(
                        "no problem with id {n} in the file's list (repaired already, or from an \
                         earlier scan: see get_audio_problems)"
                    );
                }
                ids.iter().copied().collect()
            }
        };
        if chosen.is_empty() {
            bail!("nothing to repair: the list is empty");
        }
        let mut channels = doc.audio.channels.clone();
        let fixes: Vec<&diagnostics::Found> = list
            .found
            .iter()
            .filter(|(n, _)| chosen.contains(n))
            .map(|(_, f)| f)
            .collect();
        diagnostics::write(&mut channels, &fixes);
        let name = format!("Repair {}", list.kind.noun(chosen.len()));
        list.found.retain(|(n, _)| !chosen.contains(n));
        let rate = doc.audio.sample_rate;
        doc.push(name, AudioData::new(rate, channels));
        // The rest of the list stays with the file as repaired.
        let audio = doc.audio.clone();
        if let Some(list) = doc.problems.as_mut() {
            list.source = audio;
        }
        if self.previewing == Some(id) {
            self.stop_preview();
        }
        Ok(chosen.len())
    }

    /// Copy part of a document (the file's own samples) for pasting into any document.
    pub fn copy_document(&mut self, id: DocumentId, range: (u64, u64)) -> Result<u64> {
        let doc = self
            .document(id)
            .ok_or_else(|| anyhow!("no open audio file with id {id}"))?;
        let len = doc.audio.len() as u64;
        if range.1 <= range.0 || range.1 > len {
            bail!(
                "{}..{} isn't a range in the file ({len} samples)",
                range.0,
                range.1
            );
        }
        let (a, b) = (range.0 as usize, range.1 as usize);
        self.audio_clipboard = Some(audio_edit::Clipboard {
            sample_rate: doc.audio.sample_rate,
            channels: doc
                .audio
                .channels
                .iter()
                .map(|c| c[a..b].to_vec())
                .collect(),
        });
        Ok(range.1 - range.0)
    }

    /// What `copy_document` copied last.
    pub fn audio_clipboard(&self) -> Option<&audio_edit::Clipboard> {
        self.audio_clipboard.as_ref()
    }

    /// The clips with a take (any take) on `path`: the ones a save over it changes.
    pub fn clips_using(&self, path: &Path) -> Vec<ClipId> {
        self.project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| {
                c.takes
                    .iter()
                    .any(|take| same_file(&self.resolve(&take.file), path))
            })
            .map(|c| c.id)
            .collect()
    }

    /// Write a document over its file, in the file's own WAV format (written beside it, then
    /// renamed over it). Clips using the file reload it, so the project plays the new audio;
    /// the file's history is kept, so undo and save again puts it back. Only WAV files: others
    /// need Save As. Project undo never changes files.
    pub fn save_document(&mut self, id: DocumentId) -> Result<PathBuf> {
        let doc = self.document_mut(id)?;
        let Some(format) = doc.format else {
            bail!(
                "{} isn't a WAV file, so it can't be written back: use Save As (WAV)",
                doc.path.display()
            );
        };
        let path = doc.path.clone();
        write_document(doc, &path, format)?;
        doc.saved = Some(doc.cursor);
        self.reload_file(&path);
        Ok(path)
    }

    /// Write a document to `path` (absolute, `.wav`; replaced if it exists) in its own WAV
    /// format, or 32-bit float for a FLAC, MP3 or AIFF, and carry on editing that file.
    pub fn save_document_as(&mut self, id: DocumentId, path: &Path) -> Result<PathBuf> {
        if !path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("wav"))
        {
            bail!("{} must be a .wav file", path.display());
        }
        if self
            .documents
            .iter()
            .any(|d| d.id != id && same_file(&d.path, path))
        {
            bail!("{} is open as another file; close it first", path.display());
        }
        let doc = self.document_mut(id)?;
        let format = doc.format.unwrap_or(wav::Format::FLOAT);
        write_document(doc, path, format)?;
        doc.path = path.to_path_buf();
        doc.format = Some(format);
        doc.saved = Some(doc.cursor);
        self.reload_file(path);
        Ok(path.to_path_buf())
    }

    /// A file on disk changed: forget what was loaded from it (the audio, stretched renders)
    /// and load it again, so every clip on it plays the new audio after the graph rebuilds.
    fn reload_file(&mut self, path: &Path) {
        self.media.forget(|p| same_file(p, path));
        self.stretches.retain(|key| !same_file(&key.path, path));
        self.load_media();
        self.graph_dirty = true;
    }

    /// Play a document from `start` to `end` (the file's own samples; to its end without one)
    /// on the master's outputs, stopping the project first. Live only: exports never hear it.
    pub fn preview_document(&mut self, id: DocumentId, start: u64, end: Option<u64>) -> Result<()> {
        let rate = self.sample_rate;
        let doc = self
            .documents
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| anyhow!("no open audio file with id {id}"))?;
        let len = doc.audio.len() as u64;
        let end = end.unwrap_or(len).min(len);
        if start >= end {
            bail!("nothing to play: {start}..{end} (the file has {len} samples)");
        }
        let playback = doc.playback_at(rate);
        let ratio = f64::from(rate) / f64::from(doc.audio.sample_rate);
        let at = |n: u64| (n as f64 * ratio).round() as u64;
        let channel = usize::from(self.project.master.output_channel);
        if self.applying.is_some() {
            bail!("the effects rack is busy applying; wait for it to finish");
        }
        if self.is_playing() {
            self.stop();
        }
        // A preview already playing runs the same plug-ins: off the audio thread first, or it
        // would move them on between the settle and the swap.
        self.release_preview();
        // Through the rack, settled first exactly as Apply settles it, and fed on past the end
        // by its latency so the delayed output covers the range.
        let rack = self.rack_chain(false).map(|mut chain| {
            chain.set_offline(true);
            chain.settle(self.rack_preroll(), self.rack_block());
            chain.set_offline(false);
            Box::new(chain)
        });
        let latency = rack.as_ref().map_or(0, |r| r.latency() as u64);
        // Playing from now, not from when the audio thread takes the command: `update` must
        // not read the gap as the preview having ended.
        let shared = &self.engine.shared;
        shared.preview_position.store(at(start), Ordering::Relaxed);
        shared.preview_playing.store(true, Ordering::Relaxed);
        self.send(Command::Preview(Some(Box::new(Preview {
            audio: playback,
            position: at(start),
            end: at(end) + latency,
            channel,
            rack,
        }))));
        self.previewing = Some(id);
        Ok(())
    }

    /// Stop the Waveform view's playback and wait until the audio thread has let go of it:
    /// the preview's rack shares its plug-ins with Apply and the next preview, which settle
    /// them, so not one more block may run through the old chain. With a device running this
    /// takes the engine lock (one silent block), only when a preview was playing.
    fn release_preview(&mut self) {
        if self.previewing.is_none() {
            return;
        }
        self.stop_preview();
        if let Ok(mut core) = self.core.lock() {
            core.handle_commands();
        }
        self.collect_garbage();
    }

    /// Stop the Waveform view's playback.
    pub fn stop_preview(&mut self) {
        if self.previewing.take().is_some() {
            self.engine
                .shared
                .preview_playing
                .store(false, Ordering::Relaxed);
            self.send(Command::Preview(None));
        }
    }

    /// The document playing and where in it (the file's own samples), while one plays.
    pub fn preview_state(&self) -> Option<(DocumentId, u64)> {
        let id = self.previewing?;
        if !self.engine.shared.preview_playing.load(Ordering::Relaxed) {
            return None;
        }
        let doc = self.document(id)?;
        let at = self.engine.shared.preview_position.load(Ordering::Relaxed);
        let ratio = f64::from(doc.audio.sample_rate) / f64::from(self.sample_rate);
        Some((id, (at as f64 * ratio).round() as u64))
    }

    /// Insert a MIDI track: an instrument track right after `after` (and its sub-tracks, in
    /// the same folder; at the end without one), holding an empty MIDI clip from sample `start`
    /// to `end` (two bars from `start`'s bar when `end` is `None`), with `instrument` loaded
    /// on it if given (the track takes its name). What a time selection's "Insert MIDI track"
    /// does. If the instrument fails to load, nothing is added.
    pub fn insert_midi_track(
        &mut self,
        start: u64,
        end: Option<u64>,
        after: Option<TrackId>,
        instrument: Option<&Path>,
    ) -> Result<(TrackId, ClipId, Option<InsertId>)> {
        if let Some(end) = end.filter(|&end| end <= start) {
            bail!("the range must end after it starts ({start}..{end})")
        }
        let map = self.tempo_map();
        let from = map.tick_at(start as f64).round() as u64;
        let to = match end {
            Some(end) => map.tick_at(end as f64).round() as u64,
            None => {
                let (bar, _) = map.bar_at(start as f64);
                from + map
                    .tick_of_bar(bar + 2, 0)
                    .saturating_sub(map.tick_of_bar(bar, 0))
            }
        };
        self.insert_midi_track_ticks(from, to, after, instrument)
    }

    /// [`Self::insert_midi_track`] with the clip from song tick `from` to `to` (an agent
    /// asking for bars: `insert_midi_track`'s `bar` and `bars`).
    pub fn insert_midi_track_ticks(
        &mut self,
        from: u64,
        to: u64,
        after: Option<TrackId>,
        instrument: Option<&Path>,
    ) -> Result<(TrackId, ClipId, Option<InsertId>)> {
        if let Some(after) = after {
            if self.project.track(after).is_none() {
                bail!("no track with id {after}");
            }
        }
        let track = self
            .project
            .insert_track_after(TrackKind::Instrument, after);
        if let Some(t) = self.project.track_mut(track) {
            // Armed, as a new instrument track is, so the keyboard plays it at once.
            t.arm = true;
        }
        let clip = match self.project.add_midi_clip(track, from, (to - from).max(1)) {
            Ok(clip) => clip,
            Err(e) => {
                self.remove_track(track);
                bail!(e);
            }
        };
        let insert = match instrument {
            Some(path) => match self.add_insert_at(InsertTarget::Track(track), path, Some(0)) {
                Ok(id) => {
                    if let Some(t) = self.project.track_mut(track) {
                        t.name = plugins::display_name(path);
                    }
                    Some(id)
                }
                Err(e) => {
                    self.remove_track(track);
                    return Err(e.context(format!("loading {}", plugins::display_name(path))));
                }
            },
            None => None,
        };
        self.graph_dirty = true;
        Ok((track, clip, insert))
    }

    /// Whether a loaded insert is an instrument (it says so, or takes MIDI and no audio).
    pub fn is_instrument(&self, insert: InsertId) -> bool {
        self.plugins.get(&insert).is_some_and(|p| p.is_instrument)
    }

    /// The tracks a range edit applies to: `tracks`, or every track that holds clips when it
    /// is empty. Errors name a track that doesn't exist.
    fn range_tracks(&self, tracks: &[TrackId]) -> Result<Vec<TrackId>> {
        if tracks.is_empty() {
            return Ok(self
                .project
                .tracks
                .iter()
                .filter(|t| t.kind.holds_clips())
                .map(|t| t.id)
                .collect());
        }
        for &id in tracks {
            if self.project.track(id).is_none() {
                bail!("no track with id {id}");
            }
        }
        Ok(tracks.to_vec())
    }

    /// The song tick nearest sample `at`, through the tempo map: where a cut at a sample falls
    /// in a MIDI clip (a tick is about half a millisecond at 120 BPM, so the cut is within a
    /// quarter of that of the sample).
    pub fn tick_at_sample(&self, at: u64) -> u64 {
        self.tempo_map().tick_at(at as f64).round().max(0.0) as u64
    }

    /// Split a clip of either kind at song position `at` (samples): the clip keeps the part
    /// before and the returned clip starts there. A MIDI clip is cut at the tick nearest `at`
    /// ([`Project::split_midi_clip`]). `Ok(None)` when `at` (or its tick) isn't strictly
    /// inside the clip; an error when there is no such clip. Marks the graph for a rebuild.
    pub fn split_clip(&mut self, id: ClipId, at: u64) -> Result<Option<ClipId>> {
        let second = if self.project.find_clip(id).is_some() {
            self.project.split_clip(id, at)
        } else if self.project.find_midi_clip(id).is_some() {
            let tick = self.tick_at_sample(at);
            self.project.split_midi_clip(id, tick)
        } else {
            bail!("no clip with id {id}");
        };
        if second.is_some() {
            self.graph_dirty = true;
        }
        Ok(second)
    }

    /// Cut the clips on `tracks` (all clip tracks when empty) at both edges of the time range
    /// `start..end`, wherever an edge falls strictly inside one: what a time selection's Split
    /// does. MIDI clips are cut at the ticks nearest the edges ([`Self::tick_at_sample`]).
    /// Returns the new pieces.
    pub fn split_range(&mut self, start: u64, end: u64, tracks: &[TrackId]) -> Result<Vec<ClipId>> {
        if end <= start {
            bail!("the range must end after it starts ({start}..{end})");
        }
        let tracks = self.range_tracks(tracks)?;
        let (first_tick, last_tick) = (self.tick_at_sample(start), self.tick_at_sample(end));
        let mut pieces = Vec::new();
        for track in tracks {
            let (audio, midi): (Vec<ClipId>, Vec<ClipId>) = self
                .project
                .track(track)
                .map(|t| {
                    (
                        t.clips.iter().map(|c| c.id).collect(),
                        t.midi_clips.iter().map(|c| c.id).collect(),
                    )
                })
                .unwrap_or_default();
            // The end first, so the clip that keeps the start still holds it.
            for id in audio {
                pieces.extend(self.project.split_clip(id, end));
                pieces.extend(self.project.split_clip(id, start));
            }
            for id in midi {
                pieces.extend(self.project.split_midi_clip(id, last_tick));
                pieces.extend(self.project.split_midi_clip(id, first_tick));
            }
        }
        if !pieces.is_empty() {
            self.graph_dirty = true;
        }
        Ok(pieces)
    }

    /// Remove what lies in the time range `start..end` on `tracks` (all clip tracks when
    /// empty): clips are cut at the edges ([`Self::split_range`]) and the pieces inside go,
    /// the pieces outside staying where they are. MIDI pieces count as inside by ticks, the
    /// ticks the cuts were made at. Returns how many clips (or pieces) were removed.
    pub fn delete_range(&mut self, start: u64, end: u64, tracks: &[TrackId]) -> Result<usize> {
        if end <= start {
            bail!("the range must end after it starts ({start}..{end})");
        }
        let tracks = self.range_tracks(tracks)?;
        self.split_range(start, end, &tracks)?;
        let (first_tick, last_tick) = (self.tick_at_sample(start), self.tick_at_sample(end));
        let mut inside = Vec::new();
        for &track in &tracks {
            if let Some(t) = self.project.track(track) {
                inside.extend(
                    t.clips
                        .iter()
                        .filter(|c| c.start >= start && c.end() <= end)
                        .map(|c| c.id),
                );
                inside.extend(
                    t.midi_clips
                        .iter()
                        .filter(|c| c.start >= first_tick && c.end() <= last_tick)
                        .map(|c| c.id),
                );
            }
        }
        let removed = inside.len();
        if removed > 0 {
            self.delete_clips(&inside)?;
        }
        Ok(removed)
    }

    /// Copy clips to the clipboard and remove them.
    pub fn cut_clips(&mut self, ids: &[ClipId]) -> Result<usize> {
        let count = self.copy_clips(ids)?;
        self.delete_clips(ids)?;
        Ok(count)
    }

    /// How many clips the clipboard holds.
    pub fn clipboard_len(&self) -> usize {
        self.clipboard.len()
    }

    /// Paste the clipboard so its earliest clip starts at `position` (samples), the others
    /// keeping their distances. On the tracks they came from, or moved together so the topmost
    /// lands on `track`. Every clip gets new ids (MIDI notes and events too); fades, stretch,
    /// pitch and tempo following come along. Returns the new clips' ids.
    pub fn paste_clips(&mut self, position: u64, track: Option<TrackId>) -> Result<Vec<ClipId>> {
        if self.clipboard.is_empty() {
            bail!("the clipboard is empty");
        }
        self.place_clips(self.clipboard.clone(), position, track)
    }

    /// Copies of clips right after the last of them ends, on the same tracks (the clipboard
    /// is left alone). Returns the copies' ids.
    pub fn duplicate_clips(&mut self, ids: &[ClipId]) -> Result<Vec<ClipId>> {
        let items = self.clips_by_id(ids)?;
        let map = self.tempo_map();
        if items.is_empty() {
            bail!("no clips given");
        }
        let last = items
            .iter()
            .map(|(_, c)| Self::clip_span(c, &map).1)
            .fold(0.0, f64::max);
        self.place_clips(items, last.round() as u64, None)
    }

    fn place_clips(
        &mut self,
        items: Vec<(TrackId, AnyClip)>,
        position: u64,
        track: Option<TrackId>,
    ) -> Result<Vec<ClipId>> {
        let map = self.tempo_map();
        let anchor = items
            .iter()
            .map(|(_, c)| Self::clip_span(c, &map).0)
            .fold(f64::MAX, f64::min);
        let rows: Vec<usize> = items
            .iter()
            .map(|(t, c)| {
                self.project
                    .track_index(*t)
                    .ok_or_else(|| anyhow!("clip {}'s track {t} is gone", c.id()))
            })
            .collect::<Result<_>>()?;
        let shift = match track {
            Some(t) => {
                let to = self
                    .project
                    .track_index(t)
                    .ok_or_else(|| anyhow!("no track with id {t}"))?;
                to as i64 - *rows.iter().min().expect("not empty") as i64
            }
            None => 0,
        };
        let targets: Vec<TrackId> = items
            .iter()
            .zip(&rows)
            .map(|((_, clip), &row)| {
                let to = usize::try_from(row as i64 + shift).ok();
                let target = to.and_then(|i| self.project.tracks.get(i));
                self.check_clip_track(clip, target)?;
                Ok(target.expect("checked").id)
            })
            .collect::<Result<_>>()?;
        let tick_shift = map.tick_at(position as f64) - map.tick_at(anchor);
        let mut added = Vec::new();
        for ((_, clip), target) in items.into_iter().zip(targets) {
            match clip {
                AnyClip::Audio(mut c) => {
                    c.id = self.project.next_id();
                    c.start = (position as f64 + c.start as f64 - anchor).round() as u64;
                    added.push(c.id);
                    self.project
                        .track_mut(target)
                        .expect("checked")
                        .clips
                        .push(c);
                }
                AnyClip::Midi(c) => {
                    let mut c = self.project.adopt_midi_clip(c);
                    c.start = (c.start as f64 + tick_shift).round().max(0.0) as u64;
                    added.push(c.id);
                    self.project
                        .track_mut(target)
                        .expect("checked")
                        .midi_clips
                        .push(c);
                }
            }
        }
        self.graph_dirty = true;
        Ok(added)
    }

    /// Move clips (either kind) together by `delta` samples and `track_delta` rows (down is
    /// positive). All or nothing: refused if any would start before 0 or land on a track that
    /// can't hold it.
    pub fn move_clips(&mut self, ids: &[ClipId], delta: i64, track_delta: i64) -> Result<()> {
        let items = self.clips_by_id(ids)?;
        self.move_clips_from(&items, delta, track_delta)
    }

    /// Copies of clips, moved as [`Self::move_clips`] would move them; the originals stay.
    /// All or none. Returns the copies' ids, in the order of `ids`. An Alt-drag makes its
    /// copies with no move (0, 0), then drags them.
    pub fn copy_clips_to(
        &mut self,
        ids: &[ClipId],
        delta: i64,
        track_delta: i64,
    ) -> Result<Vec<ClipId>> {
        let items = self.clips_by_id(ids)?;
        if items.is_empty() {
            bail!("no clips given");
        }
        // Checked first: a refused copy makes nothing (no ids used up either).
        self.plan_moves(&items, delta, track_delta)?;
        let map = self.tempo_map();
        let first = items
            .iter()
            .map(|(_, c)| Self::clip_span(c, &map).0)
            .fold(f64::MAX, f64::min);
        let copies = self.place_clips(items, first.round() as u64, None)?;
        self.move_clips(&copies, delta, track_delta)?;
        Ok(copies)
    }

    /// [`Self::move_clips`] from where `items` (from [`Self::clips_by_id`]) were: a drag moves
    /// the clips from where it began each frame, so no rounding adds up.
    pub fn move_clips_from(
        &mut self,
        items: &[(TrackId, AnyClip)],
        delta: i64,
        track_delta: i64,
    ) -> Result<()> {
        let map = self.tempo_map();
        let moves = self.plan_moves(items, delta, track_delta)?;
        for (id, target, start) in moves {
            if let Some(mut c) = self.project.remove_clip(id) {
                c.start = start.round() as u64;
                self.project
                    .track_mut(target)
                    .expect("checked")
                    .clips
                    .push(c);
            } else if let Some(mut c) = self.project.remove_midi_clip(id) {
                c.start = map.tick_at(start).round() as u64;
                self.project
                    .track_mut(target)
                    .expect("checked")
                    .midi_clips
                    .push(c);
            }
        }
        self.graph_dirty = true;
        Ok(())
    }

    /// Where each of `items` would go, moved `delta` samples and `track_delta` rows: its id,
    /// track and start (samples). Refused, naming the clip, if one would start before 0 or
    /// land where it can't go.
    fn plan_moves(
        &self,
        items: &[(TrackId, AnyClip)],
        delta: i64,
        track_delta: i64,
    ) -> Result<Vec<(ClipId, TrackId, f64)>> {
        let map = self.tempo_map();
        let mut moves = Vec::new();
        for (track, clip) in items {
            let row = self
                .project
                .track_index(*track)
                .ok_or_else(|| anyhow!("clip {}'s track {track} is gone", clip.id()))?
                as i64
                + track_delta;
            let target = usize::try_from(row)
                .ok()
                .and_then(|i| self.project.tracks.get(i));
            self.check_clip_track(clip, target)?;
            let start = Self::clip_span(clip, &map).0 + delta as f64;
            if start < 0.0 {
                bail!(
                    "clip {} would start {:.0} samples before 0",
                    clip.id(),
                    -start
                );
            }
            moves.push((clip.id(), target.expect("checked").id, start));
        }
        Ok(moves)
    }

    /// How long a clip's playing audio is as heard: its render's length for a stretched clip
    /// (known from the warp, without waiting for the render), else its file's. `None` if the
    /// file isn't loaded.
    pub fn clip_audio_len(&self, clip: &Clip) -> Option<u64> {
        match self.stretch_key(clip, &self.tempo_map()) {
            Some(key) => Some(key.warp.iter().map(|s| s.1 as u64).sum()),
            None => self
                .media
                .get(&self.resolve(clip.file()))
                .map(|d| d.len() as u64),
        }
    }

    /// How far a clip's edges can be dragged: the earliest start (where the first of its
    /// takes' audio begins) and the latest end (where the playing take's audio as heard ends;
    /// `None` while its file isn't loaded). Trimming never goes past the audio.
    pub fn clip_trim_range(&self, clip: &Clip) -> (u64, Option<u64>) {
        let reach = clip.takes.iter().map(|t| t.offset).min().unwrap_or(0);
        let earliest = clip.start.saturating_sub(reach);
        let latest = self
            .clip_audio_len(clip)
            .map(|len| (clip.start + len).saturating_sub(clip.offset()));
        (earliest, latest)
    }

    /// Move a clip's start and/or end on the timeline (samples) while its audio stays where
    /// it is: moving an edge in hides the audio there, moving it back out brings it back, as
    /// far as the audio goes ([`Self::clip_trim_range`]). Every take moves with the start, so
    /// they stay in line; fades are kept within the new length. A stretched clip trims its
    /// render (offsets count the audio as heard); a clip that follows the tempo re-reads its
    /// place at the end of the edit.
    pub fn trim_clip(&mut self, id: ClipId, start: Option<u64>, end: Option<u64>) -> Result<()> {
        let clip = self
            .project
            .find_clip(id)
            .map(|(_, c)| c.clone())
            .ok_or_else(|| anyhow!("no audio clip with id {id}"))?;
        let (new_start, new_end) = (start.unwrap_or(clip.start), end.unwrap_or(clip.end()));
        if new_end <= new_start {
            bail!("the clip must end after it starts ({new_start}..{new_end})");
        }
        let (earliest, latest) = self.clip_trim_range(&clip);
        if new_start < earliest {
            bail!("the audio starts at sample {earliest}: the clip can't start before it ({new_start})");
        }
        match latest {
            Some(latest) if new_end > latest => {
                bail!("the audio ends at sample {latest}: the clip can't end after it ({new_end})")
            }
            None if new_end > clip.end() => {
                bail!("the clip's file isn't loaded, so it can only be shortened")
            }
            _ => {}
        }
        let c = self.project.clip_mut(id).expect("found");
        let shift = new_start as i64 - c.start as i64;
        for take in &mut c.takes {
            take.offset = (take.offset as i64 + shift) as u64;
        }
        c.start = new_start;
        c.length = new_end - new_start;
        let (fade_in, fade_out) = (c.fade_in, c.fade_out);
        c.set_fades(fade_in, fade_out);
        self.mark_dirty();
        Ok(())
    }

    /// Crossfade a clip with the clip before it on its track, over `length` samples (default:
    /// their overlap, or [`DEFAULT_CROSSFADE_SECONDS`] if they only touch). Where they overlap
    /// less than that, both are extended into the audio beyond their ends, half each as far
    /// as there is audio. The earlier clip fades out and this one fades in over the overlap.
    /// Returns the earlier clip. Refused if a gap separates them.
    pub fn crossfade_clips(
        &mut self,
        id: ClipId,
        length: Option<u64>,
        shape: crate::project::FadeShape,
    ) -> Result<ClipId> {
        use crate::project::Fade;
        let (track, later) = self
            .project
            .find_clip(id)
            .map(|(t, c)| (t, c.clone()))
            .ok_or_else(|| anyhow!("no clip with id {id}"))?;
        let earlier = self
            .project
            .track(track)
            .and_then(|t| {
                t.clips
                    .iter()
                    .filter(|c| c.id != id && c.start < later.start)
                    .max_by_key(|c| c.start)
            })
            .cloned()
            .ok_or_else(|| anyhow!("clip {id} has no clip before it on its track"))?;
        if earlier.end() < later.start {
            bail!(
                "clip {} ends {} samples before clip {id} starts: move them to touch or overlap",
                earlier.id,
                later.start - earlier.end()
            );
        }
        let overlap = earlier.end() - later.start;
        let default = (DEFAULT_CROSSFADE_SECONDS * f64::from(self.sample_rate)) as u64;
        let wanted = length.unwrap_or(overlap.max(default)).max(overlap);
        let extra = wanted - overlap;
        // Room beyond each end: the earlier clip's audio after it, the later one's before it.
        let after = self
            .clip_audio_len(&earlier)
            .unwrap_or(0)
            .saturating_sub(earlier.offset() + earlier.length);
        let before = later
            .takes
            .iter()
            .map(|t| t.offset)
            .min()
            .unwrap_or(0)
            .min(later.start);
        let mut out = extra.div_ceil(2).min(after);
        let back = (extra - out).min(before);
        out = (extra - back).min(after);
        let overlap = overlap + out + back;
        if overlap == 0 {
            bail!(
                "clips {} and {id} have no audio beyond their ends to crossfade over",
                earlier.id
            );
        }
        if let Some(a) = self.project.clip_mut(earlier.id) {
            a.length += out;
            let fade_in = a.fade_in;
            a.set_fades(
                fade_in,
                Fade {
                    length: overlap,
                    shape,
                },
            );
        }
        if let Some(b) = self.project.clip_mut(id) {
            b.start -= back;
            b.length += back;
            for take in &mut b.takes {
                take.offset = take.offset.saturating_sub(back);
            }
            let fade_out = b.fade_out;
            b.set_fades(
                Fade {
                    length: overlap,
                    shape,
                },
                fade_out,
            );
        }
        self.graph_dirty = true;
        Ok(earlier.id)
    }

    /// Stretch a clip to `ratio` from how it was when a gesture began (`original`): the
    /// field and edge drags of the clip editor. Its start stays, or with `keep_end` its end
    /// does (dragging the left edge). Refused for a clip that follows the tempo.
    pub fn restretch_clip(
        &mut self,
        id: ClipId,
        original: &Clip,
        ratio: f64,
        keep_end: bool,
    ) -> Result<()> {
        if original.follow.is_some() {
            bail!("clip {id} follows the tempo, which sets its stretch; stop following first");
        }
        stretch::check(ratio, original.pitch)?;
        let stretched = Clip::restretched(original, ratio);
        let clip = self
            .project
            .clip_mut(id)
            .ok_or_else(|| anyhow!("no clip with id {id}"))?;
        clip.length = stretched.length;
        clip.stretch = stretched.stretch;
        for (take, from) in clip.takes.iter_mut().zip(&stretched.takes) {
            take.offset = from.offset;
        }
        clip.start = if keep_end {
            original.end().saturating_sub(stretched.length)
        } else {
            original.start
        };
        self.graph_dirty = true;
        Ok(())
    }

    /// Make a clip follow the project's tempo, recorded at `tempo` quarter notes per minute
    /// (see [`Clip::set_follow`]), or stop following with `None`. Its renders follow the
    /// tempo map, changes included.
    pub fn set_clip_follow(&mut self, id: ClipId, tempo: Option<f64>) -> Result<()> {
        if let Some(t) = tempo {
            if !(t.is_finite() && (20.0..=999.0).contains(&t)) {
                bail!("a source tempo of {t} is outside 20 to 999 quarter notes per minute");
            }
        }
        let map = self.tempo_map();
        let clip = self
            .project
            .clip_mut(id)
            .ok_or_else(|| anyhow!("no clip with id {id}"))?;
        clip.set_follow(tempo, &map);
        self.graph_dirty = true;
        Ok(())
    }

    /// Keep clips that follow the tempo in step with it: laid out again when the tempo map
    /// changed, or their musical place read from their samples when an edit moved them. Runs
    /// at the end of every edit, so a tempo change and the clips it moves are one undo step,
    /// and after anything that replaces the project (where it changes nothing: a restored
    /// project is already consistent).
    fn sync_following(&mut self) {
        let map = self.tempo_map();
        let changed = self.laid_out.as_ref() != Some(&map);
        let before: Vec<_> = self.following_layouts();
        for clip in self.project.tracks.iter_mut().flat_map(|t| &mut t.clips) {
            clip.sync_follow(&map, changed);
        }
        if self.following_layouts() != before {
            self.graph_dirty = true;
        }
        self.laid_out = Some(map);
    }

    fn following_layouts(&self) -> Vec<crate::project::Layout> {
        self.project
            .tracks
            .iter()
            .flat_map(|t| &t.clips)
            .filter(|c| c.follow.is_some())
            .map(Clip::layout)
            .collect()
    }

    /// Block until every stretched clip's render is ready (for headless tools and checks;
    /// the app polls [`Self::clips_rendering`] instead).
    pub fn wait_for_stretches(&mut self, timeout: std::time::Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            self.update();
            let waiting = self.clips_rendering();
            if waiting.is_empty() {
                // The graph with the renders goes out on this update.
                self.update();
                return self.wait_for_impulses(deadline.saturating_duration_since(Instant::now()));
            }
            if Instant::now() >= deadline {
                bail!("clips {waiting:?} are still rendering");
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    // ---------------------------------------------------------------- files

    pub fn new_project(&mut self) {
        self.settle_saving();
        // A project still opening is given up.
        self.loading = None;
        self.stop();
        self.release_plugins();
        self.params.clear();
        self.media.clear();
        self.stretches.clear();
        self.project = Project::default();
        self.project.sample_rate = self.sample_rate;
        self.path = None;
        self.history.reset();
        self.sync_following();
        self.graph_dirty = true;
        // Nothing to lose yet: a new project with no plug-ins.
        self.saved_json = self.project_json().ok();
    }

    /// Start a new project called `name` in a folder of its own inside `parent` (absolute),
    /// with its subfolders ([`crate::folder`]), saved there at once so it is a project from
    /// the start: empty, or `template`'s setup with every plug-in loaded from its saved state.
    /// Returns the project file's path.
    pub fn new_project_in(
        &mut self,
        name: &str,
        parent: &Path,
        template: Option<&str>,
    ) -> Result<PathBuf> {
        // Read first, so a bad template leaves no folder behind.
        let template = match template {
            Some(t) => Some(crate::template::read(self.templates_folder()?, t)?),
            None => None,
        };
        let path = crate::folder::create(parent, name)?;
        match template {
            Some((json, source)) => {
                self.open_json(&json, &source, &path)?;
                if !self.stage_loads {
                    self.finish_loading();
                }
            }
            None => self.new_project(),
        }
        self.project.name = name.trim().to_string();
        self.save(&path)?;
        Ok(path)
    }

    /// The templates folder, made if it isn't there yet.
    fn templates_folder(&self) -> Result<&Path> {
        let dir = self
            .templates_dir
            .as_deref()
            .ok_or_else(|| anyhow!("no templates folder is set"))?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(dir)
    }

    /// Save the open project's setup as the template `name` (`crate::template::of`): every
    /// plug-in's state read now, no clips. Refused if one of that name exists, unless
    /// `replace`. Returns its path.
    pub fn save_template(&mut self, name: &str, replace: bool) -> Result<PathBuf> {
        let path = crate::template::path_of(self.templates_folder()?, name)?;
        if path.exists() && !replace {
            bail!(
                "a template called \"{}\" exists already: replace it or pick another name",
                name.trim()
            );
        }
        self.capture_plugin_states();
        let template = crate::template::of(&self.project, name);
        write_whole(&path, &serde_json::to_string_pretty(&template)?)?;
        Ok(path)
    }

    /// The saved templates, by name.
    pub fn templates(&self) -> Vec<crate::template::Template> {
        self.templates_dir
            .as_deref()
            .map(crate::template::list)
            .unwrap_or_default()
    }

    pub fn rename_template(&self, from: &str, to: &str) -> Result<PathBuf> {
        crate::template::rename(self.templates_folder()?, from, to)
    }

    pub fn delete_template(&self, name: &str) -> Result<()> {
        crate::template::delete(self.templates_folder()?, name)
    }

    /// Save to `path` now, every plug-in's state read on the calling thread (tools, tests and
    /// a new project's first save; the app saves with [`Self::save_in_background`]).
    pub fn save(&mut self, path: &Path) -> Result<()> {
        if self.saving().is_some() {
            bail!("a save is already being written; wait for it to finish");
        }
        self.drop_capture();
        self.capture_plugin_states();
        let work = self.prepare_save(path)?;
        let path = work.path.clone();
        let json = work.run()?;
        self.finish_save(path, json);
        Ok(())
    }

    /// How the last background save ended (the file written, or why not), once.
    pub fn take_saved(&mut self) -> Option<std::result::Result<PathBuf, String>> {
        self.saved.take()
    }

    /// The project saved as `json` at `path`: it is the project's file now.
    fn finish_save(&mut self, path: PathBuf, json: String) {
        self.path = Some(path);
        self.saved_json = Some(json.clone());
        self.backup_baseline = Some(json);
        self.last_autosave = Instant::now();
        self.newer_backup = None;
    }

    /// Whether the project differs from its file as last saved or opened (every plug-in's
    /// state read now), or is a backup opened in its place: what closing it would lose. A
    /// project still loading its plug-ins has none (it was only just opened).
    pub fn has_unsaved_changes(&mut self) -> bool {
        // An open's project isn't whole yet (a reload's is: its slots keep their states).
        if self
            .loading
            .as_ref()
            .is_some_and(|l| l.kind == LoadKind::Open)
        {
            return false;
        }
        match self.project_json() {
            Ok(json) => differs(&json, self.saved_json.as_deref()),
            Err(_) => true,
        }
    }

    /// What saving to `path` writes, gathered on the main thread once the plug-ins' states are
    /// read: the project named after the file (its undo snapshots too), and for a save into
    /// another folder the files to copy along.
    fn prepare_save(&mut self, path: &Path) -> Result<SaveWork> {
        let path = with_project_extension(path);
        let old_dir = self.project_dir();
        let new_dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
        // Saved into another folder: the project's files go with it (the same places inside
        // the new folder), so it stays self-contained and the old one is left as it was.
        // Every file a snapshot in the undo history uses too, so undo keeps working.
        let mut copies = Vec::new();
        if let Some(old_dir) = old_dir.filter(|d| *d != new_dir) {
            let mut files = std::collections::BTreeSet::new();
            let mut collect = |project: &mut Project| {
                let takes = project
                    .tracks
                    .iter()
                    .flat_map(|t| &t.clips)
                    .flat_map(|c| &c.takes);
                files.extend(
                    takes
                        .filter(|t| t.file.is_relative())
                        .map(|t| t.file.clone()),
                );
            };
            collect(&mut self.project);
            self.history.for_each_snapshot(&mut collect);
            // The impulses the built-in effects play (the Convolution Reverb's).
            files.extend(
                self.plugins
                    .values()
                    .filter_map(|live| match &live.instance {
                        Instance::Native(n) => {
                            n.file().filter(|f| f.is_relative()).map(Path::to_path_buf)
                        }
                        Instance::Vst3 { .. } => None,
                    }),
            );
            copies = files
                .into_iter()
                .map(|file| (old_dir.join(&file), new_dir.join(&file)))
                .collect();
        }
        // The project takes the file's name; the history's snapshots too, or undoing past
        // this save would bring back the old name.
        let name = path.file_stem().map(|n| n.to_string_lossy().into_owned());
        let rename = |project: &mut Project| {
            if let Some(name) = &name {
                project.name = name.clone();
            }
        };
        rename(&mut self.project);
        self.history.for_each_snapshot(rename);
        Ok(SaveWork {
            path,
            project: self.project.clone(),
            copies,
        })
    }

    /// Open project file `path`. With [`Self::stage_loads`] its plug-ins load over the next
    /// frames ([`Self::loading`]); otherwise they are all in when this returns.
    pub fn open(&mut self, path: &Path) -> Result<()> {
        let json =
            std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        self.open_json(&json, path, path)?;
        // Its newest backup is looked for once the plug-ins are in (at once if it has none).
        match &mut self.loading {
            Some(loading) => loading.path = Some(path.to_path_buf()),
            None => self.newer_backup = self.newest_backup_after(path),
        }
        if !self.stage_loads {
            self.finish_loading();
        }
        Ok(())
    }

    /// Moves whenever an open begins or is cancelled: what an agent's open waits on in the app
    /// was that open only while this stays the same (a cancel, or another open over it, moves
    /// it).
    pub fn open_serial(&self) -> u64 {
        self.open_serial
    }

    /// How far the project being opened has come, while it is: closing the last project's
    /// plug-ins, reading its files, loading its plug-ins.
    pub fn loading(&self) -> Option<LoadProgress> {
        let l = self.loading.as_ref()?;
        let closed = l.closing_total.saturating_sub(self.closing.len());
        let files = l.reads.files.len();
        let loaded = l.total - l.queue.len();
        let name = |p: &PathBuf| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        let (stage, stage_done, stage_total, next, track) = if !self.closing.is_empty() {
            let next = self.closing.front().map(|(n, _)| n.clone());
            (LoadStage::Closing, closed, l.closing_total, next, None)
        } else if !l.reads.done() {
            // The first file not in yet, in the project's order.
            let next = l
                .reads
                .files
                .iter()
                .zip(&l.reads.arrived)
                .find(|(_, arrived)| !**arrived)
                .map(|(p, _)| name(p));
            (LoadStage::Reading, l.reads.count, files, next, None)
        } else {
            let front = l.queue.front();
            (
                LoadStage::Loading,
                loaded,
                l.total,
                front.map(|(s, _)| s.name.clone()),
                front.map(|(_, t)| t.clone()),
            )
        };
        Some(LoadProgress {
            stage,
            done: closed + l.reads.count + loaded,
            total: l.closing_total + files + l.total,
            stage_done,
            stage_total,
            next,
            track,
            kind: l.kind,
            label: l.label.clone(),
        })
    }

    /// Finish opening now, waiting for the files and loading every plug-in (agents, which wait
    /// for an open to finish, and tests). Lets held file reads go.
    pub fn finish_loading(&mut self) {
        self.read_hold.set(None);
        self.plugins_held_after = None;
        while self.loading.is_some() {
            self.load_step(Duration::MAX);
        }
        self.close_step(Duration::MAX, true);
    }

    /// Hold back the main thread's share of an open (destroying the last project's plug-ins,
    /// loading the new one's) while set; files still read. The app holds it until its loading
    /// bar has reached the screen, so no frame stalls on a plug-in before the bar is seen.
    /// [`Self::finish_loading`] doesn't wait for it.
    pub fn hold_loading(&mut self, hold: bool) {
        self.loads_held = hold;
    }

    /// Hold an open's file reads back: `Some(n)` lets the first `n` files (in the project's
    /// order) read and holds the rest before they start; `None` lets them all go. Check runs
    /// catch the bar on its files with it, and tests see that an open returns before any
    /// file is read. [`Self::finish_loading`] and [`Self::cancel_loading`] let them go.
    pub fn hold_file_reads(&self, after: Option<usize>) {
        self.read_hold.set(after);
    }

    /// Hold a staged load's plug-ins back after the first `n` (`Some(n)`; `None` lets them
    /// all go), as [`Self::hold_file_reads`] does for files. Built-in effects load in
    /// microseconds, a whole project's in one update, so the manual's shot of the bar on its
    /// plug-ins (`--neutral`, no third-party plug-ins) holds them partway with it.
    /// [`Self::finish_loading`] and [`Self::cancel_loading`] let them go.
    pub fn hold_plugin_loads(&mut self, after: Option<usize>) {
        self.plugins_held_after = after;
    }

    /// Stop opening the project being opened and go back to the one open before, reopened
    /// from its file (behind the same loading, which can't be cancelled back again: a cancel
    /// then leaves no project). With none before (or its file gone), no project is open
    /// (`path` is None: the app's start screen). Returns the project reopened. A plug-in
    /// already loading finishes first (a VST3 load can't be interrupted).
    pub fn cancel_loading(&mut self) -> Result<Option<PathBuf>> {
        match self.loading.as_ref().map(|l| l.kind) {
            Some(LoadKind::Rack) => bail!(
                "only the Waveform view's effects rack is loading (a plug-in a frame), which \
                 can't be cancelled: it is done in a moment"
            ),
            // The step is the project already; what it needs can only come in (undo or redo
            // again once it has).
            Some(LoadKind::Undo | LoadKind::Redo) => bail!(
                "an undo or redo is bringing plug-ins back (a plug-in a frame), which can't be \
                 cancelled: undo or redo again once they are in"
            ),
            // The device runs at the new rate already: going back is another change.
            Some(LoadKind::SampleRate) => bail!(
                "every plug-in is reloading at the new sample rate, which can't be cancelled: \
                 change the rate back once they are in"
            ),
            _ => {}
        }
        let Some(loading) = self.loading.take() else {
            bail!("no project is being opened");
        };
        self.open_serial += 1;
        self.read_hold.set(None);
        self.plugins_held_after = None;
        let previous = loading.previous.clone();
        drop(loading);
        self.stop();
        // A rack synced with one of its tracks gives way to the user's own (21.2).
        self.end_rack_sync_for_close();
        self.let_go_of_plugins();
        self.params.clear();
        self.forget_media();
        self.stretches.clear();
        if let Some(previous) = previous.filter(|p| p.is_file()) {
            match self.open(&previous) {
                Ok(()) => {
                    if let Some(loading) = &mut self.loading {
                        loading.previous = None;
                    }
                    return Ok(Some(previous));
                }
                Err(e) => self
                    .notices
                    .push(format!("Reopening {}: {e:#}", previous.display())),
            }
        }
        self.loading = None;
        self.project = Project::default();
        self.project.sample_rate = self.sample_rate;
        self.path = None;
        self.history.reset();
        self.sync_following();
        self.graph_dirty = true;
        self.newer_backup = None;
        self.backup_baseline = None;
        self.saved_json = self.project_json().ok();
        Ok(None)
    }

    /// Take the engine off every plug-in (an empty graph) and queue them to be destroyed a few
    /// a frame ([`Self::close_step`]), editors closed now. Nothing waits here: they go once
    /// the engine has swapped to the empty graph.
    fn let_go_of_plugins(&mut self) {
        // The rack isn't the project's: its plug-ins come back as they are now (a synced one
        // still paired: a new sample rate keeps the ids; an open ends the sync first).
        self.release_preview();
        self.capture_rack_states();
        self.load_failures.clear();
        self.send_graph(Graph::empty(MAX_BLOCK));
        self.graph_dirty = true;
        self.collect_garbage();
        let names: HashMap<InsertId, String> = self
            .project
            .all_inserts()
            .chain(&self.editor_rack)
            .map(|s| (s.id, s.name.clone()))
            .collect();
        for (id, mut live) in self.plugins.drain() {
            if let Some(mut window) = live.window.take() {
                window.close();
            }
            live.native_open = false;
            let name = names.get(&id).cloned().unwrap_or_default();
            self.closing.push_back((name, live));
        }
        self.closing_since = Instant::now();
    }

    /// Let go of the decoded audio: freed on a worker, since a big project's takes a while.
    fn forget_media(&mut self) {
        let media = std::mem::take(&mut self.media);
        let _ = std::thread::Builder::new()
            .name("free media".into())
            .spawn(move || drop(media));
    }

    /// Destroy plug-ins let go of ([`Self::let_go_of_plugins`]) for up to `budget` (at least
    /// one), once the engine runs a graph without them; with `wait`, all of them, waiting for
    /// the engine. Returns whether it destroyed any.
    fn close_step(&mut self, budget: Duration, wait: bool) -> bool {
        if self.closing.is_empty() {
            return false;
        }
        self.collect_garbage();
        if self.graphs_in_flight > 0 {
            // A stalled device (or none) mustn't keep them forever: then under the lock.
            let stalled = self.closing_since.elapsed() > Duration::from_millis(500);
            if wait || !self.engine_running() || stalled {
                self.flush_pending();
            } else {
                return false;
            }
        }
        let started = Instant::now();
        while let Some((_, live)) = self.closing.pop_front() {
            drop(live);
            if !wait && started.elapsed() >= budget {
                break;
            }
        }
        true
    }

    /// Take what the file workers have read into the media pool: all of it, waiting, with
    /// `block`. Returns whether every file is in.
    fn poll_reads(&mut self, block: bool) -> bool {
        use std::sync::mpsc::TryRecvError;
        let Some(loading) = self.loading.as_mut() else {
            return true;
        };
        let reads = &mut loading.reads;
        let mut arrived = Vec::new();
        while !reads.done() {
            let got = if block {
                reads.results.recv().map_err(|_| TryRecvError::Disconnected)
            } else {
                reads.results.try_recv()
            };
            match got {
                Ok((i, read)) => {
                    if reads.arrived.get(i) == Some(&false) {
                        reads.arrived[i] = true;
                        reads.count += 1;
                        let path = reads.files[i].clone();
                        match read {
                            Ok(data) => arrived.push((path, Arc::new(data))),
                            Err(e) => reads.failures.push(format!("{}: {e:#}", path.display())),
                        }
                    }
                }
                Err(TryRecvError::Empty) => break,
                // Every worker gone (one panicked, or none could start): the rest unread.
                Err(TryRecvError::Disconnected) => {
                    for (path, arrived) in reads.files.iter().zip(reads.arrived.iter_mut()) {
                        if !*arrived {
                            *arrived = true;
                            reads
                                .failures
                                .push(format!("{}: it could not be read", path.display()));
                        }
                    }
                    reads.count = reads.files.len();
                }
            }
        }
        let done = reads.done();
        for (path, data) in arrived {
            self.media.insert(path, data);
        }
        done
    }

    /// One frame's share of an open, for up to `budget`: the last project's plug-ins
    /// destroyed, the files that have arrived taken in, then (once all are in) plug-ins loaded,
    /// at least one; a frame that destroyed a plug-in loads none. When everything is in, what
    /// opening does at the end: tempo-following clips, the graph, the backup baseline, the
    /// newer backup. `Duration::MAX` does it all, waiting for the files.
    fn load_step(&mut self, budget: Duration) {
        let wait = budget == Duration::MAX;
        let held = self.loads_held && !wait;
        let started = Instant::now();
        let closed = !held && self.close_step(budget, wait);
        let read = self.poll_reads(wait && self.closing.is_empty());
        if !self.closing.is_empty() || !read {
            return;
        }
        if !held && !closed {
            let limit = self.plugins_held_after.filter(|_| !wait);
            while let Some((slot, _)) = self
                .loading
                .as_mut()
                .filter(|l| limit.is_none_or(|n| l.total - l.queue.len() < n))
                .and_then(|l| l.queue.pop_front())
            {
                // An insert removed while it waited (a rack slot, an undo's plug-in) is skipped.
                let wanted = self
                    .project
                    .all_inserts()
                    .chain(&self.editor_rack)
                    .any(|s| s.id == slot.id);
                if wanted && !self.plugins.contains_key(&slot.id) {
                    // A copy of another insert (a synced rack) takes its settings as they are
                    // now, read here, a plug-in a frame; one whose original is gone keeps the
                    // settings it was queued with.
                    let copied = self
                        .loading
                        .as_mut()
                        .and_then(|l| l.copies.remove(&slot.id))
                        .and_then(|from| self.plugins.get(&from))
                        .and_then(|from| from.save_state().ok())
                        .map(Arc::new);
                    if let (Some(state), Some(kept)) = (&copied, self.insert_mut(slot.id)) {
                        kept.state = Some(state.clone());
                    }
                    let state = copied.or(slot.state.clone());
                    match self.load_live(&slot.path, state.as_deref().map(Vec::as_slice)) {
                        Ok(live) => {
                            self.plugins.insert(slot.id, live);
                            self.load_failures.remove(&slot.id);
                            // A track's insert loading beside the project (a synced rack's
                            // copy): into the graph now.
                            if self.project.find_insert(slot.id).is_some() {
                                self.graph_dirty = true;
                            }
                        }
                        Err(e) => {
                            // The slot stays, with its saved settings, greyed: saying why.
                            self.notices.push(format!("{}: {e:#}", slot.name));
                            self.load_failures.insert(slot.id, format!("{e:#}"));
                        }
                    }
                }
                if started.elapsed() >= budget {
                    break;
                }
            }
        }
        if self.loading.as_ref().is_some_and(|l| l.queue.is_empty()) {
            let mut loading = self.loading.take().expect("checked");
            // Files that didn't read, said once.
            self.notices.extend(loading.reads.failures.iter().cloned());
            match loading.kind {
                // Only the rack loaded: the project is as it was.
                LoadKind::Rack => return,
                // The step's graph at last, once; then what it dropped goes.
                LoadKind::Undo | LoadKind::Redo => {
                    if self.rendering.is_none() {
                        self.send_project_graph();
                    }
                    self.retire(std::mem::take(&mut loading.retiring));
                    return;
                }
                LoadKind::SampleRate => {
                    self.sync_following();
                    self.graph_dirty = true;
                    return;
                }
                LoadKind::Open => {}
            }
            self.sync_following();
            self.graph_dirty = true;
            // As it stands now, with the plug-ins' live states: nothing to back up yet, and
            // (unless it is a backup) nothing unsaved.
            self.backup_baseline = self.project_json().ok();
            self.saved_json = if loading.backup {
                None
            } else {
                self.backup_baseline.clone()
            };
            self.last_autosave = Instant::now();
            if let Some(path) = &loading.path {
                self.newer_backup = self.newest_backup_after(path);
            }
        }
    }

    /// The distinct audio files the project's clips play, resolved, in track order, each with
    /// whether its onsets are wanted (a stretched clip that keeps its attacks).
    fn files_to_read(&self) -> Vec<(PathBuf, bool)> {
        let mut files: Vec<(PathBuf, bool)> = Vec::new();
        let mut index: HashMap<PathBuf, usize> = HashMap::new();
        for clip in self.project.tracks.iter().flat_map(|t| &t.clips) {
            let onsets = clip.transients && (clip.is_stretched() || clip.follow.is_some());
            for take in &clip.takes {
                let path = self.resolve(&take.file);
                match index.get(&path) {
                    Some(&i) => files[i].1 |= onsets,
                    None => {
                        index.insert(path.clone(), files.len());
                        files.push((path, onsets));
                    }
                }
            }
        }
        files
    }

    /// Open project JSON read from `source` as the project file `path` (a backup opens as its
    /// project, so its files resolve in the project folder and Save writes the project). Quick:
    /// the last project's plug-ins are let go of, the files start reading on workers, and the
    /// rest happens over the next updates ([`Self::load_step`]).
    fn open_json(&mut self, json: &str, source: &Path, path: &Path) -> Result<()> {
        let mut project: Project =
            serde_json::from_str(json).with_context(|| format!("parsing {}", source.display()))?;
        project.upgrade();
        self.settle_saving();
        // An Apply renders through the rack's plug-ins, which an open lets go of: it stops.
        if self.applying.is_some() {
            self.cancel_apply();
            self.wait_for_apply(IMPULSE_WAIT)?;
        }
        // Cancelling goes back to the project open before (an open over an open keeps the
        // first one's).
        let mut before = self.loading.take();
        let previous = match &before {
            Some(loading) if loading.kind == LoadKind::Open => loading.previous.clone(),
            _ => self.path.clone(),
        };
        self.stop();
        // A rack synced with one of the last project's tracks gives way to the user's own.
        self.end_rack_sync_for_close();
        self.let_go_of_plugins();
        // Plug-ins an undo was letting go of: the empty graph is sent, so they can go.
        if let Some(l) = &mut before {
            self.retire(std::mem::take(&mut l.retiring));
        }
        drop(before);
        self.params.clear();
        self.forget_media();
        self.stretches.clear();
        self.project = project;
        self.path = Some(path.to_path_buf());
        self.history.reset();
        if self.project.sample_rate != self.sample_rate {
            self.project.convert_sample_rate(self.sample_rate);
        }
        self.send(Command::Locate(0));
        self.newer_backup = None;
        let reads = Reads::start(
            self.files_to_read(),
            self.sample_rate,
            self.read_hold.clone(),
        );
        // The plug-ins load next: all of them before anything needs them.
        let queue = self.insert_queue();
        self.open_serial += 1;
        self.loading = Some(Loading {
            kind: LoadKind::Open,
            label: None,
            retiring: Vec::new(),
            closing_total: self.closing.len(),
            reads,
            total: queue.len(),
            queue,
            path: None,
            backup: false,
            previous,
            copies: HashMap::new(),
        });
        // Nothing to close, read or load: done at once.
        let empty = self.closing.is_empty()
            && self
                .loading
                .as_ref()
                .is_some_and(|l| l.queue.is_empty() && l.reads.files.is_empty());
        if empty {
            self.load_step(Duration::ZERO);
        }
        Ok(())
    }

    // ---------------------------------------------------------------- backups

    /// The project as JSON, with every plug-in's state read now.
    fn project_json(&mut self) -> Result<String> {
        self.capture_plugin_states();
        Ok(serde_json::to_string_pretty(&self.project)?)
    }

    /// Back the project up now into `Backups/<Name> <date time>.oxide`, keeping the newest
    /// [`Self::keep_backups`]; the project file itself is never touched. Needs a saved project.
    pub fn save_backup(&mut self) -> Result<PathBuf> {
        if self.path.is_none() {
            bail!("the project has no folder yet: create or save it first");
        }
        let json = self.project_json()?;
        self.write_backup(json)
    }

    fn write_backup(&mut self, json: String) -> Result<PathBuf> {
        let folder = self.project_folder(crate::folder::BACKUPS)?;
        let stem = crate::folder::stamped(&self.project.name);
        let path = crate::folder::free_name(&folder, &stem, crate::folder::EXTENSION);
        write_whole(&path, &json)?;
        self.backup_baseline = Some(json);
        self.last_autosave = Instant::now();
        prune_backups(&folder, self.keep_backups.max(1));
        Ok(path)
    }

    /// The open project's backups, newest first.
    pub fn backups(&self) -> Vec<Backup> {
        let Some(dir) = self.path.as_deref().and_then(Path::parent) else {
            return Vec::new();
        };
        list_backups(&dir.join(crate::folder::BACKUPS))
    }

    /// The newest backup written after the project file `path` was saved, if any.
    fn newest_backup_after(&self, path: &Path) -> Option<PathBuf> {
        let saved = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
        self.backups()
            .into_iter()
            .find(|b| b.modified > saved)
            .map(|b| b.path)
    }

    /// A backup newer than the project file when it was opened (a crash, or a close without
    /// saving): the app offers to open it instead.
    pub fn newer_backup(&self) -> Option<&Path> {
        self.newer_backup.as_deref()
    }

    /// Open one of the project's backups in place of it: it becomes the open project, unsaved
    /// (Save writes it over the project file). Only a file in the open project's `Backups/`.
    pub fn open_backup(&mut self, backup: &Path) -> Result<()> {
        let Some(path) = self.path.clone() else {
            bail!("open the project first: a backup opens as its project");
        };
        let folder = path
            .parent()
            .map(|d| d.join(crate::folder::BACKUPS))
            .unwrap_or_default();
        let inside = backup
            .parent()
            .zip(std::fs::canonicalize(&folder).ok())
            .and_then(|(p, folder)| Some(std::fs::canonicalize(p).ok()? == folder))
            .unwrap_or(false);
        if !inside {
            bail!(
                "{} isn't one of this project's backups (in {})",
                backup.display(),
                folder.display()
            );
        }
        let json = std::fs::read_to_string(backup)
            .with_context(|| format!("reading {}", backup.display()))?;
        self.open_json(&json, backup, &path)?;
        match &mut self.loading {
            Some(loading) => loading.backup = true,
            None => self.saved_json = None,
        }
        if !self.stage_loads {
            self.finish_loading();
        }
        self.newer_backup = None;
        Ok(())
    }

    /// Back up the project if it is due and has changed, while the transport is stopped (a
    /// plug-in's state is read under its lock, which can skip a block of its audio: silence,
    /// when stopped). Runs from [`Self::update`]; the states are read a VST3 an update and
    /// the backup written on a worker (`saving.rs`), so it never holds the window long.
    fn autosave_tick(&mut self) {
        let Some(interval) = self.autosave else {
            return;
        };
        if self.path.is_none() || self.last_autosave.elapsed() < interval {
            return;
        }
        if self.autosave_busy() || self.capture.is_some() || self.autosaving() {
            return;
        }
        self.last_autosave = Instant::now();
        self.begin_autosave();
    }

    /// What an autosave waits for (and gives way to): playing, recording, an Apply, a save,
    /// plug-ins loading, an export.
    fn autosave_busy(&self) -> bool {
        self.is_playing()
            || self.is_recording()
            || self.applying.is_some()
            || self.saving().is_some()
            || self.loading.is_some()
            || self.export.as_ref().is_some_and(|e| !e.is_finished())
    }

    // ---------------------------------------------------------------- export

    /// Bounce the project (or `range`) to a WAV file in the background, at its level.
    pub fn export(
        &mut self,
        path: &Path,
        range: Option<(u64, u64)>,
        tail_seconds: f64,
    ) -> Result<()> {
        self.export_with(path, range, tail_seconds, None)
    }

    /// [`Self::export`], normalised to a loudness target if `normalize` says so.
    pub fn export_with(
        &mut self,
        path: &Path,
        range: Option<(u64, u64)>,
        tail_seconds: f64,
        normalize: Option<export::Normalize>,
    ) -> Result<()> {
        let encoding = crate::encode::Encoding::default();
        self.export_as(path, range, tail_seconds, normalize, encoding)
    }

    /// [`Self::export_with`] in the format `encoding` says (checked before anything renders).
    pub fn export_as(
        &mut self,
        path: &Path,
        range: Option<(u64, u64)>,
        tail_seconds: f64,
        normalize: Option<export::Normalize>,
        encoding: crate::encode::Encoding,
    ) -> Result<()> {
        encoding.check(self.sample_rate)?;
        if self.export.as_ref().is_some_and(|e| !e.is_finished()) {
            bail!("an export is already running");
        }
        self.check_plugins_in("export")?;
        self.update();
        // Impulses being prepared are waited for (quick, unlike a stretch's render).
        self.wait_for_impulses(IMPULSE_WAIT)?;
        // The bounce must be what playback plays once the renders are in, never the
        // originals standing in for them.
        let waiting = self.clips_rendering();
        if !waiting.is_empty() {
            bail!("stretched clips {waiting:?} are still rendering; export when they are ready");
        }
        let (start, end) = range.unwrap_or((0, self.content_end()));
        if end <= start {
            bail!("nothing to export");
        }
        self.start_export(path, start, end, tail_seconds, normalize, encoding);
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        self.export_kind = ExportKind::Export;
        self.export_what = format!("Exporting {name}");
        Ok(())
    }

    fn start_export(
        &mut self,
        path: &Path,
        start: u64,
        end: u64,
        tail_seconds: f64,
        normalize: Option<export::Normalize>,
        encoding: crate::encode::Encoding,
    ) {
        let block = match self.stats().block_size {
            0 => 512,
            n => n,
        };
        let spec = ExportSpec {
            path: path.to_path_buf(),
            start,
            end,
            tail: (tail_seconds * f64::from(self.sample_rate)) as u64,
            channel: usize::from(self.project.master.output_channel),
            block,
            sample_rate: self.sample_rate,
            preroll: (f64::from(self.sample_rate) * PREROLL_SECONDS) as usize,
            normalize,
            encoding,
            // A track's render: its file goes on a new track, read on the render's thread.
            read_back: self.rendering.is_some(),
            hold: self.export_hold.clone(),
        };
        self.export = Some(export::start(self.core.clone(), spec));
    }

    /// A bounce plays the project's graph with every plug-in: refused while plug-ins of the
    /// project are still coming in (an open, an undo, a new sample rate; the effects rack's
    /// aren't in the project).
    fn check_plugins_in(&self, what: &str) -> Result<()> {
        match self.loading.as_ref().map(|l| l.kind) {
            None | Some(LoadKind::Rack) => Ok(()),
            Some(_) => bail!("the project's plug-ins are still loading; {what} once they are in"),
        }
    }

    /// Where a track's sound runs on the timeline: its clips' and MIDI clips' span, and those
    /// of the tracks that reach the mix through it (a folder's). None if it has nothing.
    pub fn track_range(&self, track: TrackId) -> Option<(u64, u64)> {
        let map = self.tempo_map();
        // A plug-in's output tracks play the instrument's clips.
        let hosts: HashSet<TrackId> = self
            .project
            .tracks
            .iter()
            .filter(|t| self.project.main_feeds(t.id, track))
            .filter_map(|t| match t.input {
                TrackInput::Plugin { insert, .. } => self.project.insert_track(insert),
                _ => None,
            })
            .collect();
        self.project
            .tracks
            .iter()
            .filter(|t| self.project.main_feeds(t.id, track) || hosts.contains(&t.id))
            .flat_map(|t| {
                let audio = t.clips.iter().map(|c| (c.start, c.end()));
                let midi = t.midi_clips.iter().map(|c| {
                    let at = |tick: u64| map.sample_at_tick(tick as f64).max(0.0);
                    (at(c.start).floor() as u64, at(c.end()).ceil() as u64)
                });
                audio.chain(midi).collect::<Vec<_>>()
            })
            .reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
    }

    /// Render `track` as it sounds (its clips or MIDI through its instrument, its inserts,
    /// fader, pan and automation, and any folder's tracks; not its sends) from its first clip
    /// to its last plus `tail_seconds`, through the same engine and pre-roll as an export, into
    /// `Rendered/<track> <date time>.wav` (32-bit float). When it is done (`update`), the file
    /// goes on a new audio track right under it, routed where it goes, at 0 dB and centre;
    /// with `disable`, the track is switched off in the same undo step. Stops playback.
    pub fn render_track(
        &mut self,
        track: TrackId,
        tail_seconds: f64,
        disable: bool,
    ) -> Result<PathBuf> {
        if self.export.as_ref().is_some_and(|e| !e.is_finished()) {
            bail!("an export or render is already running");
        }
        if self.is_recording() {
            bail!("stop recording first");
        }
        self.check_plugins_in("render")?;
        let name = match self.project.track(track) {
            Some(t) if t.mixer.mute => {
                bail!(
                    "track {track} ({}) is muted: unmute it to render it",
                    t.name
                )
            }
            Some(t) => t.name.clone(),
            None => bail!("no track with id {track}"),
        };
        self.update();
        self.wait_for_impulses(IMPULSE_WAIT)?;
        let waiting = self.clips_rendering();
        if !waiting.is_empty() {
            bail!("stretched clips {waiting:?} are still rendering; try again when they are ready");
        }
        let (start, end) = self
            .track_range(track)
            .ok_or_else(|| anyhow!("track {track} ({name}) has no clips to render"))?;
        let folder = self.project_folder(crate::folder::RENDERED)?;
        let path = crate::folder::free_name(&folder, &crate::folder::stamped(&name), "wav");
        self.stop();
        self.rendering = Some(TrackRender {
            track,
            start,
            disable,
        });
        let graph = self.build_graph_of(Some(track));
        self.sync_params();
        self.send_graph(graph);
        let encoding = crate::encode::Encoding::default();
        self.start_export(&path, start, end, tail_seconds.max(0.0), None, encoding);
        self.export_kind = ExportKind::Render;
        self.export_what = format!("Rendering {name}");
        Ok(path)
    }

    /// Mixdown to a new file: the mix as it plays (every track, level,
    /// plug-in, automation and send, the master included), rendered as an export renders it,
    /// over `range` (the whole project without one) plus `tail_seconds`, into
    /// `Rendered/<Name> mixdown <date time>.wav` (32-bit float, not normalised). When it is
    /// done (`update`), the file opens as a document for the Waveform view
    /// ([`Self::take_opened_mixdown`]). Returns its path.
    pub fn mixdown_to_new_file(
        &mut self,
        range: Option<(u64, u64)>,
        tail_seconds: f64,
    ) -> Result<PathBuf> {
        if self.project.master.mixer.mute {
            bail!("the master is muted: unmute it to mix down");
        }
        let folder = self.project_folder(crate::folder::RENDERED)?;
        let stem = crate::folder::stamped(&format!("{} mixdown", self.project.name));
        let path = crate::folder::free_name(&folder, &stem, "wav");
        self.export_with(&path, range, tail_seconds.max(0.0), None)?;
        self.export_kind = ExportKind::Mixdown;
        self.export_what = "Mixing down to a new file".into();
        self.mixdown_pending = true;
        Ok(path)
    }

    /// Export Waveform view document `id` (as it is now, unsaved edits included) to `path` in
    /// the format `encoding` says, in the background like an export (`export_status`). The
    /// document and its saved state are untouched.
    pub fn export_document(
        &mut self,
        id: DocumentId,
        path: &Path,
        encoding: crate::encode::Encoding,
    ) -> Result<()> {
        if self.export.as_ref().is_some_and(|e| !e.is_finished()) {
            bail!("an export is already running");
        }
        let doc = self
            .document(id)
            .ok_or_else(|| anyhow!("no open document with id {id}"))?;
        encoding.check(doc.audio.sample_rate)?;
        let audio = wav::Audio {
            sample_rate: doc.audio.sample_rate,
            channels: doc.audio.channels.clone(),
        };
        if audio.channels.first().is_none_or(Vec::is_empty) {
            bail!("document {id} is empty");
        }
        let metadata = (*doc.metadata).clone();
        self.export = Some(export::start_encode(
            audio,
            path.to_path_buf(),
            encoding,
            metadata,
        ));
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        self.export_kind = ExportKind::File;
        self.export_what = format!("Exporting {name}");
        Ok(())
    }

    /// Change document `id`'s tags: a step of its own history ("Metadata"), written by Save and
    /// Export. No step when nothing changed.
    pub fn set_document_metadata(
        &mut self,
        id: DocumentId,
        metadata: crate::metadata::Metadata,
    ) -> Result<()> {
        let doc = self
            .documents
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| anyhow!("no open document with id {id}"))?;
        if *doc.metadata != metadata {
            doc.push_metadata("Metadata".into(), metadata);
        }
        Ok(())
    }

    /// The document a finished mixdown opened (once): the app shows it in the Waveform view.
    pub fn take_opened_mixdown(&mut self) -> Option<DocumentId> {
        self.opened_mixdown.take()
    }

    /// The track the last finished render made (once).
    pub fn take_rendered_track(&mut self) -> Option<TrackId> {
        self.rendered_track.take()
    }

    /// A finished render: its file on a new track under the one rendered (one undo step).
    fn place_render(&mut self, render: TrackRender, path: &Path) -> Result<TrackId> {
        let source = self
            .project
            .track(render.track)
            .cloned()
            .ok_or_else(|| anyhow!("the rendered track was deleted"))?;
        self.begin_edit("Render track", None);
        let made = (|| -> Result<TrackId> {
            let new = self
                .project
                .insert_track_after(TrackKind::Audio, Some(render.track));
            let clip = self.add_clip_from_file(new, path, render.start)?;
            // Its sends, so the mix is the same with the original switched off (a post-fader
            // send taps the same signal; a pre-fader one now taps it after the old fader).
            let sends: Vec<_> = source
                .sends
                .iter()
                .map(|s| crate::project::TrackSend {
                    id: self.project.next_id(),
                    ..*s
                })
                .collect();
            let t = self.project.track_mut(new).expect("just added");
            t.sends = sends;
            t.name = format!("{} (rendered)", source.name);
            t.color = source.color;
            t.output = source.output;
            if let Some(c) = t.clips.iter_mut().find(|c| c.id == clip) {
                c.name = t.name.clone();
            }
            if render.disable {
                if let Some(t) = self.project.track_mut(render.track) {
                    t.enabled = false;
                }
            }
            Ok(new)
        })();
        self.end_edit();
        self.graph_dirty = true;
        made
    }

    /// The running export's progress, or how the last one ended.
    pub fn export_status(&self) -> ExportStatus {
        match &self.export {
            Some(job) => ExportStatus::Running {
                progress: job.progress.load(),
                kind: self.export_kind,
                what: self.export_what.clone(),
            },
            None => self.export_outcome.clone(),
        }
    }

    /// Block until the running export ends (for headless tools; the app polls instead).
    pub fn wait_for_export(&mut self, timeout: std::time::Duration) -> Result<PathBuf> {
        let deadline = Instant::now() + timeout;
        loop {
            self.update();
            match self.export_status() {
                ExportStatus::Running { .. } if Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
                ExportStatus::Running { .. } => bail!("export timed out"),
                ExportStatus::Finished { path, .. } => return Ok(path),
                ExportStatus::Failed { error } => bail!("export failed: {error}"),
                ExportStatus::Idle => bail!("no export was started"),
            }
        }
    }

    /// Collect a finished export: keep its outcome for `export_status` and tell the user.
    fn poll_export(&mut self) {
        let Some(result) = self.export.as_mut().and_then(ExportJob::take_result) else {
            return;
        };
        self.export = None;
        // The file a render read back on its thread: into the media pool, so placing it reads
        // nothing here.
        let result = result.map(|done| {
            if let Some(audio) = done.audio {
                if let Ok(path) = std::path::absolute(&done.report.path) {
                    self.media.insert(path, audio);
                }
            }
            done.report
        });
        let render = self.rendering.is_some();
        let mixdown = self.mixdown_pending;
        // A mixdown to a new file: open it, for the Waveform view.
        if std::mem::take(&mut self.mixdown_pending) {
            if let Ok(report) = &result {
                match self.open_document(&report.path.clone()) {
                    Ok(doc) => self.opened_mixdown = Some(doc),
                    Err(e) => self.notices.push(format!("Mixdown: {e:#}")),
                }
            }
        }
        // A track's render: the whole project's graph again, and its file on a new track.
        let mut placed = None;
        if let Some(render) = self.rendering.take() {
            self.graph_dirty = true;
            let made = match &result {
                Ok(report) => self.place_render(render, &report.path).map(Some),
                Err(_) => Ok(None),
            };
            match made {
                Ok(Some(track)) => {
                    self.rendered_track = Some(track);
                    placed = Some(track);
                }
                Ok(None) => {}
                Err(e) => self.notices.push(format!("Render track: {e:#}")),
            }
        }
        self.export_outcome = match result {
            Ok(report) if render || mixdown => {
                let name = report
                    .path
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy();
                self.notices.push(if render {
                    format!("Rendered {name} onto a new track")
                } else {
                    format!("Mixed down to {name}, open in the Waveform view")
                });
                ExportStatus::Finished {
                    path: report.path,
                    loudness: report.loudness,
                    normalized: report.normalized,
                    track: placed,
                }
            }
            Ok(report) => {
                self.notices.push(export_notice(&report));
                ExportStatus::Finished {
                    path: report.path,
                    loudness: report.loudness,
                    normalized: report.normalized,
                    track: None,
                }
            }
            Err(e) => {
                self.notices.push(format!("Export: {e:#}"));
                ExportStatus::Failed {
                    error: format!("{e:#}"),
                }
            }
        };
    }

    pub fn cancel_export(&self) {
        if let Some(job) = &self.export {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Hold an export's (or a track's render's) render halfway while set: check runs catch the
    /// progress bar with it, and tests see the UI thread go on meanwhile. A cancel still ends it.
    pub fn hold_export(&self, hold: bool) {
        self.export_hold.store(hold, Ordering::Relaxed);
    }
}

/// "Exported mix.wav: -14.0 LUFS, -1.3 dBTP, +6.2 dB applied" (the file name: the report has
/// the path).
fn export_notice(report: &export::ExportReport) -> String {
    let l = &report.loudness;
    let level = |v: Option<f64>, unit: &str| v.map_or("–".into(), |v| format!("{v:.1} {unit}"));
    let name = report
        .path
        .file_name()
        .map_or(report.path.display().to_string(), |n| {
            n.to_string_lossy().into()
        });
    let mut text = format!(
        "Exported {}: {}, {}",
        name,
        level(l.integrated, "LUFS"),
        level(l.true_peak, "dBTP")
    );
    if let Some(n) = &report.normalized {
        text += &match n.gain_db {
            Some(db) if n.held_by_peak => {
                format!(
                    ", {db:+.1} dB applied, held by the {:.1} dBTP ceiling",
                    n.ceiling
                )
            }
            Some(db) => format!(", {db:+.1} dB applied"),
            None => ", nothing loud enough to normalise".into(),
        };
    }
    text
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(device) = self.device.take() {
            device.pause();
        }
        self.virtual_device = None;
        self.release_plugins();
    }
}

/// A track's render under way ([`Session::render_track`]).
struct TrackRender {
    track: TrackId,
    /// Where its audio starts on the timeline: the new clip goes there.
    start: u64,
    /// Switch the track off once its render is in.
    disable: bool,
}

/// One of a project's backups.
#[derive(Debug, Clone, PartialEq)]
pub struct Backup {
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
    pub size: u64,
}

/// Write `text` to `path` whole: into a file beside it, then renamed over it, so a failure
/// part-way never leaves half a file where the old one was.
/// `path`, with the project file's extension when it has none.
fn with_project_extension(path: &Path) -> PathBuf {
    if path.extension().is_none() {
        path.with_extension(crate::folder::EXTENSION)
    } else {
        path.to_path_buf()
    }
}

/// Whether the project as `json` differs from the file as `saved` (none: a backup opened in
/// its place, always unsaved). Read as values without the id counter, which undo never winds
/// back (ids are not reused), so an edit undone is the file again.
fn differs(json: &str, saved: Option<&str>) -> bool {
    fn content(json: &str) -> Option<serde_json::Value> {
        let mut value: serde_json::Value = serde_json::from_str(json).ok()?;
        value.as_object_mut()?.remove("next_id");
        Some(value)
    }
    let Some(saved) = saved.and_then(content) else {
        return true;
    };
    content(json) != Some(saved)
}

/// The backups in `folder`, newest first.
fn list_backups(folder: &Path) -> Vec<Backup> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut list: Vec<Backup> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(crate::folder::EXTENSION))
        })
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            Some(Backup {
                modified: meta.modified().ok()?,
                size: meta.len(),
                path,
            })
        })
        .collect();
    // Newest first. Windows stamps files only to its clock tick (about 15 ms), so two backups
    // in one second can share a time: then the later copy number ("(2)") is newer.
    let copy = |path: &Path| -> u32 {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default();
        stem.strip_suffix(')')
            .and_then(|s| s.rsplit_once(" ("))
            .and_then(|(_, n)| n.parse().ok())
            .unwrap_or(1)
    };
    list.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then(copy(&b.path).cmp(&copy(&a.path)))
    });
    list
}

/// Delete all but the newest `keep` backups in `folder`.
fn prune_backups(folder: &Path, keep: usize) {
    for old in list_backups(folder).into_iter().skip(keep) {
        let _ = std::fs::remove_file(old.path);
    }
}

pub(crate) fn write_whole(path: &Path, text: &str) -> Result<()> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".writing");
    let temp = PathBuf::from(temp);
    std::fs::write(&temp, text).with_context(|| format!("writing {}", temp.display()))?;
    std::fs::rename(&temp, path).with_context(|| format!("writing {}", path.display()))
}

fn sanitize(name: &str) -> String {
    let s: String = name
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || " -_".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    if s.trim().is_empty() {
        "Untitled".into()
    } else {
        s.trim().to_string()
    }
}

/// A tempo the project accepts, in quarter notes per minute.
pub fn check_tempo(tempo: f64) -> Result<()> {
    if !(20.0..=300.0).contains(&tempo) {
        bail!("tempo {tempo} is outside 20..300 BPM");
    }
    Ok(())
}

/// A time signature the project (and VST3) accepts.
pub fn check_time_signature(numerator: u32, denominator: u32) -> Result<()> {
    if !(1..=16).contains(&numerator) || ![2, 4, 8, 16].contains(&denominator) {
        bail!("{numerator}/{denominator} is not a supported time signature");
    }
    Ok(())
}

/// Where a new insert goes in a chain of `len`: at `position` (clamped to the end), or without
/// one first for an instrument (MIDI reaches the chain's first plug-in) and last for an effect.
fn insert_index(len: usize, is_instrument: bool, position: Option<usize>) -> usize {
    match position {
        Some(at) => at.min(len),
        None if is_instrument => 0,
        None => len,
    }
}

/// Write a document's showing audio to `path` in `format`.
fn write_document(doc: &AudioDocument, path: &Path, format: wav::Format) -> Result<()> {
    let audio = wav::Audio {
        sample_rate: doc.audio.sample_rate,
        channels: doc.audio.channels.clone(),
    };
    wav::write_with_format(path, &audio, format)
        .with_context(|| format!("saving {}", path.display()))?;
    // Its tags go with it, as chunks after the audio.
    if !doc.metadata.is_empty() {
        crate::metadata::tag_wav(path, &doc.metadata)
            .with_context(|| format!("tagging {}", path.display()))?;
    }
    Ok(())
}

/// Whether two paths are the same file: compared canonicalised (takes store paths relative to
/// the project, and Windows ignores case), or as written when either doesn't exist.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Rack insert ids start here, far beyond any project's ids.
const RACK_IDS: InsertId = 0xF000_0000;

/// What an Apply's thread hands back: the render (None: cancelled), the blocks plug-ins
/// skipped, and its chain, to be dropped on the main thread.
type ApplyDone = (Option<Vec<Vec<f32>>>, u64, rack::RackChain);

/// An Apply rendering on its own thread.
struct ApplyJob {
    document: DocumentId,
    range: (u64, u64),
    /// The file as it was when Apply began: a result for an edited file is discarded.
    source: Arc<AudioData>,
    name: String,
    progress: Arc<AtomicF32>,
    cancel: Arc<AtomicBool>,
    done: std::sync::mpsc::Receiver<ApplyDone>,
    /// Asked of a synced rack (21.2): once it lands, the track's inserts it printed are
    /// switched off.
    turn_off: Option<(TrackId, Vec<InsertId>)>,
}

/// An audio file open in the Waveform view, by its id.
pub type DocumentId = u32;

/// An audio file open in the Waveform view: its samples at its own rate (what an edit changes
/// and a save writes), and a copy at the engine's rate for playback, made when first needed.
pub struct AudioDocument {
    pub id: DocumentId,
    pub path: PathBuf,
    pub audio: Arc<AudioData>,
    /// Its tags (title, artist, cover…), as the showing history step has them: written by
    /// Save and Export.
    pub metadata: Arc<crate::metadata::Metadata>,
    playback: Option<Arc<AudioData>>,
    /// Its WAV format, written back on Save; `None` for FLAC, MP3 or AIFF (Save As only).
    pub format: Option<wav::Format>,
    /// The file's own history: each step's name and the whole file (and its tags) after it
    /// (copy on write; the first is the file as opened). `cursor` is the step showing; `saved`
    /// the one on disk.
    history: Vec<Step>,
    cursor: usize,
    saved: Option<usize>,
    /// What the Diagnostics panel last found in it, for the audio it was found in.
    problems: Option<ProblemList>,
    /// The next problem's id: never reused while the file is open, so a stale id is refused
    /// by name instead of repairing another problem.
    next_problem: u32,
    /// The clip it was last opened from (a double-click in the Multitrack, `open_audio_editor`
    /// with a clip) and that clip's track: what the effects rack can sync with (dev plan 21.2).
    /// `None` for a file opened on its own.
    pub source: Option<(TrackId, ClipId)>,
}

/// The Diagnostics' list for a file: valid while the file shows the audio it was found in
/// (`source`); any other edit, undo or redo of the file's audio leaves it behind.
struct ProblemList {
    kind: crate::diagnostics::ProblemKind,
    source: Arc<AudioData>,
    found: Vec<(u32, crate::diagnostics::Found)>,
}

/// A step of a file's history: its name, and the file and its tags after it. A tags-only edit
/// shares the audio of the step before.
struct Step {
    name: String,
    audio: Arc<AudioData>,
    metadata: Arc<crate::metadata::Metadata>,
}

/// How many bytes of audio a file's history keeps before dropping its oldest steps: each step
/// holds the whole file (TD94).
pub const DOCUMENT_HISTORY_BYTES: usize = 1 << 30;

impl AudioDocument {
    /// Edits since the file was last saved (or opened).
    pub fn is_dirty(&self) -> bool {
        self.saved != Some(self.cursor)
    }

    /// The history's step names, oldest first, the showing one, and the saved one.
    pub fn history(&self) -> (Vec<String>, usize, Option<usize>) {
        (
            self.history.iter().map(|h| h.name.clone()).collect(),
            self.cursor,
            self.saved,
        )
    }

    /// Show history step `at`.
    fn show_step(&mut self, at: usize) {
        self.cursor = at;
        if !Arc::ptr_eq(&self.audio, &self.history[at].audio) {
            self.playback = None;
        }
        self.audio = self.history[at].audio.clone();
        self.metadata = self.history[at].metadata.clone();
    }

    /// Add an edit's result after the showing step (dropping any steps undone), then drop the
    /// oldest steps while the history holds more than [`DOCUMENT_HISTORY_BYTES`].
    fn push(&mut self, name: String, audio: AudioData) {
        let metadata = self.metadata.clone();
        self.push_step(Step {
            name,
            audio: Arc::new(audio),
            metadata,
        });
    }

    /// A tags-only edit: the same audio, new tags.
    fn push_metadata(&mut self, name: String, metadata: crate::metadata::Metadata) {
        let audio = self.audio.clone();
        self.push_step(Step {
            name,
            audio,
            metadata: Arc::new(metadata),
        });
    }

    fn push_step(&mut self, step: Step) {
        self.history.truncate(self.cursor + 1);
        if self.saved.is_some_and(|s| s > self.cursor) {
            self.saved = None;
        }
        self.history.push(step);
        self.show_step(self.history.len() - 1);
        // Each file once: a tags-only step shares the step before's.
        let bytes = |history: &[Step]| {
            let mut seen = std::collections::HashSet::new();
            history
                .iter()
                .filter(|h| seen.insert(Arc::as_ptr(&h.audio)))
                .map(|h| h.audio.channels.len() * h.audio.len() * 4)
                .sum::<usize>()
        };
        while self.history.len() > 1 && bytes(&self.history) > DOCUMENT_HISTORY_BYTES {
            self.history.remove(0);
            self.cursor -= 1;
            self.saved = self.saved.and_then(|s| s.checked_sub(1));
        }
    }

    /// The audio at `rate`: the file itself when it is at that rate, else a converted copy,
    /// kept until the rate changes.
    fn playback_at(&mut self, rate: u32) -> Arc<AudioData> {
        if self.audio.sample_rate == rate {
            return self.audio.clone();
        }
        if let Some(copy) = self.playback.as_ref().filter(|c| c.sample_rate == rate) {
            return copy.clone();
        }
        let channels = self
            .audio
            .channels
            .iter()
            .map(|c| media::resample(c, self.audio.sample_rate, rate))
            .collect();
        let copy = Arc::new(AudioData::new(rate, channels));
        self.playback = Some(copy.clone());
        copy
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bar of 4/4 counted in to beat 0: 4 from the latency's lead-in through the first
    /// click, then one less at each click; a record point off the beat counts on the clicks.
    #[test]
    fn a_count_in_counts_its_clicks() {
        let left = |heard: f64| clicks_left(-4.0, heard, 0.0);
        assert_eq!(left(-4.4), 4, "the lead-in before the first click");
        assert_eq!(left(-4.0), 4);
        assert_eq!(left(-4.0 - 1e-9), 4, "a hair before a click is the click");
        assert_eq!(left(-3.5), 4);
        assert_eq!(left(-3.0), 3);
        assert_eq!(left(-1.0), 1);
        assert_eq!(left(-0.01), 1);
        // Two bars.
        assert_eq!(clicks_left(-8.0, -8.0, 0.0), 8);
        // From beat 0.4: clicks on -3, -2, -1 and 0.
        let off = |heard: f64| clicks_left(-3.6, heard, 0.4);
        assert_eq!(off(-3.6), 4);
        assert_eq!(off(-3.0), 4, "the first click");
        assert_eq!(off(-2.0), 3);
        assert_eq!(off(0.0), 1);
        assert_eq!(off(0.3), 1);
        // Behind a latency longer than the 0.6 beat to the first click: a beat of the lead-in
        // (-4) isn't the count-in's, so it doesn't count.
        assert_eq!(off(-4.02), 4);
        assert_eq!(off(-4.0), 4);
        assert_eq!(off(-3.5), 4);
    }

    #[test]
    fn a_new_insert_goes_where_asked_or_by_its_kind() {
        assert_eq!(insert_index(3, false, None), 3);
        assert_eq!(insert_index(3, true, None), 0);
        assert_eq!(insert_index(3, false, Some(1)), 1);
        // A position wins over the instrument rule, and past the end appends.
        assert_eq!(insert_index(3, true, Some(2)), 2);
        assert_eq!(insert_index(3, false, Some(9)), 3);
        assert_eq!(insert_index(0, false, Some(4)), 0);
    }

    #[test]
    fn a_virtual_device_runs_the_engine_beside_the_main_thread() {
        let mut s = Session::new(AudioSettings {
            output_device: Some("<none>".into()),
            sample_rate: Some(48_000),
            threads: Some(0),
            ..Default::default()
        });
        let track = s.add_track(TrackKind::Audio, None);
        let amplify = s
            .add_insert(
                InsertTarget::Track(track),
                &crate::native::path_of(&crate::native::amplify::KIND),
            )
            .unwrap();
        assert!(s.start_virtual_device(0).is_err());
        assert!(s.start_virtual_device(MAX_BLOCK + 1).is_err());
        s.update();
        s.start_virtual_device(256).unwrap();
        s.play();
        let started = Instant::now();
        let mut set = 0;
        // Frames and parameter sets while the engine runs on its own thread, as live.
        while s.position() < 9_600 && started.elapsed() < Duration::from_secs(5) {
            s.set_parameter(amplify, 0, if set % 2 == 0 { 0.4 } else { 0.6 })
                .unwrap();
            set += 1;
            s.update();
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(s.position() >= 9_600, "the engine ran: {}", s.position());
        assert_eq!(s.insert_misses(amplify), Some(0));
        // A graph sent now is taken by the clock's thread, not applied here.
        s.mark_dirty();
        s.update();
        s.flush_pending();
        assert_eq!(s.graphs_in_flight, 0);
        s.stop_virtual_device();
    }
}
