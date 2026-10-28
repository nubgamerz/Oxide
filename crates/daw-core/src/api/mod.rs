//! The action registry: every operation an AI agent (and, over time, the UI) can perform.
//!
//! An action has a name, a description, a JSON Schema for its input, a [`Safety`] class and a
//! handler. The in-app MCP server publishes [`Registry::specs`] as tools and routes calls to
//! [`Registry::call`], so making a feature AI-controllable means registering its actions in
//! [`actions`]; there is no per-tool MCP code. `Registry::call` is also the single choke point
//! undo (dev plan phase 2) will wrap.
//!
//! Handlers run on the thread that owns the [`Session`] (the UI thread in the app) through a
//! [`Host`], and answer through a [`Reply`] rather than a return value, so actions that finish
//! later (a screenshot is taken on the next frame) fit the same shape as instant ones.

mod actions;
pub mod prompts;

pub use actions::AREAS;

use crate::session::Session;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::Value;
use std::fmt;

/// How an action affects the session. Mapped to MCP tool hints so clients can decide what to
/// ask the user about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Safety {
    /// Reads state; changes nothing.
    Read,
    /// Changes the project in a way the user can see and reverse.
    Edit,
    /// Starts, stops or moves playback or recording.
    Transport,
    /// Deletes or overwrites something (tracks, files, settings).
    Destructive,
}

/// Edit or destructive actions that are not undo steps: saving rewrites the project's stored
/// plug-in states and paths without editing it; undo and redo move through the history;
/// opening or starting a project, and changing the audio settings (which can convert every
/// position to a new sample rate), start the history afresh; the Waveform view's open files
/// (saved with the project, but never undone with it) and its effects rack (and the rack's
/// presets) aren't part of the project's history; showing the
/// manual changes nothing in it.
const NOT_UNDO_STEPS: &[&str] = &[
    "save_project",
    "open_audio_editor",
    "close_audio_editor",
    "edit_audio",
    "apply_audio_effects",
    "undo_audio_edit",
    "find_audio_problems",
    "repair_audio_problems",
    "save_audio",
    "save_audio_as",
    "set_metadata",
    "undo",
    "redo",
    "new_project",
    "open_project",
    "save_backup",
    "open_backup",
    "cancel_open",
    "set_audio_settings",
    "save_template",
    "rename_template",
    "delete_template",
    "save_rack_preset",
    "load_rack_preset",
    "delete_rack_preset",
    "set_rack_sync",
    "set_layout",
    "save_layout",
    "delete_layout",
    "remove_recent_project",
    "copy_plugin",
    "open_manual",
];

/// Calls the app answers at once while a project opens; every other call waits, in order, until
/// the project is open (get_busy says so). These report the open, stop it, or show it. Here
/// rather than in the app's server so the manual can say which actions run while opening.
pub const ANSWERED_WHILE_OPENING: &[&str] = &["get_busy", "cancel_open", "screenshot"];

/// The changes an agent may make with no project open (the app's start screen up): making,
/// opening or saving one, and the app's own settings, devices, templates and layouts. Reads
/// always run; any other change would edit a project the app doesn't show, so the app's server
/// refuses it (`daw-app`'s `mcp::refusal`).
pub const WITHOUT_A_PROJECT: &[&str] = &[
    "new_project",
    "open_project",
    "open_backup",
    "cancel_open",
    // With a path: the start screen's empty project saved into a folder becomes a project.
    "save_project",
    "rescan_plugins",
    "set_audio_settings",
    "reconnect_midi",
    "rename_template",
    "delete_template",
    "delete_rack_preset",
    "set_layout",
    "save_layout",
    "delete_layout",
    // Shows the manual (27.3): nothing in a project. search_manual, a read, runs anyway.
    "open_manual",
];

/// Edit actions [`Registry::call`] wraps in an edit like any other, but whose calls never
/// change the project, so they never record an undo step: what they change lives outside it.
/// A plug-in's parameters, an impulse file and a learnt noise print are the plug-in's live
/// state (the project holds plug-in states only as saved); the clipboard, the plug-in list,
/// MIDI ports, editor windows and the view are the app's; exports write files. Still wrapped,
/// so `end_edit`'s syncs run after them as before (27.6 documents, it doesn't change, what
/// they do). The examples' runner checks each records nothing.
pub(crate) const NO_PROJECT_CHANGE: &[&str] = &[
    "rescan_plugins",
    "reconnect_midi",
    "set_view",
    "set_plugin_editor",
    "set_plugin_parameter",
    "set_impulse_file",
    "capture_noise_print",
    "copy_clips",
    "copy_notes",
    "export_midi_clip",
    "export_midi_track",
    "export_audio",
    "export_mixdown",
    "mixdown_to_new_file",
];

