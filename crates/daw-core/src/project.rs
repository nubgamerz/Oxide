//! The project: plain, serializable data describing tracks, routing, inserts and clips.
//!
//! Nothing here touches audio or plugins. [`crate::session::Session`] owns a `Project` and
//! turns it into a runnable [`crate::graph::Graph`] whenever its structure changes.

use crate::automation::{AutomationLane, AutomationMode, AutomationTarget};
use crate::midi_clip::{ControllerEvent, EventId, MidiClip, Note, NoteId};
use crate::tempo::{quarter_note_samples, TempoMap};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub type TrackId = u32;
pub type ClipId = u32;
pub type InsertId = u32;

/// The project file format. 1: tempo counts quarter notes (1.8); before that it counted beats
/// of the time signature's lower number.
pub const PROJECT_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    /// See [`PROJECT_VERSION`]; files from before versions read as 0.
    #[serde(default)]
    pub version: u32,
    pub name: String,
    /// The opening tempo, in quarter notes per minute.
    pub tempo: f64,
    /// The opening time signature.
    pub time_signature: (u32, u32),
    /// Tempo and time-signature changes after the opening, in musical time.
    #[serde(default)]
    pub tempo_changes: Vec<TempoChange>,
    /// Sample rate the project's clips are stored at. Audio is converted to this on import.
    pub sample_rate: u32,
    /// Tracks in display order. Folder membership is given by [`Track::parent`]; children
    /// always follow their folder in this list.
    pub tracks: Vec<Track>,
    pub master: Master,
    /// While enabled, playback wraps from its end back to its start.
    #[serde(default)]
    pub loop_region: Region,
    /// While enabled, recording happens only inside it.
    #[serde(default)]
    pub punch: Region,
    /// Named places in the song, in time order: they sit in musical time, so they stay on
    /// their bars when the tempo changes. Numbered 1, 2, 3… in that order.
    #[serde(default)]
    pub markers: Vec<Marker>,
    #[serde(default)]
    pub metronome: MetronomeSettings,
    /// When playback starts, loops or jumps into the middle of a MIDI note, strike it.
    #[serde(default = "chase_by_default")]
    pub chase_notes: bool,
    /// The integrated loudness the mix aims at, in LUFS: -14 for streaming, -23 for broadcast
    /// (EBU R 128). The loudness meter shows the master against it.
    #[serde(default = "streaming_loudness")]
    pub loudness_target: f64,
    #[serde(default)]
    pub midi_record: MidiRecordSettings,
    /// The song's grid (17.4): the division the Multitrack draws and snaps to, which the
    /// piano roll and the Waveform view's bars and beats show too. Files from before it open
    /// as 1/8.
    #[serde(default)]
    pub grid: crate::grid::Grid,
    /// The files open in the Waveform view when the project was written, reopened with it.
    /// Only in the file: the session fills it as it writes one (a save, a backup) and takes it
    /// out as it opens one, so in memory it is always empty, and it is never part of an undo
    /// step, a template or the unsaved-changes check (view state, like track heights, but not
    /// a change of the project).
    #[serde(default, skip_serializing_if = "OpenFiles::is_empty")]
    pub open_files: OpenFiles,
    next_id: u32,
}

/// The Waveform view's open files as a project file remembers them ([`Project::open_files`]).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenFiles {
    /// In the order they were opened (their tabs').
    pub files: Vec<OpenFile>,
    /// The one showing in the Waveform view, an index into `files`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shown: Option<usize>,
    /// The Waveform view was the view in front.
    pub in_front: bool,
}

impl OpenFiles {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// One file open in the Waveform view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OpenFile {
    /// Relative to the project's folder when inside it (as clips' files are), else absolute.
    pub path: PathBuf,
    /// The clip it was opened from (a double-click), so the effects rack can sync with its
    /// track again; dropped on reopening if the clip is gone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clip: Option<ClipId>,
}

fn chase_by_default() -> bool {
    true
}

/// Loudness targets the app offers by name, in LUFS.
pub const LOUDNESS_PRESETS: [(&str, f64); 2] =
    [("Streaming", -14.0), ("Broadcast (EBU R 128)", -23.0)];

/// The range a loudness target may take, in LUFS.
pub const LOUDNESS_TARGET_RANGE: (f64, f64) = (-40.0, -5.0);

fn streaming_loudness() -> f64 {
    LOUDNESS_PRESETS[0].1
}

/// How recorded MIDI lands on an instrument track.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct MidiRecordSettings {
    /// Delete what the track already has in the recorded range first. Off: merge (overdub).
    pub replace: bool,
    /// Snap recorded note starts to this grid division ("1/16", "1/8t", "beat"…); `None` keeps
    /// them as played.
    pub quantize: Option<String>,
}

impl Default for Project {
    fn default() -> Self {
        Self {
            version: PROJECT_VERSION,
            name: "Untitled".into(),
            tempo: 120.0,
            time_signature: (4, 4),
            tempo_changes: Vec::new(),
            sample_rate: 48_000,
            tracks: Vec::new(),
            master: Master::default(),
            loop_region: Region::default(),
            punch: Region::default(),
            markers: Vec::new(),
            metronome: MetronomeSettings::default(),
            chase_notes: true,
            loudness_target: streaming_loudness(),
            midi_record: MidiRecordSettings::default(),
            grid: crate::grid::Grid::default(),
            open_files: OpenFiles::default(),
            next_id: 1,
        }
    }
}

/// A tempo and/or time-signature change, placed in musical time so it stays on its bar when
/// an earlier tempo changes. Bars and beats count from 0 here (the UI and MCP show them from
/// 1). A time-signature change sits on the first beat of its bar; a tempo change on any beat.
/// Changes are instant: there are no ramps.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TempoChange {
    pub bar: u32,
    #[serde(default)]
    pub beat: u32,
    /// Quarter notes per minute from here on.
    #[serde(default)]
    pub tempo: Option<f64>,
    #[serde(default)]
    pub time_signature: Option<(u32, u32)>,
}

impl Project {
    /// Take the view state (track heights, which tracks show their lanes) from `other`, track
    /// by track. Undo compares and restores projects with this applied, so view changes are
    /// never undo steps and undo never changes the view.
    pub fn copy_view_from(&mut self, other: &Project) {
        for track in &mut self.tracks {
            if let Some(theirs) = other.track(track.id) {
                track.height = theirs.height;
                track.show_automation = theirs.show_automation;
            }
        }
        self.master.height = other.master.height;
        self.master.show_automation = other.master.show_automation;
    }

    /// Never hand out an id `other` has already used: restoring an older snapshot must not
    /// reuse ids that live state (plug-ins, mixer slots, the redo history) still knows.
    pub fn keep_ids_beyond(&mut self, other: &Project) {
        self.next_id = self.next_id.max(other.next_id);
    }

    /// Bring a project loaded from an older file up to [`PROJECT_VERSION`].
    pub fn upgrade(&mut self) {
        if self.version < 1 {
            // Tempo counted beats of the lower number: 120 in 6/8 meant 120 eighth notes a
            // minute, which is 60 quarter notes. Same speed, new unit.
            self.tempo *= 4.0 / f64::from(self.time_signature.1.max(1));
        }
        self.version = PROJECT_VERSION;
    }
}

/// A range of the timeline that can be switched on and off (the loop, the punch range), in
/// samples.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct Region {
    pub start: u64,
    pub end: u64,
    pub enabled: bool,
}

/// The click. It plays on the master's output pair, after the master, and never in exports.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct MetronomeSettings {
    pub enabled: bool,
    /// Click level in dB (−60 to +6).
    pub volume_db: f32,
    /// Click only while recording.
    pub only_while_recording: bool,
    /// Bars of click before recording starts (0 = off, up to 2). Clicks even with the
    /// metronome off.
    pub count_in_bars: u32,
}

impl Default for MetronomeSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            volume_db: -6.0,
            only_while_recording: false,
            count_in_bars: 0,
        }
    }
}

impl Region {
    /// The range, if it is switched on and not empty.
    pub fn active(&self) -> Option<(u64, u64)> {
        (self.enabled && self.end > self.start).then_some((self.start, self.end))
    }
}

/// Where [`Project::move_track_to`] puts a track (with its sub-tracks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// Just above this track, beside it (in its folder, or at the top level).
    Before(TrackId),
    /// Just below this track and its sub-tracks, beside it.
    After(TrackId),
    /// Inside this folder, as its `index`th sub-track (past the last: at the end).
    Into { folder: TrackId, index: usize },
    /// At the end of the top level.
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum TrackKind {
    /// Records and plays audio clips.
    Audio,
    /// Fed by MIDI; its first insert is normally an instrument plugin.
    Instrument,
    /// A submix folder: holds sub-tracks and acts as their bus.
    Folder,
    /// A return track (for reverbs, delays, parallel compression): fed only by other tracks'
    /// outputs and sends. It holds no clips, input or sub-tracks.
    Bus,
}

impl TrackKind {
    /// Folders and buses mix other tracks: they can take a main output or a send.
    pub fn mixes_tracks(self) -> bool {
        matches!(self, Self::Folder | Self::Bus)
    }

    /// Audio and instrument tracks hold clips.
    pub fn holds_clips(self) -> bool {
        matches!(self, Self::Audio | Self::Instrument)
    }
}

pub type SendId = u32;

/// A copy of a track's signal into a folder or bus, alongside its main output.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct TrackSend {
    /// Unique in the project, like track and clip ids.
    pub id: SendId,
    /// The folder or bus it feeds.
    pub target: TrackId,
    pub gain_db: f32,
    /// Taken before the track's fader and pan, so the fader doesn't change it.
    pub pre_fader: bool,
    pub mute: bool,
}

/// What mute and solo leave of each track and send ([`Project::audibility`]).
#[derive(Debug, Default, PartialEq)]
pub struct Audibility {
    /// Silent altogether: main output and sends.
    pub muted: std::collections::HashSet<TrackId>,
    /// Main output silent, sends as `muted_sends` says: heard only through a soloed bus.
    pub main_muted: std::collections::HashSet<TrackId>,
    /// Sends silenced by solo, on tracks heard only through other sends.
    pub muted_sends: std::collections::HashSet<SendId>,
}