/// Undo-step actions whose every successful call changes the project (each makes or removes
/// something), so each is one undo step. The other undo-step actions record one only when the
/// call changes something: setting a value to what it already is records nothing.
pub(crate) const ALWAYS_CHANGES: &[&str] = &[
    "add_track",
    "remove_track",
    "add_send",
    "add_send_to_new_bus",
    "remove_send",
    "add_insert",
    "remove_insert",
    "copy_insert",
    "paste_plugin",
    "import_audio",
    "split_clip",
    "delete_clip",
    "delete_take",
    "insert_midi_track",
    "add_midi_clip",
    "remove_midi_clip",
    "cut_notes",
    "paste_notes",
    "add_automation_lane",
    "remove_automation_lane",
    "add_marker",
    "remove_marker",
    "remove_tempo_change",
];

/// What calling an action does to the undo history, as the manual says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undo {
    /// Never an undo step: a read, a transport action, one of [`NOT_UNDO_STEPS`], or one that
    /// never changes the project.
    Never,
    /// One undo step when the call changes the project; nothing when it changes nothing.
    WhenChanged,
    /// Every successful call is one undo step.
    Always,
}

impl Undo {
    /// How `mcp.json` and the manual name it.
    pub fn name(self) -> &'static str {
        match self {
            Undo::Never => "never",
            Undo::WhenChanged => "when_changed",
            Undo::Always => "always",
        }
    }
}

/// "move_clip" → "Move clip", "add_midi_clip" → "Add MIDI clip": the undo step's name.
pub(crate) fn title(action: &str) -> String {
    let words = action
        .split('_')
        .map(|w| if w == "midi" { "MIDI" } else { w })
        .collect::<Vec<_>>()
        .join(" ");
    let mut chars = words.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Setting the same thing on the same object again soon is one undo step.
fn merge_key(action: &str, args: &Value) -> Option<String> {
    if !action.starts_with("set_") {
        return None;
    }
    let id = |field: &str| args.get(field).map(Value::to_string).unwrap_or_default();
    Some(format!(
        "{action}:{}:{}:{}:{}:{}:{}",
        id("track"),
        id("clip"),
        id("insert"),
        id("send"),
        id("lane"),
        id("point")
    ))
}

/// What a client needs to know to call an action.
#[derive(Debug, Clone)]
pub struct ActionSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub safety: Safety,
    /// JSON Schema (an object schema) for the arguments.
    pub input_schema: Value,
    /// JSON Schema for the result, when the registry knows it: every action registered with
    /// [`Registry::add`], and deferred ones given one by [`Registry::returns`]. `Null` when it
    /// isn't described (a PNG, or a deferred answer). Only the manual shows it (27.6); MCP's
    /// `tools/list` doesn't publish it.
    pub output_schema: Value,
    /// The area it belongs to: its module in `api/actions/` (`tracks`, `midi_clips`), one of
    /// [`AREAS`]. The manual has a page per area.
    pub area: &'static str,
}

/// The result of an action.
#[derive(Debug, Clone)]
pub enum ActionOutput {
    Json(Value),
    /// A PNG image, e.g. a screenshot.
    Png(Vec<u8>),
}

/// A failed action, with a message meant for the agent that called it.
#[derive(Debug, Clone)]
pub struct ApiError(pub String);

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ApiError {}

impl From<anyhow::Error> for ApiError {
    fn from(e: anyhow::Error) -> Self {
        Self(format!("{e:#}"))
    }
}

pub type ApiResult<T> = std::result::Result<T, ApiError>;

/// Sends an action's result back to the caller. May be called later, from another frame.
pub type Reply = Box<dyn FnOnce(ApiResult<ActionOutput>) + Send>;

/// An opening action's answer, made once its project is in ([`Host::when_opened`]).
pub type AfterOpen = Box<dyn FnOnce(&mut Session) -> ApiResult<ActionOutput>>;

/// What actions run against: the session, plus whatever the UI can add (screenshots, the
/// current selection, the view). Headless hosts (tests, tools) only provide the session.
pub trait Host {
    fn session(&mut self) -> &mut Session;

    /// Answer `reply` with `then`'s answer once the project open just begun is in (its files
    /// read, its plug-ins loaded). A headless host finishes the open here, before answering;
    /// the app lets it run behind its loading bar, a plug-in a frame, and answers when it
    /// ends, or with an error if it is cancelled or another open replaces it first.
    fn when_opened(&mut self, reply: Reply, then: AfterOpen) {
        let session = self.session();
        session.finish_loading();
        reply(then(session));
    }

    /// Answer `reply` with `then`'s answer once the save just begun
    /// ([`Session::save_in_background`]) is written, or with why it failed. A headless host
    /// finishes it here; the app lets it run behind its busy bar, a plug-in's state a frame,
    /// and answers when the file is written.
    fn when_saved(&mut self, reply: Reply, then: AfterOpen) {
        let session = self.session();
        match session.finish_saving() {
            Ok(()) => reply(then(session)),
            Err(e) => reply(Err(e.into())),
        }
    }

    /// Capture the app window as PNG and reply with [`ActionOutput::Png`].
    fn screenshot(&mut self, reply: Reply) {
        reply(Err(needs_window("screenshots")));
    }

    /// Current meter levels as the UI shows them (per track, master, inputs).
    fn meters(&mut self) -> ApiResult<Value> {
        Err(needs_window("meters"))
    }

    /// What the UI shows: selection, visible timeline range, open panels, snap.
    fn view(&mut self) -> ApiResult<Value> {
        Err(needs_window("the view"))
    }

    /// Change what the UI shows; returns the new view.
    fn set_view(&mut self, _change: ViewChange) -> ApiResult<Value> {
        Err(needs_window("the view"))
    }

    /// The window's layout (where each pane is) and the saved layouts' names.
    fn layout(&mut self) -> ApiResult<Value> {
        Err(needs_window("the window's layout"))
    }

    /// Show the saved layout `name` ("Default": the built-in one); returns the layout.
    fn set_layout(&mut self, _name: &str) -> ApiResult<Value> {
        Err(needs_window("the window's layout"))
    }

    /// Keep the window's layout as `name`, replacing one of that name.
    fn save_layout(&mut self, _name: &str) -> ApiResult<Value> {
        Err(needs_window("the window's layout"))
    }

    /// Delete the saved layout `name`.
    fn delete_layout(&mut self, _name: &str) -> ApiResult<Value> {
        Err(needs_window("the window's layout"))
    }

    /// The start screen's recent projects (the app's settings), newest first.
    fn recent_projects(&mut self) -> ApiResult<Value> {
        Err(needs_window("the recent projects"))
    }

    /// Take `path` off the recent projects; returns the list left.
    fn remove_recent_project(&mut self, _path: &std::path::Path) -> ApiResult<Value> {
        Err(needs_window("the recent projects"))
    }

    /// Show the manual's page `target` (in the manual's folder `dir`) in the default browser,
    /// as Help > Manual and F1 do; returns what to add to the answer (`opened`, the `url`).
    /// Check runs only note the request (no browser).
    fn open_manual(
        &mut self,
        _dir: &std::path::Path,
        _target: &crate::manual::Target,
    ) -> ApiResult<Value> {
        Err(needs_window("opening the manual"))
    }

    /// Help > About's facts: the version, the commit and the program's folder. Without the
    /// app, the engine's own version (the workspace's, the app's too) and nothing else.
    fn app_info(&mut self) -> ApiResult<Value> {
        Ok(serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "commit": null,
            "folder": null,
        }))
    }
}

fn needs_window(what: &str) -> ApiError {
    ApiError(format!(
        "{what} need the app window (this host is headless)"
    ))
}