/// Send levels are clamped to this range, in dB, like faders.
pub const SEND_GAIN_RANGE: (f32, f32) = (-60.0, 12.0);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum TrackInput {
    None,
    /// Hardware input channel, or the pair starting at it.
    Audio {
        channel: u16,
        stereo: bool,
    },
    /// A MIDI input port by name (`None` = all ports), optionally filtered to one channel (0-15).
    Midi {
        port: Option<String>,
        channel: Option<u8>,
    },
    /// Extra output `output` (from 1: a multi-out instrument's kick, snare...) of insert
    /// `insert` on another track, heard at unity (dev plan 14.4). The plug-in turns the output
    /// on while some track listens to it.
    Plugin {
        insert: InsertId,
        output: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum TrackOutput {
    /// The folder the track sits in, or the master when it has none.
    Parent,
    Master,
    /// Another folder track, used as a bus.
    Track(TrackId),
    /// Straight to the hardware output pair starting at this channel, bypassing the master.
    Hardware {
        channel: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum Monitor {
    /// Hear the input while the track is armed.
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Mixer {
    pub gain_db: f32,
    /// -1 (left) to 1 (right).
    pub pan: f32,
    pub mute: bool,
    pub solo: bool,
}

impl Default for Mixer {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            pan: 0.0,
            mute: false,
            solo: false,
        }
    }
}

/// A fader's range in dB (tracks and the master): its bottom is as low as it goes, so "-inf"
/// typed into a volume lands there.
pub const GAIN_RANGE: (f32, f32) = (-60.0, 12.0);

impl Mixer {
    /// Set the volume, clamped to [`GAIN_RANGE`]. The caller refuses a value that isn't a
    /// number.
    pub fn set_gain_db(&mut self, db: f32) {
        self.gain_db = db.clamp(GAIN_RANGE.0, GAIN_RANGE.1);
    }

    /// Set the pan, clamped to -1 (left) to 1 (right).
    pub fn set_pan(&mut self, pan: f32) {
        self.pan = pan.clamp(-1.0, 1.0);
    }
}

/// A track's name as it is kept: trimmed, and never empty (dev plan 22.1). Renaming a track
/// anywhere (its header, the inspector, `set_track`) goes through this.
pub fn track_name(name: &str) -> Result<&str, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        Err("a track name cannot be empty")
    } else {
        Ok(name)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub color: [u8; 3],
    pub parent: Option<TrackId>,
    pub input: TrackInput,
    pub output: TrackOutput,
    pub mixer: Mixer,
    pub arm: bool,
    pub monitor: Monitor,
    pub inserts: Vec<InsertSlot>,
    pub clips: Vec<Clip>,
    /// Instrument tracks only. They may overlap.
    #[serde(default)]
    pub midi_clips: Vec<MidiClip>,
    /// Copies of the signal into folders and buses, summed in this order.
    #[serde(default)]
    pub sends: Vec<TrackSend>,
    /// One lane per target at most.
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
    /// What moving its controls while playing does to its lanes.
    #[serde(default)]
    pub automation_mode: AutomationMode,
    /// Folder only: hide the sub-tracks in the arrangement.
    pub collapsed: bool,
    /// Row height in the arrangement, in points; `None` is the default for the kind. View
    /// state: saved with the project, but never an undo step.
    #[serde(default)]
    pub height: Option<f32>,
    /// Show the automation lanes under the track in the arrangement. View state, like
    /// `height`.
    #[serde(default)]
    pub show_automation: bool,
    /// Off: the track (and whatever reaches the mix only through it) is left out of the
    /// graph, so its plug-ins cost no CPU, unlike a mute. Its settings are all kept.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

fn enabled_by_default() -> bool {
    true
}

/// Track rows are at least this tall, and at most [`MAX_TRACK_HEIGHT`].
pub const MIN_TRACK_HEIGHT: f32 = 32.0;
pub const MAX_TRACK_HEIGHT: f32 = 400.0;

impl Track {
    /// The row height in the arrangement.
    pub fn height(&self) -> f32 {
        let default = if self.kind.holds_clips() { 64.0 } else { 44.0 };
        self.height
            .unwrap_or(default)
            .clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "InsertSlotFile")]
pub struct InsertSlot {
    pub id: InsertId,
    pub name: String,
    pub path: PathBuf,
    pub bypass: bool,
    /// Plugin state captured on save, restored on load. Shared, so the undo history's
    /// snapshots of the project do not each copy a state that can be tens of MB.
    #[serde(default, with = "base64_bytes")]
    pub state: Option<Arc<Vec<u8>>>,
}

/// An insert slot as a file holds it: a built-in effect saved under an older id or name
/// (`native::RENAMED`) is read as the effect it became, so it saves with today's id.
#[derive(Deserialize)]
struct InsertSlotFile {
    id: InsertId,
    name: String,
    path: PathBuf,
    bypass: bool,
    #[serde(default, with = "base64_bytes")]
    state: Option<Arc<Vec<u8>>>,
}

impl From<InsertSlotFile> for InsertSlot {
    fn from(f: InsertSlotFile) -> Self {
        let name = crate::native::current_name(&f.path, f.name);
        Self {
            id: f.id,
            name,
            path: crate::native::current_path(f.path),
            bypass: f.bypass,
            state: f.state,
        }
    }
}

/// One recording a clip can play: a file, and where in it the clip begins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Take {
    /// Audio file, relative to the project folder when it lives inside it.
    pub file: PathBuf,
    /// Where in the clip's audio as heard the clip begins: the file, or a stretched clip's
    /// render of it (see [`Clip`]). Past the end the take is silent (a pass stopped part-way).
    pub offset: u64,
    pub name: String,
}

/// A stretch of the timeline playing one of its takes. Loop recording gives a clip one take
/// per pass; every take shares the clip's start, length and gain, so edits (split, move,
/// quantize) apply to all of them, and splitting is basic comping: each piece picks its take.
///
/// A clip can be time-stretched (`stretch`) and transposed (`pitch`). It then plays its take's
/// whole file rendered at that ratio and pitch (`stretch::Cache`), and every take's `offset`
/// counts samples of that rendered audio, the audio as heard. So edits that work in timeline
/// samples (split, quantize, move) need no idea of the stretch, and the pieces of a split
/// clip read one render and join seamlessly. Only changing the ratio rescales the offsets and
/// the length ([`Clip::set_stretch`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "ClipFile")]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    /// Timeline position of the clip's first sample.
    pub start: u64,
    pub length: u64,
    pub gain_db: f32,
    /// Never empty.
    pub takes: Vec<Take>,
    /// The take that plays.
    pub active: usize,
    /// Time-stretch ratio: 2.0 plays twice as long, at the same pitch. 1.0 is untouched.
    pub stretch: f64,
    /// Transposition in semitones, at the same length. 0.0 is untouched.
    pub pitch: f64,
    /// A stretch keeps the attacks: each plays unstretched and on time, and the audio between
    /// them takes up the stretch (`stretch::protect_transients`). On for clips made now; off for
    /// clips saved before it existed, so they keep their sound.
    pub transients: bool,
    /// Set when the clip follows the project's tempo: then its place is musical and its
    /// samples (start, length, offsets) are laid out from it, and `stretch` is unused.
    pub follow: Option<Follow>,
    /// Fades at the clip's ends, in timeline samples. Together they never exceed the clip
    /// ([`Clip::set_fades`]). Two overlapping clips on a track sum, so a crossfade is the
    /// earlier one's fade-out over the later one's fade-in.
    pub fade_in: Fade,
    pub fade_out: Fade,
}

/// A fade at one end of a clip.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Fade {
    /// Timeline samples; 0 is no fade.
    pub length: u64,
    pub shape: FadeShape,
}

/// How a fade's gain rises from silence to full.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FadeShape {
    /// Gain in a straight line: a crossfade between two pieces of the same audio sums to it
    /// exactly.
    Linear,
    /// Constant power (a sine quarter): a crossfade between different audio (takes, other
    /// recordings) keeps its loudness.
    #[default]
    EqualPower,
}

impl FadeShape {
    /// The gain at `t` (0 to 1) of the way through a fade-in; a fade-out is `gain(1 - t)`.
    pub fn gain(self, t: f32) -> f32 {
        match self {
            FadeShape::Linear => t,
            FadeShape::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
        }
    }
}

/// Where a clip that follows the tempo sits, in music: the truth its samples are laid out
/// from whenever the tempo map changes, and that edits of its samples update. Quarter notes
/// are kept at full precision, so changing the tempo back and forth never drifts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Follow {
    /// The recording's tempo, in quarter notes per minute.
    pub tempo: f64,
    /// Where the clip starts and ends, in quarter notes from the timeline start.
    pub start: f64,
    pub end: f64,
    /// Per take: how far into its file the clip starts, in the file's own quarter notes (at
    /// `tempo`). Fewer than there are takes means takes were added since: they're read from
    /// the samples.
    pub offsets: Vec<f64>,
}

/// A clip's samples: start, length and each take's offset.
pub type Layout = (u64, u64, Vec<u64>);

impl Clip {
    /// A clip with one take.
    pub fn new(
        id: ClipId,
        name: String,
        file: PathBuf,
        start: u64,
        offset: u64,
        length: u64,
    ) -> Self {
        Self {
            id,
            takes: vec![Take {
                file,
                offset,
                name: name.clone(),
            }],
            name,
            start,
            length,
            gain_db: 0.0,
            active: 0,
            stretch: 1.0,
            pitch: 0.0,
            transients: true,
            follow: None,
            fade_in: Fade::default(),
            fade_out: Fade::default(),
        }
    }

    /// Set both fades, shortening them so together they fit the clip (the fade-in keeps what
    /// it asked for first).
    pub fn set_fades(&mut self, fade_in: Fade, fade_out: Fade) {
        let fade_in_length = fade_in.length.min(self.length);
        self.fade_in = Fade {
            length: fade_in_length,
            ..fade_in
        };
        self.fade_out = Fade {
            length: fade_out.length.min(self.length - fade_in_length),
            ..fade_out
        };
    }

    /// The clip's samples as they are.
    pub fn layout(&self) -> Layout {
        (
            self.start,
            self.length,
            self.takes.iter().map(|t| t.offset).collect(),
        )
    }

    /// The samples a following clip should have under `map`, or `None` if it doesn't follow
    /// or its musical place doesn't cover every take. Each position rounds once, so laying
    /// out what [`Self::follow_samples`] read gives back the same samples.
    pub fn laid_out(&self, map: &TempoMap) -> Option<Layout> {
        let follow = self.follow.as_ref()?;
        if follow.offsets.len() != self.takes.len() {
            return None;
        }
        let at = |qn: f64| map.sample_at_quarter_notes(qn).round() as i64;
        let start = at(follow.start).max(0);
        let length = (at(follow.end) - start).max(1) as u64;
        let offsets = follow
            .offsets
            .iter()
            .map(|o| (start - at(follow.start - o)).max(0) as u64)
            .collect();
        Some((start as u64, length, offsets))
    }

    /// Read a following clip's musical place from its samples under `map` (after an edit
    /// moved, split or trimmed it).
    pub fn follow_samples(&mut self, map: &TempoMap) {
        let qn = |sample: i64| map.quarter_notes_at(sample as f64);
        let (start, end) = (self.start as i64, self.end() as i64);
        let offsets = self
            .takes
            .iter()
            .map(|t| qn(start) - qn(start - t.offset as i64))
            .collect();
        if let Some(follow) = &mut self.follow {
            follow.start = qn(start);
            follow.end = qn(end);
            follow.offsets = offsets;
        }
    }

    /// Start following the tempo, recorded at `tempo` quarter notes per minute: the clip keeps
    /// its start and the stretch of its file it plays, which now lasts as long as the tempo
    /// map says. `None` stops following: the clip keeps where it is and how long, and plays
    /// at the ratio the map gave its start.
    pub fn set_follow(&mut self, tempo: Option<f64>, map: &TempoMap) {
        match tempo {
            Some(tempo) => {
                // The file frame at the clip's start, and how many frames the clip plays.
                let per_quarter = quarter_note_samples(map.sample_rate, tempo);
                let (from, frames) = match &self.follow {
                    // Already following at another tempo: the same file frames, counted in
                    // quarter notes of the new one.
                    Some(f) => (
                        f.offsets.iter().map(|o| o * tempo / f.tempo).collect(),
                        (f.end - f.start) * per_quarter * tempo / f.tempo,
                    ),
                    None => (
                        self.takes
                            .iter()
                            .map(|t| t.offset as f64 / self.stretch / per_quarter)
                            .collect::<Vec<f64>>(),
                        self.length as f64 / self.stretch,
                    ),
                };
                let start = map.quarter_notes_at(self.start as f64);
                self.follow = Some(Follow {
                    tempo,
                    start,
                    end: start + frames / per_quarter,
                    offsets: from,
                });
                if let Some(layout) = self.laid_out(map) {
                    self.apply(layout);
                }
                self.stretch = 1.0;
            }
            None => {
                if let Some(f) = self.follow.take() {
                    let per_quarter = quarter_note_samples(map.sample_rate, f.tempo);
                    // The ratio at the start: the map's samples per quarter note there over
                    // the file's.
                    let s = map.sample_at_quarter_notes(f.start);
                    let local = map.sample_at_quarter_notes(f.start + 1.0) - s;
                    let ratio = (local / per_quarter)
                        .clamp(crate::stretch::RATIO_RANGE.0, crate::stretch::RATIO_RANGE.1);
                    self.stretch = ratio;
                    for (take, o) in self.takes.iter_mut().zip(&f.offsets) {
                        take.offset = (o * per_quarter * ratio).round() as u64;
                    }
                }
            }
        }
    }

    fn apply(&mut self, (start, length, offsets): Layout) {
        self.start = start;
        self.length = length;
        for (take, offset) in self.takes.iter_mut().zip(offsets) {
            take.offset = offset;
        }
    }

    /// Lay a following clip out under `map` if `map` is new, or read its place from its
    /// samples if an edit changed them under the same map. When
    /// one edit changes both the map and the clip's samples, the map wins (the clip is laid
    /// out from its musical place).
    pub fn sync_follow(&mut self, map: &TempoMap, map_changed: bool) {
        if self.follow.is_none() {
            return;
        }
        match self.laid_out(map) {
            Some(laid) if laid == self.layout() => {}
            Some(laid) if map_changed => self.apply(laid),
            _ => self.follow_samples(map),
        }
    }