/// A change to what the UI shows. Fields left out stay as they are.
#[derive(Debug, Default, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ViewChange {
    /// Select this track (and show it in the inspector).
    #[serde(default)]
    pub select_track: Option<crate::project::TrackId>,
    /// Select the master bus.
    #[serde(default)]
    pub select_master: Option<bool>,
    /// Select this clip (and its track).
    #[serde(default)]
    pub select_clip: Option<crate::project::ClipId>,
    /// Select exactly these clips (audio or MIDI; an empty list clears the selection). The
    /// first is the one the inspector shows.
    #[serde(default)]
    pub select_clips: Option<Vec<crate::project::ClipId>>,
    /// First visible timeline sample.
    #[serde(default)]
    pub start: Option<u64>,
    /// Last visible timeline sample (with `start`, sets the zoom: from 32 points a sample to 2
    /// seconds a point).
    #[serde(default)]
    pub end: Option<u64>,
    #[serde(default)]
    pub show_mixer: Option<bool>,
    #[serde(default)]
    pub show_inspector: Option<bool>,
    /// Show or hide the Diagnostics pane (the Waveform view's Click Repair and Clip Repair list:
    /// find_audio_problems, repair_audio_problems).
    #[serde(default)]
    pub show_diagnostics: Option<bool>,
    /// Snap the playhead, clip moves and edges, selections, markers and automation points to
    /// the song's grid (set_grid); off, they go to the sample.
    #[serde(default)]
    pub snap: Option<bool>,
    /// Scroll the timeline with the playhead.
    #[serde(default)]
    pub follow_playhead: Option<bool>,
    /// Zoom the timeline: `in` / `out` (by a step, around the playhead), `fit` (the whole
    /// project) or `clip` (the selected clip, or `select_clip` if given).
    #[serde(default)]
    pub zoom: Option<Zoom>,
    /// Set every track's row height, in points (32 to 400). `set_track` sets one track.
    #[serde(default)]
    pub track_height: Option<f32>,
    /// Waveform display gain: 1 shows the audio as it is, up to 16 makes quiet audio
    /// readable. Display only; the sound does not change.
    #[serde(default)]
    pub waveform_scale: Option<f32>,
    /// Open this MIDI clip in the piano roll (the editor under the arrangement); 0 closes it.
    #[serde(default)]
    pub piano_roll: Option<crate::project::ClipId>,
    /// The loudness meter's scale (EBU Tech 3341): `ebu9` (-18 to +9 LU around the target)
    /// or `ebu18` (-36 to +18 LU).
    #[serde(default)]
    pub loudness_scale: Option<LoudnessScale>,
    /// Show loudness relative to the target, in LU (true), or absolute, in LUFS (false).
    #[serde(default)]
    pub loudness_relative: Option<bool>,
    /// Select a time range across tracks, as dragging in empty lane space does: it also
    /// selects the clips inside. `split_range` and `delete_range` act on such a range.
    #[serde(default)]
    pub time_selection: Option<TimeSelection>,
    /// Clear the time selection.
    #[serde(default)]
    pub clear_time_selection: Option<bool>,
    /// Show the `multitrack` (the arrangement) or the `waveform` view (one audio file).
    #[serde(default)]
    pub view_mode: Option<ViewMode>,
    /// Show this open audio file (open_audio_editor's `document`) in the Waveform view.
    #[serde(default)]
    pub document: Option<u32>,
    /// The Waveform view's visible span, the file's own samples (both, to zoom; as close as 32
    /// points a sample).
    #[serde(default)]
    pub editor_start: Option<u64>,
    #[serde(default)]
    pub editor_end: Option<u64>,
    /// The Waveform view's selection (the file's own samples), on every channel.
    #[serde(default)]
    pub editor_selection: Option<EditorSelection>,
    /// Clear the Waveform view's selection (and its spectral box).
    #[serde(default)]
    pub clear_editor_selection: Option<bool>,
    /// A box on the Waveform view's spectrogram: a time range and a band, for edit_audio's
    /// spectral ops. It replaces the time selection (and the selection replaces it).
    #[serde(default)]
    pub editor_box: Option<EditorBox>,
    /// The Waveform view's playhead, the file's own samples.
    #[serde(default)]
    pub editor_playhead: Option<u64>,
    /// The Waveform view's display: `off` (waveform), `spectral` (spectrogram) or `both`.
    #[serde(default)]
    pub spectral: Option<SpectralMode>,
    /// The spectrogram's colour range: this many dB below full scale is black (20 to 200).
    #[serde(default)]
    pub spectral_range_db: Option<f32>,
    /// The Waveform view's time display (its ruler and clock): `time` (minutes and seconds),
    /// `samples`, or `bars_beats` (the song's tempo map from the file's start, with the
    /// song's grid; set_grid).
    #[serde(default)]
    pub editor_time: Option<TimeDisplay>,
}