    /// The render a following clip's playing take needs under `map`: `None` if it doesn't
    /// follow. Spans of `(file frames, timeline frames)` from the file's first frame, which
    /// sits at the timeline sample `start - offset`; worked out from those whole samples, so
    /// the pieces of a split clip get the same warp exactly.
    pub fn follow_warp(&self, map: &TempoMap, file_frames: usize) -> Option<Vec<(usize, usize)>> {
        let follow = self.follow.as_ref()?;
        let file_start = self.start as f64 - self.offset() as f64;
        let anchor = map.quarter_notes_at(file_start);
        let per_quarter = quarter_note_samples(map.sample_rate, follow.tempo);
        let end = anchor + file_frames as f64 / per_quarter;
        let out = |qn: f64| {
            (map.sample_at_quarter_notes(qn) - file_start)
                .round()
                .max(0.0) as usize
        };
        let mut points = vec![(0usize, 0usize)];
        for segment in map.segments() {
            let b = segment.quarter_notes;
            if b > anchor && b < end {
                points.push((((b - anchor) * per_quarter).round() as usize, out(b)));
            }
        }
        points.push((file_frames, out(end)));
        // A tempo change within half a frame of the file's start or end rounds onto it: keep
        // its output with the neighbour rather than lose it.
        points.dedup_by(|later, earlier| {
            let same = later.0 <= earlier.0;
            if same {
                earlier.1 = earlier.1.max(later.1);
            }
            same
        });
        Some(
            points
                .windows(2)
                .map(|w| (w[1].0 - w[0].0, w[1].1 - w[0].1))
                .collect(),
        )
    }

    /// Where in its file the playing take is at the clip's start, in file frames: what the
    /// clip plays from while its render isn't there.
    pub fn file_offset(&self, rate: u32) -> u64 {
        match &self.follow {
            Some(f) => (f.offsets.get(self.active).copied().unwrap_or(0.0)
                * quarter_note_samples(rate, f.tempo))
            .round() as u64,
            None => (self.offset() as f64 / self.stretch).round() as u64,
        }
    }

    /// Whether the clip plays rendered audio rather than its file as it is.
    pub fn is_stretched(&self) -> bool {
        self.stretch != 1.0 || self.pitch != 0.0
    }

    /// Change the stretch ratio, keeping the start: the length and every take's offset scale
    /// with it, so the clip covers the same part of its audio. Each change rounds to whole
    /// samples, so set a dragged ratio from where the gesture started, not frame by frame.
    pub fn set_stretch(&mut self, ratio: f64) {
        let factor = ratio / self.stretch;
        let scale = |v: u64| (v as f64 * factor).round() as u64;
        self.length = scale(self.length).max(1);
        for take in &mut self.takes {
            take.offset = scale(take.offset);
        }
        self.stretch = ratio;
        // The fades cover the same audio as before.
        let (fade_in, fade_out) = (self.fade_in, self.fade_out);
        self.set_fades(
            Fade {
                length: scale(fade_in.length),
                ..fade_in
            },
            Fade {
                length: scale(fade_out.length),
                ..fade_out
            },
        );
    }

    /// `original`'s length, offsets and ratio as stretched to `ratio`, computed from
    /// `original` itself: a drag sets each frame's ratio from where the gesture began, so the
    /// roundings of earlier frames never add up.
    pub fn restretched(original: &Clip, ratio: f64) -> Clip {
        let mut clip = original.clone();
        clip.set_stretch(ratio);
        clip
    }

    pub fn end(&self) -> u64 {
        self.start + self.length
    }

    /// The take that plays.
    pub fn take(&self) -> &Take {
        &self.takes[self.active.min(self.takes.len() - 1)]
    }

    /// The playing take's file.
    pub fn file(&self) -> &std::path::Path {
        &self.take().file
    }

    /// Where in the playing take's file the clip begins.
    pub fn offset(&self) -> u64 {
        self.take().offset
    }

    /// Move where every take begins: later (`by` > 0) or earlier.
    fn shift_offsets(&mut self, by: i64) {
        for take in &mut self.takes {
            take.offset = take.offset.saturating_add_signed(by);
        }
    }
}

/// A named place in the song.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub id: u32,
    pub name: String,
    /// MIDI ticks from the timeline start (960 to a quarter note).
    pub tick: u64,
}

impl Project {
    /// Add a marker at `tick`; returns its id. Unnamed, it's called "Marker N" (N one more than
    /// there are).
    pub fn add_marker(&mut self, tick: u64, name: Option<String>) -> u32 {
        let id = self.next_id();
        let name = name.unwrap_or_else(|| format!("Marker {}", self.markers.len() + 1));
        self.markers.push(Marker { id, name, tick });
        self.sort_markers();
        id
    }

    pub fn marker(&self, id: u32) -> Option<&Marker> {
        self.markers.iter().find(|m| m.id == id)
    }

    /// Rename a marker and/or move it to `tick`.
    pub fn set_marker(
        &mut self,
        id: u32,
        name: Option<String>,
        tick: Option<u64>,
    ) -> Result<(), String> {
        let marker = self
            .markers
            .iter_mut()
            .find(|m| m.id == id)
            .ok_or(format!("no marker with id {id}"))?;
        if let Some(name) = name {
            marker.name = name;
        }
        if let Some(tick) = tick {
            marker.tick = tick;
        }
        self.sort_markers();
        Ok(())
    }

    pub fn remove_marker(&mut self, id: u32) -> Result<Marker, String> {
        let at = self
            .markers
            .iter()
            .position(|m| m.id == id)
            .ok_or(format!("no marker with id {id}"))?;
        Ok(self.markers.remove(at))
    }

    /// Time order; markers on the same tick keep theirs.
    fn sort_markers(&mut self) {
        self.markers.sort_by_key(|m| m.tick);
    }
}

/// A clip of either kind: what the clipboard and edits of several clips hold.
#[derive(Debug, Clone, PartialEq)]
pub enum AnyClip {
    Audio(Clip),
    Midi(MidiClip),
}

impl AnyClip {
    pub fn id(&self) -> ClipId {
        match self {
            AnyClip::Audio(c) => c.id,
            AnyClip::Midi(c) => c.id,
        }
    }
}

/// A clip as saved. Projects from before takes (1.6) have `file` and `offset` instead of
/// `takes`; they load as a clip with that one take.
#[derive(Deserialize)]
struct ClipFile {
    id: ClipId,
    name: String,
    start: u64,
    length: u64,
    gain_db: f32,
    #[serde(default)]
    takes: Vec<Take>,
    #[serde(default)]
    active: usize,
    #[serde(default)]
    file: Option<PathBuf>,
    #[serde(default)]
    offset: u64,
    #[serde(default = "unstretched")]
    stretch: f64,
    #[serde(default)]
    pitch: f64,
    #[serde(default)]
    transients: bool,
    #[serde(default)]
    follow: Option<Follow>,
    #[serde(default)]
    fade_in: Fade,
    #[serde(default)]
    fade_out: Fade,
}

fn unstretched() -> f64 {
    1.0
}

impl From<ClipFile> for Clip {
    fn from(f: ClipFile) -> Self {
        let mut takes = f.takes;
        if takes.is_empty() {
            takes.push(Take {
                file: f.file.unwrap_or_default(),
                offset: f.offset,
                name: f.name.clone(),
            });
        }
        Clip {
            id: f.id,
            name: f.name,
            start: f.start,
            length: f.length,
            gain_db: f.gain_db,
            active: f.active.min(takes.len() - 1),
            takes,
            stretch: f.stretch,
            pitch: f.pitch,
            transients: f.transients,
            follow: f.follow,
            fade_in: f.fade_in,
            fade_out: f.fade_out,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Master {
    pub mixer: Mixer,
    pub inserts: Vec<InsertSlot>,
    /// First channel of the hardware output pair.
    pub output_channel: u16,
    /// One lane per target at most; no send lanes (the master has no sends).
    #[serde(default)]
    pub automation: Vec<AutomationLane>,
    #[serde(default)]
    pub automation_mode: AutomationMode,
    /// Its row at the top of the arrangement: the height (`None`: the default) and whether its
    /// automation lanes show. View state, as a track's.
    #[serde(default)]
    pub height: Option<f32>,
    #[serde(default)]
    pub show_automation: bool,
}

impl Master {
    /// The master's row height in the arrangement: tall enough for its knobs by default.
    pub fn height(&self) -> f32 {
        self.height
            .unwrap_or(84.0)
            .clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT)
    }
}

/// Where a track's signal actually goes once [`TrackOutput::Parent`] and invalid targets are
/// resolved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Destination {
    Master,
    Bus(TrackId),
    Hardware(u16),
}

const PALETTE: [[u8; 3]; 8] = [
    [0xe0, 0x6c, 0x5a],
    [0xe0, 0xa4, 0x4a],
    [0xc8, 0xc8, 0x50],
    [0x6c, 0xc0, 0x6a],
    [0x4a, 0xb8, 0xc8],
    [0x5a, 0x86, 0xe0],
    [0x9a, 0x6c, 0xe0],
    [0xd8, 0x6a, 0xb0],
];

impl Project {
    pub fn next_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn track(&self, id: TrackId) -> Option<&Track> {
        self.tracks.iter().find(|t| t.id == id)
    }

    pub fn track_mut(&mut self, id: TrackId) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|t| t.id == id)
    }

    pub fn index_of(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    /// Add a track at the end of `parent`'s sub-tracks (or of the project) and return its id.
    pub fn add_track(&mut self, kind: TrackKind, parent: Option<TrackId>) -> TrackId {
        let id = self.next_id();
        let parent = parent.filter(|p| self.track(*p).is_some_and(|t| t.kind == TrackKind::Folder));
        let count = self.tracks.iter().filter(|t| t.kind == kind).count() + 1;
        let name = match kind {
            TrackKind::Audio => format!("Audio {count}"),
            TrackKind::Instrument => format!("Instrument {count}"),
            TrackKind::Folder => format!("Folder {count}"),
            TrackKind::Bus => format!("Bus {count}"),
        };
        let input = match kind {
            TrackKind::Audio => TrackInput::Audio {
                channel: 0,
                stereo: false,
            },
            TrackKind::Instrument => TrackInput::Midi {
                port: None,
                channel: None,
            },
            TrackKind::Folder | TrackKind::Bus => TrackInput::None,
        };
        let track = Track {
            id,
            name,
            kind,
            color: PALETTE[id as usize % PALETTE.len()],
            parent,
            input,
            output: TrackOutput::Parent,
            mixer: Mixer::default(),
            arm: false,
            monitor: Monitor::Auto,
            inserts: Vec::new(),
            clips: Vec::new(),
            midi_clips: Vec::new(),
            sends: Vec::new(),
            automation: Vec::new(),
            automation_mode: AutomationMode::Read,
            collapsed: false,
            height: None,
            show_automation: false,
            enabled: true,
        };
        let at = match parent {
            Some(p) => self.subtree_end(p),
            None => self.tracks.len(),
        };
        self.tracks.insert(at, track);
        id
    }

    /// Add a track right after `after` (and its sub-tracks), in the same folder; at the end
    /// without one. For placing dropped files next to the track they were dropped on.
    pub fn insert_track_after(&mut self, kind: TrackKind, after: Option<TrackId>) -> TrackId {
        let parent = after.and_then(|a| self.track(a)).and_then(|t| t.parent);
        let id = self.add_track(kind, parent);
        if let Some(after) = after.filter(|a| self.track(*a).is_some()) {
            let from = self.index_of(id).expect("just added");
            let track = self.tracks.remove(from);
            let at = self.subtree_end(after);
            self.tracks.insert(at, track);
        }
        id
    }

    /// Index one past the last track inside `id`'s subtree.
    pub fn subtree_end(&self, id: TrackId) -> usize {
        let Some(start) = self.index_of(id) else {
            return self.tracks.len();
        };
        let mut end = start + 1;
        while end < self.tracks.len() && self.is_descendant(self.tracks[end].id, id) {
            end += 1;
        }
        end
    }

    /// Whether `id` sits (at any depth) inside folder `ancestor`.
    pub fn is_descendant(&self, id: TrackId, ancestor: TrackId) -> bool {
        let mut current = self.track(id).and_then(|t| t.parent);
        // Bounded walk: a corrupt file with a parent cycle must not hang the UI.
        for _ in 0..self.tracks.len() {
            match current {
                Some(p) if p == ancestor => return true,
                Some(p) => current = self.track(p).and_then(|t| t.parent),
                None => return false,
            }
        }
        false
    }

    pub fn depth(&self, id: TrackId) -> usize {
        let mut depth = 0;
        let mut current = self.track(id).and_then(|t| t.parent);
        while let Some(p) = current {
            depth += 1;
            if depth > self.tracks.len() {
                break;
            }
            current = self.track(p).and_then(|t| t.parent);
        }
        depth
    }

    /// Whether `id` is hidden because a folder above it is collapsed.
    pub fn is_hidden(&self, id: TrackId) -> bool {
        let mut current = self.track(id).and_then(|t| t.parent);
        for _ in 0..self.tracks.len() {
            match current.and_then(|p| self.track(p)) {
                Some(folder) if folder.collapsed => return true,
                Some(folder) => current = folder.parent,
                None => return false,
            }
        }
        false
    }

    /// Remove a track. A folder's sub-tracks move up to the folder's own parent.
    pub fn remove_track(&mut self, id: TrackId) -> Option<Track> {
        let index = self.index_of(id)?;
        let removed = self.tracks.remove(index);
        let mut gone = Vec::new();
        for track in &mut self.tracks {
            if track.parent == Some(id) {
                track.parent = removed.parent;
            }
            if track.output == TrackOutput::Track(id) {
                track.output = TrackOutput::Parent;
            }
            gone.extend(track.sends.iter().filter(|s| s.target == id).map(|s| s.id));
            track.sends.retain(|s| s.target != id);
        }
        self.forget_lanes(
            |t| matches!(t, AutomationTarget::SendLevel { send } if gone.contains(send)),
        );
        Some(removed)
    }

    /// Whether track `id` may go into `folder` (`None`: the top level): it must be a folder,
    /// not `id` or inside it, and not one that already feeds `id` (a track that follows its
    /// parent would close a loop).
    fn check_parent(&self, id: TrackId, folder: Option<TrackId>) -> Result<(), String> {
        let track = self.track(id).ok_or(format!("no track with id {id}"))?;
        if let Some(f) = folder {
            if !self.track(f).is_some_and(|t| t.kind == TrackKind::Folder) {
                return Err(format!("track {f} is not a folder"));
            }
            if f == id || self.is_descendant(f, id) {
                return Err(format!("folder {id} cannot go inside itself"));
            }
            // Checked before `parent` changes: `destination` reads it.
            if track.output == TrackOutput::Parent && self.feeds(f, id) {
                return Err(format!(
                    "moving track {id} into folder {f} would create a loop: {f} already feeds {id}"
                ));
            }
        }
        Ok(())
    }

    /// Move a track (with its sub-tracks) to `place`: beside another track (taking its
    /// folder), into a folder at a position, or to the end. Refused, changing nothing, where
    /// [`Self::set_parent`] would refuse the folder, or beside a track inside the one moved.
    pub fn move_track_to(&mut self, id: TrackId, place: Place) -> Result<(), String> {
        let other = match place {
            Place::Before(t) | Place::After(t) => Some(t),
            Place::Into { folder, .. } => Some(folder),
            Place::End => None,
        };
        if let Some(t) = other {
            if self.track(t).is_none() {
                return Err(format!("no track with id {t}"));
            }
            if matches!(place, Place::Before(_) | Place::After(_))
                && (t == id || self.is_descendant(t, id))
            {
                return Err(format!(
                    "track {id} can't go beside track {t}: that is itself or inside it"
                ));
            }
        }
        let parent = match place {
            Place::Before(t) | Place::After(t) => self.track(t).and_then(|t| t.parent),
            Place::Into { folder, .. } => Some(folder),
            Place::End => None,
        };
        self.check_parent(id, parent)?;
        let start = self.index_of(id).ok_or(format!("no track with id {id}"))?;
        let end = self.subtree_end(id);
        let block: Vec<Track> = self.tracks.drain(start..end).collect();
        let at = match place {
            Place::Before(t) => self.index_of(t).expect("checked"),
            Place::After(t) => self.subtree_end(t),
            Place::Into { folder, index } => {
                let children: Vec<TrackId> = self
                    .tracks
                    .iter()
                    .filter(|t| t.parent == Some(folder))
                    .map(|t| t.id)
                    .collect();
                match children.get(index) {
                    Some(&child) => self.index_of(child).expect("listed"),
                    None => self.subtree_end(folder),
                }
            }
            Place::End => self.tracks.len(),
        };
        self.tracks.splice(at..at, block);
        if let Some(track) = self.track_mut(id) {
            track.parent = parent;
        }
        Ok(())
    }

    /// Every track's id and folder, in order: what [`Self::restore_order`] puts back (a track
    /// drag that is cancelled).
    pub fn order(&self) -> Vec<(TrackId, Option<TrackId>)> {
        self.tracks.iter().map(|t| (t.id, t.parent)).collect()
    }

    /// Put the tracks back in an order from [`Self::order`], with their folders. Tracks it
    /// doesn't list keep their place at the end.
    pub fn restore_order(&mut self, order: &[(TrackId, Option<TrackId>)]) {
        let rank = |id: TrackId| {
            order
                .iter()
                .position(|(t, _)| *t == id)
                .unwrap_or(usize::MAX)
        };
        self.tracks.sort_by_key(|t| rank(t.id));
        for track in &mut self.tracks {
            if let Some((_, parent)) = order.iter().find(|(t, _)| *t == track.id) {
                track.parent = *parent;
            }
        }
    }

    /// Move a track (with its sub-tracks) into `folder`, or to the top level with `None`.
    /// Refuses to put a folder inside itself, or to close a loop: a track that follows its
    /// parent would feed a folder that already feeds it.
    pub fn set_parent(&mut self, id: TrackId, folder: Option<TrackId>) -> Result<(), String> {
        self.check_parent(id, folder)?;
        let Some(start) = self.index_of(id) else {
            return Err(format!("no track with id {id}"));
        };
        let end = self.subtree_end(id);
        let block: Vec<Track> = self.tracks.drain(start..end).collect();
        let at = match folder {
            Some(f) => self.subtree_end(f),
            None => self.tracks.len(),
        };
        self.tracks.splice(at..at, block);
        if let Some(track) = self.track_mut(id) {
            track.parent = folder;
        }
        Ok(())
    }

    /// Move a track (with its sub-tracks) one place up or down among its siblings.
    pub fn move_track(&mut self, id: TrackId, up: bool) {
        let Some(track) = self.track(id) else {
            return;
        };
        let parent = track.parent;
        let siblings: Vec<TrackId> = self
            .tracks
            .iter()
            .filter(|t| t.parent == parent)
            .map(|t| t.id)
            .collect();
        let Some(pos) = siblings.iter().position(|s| *s == id) else {
            return;
        };
        let other = match (up, pos) {
            (true, 0) => return,
            (true, p) => siblings[p - 1],
            (false, p) if p + 1 >= siblings.len() => return,
            (false, p) => siblings[p + 1],
        };
        // Swap the two sibling subtrees.
        let (first, second) = if up { (other, id) } else { (id, other) };
        let a = self.index_of(first).unwrap();
        let a_end = self.subtree_end(first);
        let b_end = self.subtree_end(second);
        self.tracks[a..b_end].rotate_left(a_end - a);
    }

    /// Resolve where a track's output goes. Invalid or cyclic targets fall back to the master.
    /// Whether `track` plays: it is enabled, and so is every track its main output passes
    /// through on the way to the master (a track inside a disabled folder is silent too).
    pub fn plays(&self, track: TrackId) -> bool {
        std::iter::once(track)
            .chain(self.main_path(track))
            .all(|id| self.track(id).is_some_and(|t| t.enabled))
    }

    /// Whether `track`'s main output reaches `to` (itself included), through folders and
    /// buses: what a render of `to` holds.
    pub fn main_feeds(&self, track: TrackId, to: TrackId) -> bool {
        track == to || self.main_path(track).contains(&to)
    }

    pub fn destination(&self, track: &Track) -> Destination {
        let bus = |id: TrackId| {
            let valid = id != track.id
                && self.track(id).is_some_and(|t| t.kind.mixes_tracks())
                && !self.is_descendant(id, track.id);
            valid.then_some(Destination::Bus(id))
        };
        match track.output {
            TrackOutput::Parent => track.parent.and_then(bus).unwrap_or(Destination::Master),
            TrackOutput::Master => Destination::Master,
            TrackOutput::Track(id) => bus(id).unwrap_or(Destination::Master),
            TrackOutput::Hardware { channel } => Destination::Hardware(channel),
        }
    }

    /// Folders and buses `track` may route its main output or a send to without creating a
    /// loop.
    pub fn valid_buses(&self, track: TrackId) -> Vec<&Track> {
        self.tracks
            .iter()
            .filter(|t| self.route_error(track, t.id).is_none())
            .collect()
    }

    /// Why `from` may not feed `to` (by main output or send), if it may not: `to` must be a
    /// folder or bus, not `from` itself or inside it, and must not already feed `from`.
    pub fn route_error(&self, from: TrackId, to: TrackId) -> Option<String> {
        match self.track(to) {
            None => return Some(format!("no track with id {to}")),
            Some(t) if !t.kind.mixes_tracks() => {
                return Some(format!("track {to} is not a folder or bus"))
            }
            Some(_) => {}
        }
        if to == from {
            return Some(format!(
                "routing track {from} into itself would create a loop"
            ));
        }
        if self.is_descendant(to, from) || self.feeds(to, from) {
            return Some(format!(
                "routing track {from} into {to} would create a loop: {to} already feeds {from}"
            ));
        }
        None
    }

    /// The tracks `track` feeds directly: the folder or bus of its main output and of its
    /// sends, and the tracks listening to its plug-ins' extra outputs.
    pub fn routes<'a>(&'a self, track: &'a Track) -> impl Iterator<Item = TrackId> + 'a {
        let main = match self.destination(track) {
            Destination::Bus(id) => Some(id),
            _ => None,
        };
        let sends = track
            .sends
            .iter()
            .map(|s| s.target)
            .filter(|&t| t != track.id && self.track(t).is_some_and(|t| t.kind.mixes_tracks()));
        let outputs = track
            .inserts
            .iter()
            .flat_map(|slot| self.listeners(slot.id))
            .map(|(t, _)| t)
            .filter(move |&t| t != track.id);
        main.into_iter().chain(sends).chain(outputs)
    }

    /// The track whose chain holds insert `insert` (not the master's).
    pub fn insert_track(&self, insert: InsertId) -> Option<TrackId> {
        self.tracks
            .iter()
            .find(|t| t.inserts.iter().any(|s| s.id == insert))
            .map(|t| t.id)
    }

    /// The tracks listening to insert `insert`'s extra outputs, with the output each hears,
    /// in track order.
    pub fn listeners(&self, insert: InsertId) -> Vec<(TrackId, u16)> {
        self.tracks
            .iter()
            .filter_map(|t| match t.input {
                TrackInput::Plugin { insert: i, output } if i == insert => Some((t.id, output)),
                _ => None,
            })
            .collect()
    }

    /// Every track `from`'s signal reaches, through main outputs and sends. Bounded by the
    /// visited set, so a loop in a damaged file can't hang the UI.
    pub fn reach(&self, from: TrackId) -> std::collections::HashSet<TrackId> {
        let mut seen = std::collections::HashSet::new();
        let mut stack: Vec<TrackId> = self
            .track(from)
            .map(|t| self.routes(t).collect())
            .unwrap_or_default();
        while let Some(id) = stack.pop() {
            if seen.insert(id) {
                if let Some(track) = self.track(id) {
                    stack.extend(self.routes(track));
                }
            }
        }
        seen
    }

    /// Whether `from`'s signal reaches `to`, through main outputs and sends.
    pub fn feeds(&self, from: TrackId, to: TrackId) -> bool {
        self.reach(from).contains(&to)
    }

    /// The buses `id`'s main output passes through on its way to the master or the hardware.
    fn main_path(&self, id: TrackId) -> Vec<TrackId> {
        let mut path = Vec::new();
        let mut current = self.track(id);
        while let Some(track) = current {
            match self.destination(track) {
                Destination::Bus(next) if !path.contains(&next) => {
                    path.push(next);
                    current = self.track(next);
                }
                _ => break,
            }
        }
        path
    }

    /// What mute and solo leave of each track and send.
    ///
    /// With any track soloed, a track plays fully if it is soloed, sits in a soloed folder,
    /// feeds a soloed track by its main output, or is fed by a soloed track (its folders, and
    /// the buses it sends to, so a soloed guitar keeps its reverb).
    ///
    /// Solo in place: a track whose signal leads into a soloed track by another way (a guitar
    /// sending to a soloed reverb) plays only the outputs that lead there. Its other outputs,
    /// the main one included, are silent, so the soloed bus is heard with exactly what feeds
    /// it. An explicit mute always wins.
    pub fn audibility(&self) -> Audibility {
        self.audibility_with(|t| t.mixer.mute)
    }

    /// [`Self::audibility`] with `muted` saying which tracks are muted by their own button:
    /// the engine leaves out a track whose mute lane decides instead.
    pub fn audibility_with(&self, muted: impl Fn(&Track) -> bool) -> Audibility {
        use std::collections::HashSet;
        let mut result = Audibility::default();
        let soloed: HashSet<TrackId> = self
            .tracks
            .iter()
            .filter(|t| t.mixer.solo)
            .map(|t| t.id)
            .collect();
        let explicit = self.tracks.iter().filter(|t| muted(t)).map(|t| t.id);
        if soloed.is_empty() {
            result.muted.extend(explicit);
            return result;
        }
        // Heard fully: the soloed tracks, everything they feed, and what is above or below
        // them by folder or main output.
        let mut full: HashSet<TrackId> = HashSet::new();
        for &s in &soloed {
            full.insert(s);
            full.extend(self.reach(s));
        }
        for t in &self.tracks {
            let related = soloed.iter().any(|&s| {
                self.is_descendant(t.id, s)
                    || self.is_descendant(s, t.id)
                    || self.main_path(t.id).contains(&s)
            });
            if related {
                full.insert(t.id);
            }
        }
        // Leads into a soloed track: its signal gets there by some route.
        let leads: HashSet<TrackId> = self
            .tracks
            .iter()
            .map(|t| t.id)
            .filter(|&id| soloed.contains(&id) || self.reach(id).iter().any(|r| soloed.contains(r)))
            .collect();
        for t in &self.tracks {
            if muted(t) {
                result.muted.insert(t.id);
            } else if full.contains(&t.id) {
                // Plays as it is.
            } else if leads.contains(&t.id) {
                let main_leads = match self.destination(t) {
                    Destination::Bus(b) => leads.contains(&b),
                    _ => false,
                };
                if !main_leads {
                    result.main_muted.insert(t.id);
                }
                result.muted_sends.extend(
                    t.sends
                        .iter()
                        .filter(|s| !leads.contains(&s.target))
                        .map(|s| s.id),
                );
            } else {
                result.muted.insert(t.id);
            }
        }
        result
    }

    /// Which tracks are silent altogether once solo is applied (see [`Self::audibility`]).
    /// A track heard only through its sends counts as playing.
    pub fn effective_mutes(&self) -> Vec<(TrackId, bool)> {
        let audibility = self.audibility();
        self.tracks
            .iter()
            .map(|t| (t.id, audibility.muted.contains(&t.id)))
            .collect()
    }

    pub fn find_send(&self, id: SendId) -> Option<(TrackId, &TrackSend)> {
        self.tracks
            .iter()
            .find_map(|t| t.sends.iter().find(|s| s.id == id).map(|s| (t.id, s)))
    }

    pub fn find_send_mut(&mut self, id: SendId) -> Option<&mut TrackSend> {
        self.tracks
            .iter_mut()
            .find_map(|t| t.sends.iter_mut().find(|s| s.id == id))
    }

    /// Give `from` a send into the folder or bus `to`, at 0 dB, and return its id.
    pub fn add_send(
        &mut self,
        from: TrackId,
        to: TrackId,
        pre_fader: bool,
    ) -> Result<SendId, String> {
        if self.track(from).is_none() {
            return Err(format!("no track with id {from}"));
        }
        if let Some(error) = self.route_error(from, to) {
            return Err(error);
        }
        let id = self.next_id();
        self.track_mut(from)
            .expect("checked")
            .sends
            .push(TrackSend {
                id,
                target: to,
                gain_db: 0.0,
                pre_fader,
                mute: false,
            });
        Ok(id)
    }

    /// Point a send at another folder or bus.
    pub fn set_send_target(&mut self, id: SendId, to: TrackId) -> Result<(), String> {
        let (from, _) = self.find_send(id).ok_or(format!("no send with id {id}"))?;
        if let Some(error) = self.route_error(from, to) {
            return Err(error);
        }
        self.find_send_mut(id).expect("found").target = to;
        Ok(())
    }

    /// Remove a send, and the lane that automated its level.
    pub fn remove_send(&mut self, id: SendId) -> Option<TrackSend> {
        let removed = self.tracks.iter_mut().find_map(|t| {
            let at = t.sends.iter().position(|s| s.id == id)?;
            Some(t.sends.remove(at))
        })?;
        self.forget_lanes(|t| *t == AutomationTarget::SendLevel { send: id });
        Some(removed)
    }

    /// Take a plug-in out of its insert chain (only the project: [`crate::session::Session::
    /// remove_insert`] also lets the live plug-in go), and the lanes automating it.
    pub fn remove_insert(&mut self, id: InsertId) {
        for track in &mut self.tracks {
            track.inserts.retain(|s| s.id != id);
        }
        self.master.inserts.retain(|s| s.id != id);
        self.forget_lanes(
            |t| matches!(t, AutomationTarget::Parameter { insert, .. } if *insert == id),
        );
    }

    pub fn find_midi_clip(&self, id: ClipId) -> Option<(TrackId, &MidiClip)> {
        self.tracks
            .iter()
            .find_map(|t| t.midi_clips.iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    pub fn find_midi_clip_mut(&mut self, id: ClipId) -> Option<&mut MidiClip> {
        self.tracks
            .iter_mut()
            .find_map(|t| t.midi_clips.iter_mut().find(|c| c.id == id))
    }

    /// Add an empty MIDI clip to an instrument track; `start` and `length` in ticks.
    pub fn add_midi_clip(
        &mut self,
        track: TrackId,
        start: u64,
        length: u64,
    ) -> Result<ClipId, String> {
        let t = self
            .track(track)
            .ok_or(format!("no track with id {track}"))?;
        if t.kind != TrackKind::Instrument {
            return Err(format!(
                "track {track} is not an instrument track; MIDI clips go on instrument tracks"
            ));
        }
        if length == 0 {
            return Err("a MIDI clip's length must be at least 1 tick".into());
        }
        let count = t.midi_clips.len() + 1;
        let id = self.next_id();
        self.track_mut(track)
            .expect("checked")
            .midi_clips
            .push(MidiClip {
                id,
                name: format!("MIDI {count}"),
                start,
                length,
                notes: Vec::new(),
                controllers: Vec::new(),
            });
        Ok(id)
    }

    /// A copied MIDI clip with fresh ids for itself, its notes and its controller events (all
    /// unique in the project), ready to add.
    pub fn adopt_midi_clip(&mut self, mut clip: MidiClip) -> MidiClip {
        clip.id = self.next_id();
        for note in &mut clip.notes {
            note.id = self.next_id();
        }
        for event in &mut clip.controllers {
            event.id = self.next_id();
        }
        clip
    }

    /// The track holding a clip of either kind.
    pub fn clip_track(&self, id: ClipId) -> Option<TrackId> {
        self.find_clip(id)
            .map(|(t, _)| t)
            .or_else(|| self.find_midi_clip(id).map(|(t, _)| t))
    }

    /// A track's row, top to bottom.
    pub fn track_index(&self, id: TrackId) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }

    pub fn remove_midi_clip(&mut self, id: ClipId) -> Option<MidiClip> {
        self.tracks.iter_mut().find_map(|t| {
            let at = t.midi_clips.iter().position(|c| c.id == id)?;
            Some(t.midi_clips.remove(at))
        })
    }

    /// Add notes to a MIDI clip (their `id`s are ignored) and return their new ids, in the
    /// order given. All or nothing: one bad note and none are added.
    pub fn add_notes(&mut self, clip: ClipId, notes: &[Note]) -> Result<Vec<NoteId>, String> {
        self.find_midi_clip(clip)
            .ok_or(format!("no MIDI clip with id {clip}"))?;
        for (i, note) in notes.iter().enumerate() {
            note.check().map_err(|e| format!("note {i}: {e}"))?;
        }
        let ids: Vec<NoteId> = notes.iter().map(|_| self.next_id()).collect();
        let c = self.find_midi_clip_mut(clip).expect("checked");
        c.notes
            .extend(notes.iter().zip(&ids).map(|(n, &id)| Note { id, ..*n }));
        c.sort();
        Ok(ids)
    }

    /// Replace a MIDI clip's notes with `notes` (ids kept as given): for edits that change
    /// several notes at once. All or nothing.
    pub fn set_notes(&mut self, clip: ClipId, notes: Vec<Note>) -> Result<(), String> {
        self.find_midi_clip(clip)
            .ok_or(format!("no MIDI clip with id {clip}"))?;
        for note in &notes {
            note.check().map_err(|e| format!("note {}: {e}", note.id))?;
        }
        let c = self.find_midi_clip_mut(clip).expect("checked");
        c.notes = notes;
        c.sort();
        Ok(())
    }

    /// Remove notes by id. All or nothing: an id the clip doesn't have removes none.
    pub fn remove_notes(&mut self, clip: ClipId, ids: &[NoteId]) -> Result<(), String> {
        let c = self
            .find_midi_clip_mut(clip)
            .ok_or(format!("no MIDI clip with id {clip}"))?;
        if let Some(missing) = ids.iter().find(|id| !c.notes.iter().any(|n| n.id == **id)) {
            return Err(format!("MIDI clip {clip} has no note with id {missing}"));
        }
        c.notes.retain(|n| !ids.contains(&n.id));
        Ok(())
    }

    /// Add controller events to a MIDI clip (their `id`s are ignored) and return their new
    /// ids. All or nothing.
    pub fn add_controller_events(
        &mut self,
        clip: ClipId,
        events: &[ControllerEvent],
    ) -> Result<Vec<EventId>, String> {
        self.find_midi_clip(clip)
            .ok_or(format!("no MIDI clip with id {clip}"))?;
        for (i, event) in events.iter().enumerate() {
            event.check().map_err(|e| format!("event {i}: {e}"))?;
        }
        let ids: Vec<EventId> = events.iter().map(|_| self.next_id()).collect();
        let c = self.find_midi_clip_mut(clip).expect("checked");
        c.controllers.extend(
            events
                .iter()
                .zip(&ids)
                .map(|(e, &id)| ControllerEvent { id, ..*e }),
        );
        c.sort();
        Ok(ids)
    }

    /// Put a recorded MIDI take on a track. `replace` first deletes the notes and controller
    /// events that start inside the take's span from that track's clips. Then the take goes
    /// into a clip that holds its whole span (overdubbing, as loop recording does pass after
    /// pass), or into a new clip. Returns the clip.
    pub fn place_midi_take(
        &mut self,
        track: TrackId,
        take: crate::midi_record::MidiTake,
        replace: bool,
    ) -> Result<ClipId, String> {
        let (from, to) = (take.start, take.start + take.length);
        let t = self
            .track_mut(track)
            .ok_or(format!("no track with id {track}"))?;
        if replace {
            for clip in &mut t.midi_clips {
                let inside = |tick: u64| (from..to).contains(&(clip.start + tick));
                clip.notes.retain(|n| !inside(n.tick));
                clip.controllers.retain(|e| !inside(e.tick));
            }
        }
        let holder = t
            .midi_clips
            .iter()
            .find(|c| c.start <= from && c.end() >= to)
            .map(|c| (c.id, c.start));
        let (clip, shift) = match holder {
            Some((id, start)) => (id, from - start),
            None => (self.add_midi_clip(track, from, take.length)?, 0),
        };
        let shifted = |tick: u64| tick + shift;
        let notes: Vec<Note> = take
            .notes
            .iter()
            .map(|n| Note {
                tick: shifted(n.tick),
                ..*n
            })
            .collect();
        let controllers: Vec<ControllerEvent> = take
            .controllers
            .iter()
            .map(|e| ControllerEvent {
                tick: shifted(e.tick),
                ..*e
            })
            .collect();
        self.add_notes(clip, &notes)?;
        self.add_controller_events(clip, &controllers)?;
        Ok(clip)
    }

    /// Remove controller events by id. All or nothing.
    pub fn remove_controller_events(
        &mut self,
        clip: ClipId,
        ids: &[EventId],
    ) -> Result<(), String> {
        let c = self
            .find_midi_clip_mut(clip)
            .ok_or(format!("no MIDI clip with id {clip}"))?;
        if let Some(missing) = ids
            .iter()
            .find(|id| !c.controllers.iter().any(|e| e.id == **id))
        {
            return Err(format!(
                "MIDI clip {clip} has no controller event with id {missing}"
            ));
        }
        c.controllers.retain(|e| !ids.contains(&e.id));
        Ok(())
    }

    pub fn find_insert(&self, id: InsertId) -> Option<&InsertSlot> {
        self.all_inserts().find(|i| i.id == id)
    }

    pub fn all_inserts(&self) -> impl Iterator<Item = &InsertSlot> {
        self.tracks
            .iter()
            .flat_map(|t| &t.inserts)
            .chain(&self.master.inserts)
    }

    pub fn all_inserts_mut(&mut self) -> impl Iterator<Item = &mut InsertSlot> {
        self.tracks
            .iter_mut()
            .flat_map(|t| &mut t.inserts)
            .chain(&mut self.master.inserts)
    }

    /// The clip with this id and the track holding it.
    pub fn find_clip(&self, id: ClipId) -> Option<(TrackId, &Clip)> {
        self.tracks
            .iter()
            .find_map(|t| t.clips.iter().find(|c| c.id == id).map(|c| (t.id, c)))
    }

    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.tracks
            .iter_mut()
            .flat_map(|t| &mut t.clips)
            .find(|c| c.id == id)
    }

    pub fn remove_clip(&mut self, id: ClipId) -> Option<Clip> {
        self.tracks.iter_mut().find_map(|t| {
            let index = t.clips.iter().position(|c| c.id == id)?;
            Some(t.clips.remove(index))
        })
    }

    /// Split a clip at timeline position `at`: the clip keeps the part before it and a new
    /// clip (returned) starts there. `None` if `at` is not strictly inside the clip.
    pub fn split_clip(&mut self, id: ClipId, at: u64) -> Option<ClipId> {
        let (track, clip) = self.find_clip(id)?;
        if at <= clip.start || at >= clip.end() {
            return None;
        }
        let new_id = self.next_id();
        let track = self.track_mut(track)?;
        let index = track.clips.iter().position(|c| c.id == id)?;
        let clip = &mut track.clips[index];
        let cut = at - clip.start;
        let mut second = clip.clone();
        clip.length = cut;
        second.id = new_id;
        second.start = at;
        second.shift_offsets(cut as i64);
        second.length -= cut;
        // Each piece keeps the fade at its outer end.
        let (fade_in, fade_out) = (clip.fade_in, clip.fade_out);
        clip.set_fades(fade_in, Fade::default());
        second.set_fades(Fade::default(), fade_out);
        track.clips.insert(index + 1, second);
        Some(new_id)
    }

    /// Split a MIDI clip at song tick `at`: the clip keeps the part before it and a new clip
    /// (returned) starts there, as [`MidiClip::split`] lays out the notes and controllers.
    /// `None` if `at` is not strictly inside the clip.
    pub fn split_midi_clip(&mut self, id: ClipId, at: u64) -> Option<ClipId> {
        let (track, clip) = self.find_midi_clip(id)?;
        if at <= clip.start || at >= clip.end() {
            return None;
        }
        let mut first = clip.clone();
        let second = first.split(at - first.start, &mut || self.next_id());
        let new_id = second.id;
        let track = self.track_mut(track)?;
        let index = track.midi_clips.iter().position(|c| c.id == id)?;
        track.midi_clips[index] = first;
        track.midi_clips.insert(index + 1, second);
        Some(new_id)
    }

    /// Play take `take` of a clip.
    pub fn set_active_take(&mut self, id: ClipId, take: usize) -> Result<(), String> {
        let clip = self.clip_mut(id).ok_or(format!("no clip with id {id}"))?;
        if take >= clip.takes.len() {
            return Err(format!(
                "clip {id} has {} takes (0 to {})",
                clip.takes.len(),
                clip.takes.len() - 1
            ));
        }
        clip.active = take;
        Ok(())
    }

    /// Remove take `take` from a clip. The last take cannot go (delete the clip instead); if
    /// the playing take goes, the one before it plays.
    pub fn delete_take(&mut self, id: ClipId, take: usize) -> Result<Take, String> {
        let clip = self.clip_mut(id).ok_or(format!("no clip with id {id}"))?;
        if take >= clip.takes.len() {
            return Err(format!("clip {id} has no take {take}"));
        }
        if clip.takes.len() == 1 {
            return Err(format!(
                "take {take} is clip {id}'s only take; delete the clip instead"
            ));
        }
        let removed = clip.takes.remove(take);
        if clip.active >= take && clip.active > 0 {
            clip.active -= 1;
        }
        Ok(removed)
    }

    /// Move a clip to `start`, optionally onto another audio or instrument track.
    pub fn move_clip(
        &mut self,
        id: ClipId,
        start: u64,
        to_track: Option<TrackId>,
    ) -> Result<(), String> {
        let (from, _) = self.find_clip(id).ok_or(format!("no clip with id {id}"))?;
        let to = to_track.unwrap_or(from);
        match self.track(to) {
            Some(t) if t.kind.holds_clips() => {}
            Some(_) => {
                return Err(format!(
                    "track {to} is a folder or bus and cannot hold clips"
                ))
            }
            None => return Err(format!("no track with id {to}")),
        }
        let mut clip = self.remove_clip(id).expect("found above");
        clip.start = start;
        self.track_mut(to).expect("checked above").clips.push(clip);
        Ok(())
    }

    /// Split a clip at every position strictly inside it. Returns the pieces in time order
    /// (the original first) and the positions that were not inside the clip.
    pub fn split_clip_at(
        &mut self,
        id: ClipId,
        positions: &[u64],
    ) -> Option<(Vec<ClipId>, Vec<u64>)> {
        self.find_clip(id)?;
        let mut sorted = positions.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        let mut pieces = vec![id];
        let mut skipped = Vec::new();
        for at in sorted {
            let last = *pieces.last().expect("never empty");
            match self.split_clip(last, at) {
                Some(next) => pieces.push(next),
                None => skipped.push(at),
            }
        }
        Some((pieces, skipped))
    }

    /// Move clips' starts toward the grid. `target(start)` is where a start would snap to;
    /// `strength` 1 moves all the way, 0.5 halfway. Afterwards, on each track, for each pair of
    /// moved clips in time order:
    /// - if the first now overlaps the second, it is shortened to end where the second starts,
    ///   so the two are never heard at once;
    /// - with `fill_gaps`, if a gap opened between them, the second starts earlier by playing
    ///   the audio just before its own start (as far as its source allows), so its hit stays
    ///   where it was put. Lengthening the first instead would replay the second's attack,
    ///   because that is the audio right after the first's end.
    ///
    /// Then, with `crossfade` (samples), each pair is joined by a short linear crossfade so a
    /// moved slice doesn't click where it meets the next: the second starts up to `crossfade`
    /// earlier, into the quiet audio just before its hit, fading in over it, and the first fades
    /// out over its last `crossfade`. Extending a clip back doesn't move its audio, so every hit
    /// stays where quantizing put it; and the first is never extended into the next hit. Linear,
    /// because unmoved slices then sum back to exactly the audio they were cut from. A slice
    /// with no audio before its hit gets no fade-in (fading in over an attack would soften it).
    /// A quantized slice therefore starts a crossfade before its hit.
    ///
    /// Returns (clip, old start, new start of its original first sample: its hit) for each.
    pub fn quantize_clips(
        &mut self,
        ids: &[ClipId],
        target: impl Fn(u64) -> u64,
        strength: f64,
        fill_gaps: bool,
        crossfade: u64,
    ) -> Vec<(ClipId, u64, u64)> {
        let strength = strength.clamp(0.0, 1.0);
        let mut moves = Vec::new();
        for &id in ids {
            let Some(clip) = self.clip_mut(id) else {
                continue;
            };
            let from = clip.start;
            let snapped = target(from) as f64;
            let to = (from as f64 + (snapped - from as f64) * strength)
                .round()
                .max(0.0) as u64;
            clip.start = to;
            moves.push((id, from, to));
        }
        let moved: std::collections::HashSet<ClipId> = ids.iter().copied().collect();
        for track in &mut self.tracks {
            let mut order: Vec<usize> = (0..track.clips.len())
                .filter(|&i| moved.contains(&track.clips[i].id))
                .collect();
            order.sort_by_key(|&i| track.clips[i].start);
            for pair in order.windows(2) {
                let (first, second) = (pair[0], pair[1]);
                let first_end = track.clips[first].end();
                let next_start = track.clips[second].start;
                if first_end > next_start {
                    let clip = &mut track.clips[first];
                    clip.length = next_start.saturating_sub(clip.start).max(1);
                } else if fill_gaps && first_end < next_start {
                    let next = &mut track.clips[second];
                    // Every take must have the audio to start that much earlier.
                    let room = next.takes.iter().map(|t| t.offset).min().unwrap_or(0);
                    let extend = (next.start - first_end).min(room);
                    next.start -= extend;
                    next.shift_offsets(-(extend as i64));
                    next.length += extend;
                }
            }
            if crossfade == 0 {
                continue;
            }
            // Joined by crossfades, in the order they are now.
            order.sort_by_key(|&i| track.clips[i].start);
            for pair in order.windows(2) {
                let (first, second) = (pair[0], pair[1]);
                let linear = |length| Fade {
                    length,
                    shape: FadeShape::Linear,
                };
                let longer = |fade: Fade, length: u64| {
                    if fade.length >= length {
                        fade
                    } else {
                        linear(length)
                    }
                };
                let a = &mut track.clips[first];
                let (fade_in, fade_out) = (a.fade_in, a.fade_out);
                a.set_fades(fade_in, longer(fade_out, crossfade));
                let b = &mut track.clips[second];
                // Every take must have the audio before the hit.
                let room = b.takes.iter().map(|t| t.offset).min().unwrap_or(0);
                let pre = crossfade.min(room).min(b.start);
                if pre > 0 {
                    b.start -= pre;
                    b.shift_offsets(-(pre as i64));
                    b.length += pre;
                    let (fade_in, fade_out) = (b.fade_in, b.fade_out);
                    b.set_fades(longer(fade_in, pre), fade_out);
                }
            }
        }
        moves
    }

    pub fn insert_mut(&mut self, id: InsertId) -> Option<&mut InsertSlot> {
        self.all_inserts_mut().find(|i| i.id == id)
    }

    /// Re-express every clip position at a new sample rate (media is resampled on load).
    pub fn convert_sample_rate(&mut self, rate: u32) {
        let from = self.sample_rate;
        if from != 0 && from != rate {
            let scale = |v: u64| (v as f64 * f64::from(rate) / f64::from(from)).round() as u64;
            for clip in self.tracks.iter_mut().flat_map(|t| &mut t.clips) {
                clip.start = scale(clip.start);
                clip.length = scale(clip.length);
                for take in &mut clip.takes {
                    take.offset = scale(take.offset);
                }
            }
            for region in [&mut self.loop_region, &mut self.punch] {
                region.start = scale(region.start);
                region.end = scale(region.end);
            }
        }
        self.sample_rate = rate;
    }

    /// End of the last clip, in samples.
    pub fn length(&self) -> u64 {
        self.tracks
            .iter()
            .flat_map(|t| &t.clips)
            .map(Clip::end)
            .max()
            .unwrap_or(0)
    }
}