/// How the Waveform view shows time: the file's own time, its
/// samples, or bars and beats from its start (the song's tempo and meter, and grid).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum TimeDisplay {
    #[default]
    Time,
    Samples,
    BarsBeats,
}

/// The Waveform view's display: the waveform, a spectrogram, or both (waveform above).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SpectralMode {
    #[default]
    Off,
    Spectral,
    Both,
}

/// The main area: the arrangement, or one audio file (dev plan 10).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ViewMode {
    #[default]
    Multitrack,
    Waveform,
}

/// A range in an audio file, its own samples.
#[derive(Debug, Clone, Copy, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditorSelection {
    pub start: u64,
    pub end: u64,
}

/// A box on the spectrogram: `start`..`end` (the file's own samples) and `low_hz`..`high_hz`,
/// on one `channel` (from 0) or every channel.
#[derive(Debug, Clone, Copy, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditorBox {
    pub start: u64,
    pub end: u64,
    pub low_hz: f64,
    pub high_hz: f64,
    #[serde(default)]
    pub channel: Option<usize>,
}

/// A time range across some tracks: the multitrack's time selection.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TimeSelection {
    /// Start, samples.
    pub start: u64,
    /// End, samples (after `start`).
    pub end: u64,
    /// The tracks it spans (ids); empty for every track.
    #[serde(default)]
    pub tracks: Vec<crate::project::TrackId>,
}

/// The loudness meter's two scales (EBU Tech 3341 §2.7), around the project's loudness target.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum LoudnessScale {
    /// -18 to +9 LU: the default.
    #[default]
    Ebu9,
    /// -36 to +18 LU, for material with a wide loudness range.
    Ebu18,
}

impl LoudnessScale {
    /// The scale's ends, in LU from the target.
    pub fn range(self) -> (f64, f64) {
        match self {
            Self::Ebu9 => (-18.0, 9.0),
            Self::Ebu18 => (-36.0, 18.0),
        }
    }
}

/// A zoom step for [`ViewChange::zoom`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Zoom {
    In,
    Out,
    Fit,
    Clip,
}

/// Waveform display gain limits (see [`ViewChange::waveform_scale`]).
pub const MAX_WAVEFORM_SCALE: f32 = 16.0;

type Handler = Box<dyn Fn(&mut dyn Host, Value, Reply) + Send + Sync>;

struct Action {
    spec: ActionSpec,
    handler: Handler,
}

/// Every registered action.
pub struct Registry {
    actions: Vec<Action>,
    /// The area actions are being registered for ([`ActionSpec::area`]).
    area: &'static str,
}

impl Registry {
    pub fn new() -> Self {
        let mut registry = Self {
            actions: Vec::new(),
            area: "",
        };
        actions::register(&mut registry);
        registry
    }

    /// What calling `name` does to the undo history ([`Undo`]).
    pub fn undo(&self, name: &str) -> Undo {
        if !self.is_undo_step(name) || NO_PROJECT_CHANGE.contains(&name) {
            Undo::Never
        } else if ALWAYS_CHANGES.contains(&name) {
            Undo::Always
        } else {
            Undo::WhenChanged
        }
    }

    /// Whether [`Registry::call`] wraps `name` in an edit: an edit or destructive action not
    /// in [`NOT_UNDO_STEPS`]. The edit is recorded only if the project changed, so for what
    /// a call does to the history ask [`Registry::undo`].
    pub fn is_undo_step(&self, name: &str) -> bool {
        self.safety(name)
            .is_some_and(|s| matches!(s, Safety::Edit | Safety::Destructive))
            && !NOT_UNDO_STEPS.contains(&name)
    }

    pub fn specs(&self) -> Vec<ActionSpec> {
        self.actions.iter().map(|a| a.spec.clone()).collect()
    }

    /// An action's safety class (`None`: no such action), without copying the specs.
    pub fn safety(&self, name: &str) -> Option<Safety> {
        self.actions
            .iter()
            .find(|a| a.spec.name == name)
            .map(|a| a.spec.safety)
    }

    /// Run an action. `args` is the JSON object from the caller (`null` means no arguments).
    ///
    /// Every edit is an undo step: the action runs inside an edit named after it ("move_clip"
    /// becomes "Move clip"), which is recorded only if the project changed. Repeated `set_…`
    /// calls on the same track, clip or insert in quick succession merge into one step.
    pub fn call(&self, host: &mut dyn Host, name: &str, args: Value, reply: Reply) {
        let Some(action) = self.actions.iter().find(|a| a.spec.name == name) else {
            reply(Err(ApiError(format!("unknown action '{name}'"))));
            return;
        };
        let record = self.is_undo_step(name);
        if record {
            host.session()
                .begin_edit(&title(name), merge_key(name, &args));
        }
        (action.handler)(host, args, reply);
        if record {
            host.session().end_edit();
        }
    }

    /// Run an action and wait for the reply on this thread. Only for actions that reply
    /// immediately (all but UI-deferred ones); used by tests and headless tools.
    pub fn call_now(
        &self,
        host: &mut dyn Host,
        name: &str,
        args: Value,
    ) -> ApiResult<ActionOutput> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.call(
            host,
            name,
            args,
            Box::new(move |result| {
                let _ = tx.send(result);
            }),
        );
        rx.try_recv()
            .unwrap_or_else(|_| Err(ApiError(format!("'{name}' did not reply immediately"))))
    }

    /// Register an action whose handler returns its result directly.
    fn add<I, O>(
        &mut self,
        name: &'static str,
        description: &'static str,
        safety: Safety,
        run: fn(&mut dyn Host, I) -> ApiResult<O>,
    ) where
        I: DeserializeOwned + JsonSchema + 'static,
        O: Serialize + JsonSchema + 'static,
    {
        self.add_deferred::<I>(name, description, safety, move |host, input, reply| {
            reply(run(host, input).and_then(|output| {
                serde_json::to_value(output)
                    .map(ActionOutput::Json)
                    .map_err(|e| ApiError(format!("encoding the result: {e}")))
            }));
        });
        self.returns::<O>();
    }

    /// Describes the result of the action registered last as `O` (for the manual): deferred
    /// actions answer through a [`Reply`], so [`Registry::add_deferred`] can't know it.
    fn returns<O: JsonSchema>(&mut self) {
        let action = self.actions.last_mut().expect("an action registered");
        action.spec.output_schema =
            serde_json::to_value(schemars::schema_for!(O)).expect("schemas serialize");
    }

    /// Register an action that answers through its [`Reply`], possibly later.
    fn add_deferred<I>(
        &mut self,
        name: &'static str,
        description: &'static str,
        safety: Safety,
        run: impl Fn(&mut dyn Host, I, Reply) + Send + Sync + 'static,
    ) where
        I: DeserializeOwned + JsonSchema + 'static,
    {
        assert!(
            self.actions.iter().all(|a| a.spec.name != name),
            "action '{name}' registered twice"
        );
        let input_schema =
            serde_json::to_value(schemars::schema_for!(I)).expect("schemas serialize");
        let handler: Handler = Box::new(move |host, args, reply| {
            let args = if args.is_null() {
                Value::Object(Default::default())
            } else {
                args
            };
            match serde_json::from_value::<I>(args) {
                Ok(input) => run(host, input, reply),
                Err(e) => reply(Err(ApiError(format!(
                    "invalid arguments for '{name}': {e}"
                )))),
            }
        });
        self.actions.push(Action {
            spec: ActionSpec {
                name,
                description,
                safety,
                input_schema,
                output_schema: Value::Null,
                area: self.area,
            },
            handler,
        });
    }
}

impl Default for Registry {
    fn default() -> Self {
        Self::new()
    }
}

/// What the MCP server tells a client when it connects (MCP `instructions`). The bridge
/// (`oxide-mcp.exe`) answers `initialize` with these too, plus a line that Oxide must be running.
pub const INSTRUCTIONS: &str = "Controls the running DAW. Start with get_project to learn track, \
     clip and insert ids; positions are in samples at the project's sample rate. screenshot \
     shows the window as the user sees it. MIDI is in ticks (960 per quarter note) or \
     \"bar:beat:tick\" from 1: describe_midi_clip reads a clip as music (key, chords, note \
     names), add_notes takes those forms back, and the write_midi_part prompt walks through \
     writing a part. search_manual answers \"how do I…\" from Oxide's manual, and open_manual \
     shows the user a page of it.";