pub(crate) mod base64_bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S: Serializer>(value: &Option<Arc<Vec<u8>>>, s: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => s.serialize_some(&STANDARD.encode(bytes.as_slice())),
            None => s.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Arc<Vec<u8>>>, D::Error> {
        Option::<String>::deserialize(d)?
            .map(|s| {
                STANDARD
                    .decode(s)
                    .map(Arc::new)
                    .map_err(serde::de::Error::custom)
            })
            .transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(p: &Project) -> Vec<TrackId> {
        p.tracks.iter().map(|t| t.id).collect()
    }

    #[test]
    fn track_names_are_trimmed_and_never_empty() {
        assert_eq!(track_name("  Lead guitar "), Ok("Lead guitar"));
        assert_eq!(track_name("Gtr 2"), Ok("Gtr 2"));
        assert!(track_name("").is_err());
        assert!(track_name("   \t ").is_err());
    }

    #[test]
    fn mixer_levels_are_clamped_to_the_fader() {
        let mut mixer = Mixer::default();
        mixer.set_gain_db(-6.5);
        assert_eq!(mixer.gain_db, -6.5);
        mixer.set_gain_db(40.0);
        assert_eq!(mixer.gain_db, GAIN_RANGE.1);
        mixer.set_gain_db(f32::NEG_INFINITY);
        assert_eq!(mixer.gain_db, GAIN_RANGE.0);
        mixer.set_pan(-0.3);
        assert_eq!(mixer.pan, -0.3);
        mixer.set_pan(2.0);
        assert_eq!(mixer.pan, 1.0);
    }

    #[test]
    fn stretching_scales_length_and_offsets_from_the_start() {
        let mut clip = Clip::new(1, "di".into(), "di.wav".into(), 1000, 300, 4410);
        clip.takes.push(Take {
            file: "b.wav".into(),
            offset: 701,
            name: "b".into(),
        });
        clip.set_stretch(2.0);
        assert_eq!((clip.start, clip.length), (1000, 8820));
        assert_eq!((clip.takes[0].offset, clip.takes[1].offset), (600, 1402));
        // Back exactly from twice as long...
        clip.set_stretch(1.0);
        assert_eq!(
            (clip.length, clip.offset(), clip.takes[1].offset),
            (4410, 300, 701)
        );
        // ...and within a sample from anything else (each change rounds).
        clip.set_stretch(1.3);
        clip.set_stretch(1.0);
        assert!(clip.length.abs_diff(4410) <= 1 && clip.takes[1].offset.abs_diff(701) <= 1);
        assert!(!clip.is_stretched());
        clip.pitch = -2.0;
        assert!(clip.is_stretched());
    }

    #[test]
    fn following_clips_round_trip_exactly_and_never_drift() {
        let mut p = Project::default();
        for (bar, tempo) in [(1, 97.0), (3, 141.3)] {
            p.tempo_changes.push(TempoChange {
                bar,
                beat: 0,
                tempo: Some(tempo),
                time_signature: None,
            });
        }
        let map = TempoMap::of(&p, 48_000);
        assert_eq!(map.segments().len(), 3);
        let mut clip = Clip::new(1, "di".into(), "di.wav".into(), 123_457, 10_001, 500_003);
        clip.takes.push(Take {
            file: "b.wav".into(),
            offset: 77,
            name: "b".into(),
        });
        clip.set_follow(Some(120.0), &map);
        // Read from samples and laid out again: the very same samples.
        let laid = clip.layout();
        clip.follow_samples(&map);
        assert_eq!(clip.laid_out(&map), Some(laid.clone()));
        // Another tempo and back, a hundred times: the samples come back exactly.
        let mut faster = p.clone();
        faster.tempo = 133.0;
        let other = TempoMap::of(&faster, 48_000);
        for _ in 0..100 {
            clip.sync_follow(&other, true);
            clip.sync_follow(&map, true);
        }
        assert_eq!(clip.layout(), laid);
        // Takes added since (loop recording) are read from the samples.
        clip.takes.push(Take {
            file: "c.wav".into(),
            offset: 500,
            name: "c".into(),
        });
        clip.sync_follow(&map, false);
        assert_eq!(clip.follow.as_ref().unwrap().offsets.len(), 3);
        assert_eq!(clip.laid_out(&map), Some(clip.layout()));
    }

    #[test]
    fn a_drag_stretches_from_where_it_began_without_drift() {
        let mut origin = Clip::new(1, "di".into(), "di.wav".into(), 1000, 777, 44_101);
        origin.set_stretch(1.1);
        // A drag through 200 ratios, ending at 1.7: stretching from the origin each frame
        // ends exactly where one stretch would.
        let ratios: Vec<f64> = (0..200)
            .map(|i| 1.1 + 0.6 * f64::from(i + 1) / 200.0)
            .collect();
        let mut dragged = origin.clone();
        for &ratio in &ratios {
            dragged = Clip::restretched(&origin, ratio);
        }
        let mut once = origin.clone();
        once.set_stretch(1.7);
        assert_eq!(dragged.layout(), once.layout());
        assert_eq!(dragged.start, origin.start);
        // Frame by frame from the last frame's (rounded) clip instead, it drifts: why the
        // drag keeps its origin.
        let mut drifting = origin.clone();
        for &ratio in &ratios {
            drifting.set_stretch(ratio);
        }
        assert_ne!(drifting.layout(), once.layout());
    }

    #[test]
    fn fades_fit_the_clip_and_a_split_keeps_the_outer_ones() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        let mut clip = Clip::new(p.next_id(), "a".into(), "a.wav".into(), 0, 0, 10_000);
        let fade = |length| Fade {
            length,
            shape: FadeShape::Linear,
        };
        clip.set_fades(fade(7000), fade(7000));
        assert_eq!((clip.fade_in.length, clip.fade_out.length), (7000, 3000));
        clip.set_fades(fade(1000), fade(2000));
        let id = clip.id;
        p.track_mut(t).unwrap().clips.push(clip);
        let second = p.split_clip(id, 4000).unwrap();
        let (_, a) = p.find_clip(id).unwrap();
        assert_eq!((a.fade_in.length, a.fade_out.length), (1000, 0));
        let (_, b) = p.find_clip(second).unwrap();
        assert_eq!((b.fade_in.length, b.fade_out.length), (0, 2000));
        // Stretching takes the fades along.
        let b = p.clip_mut(second).unwrap();
        b.set_stretch(2.0);
        assert_eq!(b.fade_out.length, 4000);
    }

    #[test]
    fn projects_saved_before_markers_load_without_them() {
        let mut p = Project::default();
        p.add_marker(0, None);
        let mut json = serde_json::to_value(&p).unwrap();
        json.as_object_mut().unwrap().remove("markers");
        let old: Project = serde_json::from_value(json).unwrap();
        assert!(old.markers.is_empty());
    }
    #[test]
    fn clips_saved_before_stretching_load_untouched() {
        let mut clip = Clip::new(1, "di".into(), "di.wav".into(), 0, 0, 10);
        clip.set_stretch(2.0);
        clip.pitch = 5.0;
        let mut json = serde_json::to_value(&clip).unwrap();
        assert_eq!(serde_json::from_value::<Clip>(json.clone()).unwrap(), clip);
        let fields = json.as_object_mut().unwrap();
        fields.remove("stretch");
        fields.remove("pitch");
        fields.remove("fade_in");
        fields.remove("fade_out");
        fields.remove("transients");
        let old: Clip = serde_json::from_value(json).unwrap();
        assert!(
            !old.transients,
            "saved before attacks were kept: they keep their sound"
        );
        assert!(clip.transients, "new clips keep attacks");
        assert_eq!((old.stretch, old.pitch), (1.0, 0.0));
        assert_eq!((old.fade_in.length, old.fade_out.length), (0, 0));
    }

    #[test]
    fn folders_keep_children_together() {
        let mut p = Project::default();
        let a = p.add_track(TrackKind::Audio, None);
        let f = p.add_track(TrackKind::Folder, None);
        let b = p.add_track(TrackKind::Audio, Some(f));
        let c = p.add_track(TrackKind::Audio, Some(f));
        assert_eq!(ids(&p), vec![a, f, b, c]);
        assert_eq!(p.depth(c), 1);
        assert_eq!(p.destination(p.track(b).unwrap()), Destination::Bus(f));
        assert_eq!(p.destination(p.track(a).unwrap()), Destination::Master);

        p.move_track(f, true);
        assert_eq!(ids(&p), vec![f, b, c, a]);

        assert!(p.set_parent(a, Some(f)).is_ok());
        assert_eq!(ids(&p), vec![f, b, c, a]);
        assert_eq!(p.track(a).unwrap().parent, Some(f));
        assert!(p.set_parent(f, Some(f)).is_err());

        p.remove_track(f);
        assert_eq!(ids(&p), vec![b, c, a]);
        assert!(p.tracks.iter().all(|t| t.parent.is_none()));
    }

    #[test]
    fn routing_never_loops() {
        let mut p = Project::default();
        let outer = p.add_track(TrackKind::Folder, None);
        let inner = p.add_track(TrackKind::Folder, Some(outer));
        // Routing the outer folder into its own child would loop; it falls back to master.
        p.track_mut(outer).unwrap().output = TrackOutput::Track(inner);
        assert_eq!(p.destination(p.track(outer).unwrap()), Destination::Master);
        assert!(p.valid_buses(outer).is_empty());
        assert_eq!(p.valid_buses(inner).len(), 1);
    }

    #[test]
    fn solo_keeps_parents_and_children_audible() {
        let mut p = Project::default();
        let f = p.add_track(TrackKind::Folder, None);
        let a = p.add_track(TrackKind::Audio, Some(f));
        let b = p.add_track(TrackKind::Audio, None);
        p.track_mut(a).unwrap().mixer.solo = true;
        let mutes: std::collections::HashMap<_, _> = p.effective_mutes().into_iter().collect();
        assert!(!mutes[&f]);
        assert!(!mutes[&a]);
        assert!(mutes[&b]);
    }

    /// A track drag cancelled with Escape: the order and folders from before it come back,
    /// however far it went.
    #[test]
    fn restoring_an_order_undoes_any_moves() {
        let mut p = Project::default();
        let folder = p.add_track(TrackKind::Folder, None);
        let inside = p.add_track(TrackKind::Audio, Some(folder));
        let a = p.add_track(TrackKind::Audio, None);
        let b = p.add_track(TrackKind::Audio, None);
        let before = p.order();
        p.move_track_to(b, Place::Into { folder, index: 0 })
            .unwrap();
        p.move_track_to(folder, Place::End).unwrap();
        p.move_track_to(inside, Place::Before(a)).unwrap();
        assert_ne!(p.order(), before);
        p.restore_order(&before);
        assert_eq!(p.order(), before);
        assert_eq!(p.track(inside).unwrap().parent, Some(folder));
    }

    #[test]
    fn a_plug_ins_output_tracks_solo_with_their_instrument() {
        let mut p = Project::default();
        let kit = p.add_track(TrackKind::Instrument, None);
        let outs = p.add_track(TrackKind::Folder, None);
        let kick = p.add_track(TrackKind::Audio, Some(outs));
        let bass = p.add_track(TrackKind::Audio, None);
        let id = p.next_id();
        p.track_mut(kit).unwrap().inserts.push(InsertSlot {
            id,
            name: "Kit".into(),
            path: "kit.vst3".into(),
            bypass: false,
            state: None,
        });
        p.track_mut(kick).unwrap().input = TrackInput::Plugin {
            insert: id,
            output: 1,
        };
        assert_eq!(p.listeners(id), [(kick, 1)]);
        assert_eq!(p.insert_track(id), Some(kit));
        assert!(
            p.feeds(kit, kick) && p.feeds(kit, outs),
            "through the output"
        );

        // The kick soloed: the kit plays into it, its own main output silent; the bass not.
        p.track_mut(kick).unwrap().mixer.solo = true;
        let a = p.audibility();
        assert!(!a.muted.contains(&kick) && !a.muted.contains(&outs));
        assert!(!a.muted.contains(&kit) && a.main_muted.contains(&kit));
        assert!(a.muted.contains(&bass));
        // The bass soloed: the kick track is silent itself.
        p.track_mut(kick).unwrap().mixer.solo = false;
        p.track_mut(bass).unwrap().mixer.solo = true;
        let a = p.audibility();
        assert!(a.muted.contains(&kick) && a.muted.contains(&kit));
    }

    #[test]
    fn solo_keeps_the_buses_a_track_sends_to() {
        let mut p = Project::default();
        let guitar = p.add_track(TrackKind::Audio, None);
        let bass = p.add_track(TrackKind::Audio, None);
        let reverb = p.add_track(TrackKind::Bus, None);
        let delay = p.add_track(TrackKind::Bus, None);
        p.add_send(guitar, reverb, false).unwrap();
        p.add_send(reverb, delay, false).unwrap();
        p.add_send(bass, delay, false).unwrap();
        let mutes = |p: &Project| -> std::collections::HashMap<_, _> {
            p.effective_mutes().into_iter().collect()
        };

        p.track_mut(guitar).unwrap().mixer.solo = true;
        let m = mutes(&p);
        assert!(
            !m[&guitar] && !m[&reverb] && !m[&delay],
            "the guitar and its returns"
        );
        assert!(m[&bass]);

        // Soloing the reverb plays it with what feeds it (solo in place): the guitar is heard
        // only through its send there. The bass feeds the delay, not the reverb: silent.
        p.track_mut(guitar).unwrap().mixer.solo = false;
        p.track_mut(reverb).unwrap().mixer.solo = true;
        let a = p.audibility();
        let send_of = |p: &Project, from: TrackId| p.track(from).unwrap().sends[0].id;
        assert!(!a.muted.contains(&reverb) && !a.muted.contains(&delay));
        assert!(!a.muted.contains(&guitar) && a.main_muted.contains(&guitar));
        assert!(!a.muted_sends.contains(&send_of(&p, guitar)));
        assert!(a.muted.contains(&bass));

        // A second send from the guitar, to a bus that doesn't lead to the reverb, is
        // silenced with its main output.
        let other = p.add_track(TrackKind::Bus, None);
        let side = p.add_send(guitar, other, false).unwrap();
        let a = p.audibility();
        assert!(a.muted_sends.contains(&side) && a.muted.contains(&other));

        // Mute beats solo in place.
        p.track_mut(guitar).unwrap().mixer.mute = true;
        assert!(p.audibility().muted.contains(&guitar));
    }

    #[test]
    fn a_loop_in_a_damaged_file_does_not_hang_routing_or_solo() {
        let mut p = Project::default();
        let a = p.add_track(TrackKind::Bus, None);
        let b = p.add_track(TrackKind::Bus, None);
        p.add_send(a, b, false).unwrap();
        // Bypass the checks, as a hand-edited file would.
        let id = p.next_id();
        p.track_mut(b).unwrap().sends.push(TrackSend {
            id,
            target: a,
            gain_db: 0.0,
            pre_fader: false,
            mute: false,
        });
        assert!(p.feeds(a, a) && p.feeds(b, b));
        p.track_mut(a).unwrap().mixer.solo = true;
        assert!(p.effective_mutes().iter().all(|(_, muted)| !muted));
    }

    #[test]
    fn buses_hold_no_clips_or_tracks_and_moving_one_checks_for_loops() {
        let mut p = Project::default();
        let f = p.add_track(TrackKind::Folder, None);
        let bus = p.add_track(TrackKind::Bus, None);
        let a = p.add_track(TrackKind::Audio, None);
        let clip = with_clip(&mut p, a);
        assert!(p.move_clip(clip, 0, Some(bus)).is_err());
        assert!(p.set_parent(a, Some(bus)).is_err(), "a bus is not a folder");
        // The folder sends to the bus: the bus can't then sit inside it and feed it...
        p.add_send(f, bus, false).unwrap();
        let err = p.set_parent(bus, Some(f)).unwrap_err();
        assert!(err.contains("loop"), "{err}");
        // ...unless its output skips the folder.
        p.track_mut(bus).unwrap().output = TrackOutput::Master;
        assert!(p.set_parent(bus, Some(f)).is_ok());
        assert_eq!(p.destination(p.track(bus).unwrap()), Destination::Master);
    }

    fn with_clip(p: &mut Project, track: TrackId) -> ClipId {
        let id = p.next_id();
        p.track_mut(track).unwrap().clips.push(Clip::new(
            id,
            "c".into(),
            "c.wav".into(),
            100,
            10,
            50,
        ));
        id
    }

    #[test]
    fn clips_split_move_and_delete() {
        let mut p = Project::default();
        let a = p.add_track(TrackKind::Audio, None);
        let b = p.add_track(TrackKind::Instrument, None);
        let f = p.add_track(TrackKind::Folder, None);
        let clip = with_clip(&mut p, a);

        assert_eq!(p.split_clip(clip, 100), None, "at the start is not inside");
        assert_eq!(p.split_clip(clip, 150), None, "at the end is not inside");
        let second = p.split_clip(clip, 120).unwrap();
        let (_, first) = p.find_clip(clip).unwrap();
        assert_eq!((first.start, first.offset(), first.length), (100, 10, 20));
        let (_, rest) = p.find_clip(second).unwrap();
        assert_eq!((rest.start, rest.offset(), rest.length), (120, 30, 30));

        assert!(
            p.move_clip(second, 500, Some(f)).is_err(),
            "folders hold no clips"
        );
        p.move_clip(second, 500, Some(b)).unwrap();
        assert_eq!(p.find_clip(second).unwrap().0, b);
        assert_eq!(p.find_clip(second).unwrap().1.start, 500);

        assert!(p.remove_clip(clip).is_some());
        assert!(p.find_clip(clip).is_none());
    }

    #[test]
    fn a_midi_clip_splits_strictly_inside_into_the_next_place_with_fresh_ids() {
        let mut p = Project::default();
        let keys = p.add_track(TrackKind::Instrument, None);
        // From the middle of bar 2 (beat 3), a bar long, and a later clip after it.
        let clip = p.add_midi_clip(keys, 3840 + 1920, 3840).unwrap();
        let later = p.add_midi_clip(keys, 3840 * 4, 960).unwrap();
        let notes = [Note {
            id: 0,
            tick: 960,
            length: 1920,
            pitch: 60,
            velocity: 90,
            channel: 3,
        }];
        let [held] = p.add_notes(clip, &notes).unwrap()[..] else {
            panic!("one note");
        };
        assert_eq!(p.split_midi_clip(clip, 5760), None, "at its start");
        assert_eq!(p.split_midi_clip(clip, 5760 + 3840), None, "at its end");
        assert_eq!(p.split_midi_clip(clip, 100), None, "before it");
        assert_eq!(p.split_midi_clip(later + 1000, 6000), None, "no such clip");
        // On bar 3's line, 1920 ticks in: through the note.
        let second = p.split_midi_clip(clip, 3840 * 2).unwrap();
        let order: Vec<ClipId> = p
            .track(keys)
            .unwrap()
            .midi_clips
            .iter()
            .map(|c| c.id)
            .collect();
        assert_eq!(
            order,
            [clip, second, later],
            "the new piece right after the first"
        );
        let (_, first) = p.find_midi_clip(clip).unwrap();
        assert_eq!((first.start, first.length), (5760, 1920));
        assert_eq!((first.notes[0].id, first.notes[0].length), (held, 960));
        let (_, rest) = p.find_midi_clip(second).unwrap();
        assert_eq!((rest.start, rest.length), (7680, 1920));
        let copy = rest.notes[0];
        assert_eq!((copy.tick, copy.length, copy.channel), (0, 960, 3));
        assert!(
            copy.id > later && copy.id != held && second > later,
            "ids unique in the project"
        );
    }

    #[test]
    fn slices_quantize_without_overlapping() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        let id = p.next_id();
        p.track_mut(t).unwrap().clips.push(Clip::new(
            id,
            "riff".into(),
            "riff.wav".into(),
            0,
            0,
            1000,
        ));
        // Transients a little off a 100-sample grid.
        let (pieces, skipped) = p.split_clip_at(id, &[290, 0, 510, 2000, 790]).unwrap();
        assert_eq!(skipped, vec![0, 2000], "outside the clip");
        assert_eq!(pieces.len(), 4);
        // Slices now start at 0, 290, 510, 790 and are back to back.
        let grid = |s: u64| crate::grid::quantize(s, 100.0, crate::grid::Rounding::Nearest);
        let moves = p.quantize_clips(&pieces[1..], grid, 1.0, true, 0);
        assert_eq!(
            moves.iter().map(|m| m.2).collect::<Vec<_>>(),
            vec![300, 500, 800]
        );
        let clips = &p.track(t).unwrap().clips;
        let span = |id| {
            let c = clips.iter().find(|c| c.id == id).unwrap();
            (c.start, c.end(), c.offset())
        };
        // The second slice (moved later to 300) would end at 520 over the third (now at 500):
        // it is trimmed to 500. The third (moved to 500) ends at 780, leaving a gap before the
        // fourth (moved to 800): the fourth starts 20 earlier, playing the 20 samples before
        // its hit, which itself stays at 800 (source position 790).
        assert_eq!(span(pieces[1]), (300, 500, 290));
        assert_eq!(span(pieces[2]), (500, 780, 510));
        assert_eq!(span(pieces[3]), (780, 1010, 770));
        let hit = |id| {
            let (start, _, offset) = span(id);
            start + (790 - offset)
        };
        assert_eq!(hit(pieces[3]), 800, "the transient is on the grid");
        // Half strength moves halfway.
        let mut p2 = p.clone();
        let half = p2.quantize_clips(&[pieces[3]], |_| 1000, 0.5, false, 0);
        assert_eq!(half[0].2, 890);
    }

    #[test]
    fn quantized_slices_join_with_crossfades_before_their_hits() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        let id = p.next_id();
        p.track_mut(t).unwrap().clips.push(Clip::new(
            id,
            "riff".into(),
            "riff.wav".into(),
            0,
            0,
            1000,
        ));
        let (pieces, _) = p.split_clip_at(id, &[290, 510, 790]).unwrap();
        let grid = |s: u64| crate::grid::quantize(s, 100.0, crate::grid::Rounding::Nearest);
        // The first piece (its hit at the file's start) moves too.
        let moves = p.quantize_clips(&pieces, grid, 1.0, true, 20);
        assert_eq!(
            moves.iter().map(|m| m.2).collect::<Vec<_>>(),
            vec![0, 300, 500, 800]
        );
        let clip = |id| p.find_clip(id).unwrap().1.clone();
        // No audio before the first hit: no fade-in, no earlier start.
        let first = clip(pieces[0]);
        assert_eq!(
            (first.start, first.fade_in.length, first.fade_out.length),
            (0, 0, 20)
        );
        for (k, hit_source) in [(1, 290), (2, 510), (3, 790)] {
            let c = clip(pieces[k]);
            // It starts 20 before its hit, which is still on the grid.
            assert_eq!(c.start + (hit_source - c.offset()), moves[k].2, "piece {k}");
            assert_eq!(
                c.fade_in,
                Fade {
                    length: 20,
                    shape: FadeShape::Linear
                },
                "piece {k}"
            );
        }
        assert_eq!(
            clip(pieces[3]).fade_out.length,
            0,
            "the last piece's end stays"
        );
        // A longer fade the user set survives.
        let mut q = p.clone();
        let piece = q.clip_mut(pieces[2]).unwrap();
        let fade_out = piece.fade_out;
        piece.set_fades(
            Fade {
                length: 60,
                shape: FadeShape::EqualPower,
            },
            fade_out,
        );
        q.quantize_clips(&pieces[1..], grid, 1.0, true, 20);
        assert_eq!(q.find_clip(pieces[2]).unwrap().1.fade_in.length, 60);
    }
    #[test]
    fn round_trips_through_json() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        p.track_mut(t).unwrap().inserts.push(InsertSlot {
            id: 99,
            name: "X".into(),
            path: "x.vst3".into(),
            bypass: false,
            state: Some(Arc::new(vec![0, 1, 2, 255])),
        });
        let json = serde_json::to_string(&p).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(
            back.tracks[0].inserts[0].state,
            Some(Arc::new(vec![0, 1, 2, 255]))
        );
        assert_eq!(back.next_id, p.next_id);
    }

    #[test]
    fn every_take_follows_a_split() {
        let mut p = Project::default();
        let t = p.add_track(TrackKind::Audio, None);
        let id = with_clip(&mut p, t);
        let clip = p.clip_mut(id).unwrap();
        // A second pass of the same loop, 1000 samples further into the file.
        clip.takes.push(Take {
            file: "c.wav".into(),
            offset: 1010,
            name: "pass 2".into(),
        });
        clip.active = 1;
        let second = p.split_clip(id, 120).unwrap();
        for (piece, offsets) in [(id, [10, 1010]), (second, [30, 1030])] {
            let c = p.find_clip(piece).unwrap().1;
            assert_eq!(c.takes.len(), 2);
            assert_eq!(c.active, 1, "both halves keep the chosen take");
            let got: Vec<u64> = c.takes.iter().map(|t| t.offset).collect();
            assert_eq!(got, offsets);
        }
    }

    #[test]
    fn clips_saved_before_takes_still_load() {
        let old = r#"{"id":5,"name":"gtr","file":"Audio/gtr.wav","start":100,"offset":20,
                      "length":300,"gain_db":-3.0}"#;
        let clip: Clip = serde_json::from_str(old).unwrap();
        assert_eq!(clip.takes.len(), 1);
        assert_eq!(clip.file(), std::path::Path::new("Audio/gtr.wav"));
        assert_eq!((clip.offset(), clip.start, clip.length), (20, 100, 300));
        // And the new format round-trips.
        let back: Clip = serde_json::from_str(&serde_json::to_string(&clip).unwrap()).unwrap();
        assert_eq!(back.takes, clip.takes);
    }
}