/// An action as MCP's `tools/list` shows it: the app's server publishes the same fields
/// (`daw-app`'s `mcp::tool`, checked equal by its tests).
pub fn mcp_tool(spec: &ActionSpec) -> Value {
    let schema = match &spec.input_schema {
        Value::Object(_) => spec.input_schema.clone(),
        _ => Value::Object(serde_json::Map::new()),
    };
    serde_json::json!({
        "name": spec.name,
        "description": spec.description,
        "inputSchema": schema,
        "annotations": {
            "readOnlyHint": spec.safety == Safety::Read,
            "destructiveHint": spec.safety == Safety::Destructive,
        },
    })
}

/// The prompts as MCP's `prompts/list` shows them.
pub fn mcp_prompts() -> Vec<Value> {
    prompts::prompts()
        .iter()
        .map(|p| {
            let arguments: Vec<Value> = p
                .arguments
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "name": a.name,
                        "description": a.description,
                        "required": a.required,
                    })
                })
                .collect();
            serde_json::json!({
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "arguments": arguments,
            })
        })
        .collect()
}

/// What the bridge (`oxide-mcp.exe`, dev plan 23.3) answers by itself, so Claude lists Oxide's
/// tools while Oxide is closed: the instructions, every tool and every prompt. Kept in
/// `crates/mcp-link/src/catalog.json` by `docs_are_current` (`UPDATE_DOCS=1`), which the bridge
/// embeds. It holds no version, so a release changes it only when the actions change.
pub fn catalog(registry: &Registry) -> Value {
    let tools: Vec<Value> = registry.specs().iter().map(mcp_tool).collect();
    serde_json::json!({
        "instructions": INSTRUCTIONS,
        "tools": tools,
        "prompts": mcp_prompts(),
    })
}

/// Everything the manual's MCP chapter is generated from (27.6), kept in
/// `crates/manual/src/mcp.json` by `mcp_docs_are_current` (`UPDATE_DOCS=1`; the quick tier
/// fails while it is stale): the areas, and per action its area, description, safety class,
/// whether it is an undo step, when it runs (with no project open, while one opens), and its
/// input and output schemas; the prompts with their message at their defaults. Not the
/// bridge's `catalog.json`, which MCP clients read as it is.
pub fn describe(registry: &Registry) -> Value {
    let areas: Vec<Value> = AREAS
        .iter()
        .map(|(id, title, about)| serde_json::json!({ "id": id, "title": title, "about": about }))
        .collect();
    let actions: Vec<Value> = registry
        .specs()
        .iter()
        .map(|spec| {
            serde_json::json!({
                "name": spec.name,
                "area": spec.area,
                "description": spec.description,
                "safety": safety_name(spec.safety),
                "undo": registry.undo(spec.name).name(),
                // Set calls on one object soon after each other are one step (`merge_key`).
                "merges": registry.undo(spec.name) != Undo::Never
                    && spec.name.starts_with("set_"),
                "without_project": spec.safety == Safety::Read
                    || WITHOUT_A_PROJECT.contains(&spec.name),
                "while_opening": ANSWERED_WHILE_OPENING.contains(&spec.name),
                "input": spec.input_schema,
                "output": spec.output_schema,
            })
        })
        .collect();
    let prompts: Vec<Value> = prompts::prompts()
        .iter()
        .map(|p| {
            let arguments: Vec<Value> = p
                .arguments
                .iter()
                .map(|a| {
                    serde_json::json!({
                        "name": a.name,
                        "description": a.description,
                        "required": a.required,
                    })
                })
                .collect();
            serde_json::json!({
                "name": p.name,
                "title": p.title,
                "description": p.description,
                "arguments": arguments,
                "text": p.text(&serde_json::Map::new()),
            })
        })
        .collect();
    serde_json::json!({
        "instructions": INSTRUCTIONS,
        "areas": areas,
        "actions": actions,
        "prompts": prompts,
    })
}

/// Markdown reference of every action, kept in `docs/mcp-tools.md`.
pub fn markdown(registry: &Registry) -> String {
    let mut out = String::from(
        "# MCP tools\n\n\
         The full reference is the manual's AI control (MCP) chapter: every action's input and \
         output fields, and examples the tests run against a headless session \
         (`docs/manual/mcp/`, and the pages generated from the registry: `mcp-reference.html` \
         and a page per area such as `mcp-tracks.html`, in `manual` beside the program, or in \
         `out/manual` after `cargo run -p manual`). This file is the short list.\n\n\
         Generated from the action registry (`daw-core/src/api`); do not edit by hand. \
         Regenerate with `$env:UPDATE_DOCS='1'; cargo test -p daw-core docs_are_current; Remove-Item Env:\\UPDATE_DOCS` (PowerShell) or `UPDATE_DOCS=1 cargo test -p daw-core docs_are_current` (bash).\n\n\
         The DAW serves these over MCP at `http://127.0.0.1:7878/mcp` while it runs. Connect \
         Claude Code and Claude Desktop from the app's Options > AI control (MCP): both then run \
         the bridge `oxide-mcp.exe`, which lists these tools even while Oxide is closed (a call \
         then answers that it isn't running). Or connect Claude Code by hand with `claude mcp \
         add --transport http --scope user oxide http://127.0.0.1:7878/mcp`, which connects \
         only while Oxide runs.\n\n\
         | tool | kind | summary |\n|---|---|---|\n",
    );
    let specs = registry.specs();
    for spec in &specs {
        let summary = spec
            .description
            .split(". ")
            .next()
            .unwrap_or(spec.description);
        out += &format!(
            "| [`{0}`](#{0}) | {1} | {2} |\n",
            spec.name,
            safety_name(spec.safety),
            summary.trim_end_matches('.')
        );
    }
    for spec in &specs {
        out += &format!("\n## {}\n\n{}\n\n", spec.name, spec.description);
        out += &format!("Kind: {}.", safety_name(spec.safety));
        let required: Vec<&str> = spec.input_schema["required"]
            .as_array()
            .map(|r| r.iter().filter_map(Value::as_str).collect())
            .unwrap_or_default();
        match spec.input_schema["properties"].as_object() {
            Some(props) if !props.is_empty() => {
                out += "\n\n| argument | type | required | description |\n|---|---|---|---|\n";
                for (name, prop) in props {
                    let ty = match (&prop["type"], &prop["$ref"]) {
                        (Value::String(t), _) => t.clone(),
                        (Value::Array(ts), _) => ts
                            .iter()
                            .filter_map(Value::as_str)
                            .filter(|t| *t != "null")
                            .collect::<Vec<_>>()
                            .join(" or "),
                        (_, Value::String(r)) => r.rsplit('/').next().unwrap_or(r).to_string(),
                        _ => "any".to_string(),
                    };
                    let description = prop["description"].as_str().unwrap_or("");
                    let required = if required.contains(&name.as_str()) {
                        "yes"
                    } else {
                        ""
                    };
                    out += &format!("| `{name}` | {ty} | {required} | {description} |\n");
                }
            }
            _ => out += " No arguments.\n",
        }
    }
    out += "\n# Prompts\n\nGuides a client can offer (MCP `prompts/get`); each is one message.\n";
    for prompt in prompts::prompts() {
        out += &format!(
            "\n## {} ({})\n\n{}\n",
            prompt.name, prompt.title, prompt.description
        );
        if !prompt.arguments.is_empty() {
            out += "\n| argument | required | description |\n|---|---|---|\n";
            for arg in prompt.arguments {
                let required = if arg.required { "yes" } else { "" };
                out += &format!("| `{}` | {required} | {} |\n", arg.name, arg.description);
            }
        }
    }
    out
}

fn safety_name(safety: Safety) -> &'static str {
    match safety {
        Safety::Read => "read",
        Safety::Edit => "edit",
        Safety::Transport => "transport",
        Safety::Destructive => "destructive",
    }
}

/// A host with just a session, for tests and headless tools.
pub struct SessionHost<'a>(pub &'a mut Session);

impl Host for SessionHost<'_> {
    fn session(&mut self) -> &mut Session {
        self.0
    }
}
