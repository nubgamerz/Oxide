//! Top-level app state and layout.

use crate::mcp::Mcp;
use crate::settings::AppSettings;
use crate::{arrange, browser, dialogs, piano_roll, theme, transport, zoom, Options};
use daw_core::api::{
    ActionOutput, ApiError, ApiResult, Host, Registry, Reply, TimeSelection, ViewChange, ViewMode,
    Zoom, MAX_WAVEFORM_SCALE,
};
use daw_core::automation::{LaneId, PointId};
use daw_core::export::Normalize;
use daw_core::grid::{Division, Grid};
use daw_core::project::{Clip, ClipId, TrackId, TrackKind, MAX_TRACK_HEIGHT, MIN_TRACK_HEIGHT};
use daw_core::session::{ExportStatus, InsertTarget, Session};
use daw_core::util::gain_to_db;
use egui::{Color32, RichText};
use serde_json::json;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// What is selected in the arrangement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Selection {
    Track(TrackId),
    Master,
}

/// Peak meter with fall-off, fed once per frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Meter {
    pub level: [f32; 2],
    pub hold: [f32; 2],
    hold_age: [f32; 2],
}

impl Meter {
    #[allow(clippy::needless_range_loop)] // per-channel state in parallel arrays
    fn push(&mut self, peak: [f32; 2], dt: f32) {
        for c in 0..2 {
            let fall = 10f32.powf(-24.0 * dt / 20.0); // 24 dB/s
            self.level[c] = peak[c].max(self.level[c] * fall);
            self.hold_age[c] += dt;
            if peak[c] >= self.hold[c] || self.hold_age[c] > 1.5 {
                self.hold[c] = peak[c].max(self.level[c]);
                self.hold_age[c] = 0.0;
            }
        }
    }

    pub fn db(&self) -> [f32; 2] {
        [gain_to_db(self.level[0]), gain_to_db(self.level[1])]
    }
}

pub struct Toast {
    pub text: String,
    pub at: Instant,
    /// How long it stays (a failure that loses work stays longer).
    pub lasts: Duration,
}

/// Clips being dragged together.
pub struct GroupDrag {
    /// The clip whose widget is being dragged (the one pressed on).
    pub grip: ClipId,
    /// The clip under the pointer: the others follow it (`grip`, or with Alt its copy).
    pub anchor: ClipId,
    /// Every dragged clip as the drag began.
    pub origin: Vec<(TrackId, daw_core::project::AnyClip)>,
    /// The anchor's start then, in samples, and how far the pointer has moved since.
    pub anchor_start: f64,
    pub moved: f64,
    /// The anchor's track row then.
    pub row: usize,
    /// The pointer, and whether the drag moved this frame: the move is applied once every row
    /// has been drawn (`arrange::apply_clip_drag`), when the rows below are known too.
    pub pointer: Option<egui::Pos2>,
    pub due: bool,
    /// The frame that last counted the pointer's movement: a clip moved onto a lower track is
    /// drawn (and asks) a second time in that frame.
    pub frame: u64,
    /// The move as it stands: samples, and rows (the last the tracks took).
    pub delta: i64,
    pub rows: i64,
    /// The track under the pointer that can't take the clips, shown as such.
    pub refused: Option<TrackId>,
    /// Alt-drag: the dragged clips are copies made as it began.
    pub copies: bool,
    /// Ctrl as last seen (the magnet off), and whether the magnet holds the anchor on the
    /// playhead (its line is lit): `arrange::snap_clip`, 17.5.
    pub ctrl: bool,
    pub magnet: bool,
    /// The pointer has left the point it was pressed at (25.1): until then the clips stay
    /// exactly where they were (a long press starts a drag without a move, and must not snap
    /// them to the grid).
    pub moving: bool,
}

/// A slot of a rack: which rack, and the slot's place in it.
pub type RackPlace = (InsertTarget, usize);

pub struct DawApp {
    pub session: Session,
    /// The start screen (new or existing project): up whenever no project is open.
    pub start: crate::start::StartScreen,
    /// The window title last sent, so it is sent only when the project changes.
    window_title: String,
    pub settings: AppSettings,
    pub selection: Option<Selection>,
    /// The clip the inspector, piano roll and zoom act on: the last one picked. Always one
    /// of `selected_clips` while that isn't empty; change both through `select_clip` and
    /// `clear_clip_selection`.
    pub selected_clip: Option<(TrackId, ClipId)>,
    /// Every selected clip, audio or MIDI: what Delete, split, copy, cut, duplicate and a
    /// drag act on.
    pub selected_clips: std::collections::BTreeSet<ClipId>,
    /// A drag moving the selected clips: the clips as it began, so each frame moves them
    /// from there.
    pub group_drag: Option<GroupDrag>,
    /// The time selection: a range across some tracks, made by dragging in empty lane space
    /// (`arrange::time_selection_drag`). Delete, S and L act on it while it is there.
    pub time_selection: Option<TimeSelection>,
    /// A time selection being dragged on the lanes.
    pub range_drag: Option<crate::arrange::RangeDrag>,
    /// Where the lanes were last right-clicked: the track and the sample (for the lane menu).
    pub lane_menu_at: Option<(TrackId, u64)>,
    /// Installed plug-ins whose moduleinfo says they're instruments, read once.
    declared_instruments: Option<Vec<std::path::PathBuf>>,
    /// Export only the time selection (the export dialog's choice, while there is one).
    pub export_selection: bool,
    /// The main area: the arrangement, or the Waveform view's file.
    pub view_mode: ViewMode,
    /// How the Waveform view shows its files.
    pub editor: crate::waveform::Editor,
    /// The automation point last clicked: Delete removes it (unless a clip was picked since).
    pub selected_point: Option<(LaneId, PointId)>,
    /// Each plug-in's automatable parameters (id, name) by its path, read once: reading them
    /// locks the plug-in, which silences its track for a block.
    pub param_names: HashMap<PathBuf, Vec<(u32, String)>>,
    /// The add-lane menu's parameter filter.
    pub param_filter: String,
    /// Timeline view: first visible sample and zoom.
    pub view_start: f64,
    pub samples_per_pixel: f64,
    /// Width of the timeline lanes in points, as last drawn (for view ranges).
    pub lanes_width: f32,
    /// Waveform display gain (1 to 16): makes quiet recordings readable. Display only.
    pub waveform_scale: f32,
    /// The MIDI clip open in the piano roll, and how it's shown.
    pub piano_roll: Option<piano_roll::PianoRoll>,
    /// Dropped files waiting for the answer to the tempo question.
    pub pending_drop: Option<crate::drop::PendingDrop>,
    /// The visible track rows as last drawn, for dropping files on them.
    pub drop_rows: Vec<(TrackId, TrackKind, egui::Rect)>,
    /// Where the master's row (and its lanes) is this frame: files dropped there are refused.
    pub master_row: Option<egui::Rect>,
    /// The Multitrack's rows' scroll area as last drawn (its visible rect) and its vertical
    /// offset, unrounded (egui draws the rows at it rounded to a pixel), for keeping the rows
    /// in place while Alt+wheel changes their heights (26.1).
    pub track_view: Option<(egui::Rect, f32)>,
    /// The rows' vertical scroll offset to draw this frame, set where their heights change
    /// (`arrange::handle_zoom_and_scroll`) and taken by the scroll area the same frame.
    pub track_scroll_to: Option<f32>,
    /// The wheel notch going on started with Alt held: `Some(shift)`. egui spreads a notch
    /// over several frames, so the rest of it keeps changing the heights (or with Shift the
    /// waveform scale) after Alt is let go, never scrolling the rows.
    pub alt_wheel: Option<bool>,
    pub show_mixer: bool,
    pub show_inspector: bool,
    /// The Diagnostics pane (the Waveform view's Click Repair and Clip Repair list), and its state.
    pub show_diagnostics: bool,
    pub diagnostics: crate::diagnostics::Panel,
    /// The panes: where the Multitrack view, mixer, inspector, piano roll and files are.
    pub dock: crate::dock::Dock,
    /// View > Layouts > Save layout as…, while open: the name typed.
    pub layout_name: Option<String>,
    /// The MIDI clip the piano roll last showed: View > Piano roll opens it again.
    pub last_piano_clip: Option<daw_core::project::ClipId>,
    /// "Create multi-out rack in a submix", while its dialog is open.
    pub multi_out: Option<crate::multi_out::Dialog>,
    /// A job waiting for the busy overlay to be on screen (`busy::run`).
    pub busy: Option<crate::busy::Job>,
    /// Frames the busy overlay has been painted (the screenshot check's cue).
    pub busy_frames: u32,
    /// The screenshot check (`--show busy`) is taken once the overlay has been painted.
    pub screenshot_on_busy: bool,
    /// Frames the busy bar has been painted, for what it is up for (`opening::hold`).
    pub bar_painted: crate::opening::BarPainted,
    /// The save check (`--show save`) holds a save's reads for its shot (`frame_check`).
    pub hold_save_for_shot: bool,
    /// The check run's timed open (`--show open-files` / `open-plugins`).
    pub open_check: Option<crate::opening::Check>,
    /// The check run's timed undo or render (`--show undo-reload` / `render-progress`).
    pub frame_check: Option<crate::frame_check::Check>,
    /// The waveform zoom shots' drawing time (`--show clip-samples` and the like).
    pub draw_timer: Option<crate::frame_check::DrawTimer>,
    /// Closing or replacing the project, waiting on "Save the changes?" (`unsaved`).
    pub unsaved: Option<crate::unsaved::Pending>,
    /// The file dialog that is up, shown on a thread of its own (`file_dialog`).
    pub file_dialog: Option<crate::file_dialog::Pending>,
    /// Agents' tool calls run so far (the file dialog check counts those answered while its
    /// dialog is up).
    pub mcp_calls: u64,
    /// The close was settled (saved, discarded or nothing to lose): the window may go.
    pub closing: bool,
    /// The check's answer to the save question (`--answer`), given once it is up.
    pub answer: Option<String>,
    /// A track being dragged to another place (`track_drag`).
    pub track_drag: Option<crate::track_drag::TrackDrag>,
    /// Where each mixer strip was drawn this frame, for a strip being dragged.
    pub strip_rects: Vec<(TrackId, TrackKind, egui::Rect)>,
    pub meters: HashMap<TrackId, Meter>,
    pub master_meter: Meter,
    /// Worst block's DSP load, held for a moment so a spike stays readable.
    pub dsp_peak: f32,
    /// The CPU panel (click the DSP readout) is open.
    pub show_cpu: bool,
    /// File > Open backup… is open.
    pub show_backups: bool,
    /// Help > About Oxide… is open.
    pub show_about: bool,
    /// File > Save as template…, while open.
    pub save_template: Option<crate::templates::SaveTemplate>,
    /// File > Templates…, while open.
    pub templates_window: Option<crate::templates::TemplatesWindow>,
    /// A track's render dialog (track menu > Render track…), while open.
    pub render_dialog: Option<crate::render::RenderDialog>,
    /// The Waveform view's export dialog (File > Export file…) for this document, while open.
    pub export_document: Option<daw_core::session::DocumentId>,
    /// View > Metadata, while open.
    pub metadata_panel: Option<crate::metadata_panel::MetadataPanel>,
    /// Where each fx rack's slots were drawn last frame (the rack, the slot, the slot's row
    /// and its drag handle): for the screenshot check's drag between racks.
    pub rack_slots: Vec<(InsertTarget, usize, egui::Rect, Option<egui::Rect>)>,
    /// The screenshot check's drag (`--show rack-move`): from one rack slot to another, and
    /// how many frames it has run.
    pub demo_drag: Option<(RackPlace, RackPlace, u32)>,
    /// Where each clip was drawn this frame (the screenshot check's clip drag finds it here).
    pub clip_rects: Vec<(ClipId, egui::Rect)>,
    /// The line the last copy or cut put on the system clipboard (`clipboard::announce`), so
    /// that Ctrl+V arrives as an `Event::Paste`; a check run only keeps it here.
    pub clipboard_line: Option<String>,
    /// The screenshot check's clip drag (`--show clip-drag`): the clip, the track row to drag
    /// it over, the step, and where the press began.
    pub demo_clip_drag: Option<(ClipId, TrackId, u32, Option<egui::Pos2>)>,
    /// `--show clip-magnet`: the clip drag above, its shot taken once the magnet holds.
    pub demo_magnet: bool,
    /// The screenshot check's track drag (`--show track-drag`, `track-drag-mixer`): the track,
    /// the one to drag it over, whether in the mixer, the step, and where the press began.
    pub demo_track_drag: Option<(TrackId, TrackId, bool, u32, Option<egui::Pos2>)>,
    /// Where the playhead's handle is (a point on it under the ruler) and the lanes' left
    /// edge, as drawn this frame: the screenshot check's playhead drag finds them here.
    pub playhead_grab: Option<(egui::Pos2, f32)>,
    /// Where and when (egui's time) the last click was: while a second press there would make
    /// it a double-click, the playhead's handle in the Multitrack (26.2) and its line in the
    /// piano roll (26.4) take no presses (`arrange::double_click_waits`).
    pub last_click: Option<(egui::Pos2, f64)>,
    /// The playhead handle's egui id, as the Multitrack made it this frame: the driven
    /// `--show playhead-drag` asks egui whether it is still the one dragged.
    pub playhead_handle: Option<egui::Id>,
    /// The Multitrack's playhead drag under way (26.3): where it put the playhead and where
    /// the pointer was last seen, so the drag lasts until the button is let go wherever the
    /// pointer goes and however late the engine's position follows.
    pub(crate) playhead_drag: Option<crate::arrange::PlayheadDrag>,
    /// The screenshot check's playhead drag (`--show playhead-edge`): the step, and where the
    /// press began. The shot is taken once the view has scrolled to the start.
    pub demo_playhead_drag: Option<(u32, Option<egui::Pos2>)>,
    /// The screenshot check's pointer over the Waveform tab (`--show tab-hover`).
    pub demo_tab_hover: bool,
    /// The window's handle (Windows), to ask where the pointer is during a file drag.
    pub hwnd: Option<isize>,
    /// Where the pointer is while files hover over the window or drop, this frame
    /// (`drop::os_pointer`): winit sends none then.
    pub drag_pointer: Option<egui::Pos2>,
    /// The clips hovering files would make, for the ghost under the pointer.
    pub drop_ghost: Option<crate::drop::Ghost>,
    /// The screenshot check's file drag (`--show drop-ghost`, `drop-question`): the file, the
    /// track to hover it over, whether to drop it, and the step.
    pub demo_drop: Option<(PathBuf, TrackId, bool, u32)>,
    /// The screenshot check's click on the piano roll's ruler (`--show piano-roll-playhead`):
    /// the song tick aimed at (clicked 3 points off it, so snapping brings it back), the
    /// sample the playhead must land on, and the step. The shot waits for it to land there.
    pub demo_ruler: Option<(u64, u64, u32)>,
    /// The screenshot check's right-click on a rack slot (`--show plugin-paste`), and how
    /// many frames it has waited.
    pub demo_right_click: Option<(RackPlace, u32)>,
    /// The project just opened has this newer backup: ask whether to open it.
    pub backup_question: Option<PathBuf>,
    /// A project was just opened: its newer backup (if any) is asked about once its plug-ins
    /// are in, which is when the session has looked for one.
    pub backup_question_due: bool,
    dsp_peak_at: Instant,
    pub input_meters: Vec<f32>,
    /// Hardware output levels, after the master and the metronome.
    pub output_meters: Vec<f32>,
    pub toasts: Vec<Toast>,
    pub browser: browser::Browser,
    /// Options > Settings…, while open.
    pub settings_dialog: Option<crate::settings_window::SettingsDialog>,
    /// The recent projects' files, looked at on workers for the start screen.
    pub recent_probe: crate::recent::Probe,
    pub exporting: Option<(f32, daw_core::session::ExportKind, String)>,
    /// The export options dialog is open.
    pub export_options: bool,
    /// Seconds of tail after the end, for exports.
    pub export_tail: f64,
    /// The last export's outcome, while its report is shown.
    pub export_report: Option<ExportStatus>,
    /// The clip as it was when a drag of the inspector's stretch field began: each frame
    /// stretches from it, so roundings never add up.
    pub stretch_gesture: Option<(ClipId, Clip)>,
    /// A drag on a clip edge under way (`arrange::EdgeDrag`).
    pub edge_drag: Option<crate::arrange::EdgeDrag>,
    /// The clip edge lit this frame (the pointer on it, or it being dragged), as the lanes
    /// chose it (25.1): the driven `clip-edges` check reads it.
    pub edge_lit: Option<crate::arrange::LitEdge>,
    /// The edge being trimmed is held on the playhead by the magnet (its line is lit).
    pub edge_magnet: bool,
    /// The slicer window, open on a clip (audio quantize, 8.6).
    pub slicer: Option<crate::quantize::Slicer>,
    /// The quantize window and its settings.
    pub quantize: crate::quantize::QuantizeOptions,
    /// The marker being renamed, and its name so far.
    pub renaming_marker: Option<(u32, String)>,
    /// Where the marker strip's menu was opened (samples).
    pub marker_menu_at: u64,
    pub follow_playhead: bool,
    /// The piano roll moved the playhead: the arrangement shows it, scrolling there if it is
    /// off screen.
    pub reveal_playhead: bool,
    /// A drag on the ruler: where it started (snapped samples), and whether it draws the punch
    /// range (Shift) rather than the loop.
    pub ruler_drag: Option<(f64, bool)>,
    /// The bar a right-click on the ruler opened the tempo menu for.
    pub ruler_menu_bar: i64,
    /// Where the ruler menu's Grid button was last drawn, for the screenshot check to point
    /// at it (`--show grid-menu`).
    pub grid_menu_button: Option<egui::Rect>,
    /// The Multitrack's ruler as last drawn, and the screenshot check's right-click on it and
    /// then on to its Grid button (`--show grid-menu`): the step.
    pub ruler_rect: Option<egui::Rect>,
    pub demo_grid_menu: Option<u32>,
    /// Where the screenshot check presses S once the playhead is there (`--show midi-split`).
    demo_split_at: Option<u64>,
    /// The driven check of typed names and volumes (`--show rename`, `volume-edit`).
    pub edit_check: Option<crate::edit_check::Check>,
    /// Where each pane and window was drawn, for F1 (`help.rs`).
    pub help: crate::help::Here,
    /// The manual's pages a check run asked to show (Help > Manual, F1, `open_manual`): check
    /// runs never start a browser (`DawApp::open_manual`).
    pub manual_requests: Vec<daw_core::manual::Target>,
    /// `--neutral` (the manual's shots): what is drawn names no machine's devices or paths.
    pub neutral: bool,
    /// `--vst3 ROLE=NAME`: the VST3s the check scenarios use, by role ([`Self::demo_effect`]).
    demo_vst3: Vec<(String, String)>,
    /// The Waveform view's rack presets: the menu, its questions (`rack_presets`).
    pub rack_presets: crate::rack_presets::State,
    /// The project's tempo map, built once a frame for drawing.
    pub tempo_map: daw_core::tempo::TempoMap,
    last_frame: Instant,
    screenshot: Option<PathBuf>,
    /// Where the screenshot's marks go (`--marks`, `marks.rs`).
    marks: Option<PathBuf>,
    /// A check run (`--demo`): settings are never saved, so a check can't change the user's.
    checking: bool,
    /// Take the screenshot this long after starting, instead of after 150 frames.
    screenshot_wait: Option<Duration>,
    screenshot_taken: bool,
    started: Instant,
    frames: u32,
    /// Every action an agent (or, increasingly, the UI) can run.
    registry: Rc<Registry>,
    /// The MCP server, started and stopped on a worker; its status is the status bar's.
    pub mcp: Mcp,
    /// `--mcp-port`: the port this run serves on until the settings change it.
    mcp_port_override: Option<u16>,
    /// `--mcp-token`: the token this run requires, whatever the settings say.
    mcp_token_override: Option<String>,
    /// `--show mcp-port-in-use`: the app holds its own MCP port, so the server finds it taken.
    held_port: Option<std::net::TcpListener>,
    /// Claude Code's and Claude Desktop's link to the server (Settings > AI control).
    pub claude: crate::mcp_settings::Link,
    /// Agents' opens behind the bar, each answered when its open ends.
    agent_opens: crate::opening::AgentOpens,
    /// Agents' saves, answered once written (20.1).
    agent_saves: crate::opening::AgentSaves,
    pub egui_ctx: egui::Context,
    exit_at: Option<Instant>,
}

impl DawApp {
    pub fn new(cc: &eframe::CreationContext, options: Options) -> Self {
        theme::apply(&cc.egui_ctx);
        // The manual's shots (27.2): the parts drawn are noted only when asked.
        if options.marks.is_some() {
            crate::marks::turn_on();
        }
        let settings = AppSettings::load();
        // The manual's shots (`--neutral`): no device opened, a virtual one runs the engine.
        let mut session = if options.neutral {
            let mut session = Session::new(crate::neutral::session_settings());
            if let Err(e) = session.start_virtual_device(crate::neutral::BUFFER as usize) {
                println!("FAIL: the neutral run's virtual device: {e:#}");
            }
            session
        } else {
            Session::new(settings.audio.clone())
        };
        // The manual (27.3): `manual\` beside the program, a development build's out\manual,
        // or a check's own (`--manual`).
        session.manual_dir = match &options.manual {
            Some(dir) => Some(std::path::absolute(dir).unwrap_or_else(|_| dir.clone())),
            None => crate::help::manual_dir(),
        };
        let samples_per_pixel = f64::from(session.sample_rate()) / 60.0;
        let registry = Rc::new(Registry::new());
        // The MCP worker, idle until asked below; the tools are made from the specs there.
        let mcp = Mcp::new(registry.specs(), cc.egui_ctx.clone());
        let mut app = Self {
            session,
            settings,
            selection: None,
            selected_clip: None,
            selected_clips: Default::default(),
            group_drag: None,
            time_selection: None,
            range_drag: None,
            lane_menu_at: None,
            declared_instruments: None,
            export_selection: true,
            view_mode: ViewMode::Multitrack,
            editor: Default::default(),
            selected_point: None,
            param_names: HashMap::new(),
            param_filter: String::new(),
            view_start: 0.0,
            samples_per_pixel,
            lanes_width: 900.0,
            waveform_scale: 1.0,
            piano_roll: None,
            pending_drop: None,
            drop_rows: Vec::new(),
            show_mixer: options.show_mixer,
            show_inspector: true,
            show_diagnostics: false,
            diagnostics: Default::default(),
            dock: Default::default(),
            layout_name: None,
            last_piano_clip: None,
            multi_out: None,
            busy: None,
            busy_frames: 0,
            screenshot_on_busy: false,
            bar_painted: Default::default(),
            hold_save_for_shot: false,
            open_check: None,
            frame_check: None,
            draw_timer: None,
            file_dialog: None,
            mcp_calls: 0,
            unsaved: None,
            closing: false,
            answer: options.answer.clone(),
            track_drag: None,
            strip_rects: Vec::new(),
            meters: HashMap::new(),
            master_meter: Meter::default(),
            master_row: None,
            track_view: None,
            track_scroll_to: None,
            alt_wheel: None,
            dsp_peak: 0.0,
            show_cpu: false,
            show_backups: false,
            show_about: false,
            save_template: None,
            templates_window: None,
            render_dialog: None,
            export_document: None,
            metadata_panel: None,
            rack_slots: Vec::new(),
            demo_drag: None,
            clip_rects: Vec::new(),
            clipboard_line: None,
            demo_clip_drag: None,
            demo_magnet: false,
            demo_track_drag: None,
            playhead_grab: None,
            last_click: None,
            playhead_handle: None,
            playhead_drag: None,
            demo_playhead_drag: None,
            demo_tab_hover: false,
            hwnd: window_handle(cc),
            drag_pointer: None,
            drop_ghost: None,
            demo_drop: None,
            demo_ruler: None,
            demo_right_click: None,
            backup_question: None,
            backup_question_due: false,
            dsp_peak_at: Instant::now(),
            input_meters: Vec::new(),
            output_meters: Vec::new(),
            toasts: Vec::new(),
            browser: browser::Browser::default(),
            settings_dialog: None,
            recent_probe: crate::recent::Probe::new(Some(cc.egui_ctx.clone())),
            exporting: None,
            export_options: false,
            export_tail: 2.0,
            export_report: None,
            stretch_gesture: None,
            edge_drag: None,
            edge_lit: None,
            edge_magnet: false,
            renaming_marker: None,
            slicer: None,
            quantize: Default::default(),
            marker_menu_at: 0,
            follow_playhead: true,
            reveal_playhead: false,
            start: Default::default(),
            window_title: String::new(),
            ruler_drag: None,
            ruler_menu_bar: 0,
            grid_menu_button: None,
            ruler_rect: None,
            demo_grid_menu: None,
            demo_split_at: None,
            edit_check: None,
            help: Default::default(),
            manual_requests: Vec::new(),
            neutral: options.neutral,
            demo_vst3: options.demo_vst3.clone(),
            rack_presets: Default::default(),
            tempo_map: daw_core::tempo::TempoMap::constant(48_000, 120.0, 4, 4),
            last_frame: Instant::now(),
            screenshot: options.screenshot,
            marks: options.marks,
            checking: options.demo.is_some(),
            screenshot_wait: options.wait.map(Duration::from_secs_f64),
            screenshot_taken: false,
            started: Instant::now(),
            frames: 0,
            registry,
            mcp,
            mcp_port_override: options.mcp_port,
            mcp_token_override: options.mcp_token.clone(),
            held_port: None,
            claude: crate::mcp_settings::Link::new(
                crate::mcp_settings::locations(options.claude_profile.as_deref()),
                cc.egui_ctx.clone(),
            ),
            agent_opens: Default::default(),
            agent_saves: Default::default(),
            egui_ctx: cc.egui_ctx.clone(),
            exit_at: options
                .exit_after
                .map(|secs| Instant::now() + Duration::from_secs_f64(secs)),
        };
        // The app always works inside a project folder: with none open, recording and
        // importing are refused rather than writing somewhere else.
        app.session.scratch = None;
        // Opened projects load their plug-ins a few a frame, behind the busy overlay.
        app.session.stage_loads = true;
        app.settings.apply_backups(&mut app.session);
        // The layout last used; check runs (`--demo`) always start from the default, so
        // their screenshots don't depend on how the user left the window.
        if options.demo.is_none() {
            if let Some(layout) = app.settings.layout.clone() {
                app.load_layout(layout);
            }
        }
        // Templates are app-wide, beside the program; a check run (`--demo`) keeps its own in
        // `out/check`, away from the user's.
        app.session.templates_dir = Some(if options.demo.is_some() {
            std::path::absolute("out/check/Templates").unwrap_or_default()
        } else {
            crate::settings::app_dir().join("Templates")
        });
        // The Waveform view's effects rack is kept beside the program (`rack.json`, not in
        // settings.json: a plug-in's state can be megabytes) and comes back now, its plug-ins a
        // frame at a time behind the busy overlay. Its presets too (`RackPresets`). A check run
        // neither reads nor writes the user's: no kept rack (a `--show` may give it one in
        // `out/check`), presets in `out/check/RackPresets` (the test `save_settings` makes).
        if app.checking || app.screenshot.is_some() {
            app.session.rack_presets_dir = std::path::absolute("out/check/RackPresets").ok();
        } else {
            app.session.rack_file = Some(crate::settings::app_dir().join("rack.json"));
            app.session.rack_presets_dir = Some(crate::settings::app_dir().join("RackPresets"));
            if let Err(e) = app.session.restore_rack() {
                app.toast(format!("The effects rack: {e:#}"));
            }
            // Claude Desktop runs a copy of the bridge (`mcp-bridge\`), which an update of
            // the program doesn't touch: once Desktop has one, bring it up to this build's, on
            // a worker (the same bytes: nothing is written).
            if let Some(locations) = app.claude.locations() {
                if options.claude_profile.is_none() {
                    let _ = std::thread::Builder::new()
                        .name("bridge-refresh".into())
                        .spawn(move || {
                            if locations.bridge.is_file() {
                                if let Err(e) = mcp_link::clients::install_bridge(&locations) {
                                    log::warn!("refreshing Claude Desktop's bridge: {e}");
                                }
                            }
                        });
                }
            }
        }
        // MCP starts on its worker (the frame isn't held up binding), before any project: it
        // serves the start screen, opens and new projects alike.
        if options.no_mcp {
            app.mcp.stop(Some("--no-mcp".into()));
        } else if app.settings.mcp.enabled {
            if options.show.as_deref() == Some("mcp-port-in-use") {
                // The port taken before the server tries it, as another program would.
                app.held_port = std::net::TcpListener::bind(("127.0.0.1", app.mcp_port())).ok();
            }
            app.start_mcp();
        }
        if options.minimize {
            cc.egui_ctx
                .send_viewport_cmd(egui::ViewportCommand::Minimized(true));
        }
        if let Some(path) = &options.open {
            app.open_project(path);
        }
        if let Some(wav) = &options.demo {
            app.build_demo(wav, &options.demo_plugins);
            app.demo_recents();
        }
        if options.mute {
            app.session.project.master.mixer.mute = true;
        }
        if options.silent {
            app.session.set_silent(true);
        }
        if options.play {
            app.session.update();
            app.session.play();
        }
        if options.editor {
            app.open_first_editor();
        }
        match options.show.as_deref() {
            Some("browser") => {
                let target = match app.selection {
                    Some(Selection::Track(id)) => InsertTarget::Track(id),
                    _ => InsertTarget::Master,
                };
                app.browser.open_for(target);
            }
            Some("native") => {
                // Amplify (a built-in effect) on the first track, +3 dB left, with its editor.
                let first = app.session.project.tracks.first().map(|t| t.id);
                let added = first.map(|track| {
                    app.session.add_insert(
                        InsertTarget::Track(track),
                        &daw_core::native::path_of(&daw_core::native::amplify::KIND),
                    )
                });
                match added {
                    Some(Ok(id)) => {
                        let three = (3.0 + 96.0) / 144.0;
                        let _ = app.session.set_parameter(id, 0, three);
                        let _ = app.session.toggle_editor(id);
                        app.selection = first.map(Selection::Track);
                    }
                    Some(Err(e)) => app.toast(format!("Amplify: {e:#}")),
                    None => {}
                }
            }
            Some(which) if which.starts_with("effect:") => {
                // The manual's (27.5): any built-in effect by its id (`effect:hard-limiter`) on
                // the first track at its defaults, with its editor open, for the effects that
                // have no scenario of their own.
                let id = &which["effect:".len()..];
                let first = app.session.project.tracks.first().map(|t| t.id);
                match (daw_core::native::find(id), first) {
                    (Some(kind), Some(track)) => {
                        let path = daw_core::native::path_of(kind);
                        match app.session.add_insert(InsertTarget::Track(track), &path) {
                            Ok(insert) => {
                                let _ = app.session.toggle_editor(insert);
                                app.selection = Some(Selection::Track(track));
                            }
                            Err(e) => app.toast(format!("{}: {e:#}", kind.name)),
                        }
                    }
                    (None, _) => app.toast(format!("No built-in effect is called {id}")),
                    (_, None) => {}
                }
            }
            Some(
                which @ ("eq" | "graphic-eq" | "compressor" | "multiband" | "dehummer"
                | "auto-click-remover" | "declipper" | "click-pop"),
            ) => {
                // The Parametric or 10-band Graphic EQ, the Compressor or Multiband
                // Compressor, the Hum Remover (50 Hz and five harmonics), the Click Remover,
                // Clip Repair or the Click & Pop Repair (its defaults; for both repairs, the
                // first clip's file scanned into it), on the first track, set, with its editor
                // and curve.
                let kind = match which {
                    "eq" => &daw_core::native::parametric_eq::KIND,
                    "graphic-eq" => &daw_core::native::graphic_eq::KIND_10,
                    "multiband" => &daw_core::native::multiband::KIND,
                    "dehummer" => &daw_core::native::dehummer::KIND,
                    "auto-click-remover" => &daw_core::native::click_remover::KIND,
                    "declipper" => &daw_core::native::declipper::KIND,
                    "click-pop" => &daw_core::native::click_pop::KIND,
                    _ => &daw_core::native::compressor::KIND,
                };
                let first = app.session.project.tracks.first().map(|t| t.id);
                let added = first.map(|track| {
                    app.session
                        .add_insert(InsertTarget::Track(track), &daw_core::native::path_of(kind))
                });
                match added {
                    Some(Ok(id)) => {
                        let settings: &[(&str, f64)] = if which == "dehummer" {
                            &[
                                ("Frequency", 50.0),
                                ("Number of Harmonics", 5.0),
                                ("Gain", -45.0),
                            ]
                        } else if which == "auto-click-remover" {
                            &[("Threshold", 40.0), ("Complexity", 25.0)]
                        } else if which == "declipper" {
                            &[("Gain", -6.0), ("Tolerance", 15.0)]
                        } else if which == "click-pop" {
                            &[]
                        } else if which == "multiband" {
                            &[
                                ("Band 1 Threshold", -22.0),
                                ("Band 1 Ratio", 2.3),
                                ("Band 2 Threshold", -22.0),
                                ("Limiter Threshold", -12.0),
                                ("Limiter Margin", -1.0),
                                ("Brickwall Limiter", 1.0),
                            ]
                        } else if which == "compressor" {
                            &[
                                ("Threshold", -24.0),
                                ("Ratio", 4.0),
                                ("Attack", 5.0),
                                ("Release", 150.0),
                                ("Output Gain", 4.0),
                            ]
                        } else if which == "eq" {
                            &[
                                ("High-Pass Enable", 1.0),
                                ("High-Pass Frequency", 80.0),
                                ("Low Shelf Gain", 3.0),
                                ("Low Shelf Frequency", 150.0),
                                ("Band 2 Gain", -4.0),
                                ("Band 3 Frequency", 1000.0),
                                ("Band 3 Gain", 6.0),
                                ("High Shelf Gain", -6.0),
                                ("High Shelf Frequency", 9000.0),
                            ]
                        } else {
                            &[
                                ("63 Hz", 4.0),
                                ("250 Hz", -3.0),
                                ("2 kHz", 5.0),
                                ("8 kHz", 2.0),
                            ]
                        };
                        let _ = app.set_named(id, kind, settings);
                        if which == "declipper" || which == "click-pop" {
                            // Scanned from the first clip's whole file (the DI is not clipped:
                            // its loudest samples become the levels, as a scan of the same
                            // selection makes them).
                            if let Err(e) = app.scan_first_clip(id) {
                                app.toast(format!("Scan: {e:#}"));
                            }
                        }
                        let _ = app.session.toggle_editor(id);
                        app.selection = first.map(Selection::Track);
                    }
                    Some(Err(e)) => app.toast(format!("{}: {e:#}", kind.name)),
                    None => {}
                }
            }
            Some(which @ ("delay" | "echo" | "analog-delay" | "chorus" | "flanger" | "phaser")) => {
                // A delay or modulation effect on Guitar L, set, with its editor open: the
                // Delay with its left copy early (a latency), the Echo bouncing, the Vintage
                // Delay on tape and tube, the Chorus with 4 voices at Highest Quality, the
                // Flanger and the Phaser (6 stages, a sine).
                if let Err(e) = app.show_delay(which) {
                    app.toast(format!("{which}: {e:#}"));
                }
            }
            Some("studio-reverb") => {
                // The Room Reverb on the demo's Reverb bus (fed by both guitars and the
                // vocal), a medium room, with its editor and decay sketch.
                let kind = &daw_core::native::studio_reverb::KIND;
                let bus = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.name == "Reverb")
                    .or(app.session.project.tracks.first())
                    .map(|t| t.id);
                let added = bus.map(|track| {
                    app.session
                        .add_insert(InsertTarget::Track(track), &daw_core::native::path_of(kind))
                });
                match added {
                    Some(Ok(id)) => {
                        let _ = app.set_named(
                            id,
                            kind,
                            &[
                                ("Room Size", 50.0),
                                ("Decay", 1800.0),
                                ("Width", 60.0),
                                ("Dry", 70.0),
                                ("Wet", 35.0),
                            ],
                        );
                        let _ = app.session.toggle_editor(id);
                        app.selection = bus.map(Selection::Track);
                    }
                    Some(Err(e)) => app.toast(format!("{}: {e:#}", kind.name)),
                    None => {}
                }
            }
            Some("full-reverb") => {
                // The Space Reverb on the demo's Reverb bus, a long hall, its impulse made
                // (waited for) and its editor open.
                if let Err(e) = app.show_full_reverb() {
                    app.toast(format!("Space Reverb: {e:#}"));
                }
            }
            Some("convolution-reverb") => {
                // The Convolution Reverb on the demo's Reverb bus, set, playing a made room
                // impulse (written into the demo project's Imported folder), with its editor
                // open.
                if let Err(e) = app.show_convolution_reverb() {
                    app.toast(format!("Convolution Reverb: {e:#}"));
                }
            }
            Some(
                show @ ("settings" | "settings-audio" | "settings-midi" | "settings-projects"
                | "settings-mcp" | "mcp-settings" | "mcp-port-in-use"),
            ) => {
                // The settings on a section (`settings` is Audio; `mcp-settings`: run with a free
                // `--mcp-port`, the server up; `mcp-port-in-use`: its port held by the app
                // itself, so the status bar and the settings say so, with Retry).
                use crate::settings_window::Section;
                app.open_settings(match show {
                    "settings-midi" => Section::Midi,
                    "settings-projects" => Section::Projects,
                    "settings-mcp" | "mcp-settings" | "mcp-port-in-use" => Section::Mcp,
                    _ => Section::Audio,
                });
            }
            Some(show @ ("mcp-connect" | "mcp-connected")) => {
                // The settings with Claude's configs seeded in `--claude-profile`: neither
                // client configured, or Claude Code configured for this port and Claude Desktop
                // for 7878 (as after a port change), the detection waited for.
                let seeded = match (&options.claude_profile, app.claude.locations()) {
                    (Some(root), Some(locations)) => crate::mcp_settings::seed_profile(
                        &locations,
                        &std::path::absolute(root).unwrap_or_else(|_| root.clone()),
                        show == "mcp-connected",
                        app.mcp_port(),
                    ),
                    _ => Err(anyhow::anyhow!("--show {show} needs --claude-profile DIR")),
                };
                if let Err(e) = seeded {
                    app.toast(format!("{e:#}"));
                }
                app.open_settings(crate::settings_window::Section::Mcp);
                app.claude.settle(Duration::from_secs(10));
            }
            Some("tempo-question") => {
                // A 140 BPM, 3/4 MIDI file dropped on the Keys track at bar 2.
                if let Err(e) = app.show_tempo_question() {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("midi-staves") => {
                if let Err(e) = app.show_midi_staves() {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("header") => {
                // The headers at every layout: one line, knobs, knobs and the bottom menu, and
                // Guitar L tall enough for its fx rack; the monitor in each state and one track
                // writing automation (Touch).
                use daw_core::automation::AutomationMode;
                use daw_core::project::Monitor;
                let heights = [100.0, 170.0, 64.0, 120.0, 40.0, 84.0];
                for (track, height) in app.session.project.tracks.iter_mut().zip(heights) {
                    track.height = Some(height);
                    match track.name.as_str() {
                        "Guitar L" => track.monitor = Monitor::On,
                        "Vocal" => track.monitor = Monitor::Off,
                        "Keys" => track.automation_mode = AutomationMode::Touch,
                        _ => {}
                    }
                }
            }
            Some(mode @ ("multi-out" | "multi-out-dialog")) => {
                // The `multi-out` instrument (`--vst3`; a synth with two scene outputs) on Keys:
                // its multi-out rack made, the first output's track selected (the inspector
                // shows its input); or the dialog choosing them.
                if let Err(e) = app.show_multi_out(mode == "multi-out-dialog") {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("layout") => {
                // A rearranged window: the mixer moved to the right of the arrangement, and
                // two files (the DI and a copy) as tabs in a row at the bottom; no playback.
                if let Err(e) = app.show_layout() {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("waveform") => {
                // The first audio clip's file in the Waveform view, with a selection; no
                // playback.
                let clip = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| c.id));
                if let Some(clip) = clip {
                    crate::waveform::open_clip(&mut app, clip);
                }
                if let Some(id) = app.editor.current {
                    let rate = app
                        .session
                        .document(id)
                        .map_or(48_000, |d| u64::from(d.audio.sample_rate));
                    let view = app.editor.view(id);
                    view.selection = Some((rate / 2, rate * 3 / 2));
                    view.playhead = rate / 2;
                }
            }
            Some("noise-reduction") => {
                // The DI in the Waveform view, its first half second captured as a noise print
                // (Effects > Capture noise print): a Noise Reduction in the rack, its editor
                // open with the print; no playback.
                let clip = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| c.id));
                if let Some(clip) = clip {
                    crate::waveform::open_clip(&mut app, clip);
                }
                if let Some(id) = app.editor.current {
                    let rate = app
                        .session
                        .document(id)
                        .map_or(48_000, |d| u64::from(d.audio.sample_rate));
                    app.editor.view(id).selection = Some((0, rate / 2));
                    crate::waveform::capture_noise_print(&mut app, id);
                }
            }
            Some("waveform-fx") => {
                // The DI in the Waveform view with a selection and two plug-ins in its effects
                // rack (the `compressor` and `eq` VST3s, else built-in effects), the second
                // bypassed; no playback.
                let clip = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| c.id));
                if let Some(clip) = clip {
                    crate::waveform::open_clip(&mut app, clip);
                }
                for (k, (name, builtin)) in [("compressor", "Compressor"), ("eq", "Parametric EQ")]
                    .into_iter()
                    .enumerate()
                {
                    let added = app
                        .demo_effect(name, builtin)
                        .ok_or_else(|| anyhow::anyhow!("not installed"))
                        .and_then(|path| app.session.add_insert(InsertTarget::Editor, &path));
                    match added {
                        Ok(id) if k == 1 => {
                            if let Some(slot) = app.session.insert_mut(id) {
                                slot.bypass = true;
                            }
                        }
                        Ok(_) => {}
                        Err(e) => app.toast(format!("{name}: {e:#}")),
                    }
                }
                if let Some(id) = app.editor.current {
                    let rate = app
                        .session
                        .document(id)
                        .map_or(48_000, |d| u64::from(d.audio.sample_rate));
                    let view = app.editor.view(id);
                    view.selection = Some((rate, rate * 3));
                    view.playhead = rate;
                }
            }
            Some(name @ ("rack-sync" | "rack-sync-off")) => {
                // The DI opened from Guitar L's clip in the Waveform view, Guitar L holding
                // built-in effects (and the `compressor` VST3 if given) and the user's own
                // rack a Room Reverb: the Sync checkbox off, or on with Guitar L's chain in
                // the rack (21.2); no playback.
                if let Err(e) = crate::waveform::show_rack_sync(&mut app, name == "rack-sync") {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("rack-restored") => {
                // The DI in the Waveform view, and a rack kept in out/check/rack coming back
                // (Amplify set, a Parametric EQ bypassed, the `eq` VST3 if given, and a
                // plug-in no longer installed, greyed with its settings).
                if let Err(e) = crate::rack_presets::show_restored(&mut app) {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("rack-presets") => {
                // The same rack saved as two presets, and the rack's Presets menu open.
                if let Err(e) = crate::rack_presets::show_presets(&mut app) {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("spectral") => {
                // The DI in the Waveform view with its spectrogram under it, and a box (1 to
                // 4 kHz, two to three seconds in) drawn with the Box tool.
                let clip = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| c.id));
                if let Some(clip) = clip {
                    crate::waveform::open_clip(&mut app, clip);
                }
                app.editor.spectral = daw_core::api::SpectralMode::Both;
                app.editor.tool = crate::waveform::Tool::Box;
                if let Some(id) = app.editor.current {
                    let rate = app
                        .session
                        .document(id)
                        .map_or(48_000, |d| u64::from(d.audio.sample_rate));
                    let view = app.editor.view(id);
                    view.boxed = Some((
                        daw_core::spectral_edit::SpectralBox {
                            start: rate * 2,
                            end: rate * 3,
                            low_hz: 1000.0,
                            high_hz: 4000.0,
                        },
                        None,
                    ));
                    view.playhead = rate * 2;
                }
            }
            Some("diagnostics") => {
                // A copy of the DI with clicks added (in the demo project's Temp folder, never
                // the DI itself) in the Waveform view, scanned with the Click Repair: the
                // Diagnostics pane with its list, the third row selected and shown.
                if let Err(e) = app.show_diagnostics_demo() {
                    app.toast(format!("{e:#}"));
                }
            }
            Some("waveform-edit") => {
                // A copy of the demo's audio (never the original), edited: faded in, part of it
                // amplified, a piece deleted; unsaved, with the Save question open.
                let file = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| app.session.resolve(c.file())));
                let copy = std::path::Path::new("out/check/waveform-edit.wav");
                let copied = file.map(|f| std::fs::copy(f, copy));
                if let Some(Ok(_)) = copied {
                    let copy = std::fs::canonicalize(copy).unwrap_or_else(|_| copy.into());
                    crate::waveform::open_file(&mut app, &copy);
                }
                if let Some(id) = app.editor.current {
                    let rate = app
                        .session
                        .document(id)
                        .map_or(48_000, |d| u64::from(d.audio.sample_rate));
                    use daw_core::audio_edit::Edit;
                    for edit in [
                        Edit::FadeIn {
                            range: (0, rate / 2),
                            channels: None,
                            shape: daw_core::project::FadeShape::Linear,
                        },
                        Edit::Amplify {
                            range: Some((rate * 2, rate * 3)),
                            channels: None,
                            db: 6.0,
                        },
                        Edit::Delete {
                            range: (rate * 4, rate * 5),
                        },
                    ] {
                        crate::waveform::apply(&mut app, edit);
                    }
                    app.editor.view(id).selection = Some((rate * 2, rate * 3));
                    crate::waveform::ask_save(&mut app);
                }
            }
            Some("insert-midi") => {
                // A time selection over bars 5 and 6, then "Insert MIDI track" after Keys with
                // the `synth` VST3 (when given): the new track, its clip, the piano roll open
                // on it.
                let map = app.session.tempo_map();
                let (start, end) = (
                    map.bar_start(4).round() as u64,
                    map.bar_start(6).round() as u64,
                );
                let keys = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.kind == TrackKind::Instrument)
                    .map(|t| t.id);
                if let Some(keys) = keys {
                    app.set_time_selection(
                        TimeSelection {
                            start,
                            end,
                            tracks: vec![keys],
                        },
                        false,
                    );
                }
                let synth = app.demo_instrument("synth").ok();
                app.insert_midi_track(start, Some(end), keys, synth);
            }
            Some("midi-split") => {
                // Keys' clip open in the piano roll and selected, then S pressed with the
                // playhead a beat in, through its first chord: the chord ends at the cut on the
                // left piece (still in the piano roll) and plays on from the right piece's
                // start (selected).
                let found = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.name == "Keys")
                    .and_then(|t| t.midi_clips.first().map(|c| (t.id, c.id, c.start)));
                if let Some((track, clip, start)) = found {
                    let map = app.session.tempo_map();
                    let sample = |tick: u64| map.sample_at_tick(tick as f64);
                    let bar = u64::from(daw_core::midi_clip::TICKS_PER_QUARTER) * 4;
                    app.samples_per_pixel = (sample(start + 2 * bar) - sample(start)) / 700.0;
                    app.view_start = sample(start) - 120.0 * app.samples_per_pixel;
                    let _ = piano_roll::open(&mut app, clip);
                    let cut = sample(start + bar / 4).round() as u64;
                    let located = app.session.locate(cut);
                    app.report(located);
                    app.select_clip(track, clip, false);
                    app.demo_split_at = Some(cut);
                }
            }
            Some(
                show @ ("rename" | "volume-edit" | "record-menu" | "count-in" | "clip-copy-paste"
                | "clip-edges" | "clip-playhead" | "track-zoom" | "playhead-drag"
                | "piano-roll-click" | "piano-roll-paste" | "help-manual"),
            ) => {
                // Guitar L renamed by typing in its header and the inspector, or its volume
                // and pan typed, as the user would (`edit_check`); the shot is the field open.
                // The Record button's count-in (22.3): its right-click menu (the shot), and
                // with `count-in` a take recorded through it from a simulated input (the shot
                // mid count-in). `clip-copy-paste` (24.1): Guitar L's clip copied and pasted on
                // Vocal at the playhead with the clipboard's events (the shot: the copy).
                // `clip-edges` (25.1): an end under another clip grabbed, dragged and pressed
                // (the shot: that end lit under the pointer). `clip-playhead` (25.2): clicks
                // on clips placing the playhead, a drag, a handle, an edge and a right-click
                // leaving it (the shot: the playhead on the click inside the selected clip).
                // `track-zoom` (26.1): Alt+wheel over a track part way down the scrolled rows,
                // its row held under the pointer every frame (the shot: the rows grown).
                // `playhead-drag` (26.3): the playhead dragged fast past the right edge, out
                // of the window and back past the left, zoomed in (the shot: mid-drag).
                // `piano-roll-click` (26.4): clicks in the piano roll's grid placing the
                // playhead, a double-click drawing a note, a note's drag, a key and the
                // velocity lane leaving it (the shot: the note drawn, the playhead on it).
                // `piano-roll-paste`: notes copied, pasted at the playhead and cut in the piano
                // roll with the clipboard's events, from the menu bar after a click in it too,
                // and the clip pasted over the Multitrack (the shot: the pasted notes).
                // `help-manual` (27.3): Help > Manual, F1 over the piano roll and in Settings >
                // Audio, the pages asked for noted, no browser (the shot: the Help menu).
                let kind = match show {
                    "rename" => crate::edit_check::Kind::Rename,
                    "volume-edit" => crate::edit_check::Kind::Volume,
                    "record-menu" => crate::edit_check::Kind::RecordMenu,
                    "clip-copy-paste" => crate::edit_check::Kind::ClipCopyPaste,
                    "clip-edges" => crate::edit_check::Kind::ClipEdges,
                    "clip-playhead" => crate::edit_check::Kind::ClipPlayhead,
                    "track-zoom" => crate::edit_check::Kind::TrackZoom,
                    "playhead-drag" => crate::edit_check::Kind::PlayheadDrag,
                    "piano-roll-click" => crate::edit_check::Kind::PianoRollClick,
                    "piano-roll-paste" => crate::edit_check::Kind::PianoRollPaste,
                    "help-manual" => crate::edit_check::Kind::HelpManual,
                    _ => crate::edit_check::Kind::CountIn,
                };
                if kind == crate::edit_check::Kind::CountIn {
                    if let Err(e) = app.virtual_input_demo(options.demo.as_deref()) {
                        println!("FAIL: the simulated input: {e:#}");
                        crate::edit_check::fail();
                    }
                }
                let shot = app.screenshot.is_some();
                app.edit_check = crate::edit_check::start(&mut app, kind, shot);
            }
            Some("automation") => app.show_automation(),
            Some("stretch") => app.show_stretch(),
            Some("fades") => {
                app.show_fades();
            }
            Some("fades-close") => {
                // The manual's (27.4): the `fades` clips zoomed to the first piece, its fade-in,
                // crossfade and gain line large enough to point at.
                if let Some(first) = app.show_fades() {
                    let zoomed = app.zoom_to_clip(first).map_err(|e| anyhow::anyhow!(e.0));
                    app.report(zoomed);
                }
            }
            Some("master") => {
                // The master's row: the Limiter in its rack, a volume lane showing, taller
                // so the rack fits, and selected.
                let added = app.session.add_insert(
                    InsertTarget::Master,
                    &daw_core::native::path_of(&daw_core::native::hard_limiter::KIND),
                );
                app.report(added.map(|_| ()));
                let lane = app.session.add_automation_lane(
                    None,
                    daw_core::automation::AutomationTarget::Volume,
                    None,
                );
                if let Err(e) = lane {
                    app.toast(e);
                }
                let master = &mut app.session.project.master;
                master.height = Some(150.0);
                master.show_automation = true;
                app.selection = Some(Selection::Master);
            }
            Some(show @ ("grid-menu" | "grid-16")) => {
                // The song's grid (17.4) in the Multitrack, zoomed in on bar 2 so a sixteenth
                // is 20 points wide (`grid-menu`, Auto: it lands on 1/16, and the ruler's menu
                // is opened on its Grid) or 12 (`grid-16`, 1/16 fixed: its lines between the
                // beats).
                let map = app.session.tempo_map();
                let sixteenth = map.sample_at_tick(240.0) - map.sample_at_tick(0.0);
                let (grid, points) = if show == "grid-menu" {
                    (daw_core::grid::Grid::Auto, 20.0)
                } else {
                    (Grid::Fixed(Division::note(16)), 12.0)
                };
                let set = app.session.set_grid(grid);
                app.report(set);
                app.samples_per_pixel = sixteenth / points;
                app.view_start =
                    map.sample_at_tick(map.tick_of_bar(1, 0) as f64) - 40.0 * app.samples_per_pixel;
                if show == "grid-menu" {
                    app.demo_grid_menu = Some(0);
                }
            }
            Some("piano-roll-grid") => {
                // The first MIDI clip in the piano roll with the song's grid at 1/16: its lines
                // (and the ruler's ticks) between the beats, the menu saying 1/16.
                let set = app.session.set_grid(Grid::Fixed(Division::note(16)));
                app.report(set);
                let first = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .flat_map(|t| &t.midi_clips)
                    .map(|c| c.id)
                    .next();
                if let Some(clip) = first {
                    let _ = piano_roll::open(&mut app, clip);
                }
            }
            Some("waveform-bars") => {
                // The DI in the Waveform view in bars and beats (the song's tempo from the
                // file's start) with the grid at 1/16: the whole file (three bars at 120, a
                // sixteenth about 19 points), the playhead on bar 3.
                let set = app.session.set_grid(Grid::Fixed(Division::note(16)));
                app.report(set);
                let clip = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| c.id));
                if let Some(clip) = clip {
                    crate::waveform::open_clip(&mut app, clip);
                }
                app.editor.time = daw_core::api::TimeDisplay::BarsBeats;
                if let Some(id) = app.editor.current {
                    let rate = app
                        .session
                        .document(id)
                        .map_or(48_000, |d| d.audio.sample_rate);
                    let map = daw_core::tempo::TempoMap::of(&app.session.project, rate);
                    let sixteenth = map.sample_at_tick(240.0) - map.sample_at_tick(0.0);
                    let view = app.editor.view(id);
                    view.spp = sixteenth / 14.0;
                    view.playhead = map.bar_start(2) as u64;
                }
            }
            Some(show @ ("clip-overview" | "clip-close" | "clip-samples")) => {
                // The first audio clip (Guitar L's DI) at three zooms (23.2): all of it, 200
                // samples a point (drawn from the 16-sample peaks), and 8 points a sample (the
                // samples as dots on a line) around its loudest sample. Its track made tall
                // enough to see the shape; every frame's drawing timed.
                let spp = match show {
                    "clip-close" => Some(200.0),
                    "clip-samples" => Some(1.0 / 8.0),
                    _ => None,
                };
                app.show_clip_zoom(spp);
                app.draw_timer = Some(Default::default());
            }
            Some("waveform-samples") => {
                // The DI in the Waveform view at its closest zoom (32 points a sample) around
                // its loudest sample: each sample a dot on the line; drawing timed.
                let clip = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| c.id));
                if let Some(clip) = clip {
                    crate::waveform::open_clip(&mut app, clip);
                }
                if let Some(id) = app.editor.current {
                    let loudest = app
                        .session
                        .document(id)
                        .map_or(0, |d| loudest_sample(&d.audio.channels[0]));
                    let view = app.editor.view(id);
                    view.spp = zoom::MIN_SPP;
                    view.start = loudest as f64 - 400.0 * zoom::MIN_SPP;
                    view.playhead = loudest as u64;
                }
                app.draw_timer = Some(Default::default());
            }
            Some("playhead") => {
                // Zoomed in until each sample is a few points wide, the playhead on an off-beat
                // sample a drag would reach (bar 2 plus 1009 samples).
                let map = app.session.tempo_map();
                let at = map.sample_at_tick(map.tick_of_bar(1, 0) as f64) as u64 + 1009;
                let located = app.session.locate(at);
                app.report(located);
                app.samples_per_pixel = 1.0 / 4.0;
                app.view_start = at as f64 - 80.0;
            }
            Some(show @ ("drop-ghost" | "drop-question")) => {
                // A copy of the DI dragged over Vocal from "Explorer": its ghost on Vocal's
                // lane, or let go there and asked into Vocal or onto a new track.
                let vocal = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.name == "Vocal")
                    .map(|t| t.id);
                let copy = app
                    .session
                    .project_folder(daw_core::folder::TEMP)
                    .map(|dir| dir.join("Lead vocal.wav"));
                if let (Some(vocal), Ok(copy), Some(wav)) = (vocal, copy, options.demo.as_ref()) {
                    if std::fs::copy(wav, &copy).is_ok() {
                        app.demo_drop = Some((copy, vocal, show == "drop-question", 0));
                    }
                }
            }
            Some("tab-hover") => {
                // The pointer on the Waveform tab, beside the active Multitrack one: lit, with
                // the arrow.
                app.demo_tab_hover = true;
            }
            Some("playhead-edge") => {
                // Zoomed in and scrolled to bar 5, the playhead a bar on, then dragged off the
                // lanes' left edge: the view scrolls back to the start with it.
                let map = app.session.tempo_map();
                let bar = |n| map.sample_at_tick(map.tick_of_bar(n, 0) as f64);
                let located = app.session.locate(bar(5) as u64);
                app.report(located);
                app.samples_per_pixel = 200.0;
                app.view_start = bar(4);
                app.demo_playhead_drag = Some((0, None));
            }
            Some("trim") => {
                // The first clip trimmed at both ends (its hidden audio no longer drawn), and the
                // second track's clip renamed long and split into short pieces: every name cut
                // short to its piece.
                let mut audio = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .filter_map(|t| t.clips.first().map(|c| (c.id, c.start, c.length)))
                    .collect::<Vec<_>>()
                    .into_iter();
                if let Some((clip, start, length)) = audio.next() {
                    let trimmed = app.session.trim_clip(
                        clip,
                        Some(start + length / 4),
                        Some(start + length * 3 / 4),
                    );
                    app.report(trimmed);
                    app.selected_clip = None;
                }
                if let Some((clip, start, length)) = audio.next() {
                    if let Some(c) = app.session.project.clip_mut(clip) {
                        c.name = "Rhythm guitar, take 3, clean amp (verse two)".into();
                    }
                    let mut piece = clip;
                    for k in 1..4 {
                        if let Some(next) = app
                            .session
                            .project
                            .split_clip(piece, start + length * k / 6)
                        {
                            piece = next;
                        }
                    }
                    app.session.mark_dirty();
                }
            }
            Some("busy") => {
                // The demo with real plug-ins on its guitars (those installed), saved, then
                // opened again: the screenshot is taken while its plug-ins load behind the
                // busy overlay (`take_screenshot_when_ready`).
                app.add_demo_plugins();
                if app.neutral {
                    // Built-in effects load a project's worth in one update: held after two,
                    // so the bar is caught on the third (27.7).
                    app.session.hold_plugin_loads(Some(2));
                }
                if let Some(path) = app.session.path.clone() {
                    let reopened = app
                        .session
                        .save(&path)
                        .and_then(|()| app.session.open(&path));
                    app.report(reopened);
                    app.screenshot_on_busy = true;
                }
            }
            Some(which @ ("open-files" | "open-plugins")) => {
                // The demo saved (with three plug-ins on each guitar for the
                // plug-ins), then opened as File > Open does, every frame timed
                // (`opening::Check`): the shot is the bar on its file (held back) or on its
                // plug-ins.
                let plugins = which == "open-plugins";
                if plugins {
                    app.add_demo_plugins();
                } else {
                    app.add_demo_files(3);
                }
                if let Some(path) = app.session.path.clone() {
                    match app.session.save(&path) {
                        Ok(()) => {
                            app.open_check = Some(crate::opening::Check::new(path, !plugins));
                        }
                        Err(e) => app.toast(format!("{e:#}")),
                    }
                }
            }
            Some("undo-reload") => {
                // The demo with three plug-ins on each guitar, Guitar L's plug-ins
                // removed in one edit; the check undoes it as Ctrl+Z does, every frame timed
                // (`frame_check`): the shot is the bar with the plug-ins coming back.
                app.add_demo_plugins();
                let left = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
                    .map(|t| (t.id, t.inserts.iter().map(|s| s.id).collect::<Vec<_>>()));
                if let Some((_, inserts)) = left {
                    app.session.begin_edit("Remove plug-ins", None);
                    for &id in &inserts {
                        app.session.remove_insert(id);
                    }
                    app.session.end_edit();
                    app.frame_check = Some(crate::frame_check::Check::new(
                        crate::frame_check::Kind::UndoReload,
                        inserts,
                    ));
                }
            }
            Some("render-progress") => {
                // Guitar L (with its three plug-ins) rendered as the render dialog's
                // Render does, every frame timed (`frame_check`): the shot is its progress bar,
                // the render held halfway.
                app.add_demo_plugins();
                let left = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
                    .map(|t| t.id);
                if let Some(left) = left {
                    app.frame_check = Some(crate::frame_check::Check::new(
                        crate::frame_check::Kind::RenderProgress(left),
                        Vec::new(),
                    ));
                }
            }
            Some("save") => {
                // The demo with three plug-ins on each guitar saved as Ctrl+S does,
                // every frame timed (`frame_check`): the shot is the bar while it saves.
                app.add_demo_plugins();
                app.frame_check = Some(crate::frame_check::Check::new(
                    crate::frame_check::Kind::Save,
                    Vec::new(),
                ));
            }
            Some("file-dialog") => {
                // File > Open's dialog shown as Ctrl+O shows it, every frame timed until
                // something closes it (`frame_check`; the maintainer's file-dialog check).
                app.frame_check = Some(crate::frame_check::Check::new(
                    crate::frame_check::Kind::FileDialog,
                    Vec::new(),
                ));
            }
            Some("plugin-paste") => {
                // Amplify on Guitar L copied (as its menu's Copy does), and Guitar R's first
                // empty slot right-clicked: the menu offers to paste it.
                let guitars: Vec<TrackId> = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
                    .map(|t| t.id)
                    .collect();
                if let [left, right, ..] = guitars[..] {
                    let path = daw_core::native::path_of(&daw_core::native::amplify::KIND);
                    if let Ok(amp) = app.session.add_insert(InsertTarget::Track(left), &path) {
                        crate::plugin_clipboard::copy(&mut app, amp);
                    }
                    for id in [left, right] {
                        if let Some(t) = app.session.project.track_mut(id) {
                            t.height = Some(176.0);
                        }
                    }
                    app.demo_right_click = Some(((InsertTarget::Track(right), 0), 0));
                }
            }
            Some("rack-move") => {
                // Amplify and the Limiter on Guitar L, an EQ on Guitar R, both tall enough
                // for their racks, and Amplify being dragged onto Guitar R's rack.
                let guitars: Vec<TrackId> = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
                    .map(|t| t.id)
                    .collect();
                if let [left, right, ..] = guitars[..] {
                    let native = |k: &daw_core::native::Kind| daw_core::native::path_of(k);
                    let add = |app: &mut DawApp, track, kind: &daw_core::native::Kind| {
                        app.session
                            .add_insert(InsertTarget::Track(track), &native(kind))
                            .ok()
                    };
                    let amp = add(&mut app, left, &daw_core::native::amplify::KIND);
                    add(&mut app, left, &daw_core::native::hard_limiter::KIND);
                    add(&mut app, right, &daw_core::native::parametric_eq::KIND);
                    for id in [left, right] {
                        if let Some(t) = app.session.project.track_mut(id) {
                            t.height = Some(176.0);
                        }
                    }
                    if amp.is_some() {
                        app.demo_drag = Some((
                            (InsertTarget::Track(left), 0),
                            (InsertTarget::Track(right), 1),
                            0,
                        ));
                    }
                }
            }
            Some("metadata") => {
                // The demo's file in the Waveform view with tags and Oxide's icon as its cover,
                // and View > Metadata open on it.
                let file = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| app.session.resolve(c.file())));
                if let Some(file) = file {
                    crate::waveform::open_file(&mut app, &file);
                }
                if let Some(doc) = app.editor.current {
                    let icon = crate::icon::window_icon();
                    let mut png = Vec::new();
                    let mut encoder = png::Encoder::new(&mut png, icon.width, icon.height);
                    encoder.set_color(png::ColorType::Rgba);
                    encoder.set_depth(png::BitDepth::Eight);
                    let written = encoder
                        .write_header()
                        .and_then(|mut w| w.write_image_data(&icon.rgba));
                    let cover = written
                        .ok()
                        .and_then(|()| daw_core::metadata::Cover::from_bytes(png).ok());
                    let tags = daw_core::metadata::Metadata {
                        title: "First Light".into(),
                        artist: "Example Artist".into(),
                        album: "Example Album".into(),
                        year: "2026".into(),
                        track: "1/10".into(),
                        genre: "Rock".into(),
                        comment: "Recorded at home".into(),
                        cover,
                        ..Default::default()
                    };
                    let _ = app.session.set_document_metadata(doc, tags);
                    app.metadata_panel = crate::metadata_panel::MetadataPanel::open(&app, doc);
                }
            }
            Some("waveform-export") => {
                // The demo's file in the Waveform view, its export dialog open on an MP3.
                let file = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find_map(|t| t.clips.first().map(|c| app.session.resolve(c.file())));
                if let Some(file) = file {
                    crate::waveform::open_file(&mut app, &file);
                }
                app.settings.export_encoding = daw_core::encode::Encoding {
                    format: daw_core::encode::Format::Mp3 {
                        bitrate: Some(320),
                        vbr: None,
                    },
                    sample_rate: Some(44_100),
                    dither: true,
                };
                app.export_document = app.editor.current;
            }
            Some("mixdown") => {
                // The demo mixed down to a new file, open in the Waveform view.
                let made = app.session.mixdown_to_new_file(None, 0.5);
                let waited =
                    made.and_then(|_| app.session.wait_for_export(Duration::from_secs(60)));
                app.report(waited.map(|_| ()));
                app.session.update();
                if let Some(doc) = app.session.take_opened_mixdown() {
                    crate::waveform::show_document(&mut app, doc);
                }
            }
            Some("render") => {
                // Guitar R switched off under its render (the Waveform-style dimming), and
                // the render dialog open on Guitar L.
                let guitars: Vec<TrackId> = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
                    .map(|t| t.id)
                    .collect();
                if let Some(&right) = guitars.get(1) {
                    let rendered = app.session.render_track(right, 0.5, true);
                    let waited =
                        rendered.and_then(|_| app.session.wait_for_export(Duration::from_secs(60)));
                    app.report(waited.map(|_| ()));
                    app.session.update();
                }
                if let Some(&left) = guitars.first() {
                    app.render_dialog = Some(crate::render::RenderDialog::open(left));
                }
            }
            Some("about") => app.show_about = true,
            Some("backups") => {
                // Two backups of the demo, an edit apart, and the list open.
                for name in ["Bass", "Vocal 2"] {
                    let track = app.session.add_track(TrackKind::Audio, None);
                    if let Some(t) = app.session.project.track_mut(track) {
                        t.name = name.into();
                    }
                    let backed = app.session.save_backup();
                    app.report(backed.map(|_| ()));
                }
                app.show_backups = true;
            }
            Some(show @ ("track-drag" | "track-drag-mixer")) => {
                // Vocal dragged up over Guitar R's upper half: it moves into the Guitars
                // folder between the two guitars, the rows below making room. In the mixer,
                // Vocal's strip over Guitar L's left half: it goes before it.
                let find = |app: &DawApp, name: &str| {
                    app.session
                        .project
                        .tracks
                        .iter()
                        .find(|t| t.name == name)
                        .map(|t| t.id)
                };
                let mixer = show == "track-drag-mixer";
                let over = if mixer { "Guitar L" } else { "Guitar R" };
                if let (Some(vocal), Some(to)) = (find(&app, "Vocal"), find(&app, over)) {
                    app.demo_track_drag = Some((vocal, to, mixer, 0, None));
                }
            }
            Some(show @ ("clip-drag" | "clip-drag-refused")) => {
                // Guitar L's clip dragged down over Vocal (it goes there), or on over the
                // Reverb bus (it can't: it stays on Vocal, and Reverb's lane says why).
                let find = |app: &DawApp, name: &str| {
                    app.session
                        .project
                        .tracks
                        .iter()
                        .find(|t| t.name == name)
                        .map(|t| t.id)
                };
                let clip = find(&app, "Guitar L")
                    .and_then(|t| app.session.project.track(t))
                    .and_then(|t| t.clips.first())
                    .map(|c| c.id);
                let over = if show == "clip-drag" {
                    "Vocal"
                } else {
                    "Reverb"
                };
                if let (Some(clip), Some(to)) = (clip, find(&app, over)) {
                    app.demo_clip_drag = Some((clip, to, 0, None));
                }
            }
            Some("clip-magnet") => {
                // Guitar L's clip dragged along its own row 60 points later, the playhead 56
                // points after the clip's start (off the grid): the magnet holds the clip's
                // start on the playhead (4 points short of the pointer), its line lit.
                let found = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .find(|t| t.name == "Guitar L")
                    .and_then(|t| t.clips.first().map(|c| (t.id, c.id, c.start)));
                if let Some((track, clip, start)) = found {
                    app.samples_per_pixel = 400.0;
                    app.view_start = 0.0;
                    let located = app.session.locate(start + 56 * 400 + 37);
                    app.report(located);
                    app.demo_clip_drag = Some((clip, track, 0, None));
                    app.demo_magnet = true;
                }
            }
            Some("unsaved") => {
                // The demo saved, then a fader moved, then File > Exit: the question.
                if let Some(path) = app.session.path.clone() {
                    let saved = app.session.save(&path);
                    app.report(saved);
                }
                if let Some(track) = app.session.project.tracks.first().map(|t| t.id) {
                    if let Some(t) = app.session.project.track_mut(track) {
                        t.mixer.gain_db -= 3.0;
                    }
                }
                crate::unsaved::exit(&mut app, &cc.egui_ctx);
            }
            Some("start") => {
                // The start screen over the demo (File > New project), so it can be cancelled.
                app.start.open = true;
            }
            Some("templates") => {
                // The demo saved as a template (in `out/check/Templates`), then the start
                // screen with it picked.
                match app.session.save_template("Band setup", true) {
                    Ok(_) => app.start.template = Some("Band setup".into()),
                    Err(e) => app.toast(format!("{e:#}")),
                }
                app.start.open = true;
            }
            Some("cpu") => {
                // The CPU panel over the demo: Guitar L's plug-ins (the ui tier passes VST3s
                // with `--plugin`; the manual's shot none), the master's Limiter,
                // all running on silence while stopped.
                let added = app.session.add_insert(
                    InsertTarget::Master,
                    &daw_core::native::path_of(&daw_core::native::hard_limiter::KIND),
                );
                app.report(added.map(|_| ()));
                app.show_cpu = true;
            }
            Some("quantize") => {
                // The slicer open on the first guitar clip, its transients drawn on it.
                let first = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio)
                    .find_map(|t| t.clips.first().map(|c| (t.id, c.id)));
                if let Some((track, clip)) = first {
                    app.select_clip(track, clip, false);
                    crate::quantize::open_slicer(&mut app, clip);
                }
            }
            Some("markers") => {
                // Four markers, one with a long name close before another, the playhead on it.
                let map = app.session.tempo_map();
                let bar = |bar: i64, beat: u32| {
                    map.sample_at_tick(map.tick_of_bar(bar, beat) as f64)
                        .round() as u64
                };
                let names = [
                    (bar(0, 0), "Intro"),
                    (bar(2, 0), "Verse 1 with a rather long name"),
                    (bar(2, 2), "Fill"),
                    (bar(4, 0), "Chorus"),
                ];
                let ids: Vec<u32> = names
                    .iter()
                    .map(|(at, name)| app.session.add_marker(*at, Some(name.to_string())))
                    .collect();
                let went = app.session.go_to_marker(ids[1]).map(|_| ());
                app.report(went);
            }
            Some("selection") => {
                // Both guitar clips selected and duplicated: the copies, after them, selected.
                let clips: Vec<ClipId> = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .filter(|t| t.kind == TrackKind::Audio)
                    .filter_map(|t| t.clips.first().map(|c| c.id))
                    .take(2)
                    .collect();
                app.select_clips(&clips);
                app.duplicate_selection();
                app.zoom_to_fit();
                // And a time selection over the copies' first half, on the guitar tracks.
                let copies = app.selected_clip_ids();
                let tracks: Vec<TrackId> = copies
                    .iter()
                    .filter_map(|&c| app.session.project.clip_track(c))
                    .collect();
                let span = copies
                    .iter()
                    .filter_map(|&c| app.session.project.find_clip(c).map(|(_, c)| c))
                    .map(|c| (c.start, c.end()))
                    .reduce(|a, b| (a.0.min(b.0), a.1.max(b.1)));
                if let Some((start, end)) = span {
                    app.set_time_selection(
                        TimeSelection {
                            start,
                            end: start + (end - start) / 2,
                            tracks,
                        },
                        false,
                    );
                }
            }
            Some("export") => {
                // A 24-bit FLAC at the project's rate, whatever the last export was.
                app.settings.export_encoding = daw_core::encode::Encoding {
                    format: daw_core::encode::Format::Flac { bits: 24, level: 5 },
                    ..Default::default()
                };
                app.settings.normalize_exports = true;
                app.export_options = true;
            }
            Some("export-report") => {
                // Export the demo, normalised, and show the report. The device runs meanwhile,
                // so it skips a few blocks: harmless for a check.
                let path = app
                    .session
                    .project_folder(daw_core::folder::TEMP)
                    .unwrap_or_default()
                    .join("export-report.wav");
                let normalize = Some(Normalize {
                    target: app.session.project.loudness_target,
                    ceiling: dialogs::EXPORT_CEILING,
                });
                let done = app
                    .session
                    .export_with(&path, None, 2.0, normalize)
                    .and_then(|_| app.session.wait_for_export(Duration::from_secs(60)));
                match done {
                    Ok(_) => app.export_report = Some(app.session.export_status()),
                    Err(e) => app.toast(format!("{e:#}")),
                }
            }
            Some("loudness") => {
                // The master's panel, with a target chosen; the check plays the demo silently.
                app.selection = Some(Selection::Master);
                app.show_inspector = true;
                app.session.project.loudness_target = -14.0;
            }
            Some("piano-roll-playhead") => {
                // The first MIDI clip in the piano roll, its ruler clicked a little past a
                // sixteenth inside the clip: the playhead snaps there, in both views.
                let first = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .flat_map(|t| &t.midi_clips)
                    .map(|c| (c.id, c.start))
                    .next();
                if let Some((clip, start)) = first {
                    let _ = piano_roll::open(&mut app, clip);
                    let map = app.session.tempo_map();
                    // On the song's grid (1/8 in the demo): the click snaps to it.
                    let division = match app.session.project.grid {
                        daw_core::grid::Grid::Fixed(d) => d,
                        daw_core::grid::Grid::Auto => daw_core::grid::Division::note(16),
                    };
                    let tick = piano_roll::demo_target(&map, start, division);
                    let sample = map.sample_at_tick(tick as f64).round() as u64;
                    app.demo_ruler = Some((tick, sample, 0));
                }
            }
            Some("recording") => {
                // A take half recorded (21.1): Vocal armed on input 1 and recording from bar 2,
                // its waveform drawn as it comes in. A virtual device takes the audio device's
                // place, playing the demo's file into that input; nothing is heard.
                if let Err(e) = app.record_demo(options.demo.as_deref()) {
                    app.toast(format!("Recording: {e:#}"));
                }
            }
            Some("loop-range-off") => {
                // A loop range drawn in the ruler over bars 2-3 as a drag draws it (20.4): the
                // range shows, dim, with Loop still off.
                let map = app.session.tempo_map();
                let (start, end) = (map.bar_start(1), map.bar_start(3));
                let drawn =
                    crate::arrange::drawn_range(&mut app.session, start as u64, end as u64, false);
                app.report(drawn);
            }
            Some("piano-roll-scroll") => {
                // The first MIDI clip in the piano roll zoomed in (2 ticks a point) and scrolled
                // into its second beat: the scroll bar's thumb partway along (20.2).
                let first = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .flat_map(|t| &t.midi_clips)
                    .map(|c| c.id)
                    .next();
                if let Some(clip) = first {
                    let _ = piano_roll::open(&mut app, clip);
                    if let Some(state) = app.piano_roll.as_mut() {
                        state.set_view(2.0, 2400.0);
                    }
                }
            }
            Some("piano-roll") => {
                let first = app
                    .session
                    .project
                    .tracks
                    .iter()
                    .flat_map(|t| &t.midi_clips)
                    .map(|c| c.id)
                    .next();
                if let Some(clip) = first {
                    let _ = piano_roll::open(&mut app, clip);
                    // Something selected, to show how it looks.
                    if let (Some(state), Some((_, c))) = (
                        app.piano_roll.as_mut(),
                        app.session.project.find_midi_clip(clip),
                    ) {
                        state.selected = c.notes.iter().take(3).map(|n| n.id).collect();
                        // The pointer on E4: its key and row lit.
                        state.point_at = Some(64);
                    }
                }
            }
            _ => {}
        }
        // The manual's shots: a third-party plug-in still in the scenario is shown by what it
        // is, not its maker's name (27.7).
        if app.neutral {
            crate::neutral::name_plugins(&mut app.session);
        }
        app
    }

    /// A scenario's effect for `role`: the VST3 named for it with `--vst3 ROLE=NAME` (the
    /// maintainer's checks name the plug-ins their machine has), or none when that isn't
    /// installed, so the scenario goes without it; with no VST3 named, or in a neutral run (the
    /// manual's shots, 27.7), the built-in effect `builtin`, so the manual shows no third-party
    /// plug-in (and the same on every machine).
    pub fn demo_effect(&self, role: &str, builtin: &str) -> Option<std::path::PathBuf> {
        let named = self.demo_vst3.iter().find(|(r, _)| r == role);
        match named {
            Some((_, name)) if !self.neutral => {
                daw_core::plugins::resolve(name, self.session.installed_plugins()).ok()
            }
            _ => daw_core::native::find(builtin).map(daw_core::native::path_of),
        }
    }

    /// A scenario's instrument for `role` (`--vst3 ROLE=NAME`; there is no built-in one to
    /// stand in): the error says when none was named or it isn't installed.
    pub fn demo_instrument(&self, role: &str) -> anyhow::Result<std::path::PathBuf> {
        let (_, name) = self
            .demo_vst3
            .iter()
            .find(|(r, _)| r == role)
            .ok_or_else(|| {
                anyhow::anyhow!("no instrument named for `{role}` (--vst3 {role}=NAME)")
            })?;
        daw_core::plugins::resolve(name, self.session.installed_plugins())
    }

    /// For the screenshot check: write a MIDI file whose tempo differs from the project's and
    /// drop it on the first instrument track at bar 2, which asks the tempo question.
    fn show_tempo_question(&mut self) -> anyhow::Result<()> {
        use daw_core::midi_clip::{MidiClip, Note};
        let clip = MidiClip {
            id: 0,
            name: "riff".into(),
            start: 0,
            length: 2880,
            notes: (0..6)
                .map(|k| Note {
                    id: 0,
                    tick: k * 480,
                    length: 400,
                    pitch: 50 + (k % 3) as u8 * 4,
                    velocity: 100,
                    channel: 0,
                })
                .collect(),
            controllers: Vec::new(),
        };
        let bytes = daw_core::smf::write_clip(&clip, "Riff", 140.0, (3, 4))?;
        let path = self
            .session
            .project_folder(daw_core::folder::TEMP)?
            .join("riff-140bpm.mid");
        std::fs::write(&path, bytes)?;
        let rows: Vec<(TrackId, TrackKind)> = self
            .session
            .project
            .tracks
            .iter()
            .filter(|t| !self.session.project.is_hidden(t.id))
            .map(|t| (t.id, t.kind))
            .collect();
        let start = rows.iter().position(|r| r.1 == TrackKind::Instrument);
        let at = self.session.tempo_map().bar_start(1).round() as u64;
        crate::drop::drop_files(self, vec![path], at, rows, start);
        // Into the instrument track it was dropped on (16.2's question), then the tempo one.
        crate::drop::answer_into(self, Some(true));
        Ok(())
    }

    /// For the screenshot check (`--show multi-out`, `multi-out-dialog`): see its arm.
    fn show_multi_out(&mut self, dialog: bool) -> anyhow::Result<()> {
        let keys = self
            .session
            .project
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Instrument)
            .map(|t| t.id)
            .ok_or_else(|| anyhow::anyhow!("the demo has no instrument track"))?;
        let synth = self.demo_instrument("multi-out")?;
        let insert = self
            .session
            .add_insert_at(InsertTarget::Track(keys), &synth, Some(0))?;
        if let Some(t) = self.session.project.track_mut(keys) {
            t.height = Some(110.0);
        }
        if dialog {
            self.multi_out = crate::multi_out::Dialog::open(self, insert);
            return Ok(());
        }
        let (_, tracks) = self.session.create_multi_out_rack(insert, None)?;
        self.selection = tracks.first().map(|&t| Selection::Track(t));
        self.session.mark_dirty();
        Ok(())
    }

    /// For the screenshot check (`--show layout`): see its `--show` arm.
    fn show_layout(&mut self) -> anyhow::Result<()> {
        use egui_tiles::{LinearDir, Tile};
        let file = self
            .session
            .project
            .tracks
            .iter()
            .find_map(|t| t.clips.first().map(|c| c.file().to_path_buf()))
            .ok_or_else(|| anyhow::anyhow!("the demo has no audio clip"))?;
        let file = self.session.resolve(&file);
        let copy = self
            .session
            .project_folder(daw_core::folder::TEMP)?
            .join("DI copy.wav");
        std::fs::copy(&file, &copy)?;
        let docs = [
            self.session.open_document(&file)?,
            self.session.open_document(&copy)?,
        ];
        self.show_mixer = true;
        let mut tree = crate::dock::default_layout();
        crate::dock::reconcile(&mut tree, &crate::dock::wanted(self));
        // The files' tabs to a row of their own under the arrangement.
        let tiles: Vec<_> = docs
            .iter()
            .filter_map(|&d| crate::dock::find(&tree, crate::dock::Pane::Waveform(Some(d))))
            .collect();
        for &t in &tiles {
            if let Some(parent) = tree.tiles.parent_of(t) {
                if let Some(Tile::Container(c)) = tree.tiles.get_mut(parent) {
                    c.remove_child(t);
                }
            }
        }
        let bottom = tree.tiles.insert_tab_tile(tiles);
        // The mixer out of the column, to the right of it.
        let mixer = crate::dock::find(&tree, crate::dock::Pane::Mixer)
            .and_then(|m| tree.tiles.parent_of(m));
        let column = crate::dock::find(&tree, crate::dock::Pane::Multitrack)
            .and_then(|m| tree.tiles.parent_of(m))
            .and_then(|tabs| tree.tiles.parent_of(tabs));
        if let (Some(mixer), Some(column)) = (mixer, column) {
            if let Some(Tile::Container(c)) = tree.tiles.get_mut(column) {
                c.remove_child(mixer);
                c.add_child(bottom);
            }
            if let Some(root) = tree.root() {
                if let Some(Tile::Container(egui_tiles::Container::Linear(l))) =
                    tree.tiles.get_mut(root)
                {
                    if l.dir == LinearDir::Horizontal {
                        l.children.insert(1, mixer);
                        l.shares.set_share(column, 2.6);
                        l.shares.set_share(mixer, 1.6);
                    }
                }
            }
        }
        tree.simplify(&crate::dock::options());
        self.dock = crate::dock::Dock::new(tree);
        self.view_mode = ViewMode::Multitrack;
        Ok(())
    }

    /// Set `kind`'s parameters on insert `id` by name, in plain values (the screenshot
    /// scenarios' own settings); a name the effect lacks is skipped.
    fn set_named(
        &mut self,
        id: daw_core::project::InsertId,
        kind: &daw_core::native::Kind,
        settings: &[(&str, f64)],
    ) -> anyhow::Result<()> {
        for &(name, plain) in settings {
            if let Some(i) = kind.params.iter().position(|p| p.name == name) {
                self.session
                    .set_parameter(id, i as u32, kind.params[i].normalized(plain))?;
            }
        }
        Ok(())
    }

    /// For the screenshot checks (`--show delay`, `echo`, `analog-delay`, `chorus`, `flanger`,
    /// `phaser`): the effect on Guitar L (or the first track), set, its editor open.
    fn show_delay(&mut self, which: &str) -> anyhow::Result<()> {
        use daw_core::native::{analog_delay, chorus, delay, echo, flanger, phaser};
        let (kind, settings): (&'static daw_core::native::Kind, &[(&str, f64)]) = match which {
            "delay" => (
                &delay::KIND,
                &[
                    ("Left Channel Delay Time", -60.0),
                    ("Right Channel Delay Time", 250.0),
                    ("Left Channel Mix", 60.0),
                ],
            ),
            "echo" => (
                &echo::KIND,
                &[
                    ("Delay Time (Left Channel)", 400.0),
                    ("Delay Time (Right Channel)", 300.0),
                    ("Feedback (Left Channel)", 50.0),
                    ("Feedback (Right Channel)", 50.0),
                    ("EQ Band 3", 3.0),
                    ("Echo Bounce", 1.0),
                ],
            ),
            "analog-delay" => (
                &analog_delay::KIND,
                &[
                    ("Mode", 1.0),
                    ("Delay", 350.0),
                    ("Feedback", 45.0),
                    ("Trash", 40.0),
                ],
            ),
            "chorus" => (
                &chorus::KIND,
                &[
                    ("Voices", 4.0),
                    ("Highest Quality", 1.0),
                    ("Modulation Rate", 1.2),
                    ("Modulation Depth", 3.0),
                    ("Wet Output Level", 50.0),
                ],
            ),
            "flanger" => (
                &flanger::KIND,
                &[
                    ("Final Delay Time", 6.0),
                    ("Modulation Rate", 0.5),
                    ("Feedback", 50.0),
                    ("Mix", 50.0),
                ],
            ),
            "phaser" => (
                &phaser::KIND,
                &[
                    ("Stages", 6.0),
                    ("Mod Rate", 0.5),
                    ("Feedback", 30.0),
                    ("Shape", 1.0),
                ],
            ),
            other => anyhow::bail!("no delay '{other}'"),
        };
        let track = self
            .session
            .project
            .tracks
            .iter()
            .find(|t| t.name == "Guitar L")
            .or(self.session.project.tracks.first())
            .map(|t| t.id)
            .ok_or_else(|| anyhow::anyhow!("no track for it"))?;
        let id = self
            .session
            .add_insert(InsertTarget::Track(track), &daw_core::native::path_of(kind))?;
        self.set_named(id, kind, settings)?;
        self.session.toggle_editor(id)?;
        self.selection = Some(Selection::Track(track));
        Ok(())
    }

    /// For the screenshot check (`--show full-reverb`): a Space Reverb on the Reverb bus, a
    /// long hall, with its impulse made before the first frame.
    fn show_full_reverb(&mut self) -> anyhow::Result<()> {
        let kind = &daw_core::native::full_reverb::KIND;
        let bus = self
            .session
            .project
            .tracks
            .iter()
            .find(|t| t.name == "Reverb")
            .or(self.session.project.tracks.first())
            .map(|t| t.id)
            .ok_or_else(|| anyhow::anyhow!("no track for it"))?;
        let id = self
            .session
            .add_insert(InsertTarget::Track(bus), &daw_core::native::path_of(kind))?;
        self.set_named(
            id,
            kind,
            &[
                ("Reverberation Decay Time", 3500.0),
                ("Reverberation Pre-Delay Time", 40.0),
                ("Output Level (Reverberation)", 60.0),
            ],
        )?;
        self.session
            .wait_for_impulses(std::time::Duration::from_secs(20))?;
        self.session.toggle_editor(id)?;
        self.selection = Some(Selection::Track(bus));
        Ok(())
    }

    /// For the screenshot check (`--show convolution-reverb`): a Convolution Reverb on the
    /// Reverb bus, set, playing a made room impulse.
    fn show_convolution_reverb(&mut self) -> anyhow::Result<()> {
        let kind = &daw_core::native::convolution_reverb::KIND;
        let bus = self
            .session
            .project
            .tracks
            .iter()
            .find(|t| t.name == "Reverb")
            .or(self.session.project.tracks.first())
            .map(|t| t.id)
            .ok_or_else(|| anyhow::anyhow!("no track for it"))?;
        let id = self
            .session
            .add_insert(InsertTarget::Track(bus), &daw_core::native::path_of(kind))?;
        self.set_named(
            id,
            kind,
            &[("Mix", 40.0), ("Pre-Delay", 12.0), ("Damping HF", 60.0)],
        )?;
        let path = self
            .session
            .project_folder(daw_core::folder::IMPORTED)?
            .join("impulse-room.wav");
        const RATE: u32 = 48_000;
        daw_core::wav::write_f32(
            &path,
            &daw_core::wav::Audio {
                sample_rate: RATE,
                channels: room_impulse(RATE),
            },
        )?;
        self.session.set_impulse_file(id, &path)?;
        self.session.toggle_editor(id)?;
        self.selection = Some(Selection::Track(bus));
        Ok(())
    }

    /// For the screenshot checks (`--show declipper`, `click-pop`): open the first clip's file
    /// as a document, have `insert` learn the whole of it, and close it again.
    fn scan_first_clip(&mut self, insert: daw_core::project::InsertId) -> anyhow::Result<()> {
        let file = self
            .session
            .project
            .tracks
            .iter()
            .find_map(|t| t.clips.first().map(|c| c.file().to_path_buf()))
            .ok_or_else(|| anyhow::anyhow!("no clip to scan"))?;
        let path = self.session.resolve(&file);
        let doc = self.session.open_document(&path)?;
        let len = self
            .session
            .document(doc)
            .map_or(0, |d| d.audio.len() as u64);
        let scanned = self
            .session
            .capture_noise_print(doc, (0, len), Some(insert));
        self.session.close_document(doc, false)?;
        scanned.map(|_| ())
    }

    /// For the screenshot check (`--show midi-staves`): a piano as Guitar Pro exports it (two
    /// staves, both "Acoustic Piano", at the project's tempo so nothing is asked) dropped on
    /// the first instrument track at bar 1, after giving it an instrument (the `synth` VST3
    /// when given, else Amplify standing in): the bass staff lands on a new track with its own.
    fn show_midi_staves(&mut self) -> anyhow::Result<()> {
        use daw_core::midi_clip::{MidiClip, Note};
        let staff = |pitches: &[u8], every: u64| {
            let clip = MidiClip {
                id: 0,
                name: String::new(),
                start: 0,
                length: pitches.len() as u64 * every,
                notes: pitches
                    .iter()
                    .enumerate()
                    .map(|(k, &pitch)| Note {
                        id: 0,
                        tick: k as u64 * every,
                        length: every - 60,
                        pitch,
                        velocity: 90,
                        channel: 0,
                    })
                    .collect(),
                controllers: Vec::new(),
            };
            let (tempo, meter) = (
                self.session.project.tempo,
                self.session.project.time_signature,
            );
            daw_core::smf::write_clip(&clip, "Acoustic Piano", tempo, meter)
        };
        let treble = staff(
            &[
                64, 67, 72, 71, 69, 67, 65, 64, 62, 64, 67, 72, 76, 74, 72, 71,
            ],
            480,
        )?;
        let bass = staff(&[36, 43, 41, 43, 36, 43, 41, 31], 960)?;
        let path = self
            .session
            .project_folder(daw_core::folder::TEMP)?
            .join("piano.mid");
        std::fs::write(&path, daw_core::smf::join(&[&treble, &bass])?)?;
        let rows: Vec<(TrackId, TrackKind)> = self
            .session
            .project
            .tracks
            .iter()
            .filter(|t| !self.session.project.is_hidden(t.id))
            .map(|t| (t.id, t.kind))
            .collect();
        let start = rows.iter().position(|r| r.1 == TrackKind::Instrument);
        if let Some(&(keys, _)) = start.and_then(|k| rows.get(k)) {
            if self.session.instrument_of(keys).is_none() {
                let path = self.demo_instrument("synth").unwrap_or_else(|_| {
                    daw_core::native::path_of(&daw_core::native::amplify::KIND)
                });
                self.session.add_insert(InsertTarget::Track(keys), &path)?;
            }
        }
        // Bar 5, clear of the clips already on Keys.
        let at = self.session.tempo_map().bar_start(4).round() as u64;
        crate::drop::drop_files(self, vec![path], at, rows, start);
        // Into Keys and a new track (16.2's question, answered as a click would).
        crate::drop::answer_into(self, Some(true));
        self.selection = None;
        Ok(())
    }

    /// For the screenshot check: lanes on the first guitar, open under it. Volume rises over
    /// bar 1 on a curve, eases off, then jumps down at bar 3; pan sweeps left to right. One
    /// point is selected.
    /// The first audio clip split at 40%, the pieces crossfaded over a quarter second, a fade
    /// in and out at the ends, and the second piece at -4 dB and selected (the `fades` shot).
    /// Returns the first piece.
    fn show_fades(&mut self) -> Option<ClipId> {
        use daw_core::project::{Fade, FadeShape};
        let (track, clip) = self
            .session
            .project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .find_map(|t| t.clips.first().map(|c| (t.id, c.clone())))?;
        let rate = f64::from(self.session.sample_rate());
        let seconds = |s: f64| (s * rate) as u64;
        let second = self
            .session
            .project
            .split_clip(clip.id, clip.start + clip.length * 2 / 5)?;
        let crossfade =
            self.session
                .crossfade_clips(second, Some(seconds(0.25)), FadeShape::EqualPower);
        if let Err(e) = crossfade {
            self.toast(format!("{e:#}"));
        }
        let fade = |length| Fade {
            length,
            shape: FadeShape::EqualPower,
        };
        if let Some(first) = self.session.project.clip_mut(clip.id) {
            let fade_out = first.fade_out;
            first.set_fades(fade(seconds(0.3)), fade_out);
        }
        if let Some(last) = self.session.project.clip_mut(second) {
            let fade_in = last.fade_in;
            last.set_fades(fade_in, fade(seconds(0.8)));
            last.gain_db = -4.0;
        }
        self.session.mark_dirty();
        self.selection = Some(Selection::Track(track));
        self.select_clip(track, second, false);
        self.show_inspector = true;
        Some(clip.id)
    }

    /// The first audio clip stretched ×1.5 and up 2 semitones, rendered, and selected so the
    /// inspector shows its fields (the `stretch` check shot).
    fn show_stretch(&mut self) {
        let Some((track, clip)) = self
            .session
            .project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio)
            .find_map(|t| t.clips.first().map(|c| (t.id, c.id)))
        else {
            return;
        };
        let stretched = self
            .session
            .set_clip_stretch(clip, Some(1.5), Some(2.0))
            .and_then(|_| self.session.wait_for_stretches(Duration::from_secs(60)));
        if let Err(e) = stretched {
            self.toast(format!("{e:#}"));
        }
        self.selection = Some(Selection::Track(track));
        self.select_clip(track, clip, false);
        self.show_inspector = true;
    }

    fn show_automation(&mut self) {
        use daw_core::automation::{AutomationTarget, NewPoint};
        let Some(guitar) = self
            .session
            .project
            .tracks
            .iter()
            .find(|t| t.kind == TrackKind::Audio)
            .map(|t| t.id)
        else {
            return;
        };
        let bar = u64::from(daw_core::tempo::TICKS_PER_QUARTER) * 4;
        let point = |tick: u64, value: f64, curve: f32| NewPoint { tick, value, curve };
        let volume = [
            point(0, -24.0, 0.5),
            point(bar, 0.0, 0.0),
            point(bar * 2, -3.0, 0.0),
            point(bar * 2, -14.0, 0.0),
            point(bar * 3, -14.0, 0.0),
        ];
        let pan = [point(0, -0.8, -0.5), point(bar * 3, 0.8, 0.0)];
        let lanes = [
            (AutomationTarget::Volume, &volume[..]),
            (AutomationTarget::Pan, &pan[..]),
        ];
        for (target, points) in lanes {
            if let Err(e) = self
                .session
                .add_automation_lane(Some(guitar), target, Some(points))
            {
                self.toast(e);
            }
        }
        let selected = self.session.project.track(guitar).and_then(|t| {
            let lane = t.automation.first()?;
            Some((lane.id, lane.points.get(1)?.id))
        });
        self.selected_point = selected;
        self.selection = Some(Selection::Track(guitar));
        // In touch mode, so the mode buttons show one lit (5.4).
        if let Some(track) = self.session.project.track_mut(guitar) {
            track.automation_mode = daw_core::automation::AutomationMode::Touch;
        }
    }

    /// A small project that exercises folders, routing, clips and inserts.
    fn build_demo(&mut self, wav: &Path, plugins: &[String]) {
        // The demo is a project of its own, made afresh under the working directory's
        // `out/check`, so its imported copy of the file and anything it writes stay there.
        let parent = std::path::absolute("out/check").unwrap_or_default();
        let _ = std::fs::remove_dir_all(parent.join("Demo"));
        // A demo still running holds its folder: this one gets its own.
        let made = self
            .session
            .new_project_in("Demo", &parent, None)
            .or_else(|_| {
                let name = format!("Demo-{}", std::process::id());
                self.session.new_project_in(&name, &parent, None)
            });
        if let Err(e) = made {
            self.toast(format!("{e:#}"));
        }
        let s = &mut self.session;
        let folder = s.add_track(TrackKind::Folder, None);
        s.project.track_mut(folder).unwrap().name = "Guitars".into();
        let left = s.add_track(TrackKind::Audio, Some(folder));
        let right = s.add_track(TrackKind::Audio, Some(folder));
        s.project.track_mut(left).unwrap().name = "Guitar L".into();
        s.project.track_mut(left).unwrap().mixer.pan = -0.8;
        s.project.track_mut(right).unwrap().name = "Guitar R".into();
        s.project.track_mut(right).unwrap().mixer.pan = 0.8;
        let synth = s.add_track(TrackKind::Instrument, None);
        s.project.track_mut(synth).unwrap().name = "Keys".into();
        // Two bars of chords from bar 2 (silent until MIDI playback, dev plan 4.2).
        let bar = u64::from(daw_core::midi_clip::TICKS_PER_QUARTER) * 4;
        if let Ok(clip) = s.project.add_midi_clip(synth, bar, 2 * bar) {
            let chords: [&[u8]; 4] = [&[60, 64, 67], &[57, 60, 64], &[53, 57, 60], &[55, 59, 62]];
            let notes: Vec<daw_core::midi_clip::Note> = chords
                .iter()
                .enumerate()
                .flat_map(|(k, chord)| {
                    chord.iter().map(move |&pitch| daw_core::midi_clip::Note {
                        id: 0,
                        tick: k as u64 * bar / 2,
                        length: bar / 2 - 120,
                        pitch,
                        velocity: 96,
                        channel: 0,
                    })
                })
                .collect();
            let _ = s.project.add_notes(clip, &notes);
        }
        let vox = s.add_track(TrackKind::Audio, None);
        s.project.track_mut(vox).unwrap().name = "Vocal".into();
        // A reverb return fed by both guitars (post-fader) and the vocal (pre-fader).
        let reverb = s.add_track(TrackKind::Bus, None);
        s.project.track_mut(reverb).unwrap().name = "Reverb".into();
        for (from, gain_db, pre_fader) in [
            (left, -8.0, false),
            (right, -8.0, false),
            (vox, -12.0, true),
        ] {
            if let Ok(send) = s.project.add_send(from, reverb, pre_fader) {
                if let Some(send) = s.project.find_send_mut(send) {
                    send.gain_db = gain_db;
                }
            }
        }
        let rate = u64::from(s.sample_rate());
        for (track, at) in [(left, 0), (right, rate / 2)] {
            if let Err(e) = self.session.add_clip_from_file(track, wav, at) {
                self.toast(format!("{e:#}"));
            }
        }
        let installed = self.session.installed_plugins().to_vec();
        for name in plugins {
            match daw_core::plugins::resolve(name, &installed)
                .and_then(|path| self.session.add_insert(InsertTarget::Track(left), &path))
            {
                Ok(_) => {}
                Err(e) => self.toast(format!("{e:#}")),
            }
        }
        self.selection = Some(Selection::Track(left));
    }

    /// The `amp`, `compressor` and `maximizer` VST3s (`--vst3`, those installed) on each of the
    /// demo's guitars, so an open lasts several frames (one heavy plug-in a frame); without
    /// them, or in a neutral run (the manual's busy shot), built-in effects in their places.
    fn add_demo_plugins(&mut self) {
        let guitars: Vec<TrackId> = self
            .session
            .project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && !t.clips.is_empty())
            .map(|t| t.id)
            .collect();
        for &track in &guitars {
            for (name, builtin) in [
                ("amp", "Compressor"),
                ("compressor", "Parametric EQ"),
                ("maximizer", "Limiter"),
            ] {
                if let Some(path) = self.demo_effect(name, builtin) {
                    let _ = self.session.add_insert(InsertTarget::Track(track), &path);
                }
            }
        }
    }

    /// `count` more files on the demo's Vocal track, a chord each (20 s, stereo), written into
    /// its `Imported` folder: an open with several files to read.
    fn add_demo_files(&mut self, count: usize) {
        let vocal = self
            .session
            .project
            .tracks
            .iter()
            .find(|t| t.name == "Vocal")
            .map(|t| t.id);
        let (Some(vocal), Ok(folder)) = (
            vocal,
            self.session.project_folder(daw_core::folder::IMPORTED),
        ) else {
            return;
        };
        let rate = self.session.sample_rate();
        for k in 0..count {
            let path = folder.join(format!("Chord {}.wav", k + 1));
            let frames = rate as usize * 20;
            let channels = (0..2)
                .map(|c| {
                    (0..frames)
                        .map(|i| {
                            let t = i as f32 / rate as f32;
                            [220.0, 277.2, 329.6]
                                .iter()
                                .map(|f| (std::f32::consts::TAU * f * (k + c + 1) as f32 * t).sin())
                                .sum::<f32>()
                                * 0.1
                        })
                        .collect()
                })
                .collect();
            let audio = daw_core::wav::Audio {
                sample_rate: rate,
                channels,
            };
            let at = u64::from(rate) * 20 * k as u64;
            let added = daw_core::wav::write_f32(&path, &audio)
                .and_then(|()| self.session.add_clip_from_file(vocal, &path, at));
            if let Err(e) = added {
                self.toast(format!("{e:#}"));
            }
        }
    }

    fn open_first_editor(&mut self) {
        let Some(Selection::Track(id)) = self.selection else {
            return;
        };
        let Some(slot) = self
            .session
            .project
            .track(id)
            .and_then(|t| t.inserts.first())
            .cloned()
        else {
            return;
        };
        match self.session.toggle_editor(slot.id) {
            Ok(()) => println!(
                "editor for {} open: {:?}",
                slot.name,
                self.session.insert_info(slot.id).map(|(_, _, open)| open)
            ),
            Err(e) => println!("editor for {} failed: {e:#}", slot.name),
        }
    }

    /// The window is minimised (nothing is painted; `logic` still runs).
    pub fn minimized(&self) -> bool {
        self.egui_ctx
            .input(|i| i.viewport().minimized)
            .unwrap_or(false)
    }

    /// The port MCP serves on: `--mcp-port` for this run, else the settings'.
    pub fn mcp_port(&self) -> u16 {
        self.mcp_port_override.unwrap_or(self.settings.mcp.port)
    }

    /// The token MCP requires: `--mcp-token` for this run, else the settings' (if any).
    pub fn mcp_wanted_token(&self) -> Option<String> {
        self.mcp_token_override
            .clone()
            .or_else(|| self.settings.mcp.token.clone())
    }

    /// Start (or restart) the MCP server on its port, on the worker: returns at once, the
    /// status saying "MCP starting…" until it is up or says why not (a busy port: another DAW
    /// window, a check run).
    pub fn start_mcp(&mut self) {
        self.mcp.start(self.mcp_port(), self.mcp_wanted_token());
    }

    /// Settings > MCP changed (Enabled, the port, the token): saved, and applied at once, the
    /// server restarted in the background. A new port replaces `--mcp-port`'s.
    pub fn apply_mcp_settings(&mut self, mcp: crate::settings::McpSettings) {
        if mcp.port != self.mcp_port() {
            self.mcp_port_override = None;
        }
        self.settings.mcp = mcp;
        self.save_settings();
        // The clients' rows judge their entries against the new port and token.
        self.claude.refresh(self.claude_wanted());
        if self.settings.mcp.enabled {
            self.start_mcp();
        } else {
            self.mcp.stop(None);
        }
    }

    /// Retry after MCP failed to start (its port taken): the status bar's and the settings'.
    pub fn retry_mcp(&mut self) {
        // A check's own hold on the port (`--show mcp-port-in-use`) lets go first.
        self.held_port = None;
        self.mcp.retry();
    }

    /// Options > Settings… on `section` (the status bar's MCP and Options > AI control open it
    /// on AI control). Already open: it turns to `section`.
    pub fn open_settings(&mut self, section: crate::settings_window::Section) {
        match &mut self.settings_dialog {
            Some(dialog) => dialog.section = section,
            None if self.neutral => {
                // The port field shows MCP's default, as the status bar does (TD194).
                self.settings_dialog = Some(crate::settings_window::SettingsDialog::neutral(
                    self.session.sample_rate(),
                    crate::neutral::MCP_PORT,
                    section,
                ))
            }
            None => {
                self.settings_dialog = Some(crate::settings_window::SettingsDialog::new(
                    &self.session.settings,
                    self.mcp_port(),
                    section,
                ))
            }
        }
        self.claude.refresh(self.claude_wanted());
    }

    /// The audio device as the status bar and Settings > Audio show it: the running one, or
    /// in a neutral run (the manual's shots) a neutral name for the virtual one.
    pub fn shown_device(&self) -> Option<daw_core::device::DeviceInfo> {
        if self.neutral {
            return Some(crate::neutral::device_info(self.session.sample_rate()));
        }
        self.session.device.as_ref().map(|d| d.info.clone())
    }

    /// The MIDI inputs as the status bar and Settings > MIDI show them.
    pub fn shown_midi_ports(&self) -> Vec<String> {
        if self.neutral {
            return crate::neutral::MIDI_PORTS
                .iter()
                .map(|p| p.to_string())
                .collect();
        }
        self.session.midi.ports.clone()
    }

    /// Help > Manual and F1 (27.3): the manual's page for `place`; why not, as a toast.
    pub fn help(&mut self, place: crate::help::Place) {
        if let Err(e) = self.open_manual(place.target()) {
            self.toast(e);
        }
    }

    /// Show the manual at `target` in the default browser (`help.rs`). A check run only notes
    /// the request (`manual_requests`). Returns what `open_manual`'s answer adds: `opened`,
    /// and the `url` when the manual is there.
    pub fn open_manual(
        &mut self,
        target: daw_core::manual::Target,
    ) -> Result<serde_json::Value, String> {
        let dir = self.session.manual_dir.clone();
        let url = match &dir {
            Some(dir) => Some(crate::help::url(dir, &target)?),
            None => None,
        };
        if self.is_check_run() {
            self.manual_requests.push(target);
            return Ok(json!({ "opened": false, "url": url }));
        }
        let Some(url) = url else {
            return Err(crate::help::missing());
        };
        crate::help::launch(&url)?;
        Ok(json!({ "opened": true, "url": url }))
    }

    /// What Claude's configs should say: the port and token MCP is asked to serve with.
    pub fn claude_wanted(&self) -> mcp_link::clients::Wanted {
        mcp_link::clients::Wanted {
            port: self.mcp_port(),
            token: self.mcp_wanted_token(),
        }
    }

    /// The command that connects Claude Code to the server, token included: on the port asked
    /// for, so it follows a change before the server is up there.
    pub fn mcp_connect_command(&self) -> String {
        let token = self.mcp_wanted_token();
        crate::mcp::connect_command(&crate::mcp::address(self.mcp_port()), token.as_deref())
    }

    /// Run the tool calls agents have sent since the last frame. While a project opens, the
    /// calls that would see it half loaded wait for it (`Mcp::take_calls`); with none open
    /// (the start screen), changes to a project are refused (`mcp::refusal`).
    fn run_mcp_calls(&mut self) {
        // A save's states are read over frames: calls wait for it as for an open, so none
        // changes the project half saved.
        let opening = self.session.loading().is_some() || self.session.save_progress().is_some();
        let pending = self.mcp.take_calls(opening);
        let registry = self.registry.clone();
        for call in pending {
            self.mcp_calls += 1;
            let project_open = self.session.path.is_some();
            let safety = registry.safety(&call.name);
            if let Some(why) = crate::mcp::refusal(&call.name, safety, project_open) {
                (call.reply)(Err(daw_core::api::ApiError(why)));
                continue;
            }
            registry.call(self, &call.name, call.args, call.reply);
        }
    }

    /// Everything that must keep running while the window is hidden or minimised: engine
    /// housekeeping (finishing recordings and exports, plug-in windows), meters, and agent
    /// calls. Painting stays in `ui`.
    fn housekeeping(&mut self, ctx: &egui::Context) {
        let now = Instant::now();
        let dt = now.duration_since(self.last_frame).as_secs_f32().min(0.2);
        self.last_frame = now;
        // The MCP worker's news (up, or why not), then the calls agents sent.
        if self.mcp.poll() && self.mcp.status().failed() {
            self.toast(self.mcp.status().text());
        }
        self.run_mcp_calls();
        self.claude.poll();
        // The start screen's recent projects: looked at while it shows, again each time it
        // opens (a drive may have come back).
        self.recent_probe.poll();
        if crate::start::visible(self) {
            self.recent_probe.want(&self.settings.recent);
        } else {
            self.recent_probe.forget();
        }
        crate::opening::hold(self);
        // The Waveform view's file and whether it is in front, which the project file
        // remembers with the open files: told before `update`, where a save is written.
        self.session
            .set_document_view(self.editor.current, self.view_mode == ViewMode::Waveform);
        self.session.update();
        // An open's Waveform view files are back: shown as the project file remembered them.
        if let Some(view) = self.session.take_reopened_view() {
            self.editor.current = view.shown;
            if let Some(id) = view.shown {
                self.editor.view(id);
            }
            self.view_mode = if view.in_front {
                ViewMode::Waveform
            } else {
                ViewMode::Multitrack
            };
        }
        // Agents waiting for an open: answered once it is in (or cancelled).
        if self.agent_opens.tick(&mut self.session) {
            self.project_opened();
        }
        for notice in std::mem::take(&mut self.session.notices) {
            self.toast(notice);
        }
        // Files hovering from Explorer (or dropping): where the pointer is, from Windows.
        let hovering =
            ctx.input(|i| !i.raw.hovered_files.is_empty() || !i.raw.dropped_files.is_empty());
        if self.demo_drop.is_none() {
            self.drag_pointer = hovering
                .then(|| crate::drop::os_pointer(self.hwnd, ctx.pixels_per_point()))
                .flatten();
        }
        // How a background save ended, if it did this frame: for the toast, the save question
        // and agents waiting on it.
        let ended = self.session.take_saved();
        self.agent_saves.tick(&mut self.session, ended.as_ref());
        let saved = crate::busy::saved(self, ended.as_ref());
        // A file dialog's answer (acted on in the next `ui`, inside its edit).
        crate::file_dialog::tick(self);
        if self.backup_question_due && self.session.loading().is_none() {
            self.backup_question_due = false;
            self.backup_question = self.session.newer_backup().map(Path::to_path_buf);
        }
        crate::unsaved::tick(self, ctx, saved);
        // Keep saved preferences in step with the session (agents can change audio settings).
        if self.settings.audio != self.session.settings {
            self.settings.audio = self.session.settings.clone();
            self.save_settings();
        }
        // The session reports how an export ended through its notices; a finished one also gets
        // its report.
        let was = self.exporting.as_ref().map(|e| e.1);
        let status = self.session.export_status();
        self.exporting = match &status {
            ExportStatus::Running {
                progress,
                kind,
                what,
            } => Some((*progress, *kind, what.clone())),
            _ => None,
        };
        // An export's report; a render or mixdown has its toast, and its new track or file.
        let reported = matches!(
            was,
            Some(daw_core::session::ExportKind::Export | daw_core::session::ExportKind::File)
        );
        if reported && matches!(status, ExportStatus::Finished { .. }) {
            self.export_report = Some(status);
        }
        self.update_meters(dt);
        if self.exit_at.is_some_and(|at| now >= at) {
            // A check run's end: nothing asked.
            self.closing = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        // Keep meters, the playhead, plug-in editors and agent calls moving; this also keeps
        // `logic` running while the window is hidden.
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    /// `--show recording`: arm the demo's Vocal on input 1, replace the audio device with a
    /// virtual one whose input plays `wav` over and over, and record from bar 2.
    fn record_demo(&mut self, wav: Option<&std::path::Path>) -> anyhow::Result<()> {
        self.virtual_input_demo(wav)?;
        self.session.record()
    }

    /// The demo's Vocal armed on input 1, the audio device replaced with a virtual one whose
    /// input plays `wav` over and over (nothing is heard), and the playhead on bar 2: ready to
    /// record (`--show recording`, `count-in`).
    fn virtual_input_demo(&mut self, wav: Option<&std::path::Path>) -> anyhow::Result<()> {
        use daw_core::project::TrackInput;
        let wav = wav.ok_or_else(|| anyhow::anyhow!("needs --demo"))?;
        let s = &mut self.session;
        let audio = daw_core::media::load(wav, s.sample_rate())?;
        let vocal = s
            .project
            .tracks
            .iter()
            .find(|t| t.name == "Vocal")
            .map(|t| t.id)
            .ok_or_else(|| anyhow::anyhow!("the demo has no Vocal track"))?;
        if let Some(track) = s.project.track_mut(vocal) {
            track.input = TrackInput::Audio {
                channel: 0,
                stereo: false,
            };
            track.arm = true;
        }
        // The file's first channel, looped: one input. Runs on the virtual device's thread,
        // allocation-free.
        let mut at = 0;
        let signal = Box::new(move |frames: &mut [f32]| {
            let samples = &audio.channels[0];
            for frame in frames.iter_mut() {
                *frame = samples.get(at).copied().unwrap_or(0.0);
                at = if at + 1 < samples.len() { at + 1 } else { 0 };
            }
        });
        s.start_virtual_input(256, 1, signal)?;
        let bar_2 = s.tempo_map().bar_start(1).round() as u64;
        s.locate(bar_2)?;
        s.mark_dirty();
        s.update();
        // Vocal selected: its header's R and its input and monitoring in the inspector are
        // what the manual's recording figure points at.
        self.selection = Some(Selection::Track(vocal));
        Ok(())
    }

    /// Select a clip (audio or MIDI): alone, or with `extend` toggled in or out of the
    /// selection.
    pub fn select_clip(&mut self, track: TrackId, clip: ClipId, extend: bool) {
        self.selected_point = None;
        self.selection = Some(Selection::Track(track));
        if !extend {
            self.selected_clips.clear();
        } else if self.selected_clips.remove(&clip) {
            // Toggled out: another selected clip becomes the primary one, if any.
            self.selected_clip = self
                .selected_clips
                .iter()
                .next()
                .and_then(|&c| self.session.project.clip_track(c).map(|t| (t, c)));
            return;
        }
        self.selected_clips.insert(clip);
        self.selected_clip = Some((track, clip));
    }

    /// Select exactly these clips (unknown ids are skipped); the first becomes the primary.
    pub fn select_clips(&mut self, clips: &[ClipId]) {
        self.clear_clip_selection();
        for &clip in clips {
            if let Some(track) = self.session.project.clip_track(clip) {
                if self.selected_clip.is_none() {
                    self.selected_clip = Some((track, clip));
                    self.selection = Some(Selection::Track(track));
                }
                self.selected_clips.insert(clip);
            }
        }
    }

    /// Copy (or cut) the selected clips to the clipboard.
    pub fn copy_selection(&mut self, cut: bool) {
        let clips = self.selected_clip_ids();
        if clips.is_empty() {
            return;
        }
        let done = if cut {
            if self
                .piano_roll
                .as_ref()
                .is_some_and(|p| clips.contains(&p.clip))
            {
                self.piano_roll = None;
            }
            self.session.label_edit("Cut clips", None);
            self.session.cut_clips(&clips)
        } else {
            self.session.copy_clips(&clips)
        };
        match done {
            Ok(n) => {
                if cut {
                    self.clear_clip_selection();
                }
                crate::clipboard::announce(self, crate::clipboard::clips_line(n));
                self.toast(format!(
                    "{} {n} clip{}",
                    if cut { "Cut" } else { "Copied" },
                    if n == 1 { "" } else { "s" }
                ));
            }
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// Paste the clipboard at the playhead: onto the selected track if a track is selected
    /// (the others keeping their rows below it), else where the clips came from. The pasted
    /// clips become the selection.
    pub fn paste(&mut self) {
        let track = match self.selection {
            Some(Selection::Track(id)) => Some(id),
            _ => None,
        };
        let at = self.session.position();
        self.session.label_edit("Paste clips", None);
        match self.session.paste_clips(at, track) {
            Ok(clips) => self.select_clips(&clips),
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// Copies of the selected clips right after them; the copies become the selection.
    pub fn duplicate_selection(&mut self) {
        let clips = self.selected_clip_ids();
        if clips.is_empty() {
            return;
        }
        self.session.label_edit("Duplicate clips", None);
        match self.session.duplicate_clips(&clips) {
            Ok(copies) => self.select_clips(&copies),
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    pub fn select_all_clips(&mut self) {
        let clips: Vec<ClipId> = self
            .session
            .project
            .tracks
            .iter()
            .flat_map(|t| {
                t.clips
                    .iter()
                    .map(|c| c.id)
                    .chain(t.midi_clips.iter().map(|c| c.id))
            })
            .collect();
        self.select_clips(&clips);
    }

    /// A marker at the playhead, named "Marker N".
    pub fn add_marker_at_playhead(&mut self) {
        let at = self.session.position();
        self.session.add_marker(at, None);
    }

    pub fn clear_clip_selection(&mut self) {
        self.selected_clip = None;
        self.selected_clips.clear();
    }

    pub fn is_clip_selected(&self, clip: ClipId) -> bool {
        self.selected_clips.contains(&clip)
    }

    /// The selected clips that still exist, in id order.
    pub fn selected_clip_ids(&self) -> Vec<ClipId> {
        self.selected_clips
            .iter()
            .copied()
            .filter(|&c| self.session.project.clip_track(c).is_some())
            .collect()
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            at: Instant::now(),
            lasts: Duration::from_secs(8),
        });
    }

    /// A toast that stays a minute: something the user must not miss (a failed save).
    pub fn toast_long(&mut self, text: impl Into<String>) {
        self.toasts.push(Toast {
            text: text.into(),
            at: Instant::now(),
            lasts: Duration::from_secs(60),
        });
    }

    /// Undo the last edit (Ctrl+Z, Edit menu), saying what it was.
    pub fn undo(&mut self) {
        match self.session.undo() {
            Ok(name) => self.toast(format!("Undid {name}")),
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// Redo the last undone edit (Ctrl+Shift+Z, Ctrl+Y, Edit menu).
    pub fn redo(&mut self) {
        match self.session.redo() {
            Ok(name) => self.toast(format!("Redid {name}")),
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// Show why something was refused, if it was.
    /// `--show diagnostics`: the first clip's file copied into the project's `Temp` with 12
    /// clicks of 1 to 20 samples added across it, opened in the Waveform view and scanned with the
    /// Click Repair; the third row picked and shown.
    fn show_diagnostics_demo(&mut self) -> anyhow::Result<()> {
        let file = self
            .session
            .project
            .tracks
            .iter()
            .find_map(|t| t.clips.first().map(|c| self.session.resolve(c.file())))
            .ok_or_else(|| anyhow::anyhow!("the demo has no audio clip"))?;
        let mut audio = daw_core::wav::read(&file)?;
        let rate = audio.sample_rate as usize;
        let length = audio.channels.first().map_or(0, Vec::len);
        let every = length.saturating_sub(rate / 2) / 12;
        for (k, len) in [1, 2, 3, 4, 5, 6, 8, 10, 12, 14, 16, 20]
            .into_iter()
            .enumerate()
        {
            // Spread over the file, from a quarter second in.
            let at = rate / 4 + k * every;
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            for c in &mut audio.channels {
                let end = (at + len).min(c.len());
                for (i, x) in c[at.min(end)..end].iter_mut().enumerate() {
                    *x += sign * (0.25 + 0.15 * ((i * 7 % 5) as f32 / 4.0));
                }
            }
        }
        let path = self
            .session
            .project_folder(daw_core::folder::TEMP)?
            .join("diagnostics-clicks.wav");
        daw_core::wav::write_f32(&path, &audio)?;
        crate::waveform::open_file(self, &path);
        let id = self
            .editor
            .current
            .ok_or_else(|| anyhow::anyhow!("{} didn't open", path.display()))?;
        self.show_diagnostics = true;
        self.diagnostics.kind = daw_core::diagnostics::ProblemKind::ClickRepair;
        // The view's width before the first frame draws it, for the zoom.
        self.editor.width = 1100.0;
        crate::diagnostics::scan(self, id, None);
        let third = self
            .session
            .audio_problems(id)
            .and_then(|(_, list)| list.get(2).cloned());
        if let Some(p) = third {
            self.diagnostics.picked.insert(p.id);
            crate::diagnostics::go_to(self, id, &p);
        }
        Ok(())
    }

    pub fn report(&mut self, result: anyhow::Result<()>) {
        if let Err(e) = result {
            self.toast(format!("{e:#}"));
        }
    }

    /// Open the project at `path` once the open one is saved or given up (`unsaved`).
    pub fn open_project_asking(&mut self, path: &Path) {
        let name = path
            .file_stem()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let path = path.to_path_buf();
        crate::unsaved::then(self, format!("opening {name}"), move |app| {
            app.open_project(&path);
        });
    }

    pub fn open_project(&mut self, path: &Path) {
        match self.session.open(path) {
            Ok(()) => {
                self.settings.add_recent(path.to_path_buf());
                self.save_settings();
                self.project_opened();
                self.start.open = false;
                self.backup_question_due = true;
            }
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// A project was opened or made: nothing of the last one stays selected or scrolled.
    pub fn project_opened(&mut self) {
        self.selection = None;
        self.clear_clip_selection();
        self.view_start = 0.0;
    }

    /// Save the settings (beside the program), saying so if that fails. A check run (one
    /// taking a screenshot) never does: what it sets up mustn't become the next run's settings.
    /// A check run (`--demo`, or a screenshot): it leaves the user's settings and clipboard
    /// as they are.
    pub fn is_check_run(&self) -> bool {
        self.screenshot.is_some() || self.checking
    }

    pub fn save_settings(&mut self) {
        // Check runs leave the user's settings (beside the program) as they are.
        if self.is_check_run() {
            return;
        }
        if let Err(e) = self.settings.save() {
            self.toast(format!("{e:#}"));
        }
    }

    /// A file dialog that starts in the project's `sub` folder (made if needed), or its
    /// folder, or with no project where new projects go; shown with `file_dialog`'s functions.
    pub fn dialog_in(&self, sub: Option<&str>) -> rfd::FileDialog {
        let dir = match sub {
            Some(sub) => self.session.project_folder(sub).ok(),
            None => self.session.project_dir(),
        }
        .or_else(|| self.settings.projects_parent.clone());
        match dir {
            Some(dir) => rfd::FileDialog::new().set_directory(dir),
            None => rfd::FileDialog::new(),
        }
    }

    /// Save (Ctrl+S), or Save As (`save_as`, or a project without a file yet): the file
    /// chosen in a dialog first (`file_dialog`, so the save starts once it has answered).
    pub fn save_project(&mut self, save_as: bool) {
        match (&self.session.path, save_as) {
            (Some(p), false) => {
                let path = p.clone();
                self.save_project_to(&path);
            }
            _ => {
                let dialog = self
                    .dialog_in(None)
                    .add_filter("Oxide project", &[daw_core::folder::EXTENSION])
                    .set_file_name(format!(
                        "{}.{}",
                        daw_core::folder::file_name(&self.session.project.name),
                        daw_core::folder::EXTENSION
                    ));
                crate::file_dialog::save_file(self, dialog, |app, path| {
                    app.save_project_to(&path);
                });
            }
        }
    }

    /// Written in the background, behind the busy overlay (`busy::saved` says when).
    fn save_project_to(&mut self, path: &Path) {
        if let Err(e) = self.session.save_in_background(path) {
            self.toast(format!("Save failed: {e:#}"));
        }
    }

    /// File > Import audio: the chosen files placed as a drop at the playhead on the selected
    /// track (asking whether into it or onto a new track), or on new tracks at the end.
    pub fn import_audio_dialog(&mut self) {
        let dialog = rfd::FileDialog::new().add_filter(
            "Audio (WAV, FLAC, MP3, AIFF)",
            &daw_core::media::AUDIO_EXTENSIONS,
        );
        crate::file_dialog::pick_files(self, dialog, |app, paths| app.import_files(paths));
    }

    /// Chosen files placed as File > Import places them: at the playhead (where it is once
    /// they are chosen) on the selected track, or on new tracks at the end.
    fn import_files(&mut self, paths: Vec<PathBuf>) {
        let (rows, start) = self.import_rows();
        let at = self.session.position();
        crate::drop::drop_files(self, paths, at, rows, start);
    }

    /// The visible tracks, and the selected one's row: where File > Import puts files.
    fn import_rows(&self) -> (Vec<(TrackId, TrackKind)>, Option<usize>) {
        let project = &self.session.project;
        let rows: Vec<(TrackId, TrackKind)> = project
            .tracks
            .iter()
            .filter(|t| !project.is_hidden(t.id))
            .map(|t| (t.id, t.kind))
            .collect();
        let start = match self.selection {
            Some(Selection::Track(track)) => rows.iter().position(|r| r.0 == track),
            _ => None,
        };
        (rows, start)
    }

    /// File > Import MIDI: the chosen files placed as a drop at the playhead on the selected
    /// track (or new tracks at the end), so they get the same names, instrument and tempo
    /// question.
    pub fn import_midi_dialog(&mut self) {
        let dialog =
            rfd::FileDialog::new().add_filter("MIDI files", &["mid", "midi", "smf", "kar"]);
        crate::file_dialog::pick_files(self, dialog, |app, paths| app.import_files(paths));
    }

    /// Mixdown to a new file (whole session, or `range`): rendered in the
    /// background; the file opens in the Waveform view when done.
    pub fn mixdown_to_new_file(&mut self, range: Option<(u64, u64)>) {
        if self.session.content_end() == 0 {
            self.toast("Nothing to mix down: the project has no clips");
            return;
        }
        match self.session.mixdown_to_new_file(range, 2.0) {
            Ok(path) => self.toast(format!("Mixing down to {}", path.display())),
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// The export options first; the file picker follows (`choose_export_file`).
    pub fn export_dialog(&mut self) {
        if self.session.content_end() == 0 {
            self.toast("Nothing to export: the project has no clips");
            return;
        }
        self.export_options = true;
    }

    /// Pick the file and start the export, normalised if the options say so.
    pub fn choose_export_file(&mut self) {
        let encoding = self.settings.export_encoding;
        let stem = format!("{} mixdown", self.session.project.name);
        let dialog = crate::export_format::save_dialog(self, &encoding, &stem);
        crate::file_dialog::save_file(self, dialog, move |app, path| {
            match daw_core::encode::with_extension(&path, &encoding.format) {
                Ok(path) => app.export_to(&path, encoding),
                Err(e) => app.toast(format!("{e:#}")),
            }
        });
    }

    /// Start the export to `path` in `encoding`, normalised if the options say so.
    fn export_to(&mut self, path: &Path, encoding: daw_core::encode::Encoding) {
        let normalize = self.settings.normalize_exports.then_some(Normalize {
            target: self.session.project.loudness_target,
            ceiling: dialogs::EXPORT_CEILING,
        });
        let range = self
            .time_selection
            .as_ref()
            .filter(|_| self.export_selection)
            .map(|r| (r.start, r.end));
        match self
            .session
            .export_as(path, range, self.export_tail, normalize, encoding)
        {
            Ok(()) => {
                self.exporting = Some((
                    0.0,
                    daw_core::session::ExportKind::Export,
                    "Exporting".into(),
                ))
            }
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// Select `range` and the clips inside it (added to the clip selection with `extend`).
    pub fn set_time_selection(&mut self, range: TimeSelection, extend: bool) {
        let clips = arrange::clips_in(self, &range);
        if !extend {
            self.clear_clip_selection();
        }
        for clip in clips {
            if !self.is_clip_selected(clip) {
                if let Some(track) = self.session.project.clip_track(clip) {
                    self.select_clip(track, clip, true);
                }
            }
        }
        self.time_selection = Some(range);
    }

    /// The time selection's tracks that still exist; clears it if none do.
    fn selection_tracks(&mut self) -> Option<(u64, u64, Vec<TrackId>)> {
        let range = self.time_selection.as_mut()?;
        let project = &self.session.project;
        range.tracks.retain(|&t| project.track(t).is_some());
        if range.tracks.is_empty() {
            self.time_selection = None;
            return None;
        }
        Some((range.start, range.end, range.tracks.clone()))
    }

    /// Loop the time selection (L, or the selection's menu).
    pub fn loop_selection(&mut self) {
        let Some(range) = self.time_selection.clone() else {
            return;
        };
        let done = self
            .session
            .set_loop_region(range.start, range.end)
            .and_then(|()| self.session.set_looping(true));
        self.report(done);
    }

    /// Cut the clips at the time selection's edges (S, or the selection's menu).
    pub fn split_selection(&mut self) {
        let Some((start, end, tracks)) = self.selection_tracks() else {
            return;
        };
        self.session.label_edit("Split at selection", None);
        let split = self.session.split_range(start, end, &tracks).map(|_| ());
        self.report(split);
    }

    /// Remove what lies in the time selection (Delete, or the selection's menu).
    pub fn delete_selection(&mut self) {
        let Some((start, end, tracks)) = self.selection_tracks() else {
            return;
        };
        self.session.label_edit("Delete in selection", None);
        match self.session.delete_range(start, end, &tracks) {
            Ok(_) => self.clear_clip_selection(),
            Err(e) => self.toast(format!("{e:#}")),
        }
    }

    /// The instruments "Insert MIDI track" offers: recent ones first, then those this project
    /// already plays, then the installed ones that declare themselves instruments.
    pub fn instrument_choices(&mut self) -> Vec<std::path::PathBuf> {
        let installed = self.session.installed_plugins().to_vec();
        let declared = self
            .declared_instruments
            .get_or_insert_with(|| {
                let mut found: Vec<_> = installed
                    .iter()
                    .filter(|p| daw_core::plugins::declares_instrument(p) == Some(true))
                    .cloned()
                    .collect();
                found.sort_by_key(|p| daw_core::plugins::display_name(p).to_lowercase());
                found
            })
            .clone();
        let used = self
            .session
            .project
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Instrument)
            .filter_map(|t| t.inserts.first().map(|i| i.path.clone()));
        let mut choices: Vec<std::path::PathBuf> = Vec::new();
        for path in self
            .settings
            .recent_instruments
            .iter()
            .cloned()
            .chain(used)
            .chain(declared)
        {
            if installed.contains(&path) && !choices.contains(&path) {
                choices.push(path);
            }
        }
        choices
    }

    /// Remember an instrument for "Insert MIDI track" (newest first, eight at most).
    pub fn remember_instrument(&mut self, path: &std::path::Path) {
        let recent = &mut self.settings.recent_instruments;
        recent.retain(|p| p != path);
        recent.insert(0, path.to_path_buf());
        recent.truncate(8);
        self.save_settings();
    }

    /// Insert a MIDI track after `after` with a clip over `start..end` (two bars without an
    /// end) and `instrument` on it, then select its clip and open it in the piano roll.
    /// Returns the new track.
    pub fn insert_midi_track(
        &mut self,
        start: u64,
        end: Option<u64>,
        after: Option<TrackId>,
        instrument: Option<std::path::PathBuf>,
    ) -> Option<TrackId> {
        self.session.label_edit("Insert MIDI track", None);
        match self
            .session
            .insert_midi_track(start, end, after, instrument.as_deref())
        {
            Ok((track, clip, _)) => {
                if let Some(path) = &instrument {
                    self.remember_instrument(path);
                }
                self.selection = Some(Selection::Track(track));
                self.select_clip(track, clip, false);
                if let Err(e) = crate::piano_roll::open(self, clip) {
                    self.toast(e);
                }
                Some(track)
            }
            Err(e) => {
                self.toast(format!("{e:#}"));
                None
            }
        }
    }

    pub fn add_track(&mut self, kind: TrackKind) {
        // New tracks go into the selected folder (or next to the selected track's folder).
        let parent = match self.selection {
            Some(Selection::Track(id)) => self.session.project.track(id).and_then(|t| {
                if t.kind == TrackKind::Folder {
                    Some(t.id)
                } else {
                    t.parent
                }
            }),
            _ => None,
        };
        let id = self.session.add_track(kind, parent);
        self.selection = Some(Selection::Track(id));
    }

    fn update_meters(&mut self, dt: f32) {
        let ids: Vec<TrackId> = self.session.project.tracks.iter().map(|t| t.id).collect();
        self.meters.retain(|id, _| ids.contains(id));
        for id in ids {
            let peak = self.session.track_peak(id);
            self.meters.entry(id).or_default().push(peak, dt);
        }
        let dsp = self.session.take_dsp_peak();
        if dsp >= self.dsp_peak || self.dsp_peak_at.elapsed() > Duration::from_secs(2) {
            self.dsp_peak = dsp;
            self.dsp_peak_at = Instant::now();
        }
        let peak = self.session.master_peak();
        self.master_meter.push(peak, dt);
        let fall = 10f32.powf(-24.0 * dt / 20.0);
        let inputs = self.session.input_channels();
        self.input_meters.resize(inputs, 0.0);
        for (c, level) in self.input_meters.iter_mut().enumerate() {
            *level = self.session.input_peak(c).max(*level * fall);
        }
        let outputs = self.session.output_channels();
        self.output_meters.resize(outputs, 0.0);
        for (c, level) in self.output_meters.iter_mut().enumerate() {
            *level = self.session.output_peak(c).max(*level * fall);
        }
    }

    /// The timeline's scroll position and zoom.
    pub fn timeline(&self) -> zoom::View {
        zoom::View {
            start: self.view_start,
            spp: self.samples_per_pixel,
        }
    }

    pub fn set_timeline(&mut self, view: zoom::View) {
        self.view_start = view.start;
        self.samples_per_pixel = view.spp;
    }

    /// An explicit jump elsewhere in the timeline: during playback, follow-playhead would undo
    /// it at once, so it turns off.
    fn jump_timeline(&mut self, view: zoom::View) {
        self.set_timeline(view);
        if self.session.is_playing() {
            self.follow_playhead = false;
        }
    }

    /// Zoom in or out one step, around the playhead when it is on screen.
    pub fn zoom_step(&mut self, zoom_in: bool) {
        let factor = if zoom_in {
            zoom::STEP
        } else {
            1.0 / zoom::STEP
        };
        let view = self.timeline().zoom_toward(
            factor,
            self.session.position() as f64,
            f64::from(self.lanes_width),
            self.session.sample_rate(),
        );
        self.set_timeline(view);
    }

    /// Show the whole project (the first 30 seconds of an empty one).
    pub fn zoom_to_fit(&mut self) {
        let rate = self.session.sample_rate();
        let end = match self.session.content_end() {
            0 => u64::from(rate) * 30,
            length => length,
        };
        let view = zoom::View::fit(0, end, f64::from(self.lanes_width), rate);
        self.jump_timeline(view);
    }

    /// Show one clip across the lanes.
    pub fn zoom_to_clip(&mut self, clip: ClipId) -> ApiResult<()> {
        let project = &self.session.project;
        let midi = project.find_midi_clip(clip).map(|(_, c)| {
            // MIDI clips sit in musical time: where they are depends on the tempo map.
            let map = self.session.tempo_map();
            let at = |tick: u64| map.sample_at_tick(tick as f64).round() as u64;
            (at(c.start), at(c.end()))
        });
        let (start, end) = project
            .find_clip(clip)
            .map(|(_, c)| (c.start, c.end()))
            .or(midi)
            .ok_or_else(|| ApiError(format!("no clip with id {clip}")))?;
        let rate = self.session.sample_rate();
        let view = zoom::View::fit(start, end, f64::from(self.lanes_width), rate);
        self.jump_timeline(view);
        Ok(())
    }

    /// The check run's waveform zoom shots (`--show clip-overview` / `clip-close` /
    /// `clip-samples`): the first audio clip selected and its track 200 points tall, fitted
    /// across the lanes (`spp` none), or at `spp` samples a point with its loudest sample about
    /// 600 points in.
    fn show_clip_zoom(&mut self, spp: Option<f64>) {
        let first = self
            .session
            .project
            .tracks
            .iter()
            .find_map(|t| t.clips.first().map(|c| (t.id, c.clone())));
        let Some((track, clip)) = first else {
            return;
        };
        if let Some(t) = self.session.project.track_mut(track) {
            t.height = Some(200.0);
        }
        self.select_clip(track, clip.id, false);
        self.selection = Some(Selection::Track(track));
        match spp {
            None => {
                // Fitted to the lanes' width before they are first drawn (900 points), so
                // the clip spans most of them.
                let _ = self.zoom_to_clip(clip.id);
            }
            Some(spp) => {
                let loudest = self.session.clip_audio(&clip).map_or(0, |(data, offset)| {
                    let channel = &data.channels[0];
                    let end = (offset + clip.length).min(channel.len() as u64) as usize;
                    let start = (offset as usize).min(end);
                    loudest_sample(&channel[start..end])
                });
                self.samples_per_pixel = spp;
                self.view_start = (clip.start as f64 + loudest as f64 - 600.0 * spp).max(0.0);
            }
        }
    }

    /// Every track's row height at once (Alt+wheel, the View menu, `set_view`).
    pub fn set_all_track_heights(&mut self, height: f32) {
        for track in &mut self.session.project.tracks {
            track.height = Some(height.clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT));
        }
    }

    fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        // F1: the manual's page for where the user is (27.3), even while typing in a field.
        if !crate::busy::active(self) && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1))
        {
            let view = match self.view_mode {
                ViewMode::Multitrack => crate::help::Place::Multitrack,
                ViewMode::Waveform => crate::help::Place::Waveform,
            };
            let place = self.help.place(ctx, view);
            self.help(place);
        }
        // Busy (opening, saving, loading a plug-in): no Space into a half-loaded project.
        if ctx.egui_wants_keyboard_input() || crate::busy::active(self) {
            return;
        }
        // The piano roll's keys first, while it has the keyboard (`piano_roll::has_keys`: the
        // pane under the pointer, else the one last clicked): Delete there erases notes, not
        // the clip.
        piano_roll::shortcuts(self, ctx);
        let pressed = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::NONE, key));
        let command = |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, key));
        // Redo first: Ctrl+Shift+Z must not be read as Ctrl+Z.
        let shifted =
            |key: Key| ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, key));
        // Ctrl+C / X / V arrive as clipboard events, never as keys (`clipboard`): read them in
        // either view, after the gate above so a focused text field keeps its own; the piano
        // roll's notes by the same rule as its other keys, else the view's clips or audio.
        let typing = ctx.egui_wants_keyboard_input();
        let target = crate::clipboard::Target::of(self, ctx);
        let clipboard = ctx.input_mut(|i| crate::clipboard::take(&mut i.events, target, typing));
        crate::clipboard::run(self, &clipboard);
        let waveform = self.view_mode == ViewMode::Waveform;
        if shifted(Key::Z) || command(Key::Y) {
            if waveform {
                crate::waveform::step(self, false);
            } else {
                self.redo();
            }
        } else if command(Key::Z) {
            if waveform {
                crate::waveform::step(self, true);
            } else {
                self.undo();
            }
        }
        if command(Key::Tab) {
            self.view_mode = match self.view_mode {
                ViewMode::Multitrack => ViewMode::Waveform,
                ViewMode::Waveform => ViewMode::Multitrack,
            };
        }
        if pressed(Key::Space) {
            transport::toggle_play(self);
        }
        if self.view_mode == ViewMode::Waveform {
            // The Waveform view's own keys; the arrangement's don't reach a hidden view.
            if pressed(Key::Home) {
                if let Some(id) = self.editor.current {
                    self.editor.view(id).playhead = 0;
                }
            }
            if pressed(Key::Escape) {
                if let Some(id) = self.editor.current {
                    let view = self.editor.view(id);
                    view.selection = None;
                    view.boxed = None;
                }
            }
            // The spectrogram's tools: E the box (marquee), T time.
            if pressed(Key::E) {
                self.editor.tool = crate::waveform::Tool::Box;
            }
            if pressed(Key::T) {
                self.editor.tool = crate::waveform::Tool::Time;
            }
            if pressed(Key::Equals) || pressed(Key::Plus) {
                crate::waveform::zoom(self, Some(true));
            }
            if pressed(Key::Minus) {
                crate::waveform::zoom(self, Some(false));
            }
            if pressed(Key::F) {
                crate::waveform::zoom(self, None);
            }
            if pressed(Key::Delete) || pressed(Key::Backspace) {
                crate::waveform::delete(self);
            }
            if shifted(Key::S) {
                crate::waveform::save_as(self);
            } else if command(Key::S) {
                crate::waveform::ask_save(self);
            }
            return;
        }
        if pressed(Key::R) {
            transport::toggle_record(self);
        }
        if pressed(Key::Home) {
            let located = self.session.locate(0);
            self.report(located);
            self.view_start = 0.0;
        }
        if pressed(Key::M) {
            self.show_mixer = !self.show_mixer;
        }
        if pressed(Key::Equals) || pressed(Key::Plus) {
            self.zoom_step(true);
        }
        if pressed(Key::Minus) {
            self.zoom_step(false);
        }
        if pressed(Key::F) {
            self.zoom_to_fit();
        }
        if pressed(Key::C) {
            let mut click = self.session.project.metronome;
            click.enabled = !click.enabled;
            let _ = self.session.set_metronome(click);
        }
        if pressed(Key::P) {
            let punching = self.session.project.punch.enabled;
            let done = self.session.set_punching(!punching);
            self.report(done);
        }
        if pressed(Key::L) {
            // With a time selection the loop takes it first; then L switches looping as ever.
            let region = self.session.project.loop_region;
            match self.time_selection.clone() {
                Some(range) if (region.start, region.end) != (range.start, range.end) => {
                    self.loop_selection();
                }
                _ => {
                    let done = self.session.set_looping(!region.enabled);
                    self.report(done);
                }
            }
        }
        if pressed(Key::Delete) || pressed(Key::Backspace) {
            // A point picked after any clip goes first, then a time selection, then clips.
            if !crate::automation::delete_selected_point(self) {
                if self.time_selection.is_some() {
                    self.delete_selection();
                } else {
                    arrange::delete_selected_clip(self);
                }
            }
        }
        if pressed(Key::S) {
            if self.time_selection.is_some() {
                self.split_selection();
            } else {
                arrange::split_selected_clip(self);
            }
        }
        if command(Key::S) {
            self.save_project(false);
        }
        if command(Key::D) {
            self.duplicate_selection();
        }
        if command(Key::A) {
            self.select_all_clips();
        }
        if pressed(Key::Escape) {
            self.clear_clip_selection();
            self.time_selection = None;
        }
        if pressed(Key::Q) {
            crate::quantize::open_quantize(self);
        }
        // Markers: Insert adds one at the playhead, 1-9 go to them, [ and ] step back and on.
        if pressed(Key::Insert) {
            self.add_marker_at_playhead();
        }
        let digits = [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ];
        for (i, key) in digits.into_iter().enumerate() {
            if pressed(key) {
                match self.session.marker_number(i + 1) {
                    Some(id) => {
                        let went = self.session.go_to_marker(id).map(|_| ());
                        self.report(went);
                    }
                    None => self.toast(format!("There is no marker {}", i + 1)),
                }
            }
        }
        for (key, forward) in [(Key::OpenBracket, false), (Key::CloseBracket, true)] {
            if pressed(key) {
                let here = self.session.position();
                if let Some(id) = self.session.adjacent_marker(here, forward) {
                    let went = self.session.go_to_marker(id).map(|_| ());
                    self.report(went);
                }
            }
        }
        if command(Key::O) {
            dialogs::open_dialog(self);
        }
        if command(Key::N) {
            self.start.open = true;
        }
        if command(Key::T) {
            self.add_track(TrackKind::Audio);
        }
    }

    fn take_screenshot_when_ready(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else {
            return;
        };
        self.frames += 1;
        ctx.request_repaint();
        // About 2.5 seconds in (or after `--wait`), so playback has moved and meters have
        // settled.
        let due = match self.screenshot_wait {
            // The first frame the overlay is up with plug-ins still to load (the capture is the
            // next frame's), or after 10 s so a project that loads at once can't hang the check.
            // Once the dragged playhead has scrolled the view back to the start and is there
            // itself (or after
            // 10 s, so a view that doesn't move can't hang the check).
            // Once the piano roll's ruler click has put the playhead on its sample (or after
            // 10 s, so a click that misses can't hang the check).
            _ if self.demo_ruler.is_some() => {
                let landed = self
                    .demo_ruler
                    .is_some_and(|(_, sample, step)| step > 8 && self.session.position() == sample);
                landed || self.frames >= 600
            }
            // Once the rack's Presets menu has been open a while.
            _ if self.rack_presets.demo_click.is_some() => {
                self.rack_presets.demo_click.is_some_and(|step| step > 40) || self.frames >= 600
            }
            // Once a restored rack's plug-ins are all in (or after 20 s).
            _ if self.rack_presets.demo_restored => {
                (self.session.loading().is_none() && self.frames >= 150) || self.frames >= 1200
            }
            // Once the ruler's menu has been open a while with the pointer on its Grid.
            _ if self.demo_grid_menu.is_some() => {
                self.demo_grid_menu.is_some_and(|step| step > 40) || self.frames >= 600
            }
            // Once the dragged clip has been held on the playhead by the magnet a while.
            _ if self.demo_magnet => {
                let held = self.group_drag.as_ref().is_some_and(|d| d.magnet);
                let still = self.demo_clip_drag.is_some_and(|(_, _, step, _)| step > 30);
                (held && still) || self.frames >= 600
            }
            _ if self.demo_playhead_drag.is_some() => {
                let held = self.demo_playhead_drag.is_some_and(|(step, _)| step > 20);
                let home = self.view_start == 0.0 && self.session.position() == 0;
                (held && home) || self.frames >= 600
            }
            // The timed undo's or render's: the bar partway.
            _ if self.frame_check.is_some() => {
                let due = self.frame_check.as_ref().is_some_and(|c| c.shot_due(self));
                due || self.frames >= 3000
            }
            // The typed fields' check: a field open with its text typed.
            _ if self.edit_check.is_some() => {
                let due = self.edit_check.as_ref().is_some_and(|c| c.shot_due());
                due || self.frames >= 3000
            }
            // The open check's: the bar on its files or on its plug-ins.
            _ if self.open_check.is_some() => {
                let due = self.open_check.as_ref().is_some_and(|c| c.shot_due(self));
                due || self.frames >= 3000
            }
            _ if self.screenshot_on_busy => {
                let loading = self.session.loading().is_some_and(|l| {
                    l.stage == daw_core::session::LoadStage::Loading
                        && l.stage_done + 2 < l.stage_total
                });
                (self.busy_frames >= 1 && loading) || self.frames >= 600
            }
            Some(wait) => self.started.elapsed() >= wait,
            None => self.frames >= 150,
        };
        if due && !self.screenshot_taken {
            self.screenshot_taken = true;
            // The check run closes once the shot is written: nothing asked. The open check
            // closes itself once the open is over (`opening::frame_ends`).
            let close = self.open_check.is_none()
                && self.frame_check.is_none()
                && self.edit_check.is_none();
            crate::opening::shot_taken(self);
            crate::frame_check::shot_taken(self);
            crate::frame_check::report_drawing(self);
            crate::edit_check::shot_taken(self);
            self.closing = close;
            let ctx2 = ctx.clone();
            // The marks of this frame, the one the screenshot is read from (eframe paints the
            // frame whose output asked for it, then reads it back).
            let marks = self
                .marks
                .clone()
                .map(|file| (file, crate::marks::taken(), ctx.pixels_per_point()));
            ctx.request_screenshot(move |image| {
                if let Err(e) = save_png(&path, &image) {
                    eprintln!("screenshot failed: {e:#}");
                } else {
                    println!("screenshot saved to {}", path.display());
                }
                if let Some((file, marks, ppp)) = marks {
                    let text = crate::marks::json(&marks, ppp, image.size);
                    match std::fs::write(&file, text) {
                        Ok(()) => println!("{} marks saved to {}", marks.len(), file.display()),
                        Err(e) => eprintln!("marks failed: {}: {e}", file.display()),
                    }
                }
                if close {
                    ctx2.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        }
    }
}

/// Where the largest magnitude in `samples` is (the first of equals; 0 when empty): the zoom
/// shots centre on it, so the closest zooms show the audio, not silence.
fn loudest_sample(samples: &[f32]) -> usize {
    samples
        .iter()
        .enumerate()
        .fold((0, 0.0f32), |(at, top), (i, s)| {
            if s.abs() > top {
                (i, s.abs())
            } else {
                (at, top)
            }
        })
        .0
}

fn save_png(path: &Path, image: &egui::ColorImage) -> anyhow::Result<()> {
    std::fs::write(path, encode_png(image)?)?;
    Ok(())
}

fn encode_png(image: &egui::ColorImage) -> anyhow::Result<Vec<u8>> {
    let mut png_bytes = Vec::new();
    let [w, h] = image.size;
    let mut encoder = png::Encoder::new(&mut png_bytes, w as u32, h as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
    writer.write_image_data(&bytes)?;
    writer.finish()?;
    Ok(png_bytes)
}

impl Host for DawApp {
    fn session(&mut self) -> &mut Session {
        &mut self.session
    }

    /// An agent's open runs behind the loading bar like File > Open's (its plug-ins a frame at
    /// a time, the window painting), and its call is answered when it ends.
    fn when_opened(&mut self, reply: daw_core::api::Reply, then: daw_core::api::AfterOpen) {
        // (Only the effects rack loading, a preset, or an undo's or a new rate's plug-ins: the
        // project stays as it is.)
        if self.session.loading().is_some_and(|l| l.opens()) {
            // As the menu's open: nothing of the last project stays selected, and the start
            // screen gives way to the bar.
            self.project_opened();
            self.start.open = false;
        }
        self.agent_opens.wait(&mut self.session, reply, then);
    }

    /// An agent's save runs behind the busy bar like Ctrl+S's (a plug-in's state a frame, the
    /// file on a worker), and its call is answered once the file is written.
    fn when_saved(&mut self, reply: daw_core::api::Reply, then: daw_core::api::AfterOpen) {
        self.agent_saves.wait(&mut self.session, reply, then);
    }

    fn meters(&mut self) -> ApiResult<serde_json::Value> {
        let db = |m: &Meter| {
            json!({
                "peak_db": m.db(),
                "hold_db": [gain_to_db(m.hold[0]), gain_to_db(m.hold[1])],
            })
        };
        let tracks: Vec<serde_json::Value> = self
            .session
            .project
            .tracks
            .iter()
            .map(|t| {
                let meter = self.meters.get(&t.id).copied().unwrap_or_default();
                json!({ "track": t.id, "name": t.name, "level": db(&meter) })
            })
            .collect();
        let inputs: Vec<f32> = self.input_meters.iter().map(|g| gain_to_db(*g)).collect();
        let outputs: Vec<f32> = self.output_meters.iter().map(|g| gain_to_db(*g)).collect();
        let stats = self.session.stats();
        Ok(json!({
            "tracks": tracks,
            "master": db(&self.master_meter),
            "inputs_db": inputs,
            "outputs_db": outputs,
            "dsp_load": stats.dsp_load,
            "dsp_peak": self.dsp_peak,
        }))
    }

    fn view(&mut self) -> ApiResult<serde_json::Value> {
        let end = self.view_start + f64::from(self.lanes_width) * self.samples_per_pixel;
        Ok(json!({
            "selected_track": match self.selection { Some(Selection::Track(id)) => Some(id), _ => None },
            "master_selected": self.selection == Some(Selection::Master),
            "selected_clip": self.selected_clip.map(|(_, clip)| clip),
            "selected_clips": self.selected_clip_ids(),
            "visible": { "start": self.view_start as u64, "end": end as u64 },
            "samples_per_pixel": self.samples_per_pixel,
            "waveform_scale": self.waveform_scale,
            "show_mixer": self.show_mixer,
            "show_inspector": self.show_inspector,
            "show_diagnostics": self.show_diagnostics,
            "snap": self.settings.snap,
            "grid": self.session.project.grid,
            "grid_division": crate::arrange::grid_division(self).to_string(),
            "follow_playhead": self.follow_playhead,
            "piano_roll": self.piano_roll.as_ref().map(|p| p.clip),
            "loudness_scale": self.settings.loudness_scale,
            "loudness_relative": self.settings.loudness_relative,
            "time_selection": self.time_selection,
            "view_mode": self.view_mode,
            "document": self.editor.current,
            "editor": self.editor.current.map(|id| {
                let view = self.editor.view(id).clone();
                let end = view.start + f64::from(self.editor.width) * view.spp;
                json!({
                    "start": view.start as u64,
                    "end": end as u64,
                    "samples_per_pixel": view.spp,
                    "selection": view.selection.map(|(a, b)| json!({"start": a, "end": b})),
                    "selection_channel": view.channel,
                    "box": view.boxed.map(|(b, channel)| json!({
                        "start": b.start,
                        "end": b.end,
                        "low_hz": b.low_hz,
                        "high_hz": b.high_hz,
                        "channel": channel,
                    })),
                    "playhead": view.playhead,
                })
            }),
            "spectral": self.editor.spectral,
            "spectral_range_db": self.editor.range_db,
            "editor_time": self.editor.time,
            "editor_grid_division": self.editor.current.and_then(|id| {
                crate::waveform::grid_division(self, id).map(|d| d.to_string())
            }),
        }))
    }

    fn layout(&mut self) -> ApiResult<serde_json::Value> {
        self.sync_dock();
        let names: Vec<&str> = std::iter::once(crate::dock::DEFAULT)
            .chain(self.settings.layouts.keys().map(String::as_str))
            .collect();
        Ok(json!({
            "layout": crate::dock::describe(&self.dock.tree),
            "layouts": names,
        }))
    }

    fn set_layout(&mut self, name: &str) -> ApiResult<serde_json::Value> {
        self.use_layout(name).map_err(ApiError)?;
        self.layout()
    }

    fn save_layout(&mut self, name: &str) -> ApiResult<serde_json::Value> {
        DawApp::save_layout(self, name).map_err(ApiError)?;
        self.layout()
    }

    fn delete_layout(&mut self, name: &str) -> ApiResult<serde_json::Value> {
        if self.settings.layouts.remove(name).is_none() {
            return Err(ApiError(format!("no saved layout called \"{name}\"")));
        }
        self.save_settings();
        self.layout()
    }

    fn open_manual(
        &mut self,
        _dir: &Path,
        target: &daw_core::manual::Target,
    ) -> ApiResult<serde_json::Value> {
        DawApp::open_manual(self, target.clone()).map_err(ApiError)
    }

    fn app_info(&mut self) -> ApiResult<serde_json::Value> {
        Ok(json!({
            "version": crate::version::VERSION,
            "commit": crate::version::COMMIT,
            "folder": crate::settings::app_dir(),
        }))
    }

    fn recent_projects(&mut self) -> ApiResult<serde_json::Value> {
        // The files looked at on workers, waited for briefly: a drive that doesn't answer
        // in time is "unknown" rather than holding up the window.
        self.recent_probe.want(&self.settings.recent);
        self.recent_probe.settle(Duration::from_millis(300));
        let current = self.session.path.clone();
        let list: Vec<serde_json::Value> = crate::recent::entries(&self.settings, &self.recent_probe)
            .into_iter()
            .map(|e| {
                use crate::recent::Found;
                let (state, modified) = match e.found {
                    None => ("unknown", None),
                    Some(Found::Missing) => ("missing", None),
                    Some(Found::Present(t)) => ("present", t),
                };
                json!({
                    "path": e.path,
                    "name": e.name,
                    "folder": e.folder,
                    "state": state,
                    "modified": modified.map(|t| chrono::DateTime::<chrono::Local>::from(t).to_rfc3339()),
                    "saved": modified.map(crate::recent::saved),
                    "template": e.template,
                    "open": current.as_deref() == Some(e.path.as_path()),
                })
            })
            .collect();
        Ok(json!(list))
    }

    fn remove_recent_project(&mut self, path: &Path) -> ApiResult<serde_json::Value> {
        if !self.settings.recent.iter().any(|p| p == path) {
            return Err(ApiError(format!(
                "{} isn't on the recent projects (list_recent_projects gives their paths)",
                path.display()
            )));
        }
        self.remove_recent(path);
        self.recent_projects()
    }

    fn set_view(&mut self, change: ViewChange) -> ApiResult<serde_json::Value> {
        let project = &self.session.project;
        if let Some(track) = change.select_track {
            if project.track(track).is_none() {
                return Err(ApiError(format!("no track with id {track}")));
            }
        }
        // Audio and MIDI clips alike.
        let clip_track = match change.select_clip {
            Some(clip) => Some(
                project
                    .find_clip(clip)
                    .map(|(track, _)| track)
                    .or_else(|| project.find_midi_clip(clip).map(|(track, _)| track))
                    .ok_or_else(|| ApiError(format!("no clip with id {clip}")))?,
            ),
            None => None,
        };
        if let Some(clip) = change.piano_roll.filter(|&c| c != 0) {
            if project.find_midi_clip(clip).is_none() {
                return Err(ApiError(format!(
                    "no MIDI clip with id {clip} to open in the piano roll"
                )));
            }
        }
        if let (Some(start), Some(end)) = (change.start, change.end) {
            if end <= start {
                return Err(ApiError("end must be after start".into()));
            }
        }
        if let Some(height) = change.track_height {
            if !(MIN_TRACK_HEIGHT..=MAX_TRACK_HEIGHT).contains(&height) {
                return Err(ApiError(format!(
                    "track_height {height} is outside {MIN_TRACK_HEIGHT} to {MAX_TRACK_HEIGHT}"
                )));
            }
        }
        if let Some(scale) = change.waveform_scale {
            if !(1.0..=MAX_WAVEFORM_SCALE).contains(&scale) {
                return Err(ApiError(format!(
                    "waveform_scale {scale} is outside 1 to {MAX_WAVEFORM_SCALE}"
                )));
            }
        }
        let zoom_clip = match change.zoom {
            Some(Zoom::Clip) => Some(
                change
                    .select_clip
                    .or(self.selected_clip.map(|(_, clip)| clip))
                    .ok_or_else(|| {
                        ApiError("zoom \"clip\" needs select_clip or a selected clip".into())
                    })?,
            ),
            _ => None,
        };
        if let Some(clip) = zoom_clip {
            if project.find_clip(clip).is_none() && project.find_midi_clip(clip).is_none() {
                return Err(ApiError(format!("no clip with id {clip}")));
            }
        }
        if change.zoom.is_some() && change.start.is_some() {
            return Err(ApiError("give either zoom or start/end, not both".into()));
        }
        if let Some(clips) = &change.select_clips {
            if let Some(bad) = clips.iter().find(|&&c| project.clip_track(c).is_none()) {
                return Err(ApiError(format!("no clip with id {bad}")));
            }
        }
        if let Some(doc) = change.document {
            if self.session.document(doc).is_none() {
                return Err(ApiError(format!("no open audio file with id {doc}")));
            }
        }
        let editing = change.document.or(self.editor.current);
        let editor_change = change.editor_start.is_some()
            || change.editor_end.is_some()
            || change.editor_selection.is_some()
            || change.editor_box.is_some()
            || change.editor_playhead.is_some();
        if editor_change && editing.is_none() {
            return Err(ApiError(
                "no audio file is open in the Waveform view (open_audio_editor)".into(),
            ));
        }
        if let Some(range) = change.spectral_range_db {
            if !(20.0..=200.0).contains(&range) {
                return Err(ApiError(format!(
                    "spectral_range_db {range} is outside 20 to 200"
                )));
            }
        }
        if let Some(sel) = change.editor_selection {
            if sel.end <= sel.start {
                return Err(ApiError(format!(
                    "the editor selection must end after it starts ({}..{})",
                    sel.start, sel.end
                )));
            }
        }
        if let (Some(b), Some(doc)) = (change.editor_box, editing) {
            let audio = self.session.document(doc).map(|d| d.audio.clone());
            if let Some(audio) = audio {
                let spectral = daw_core::spectral_edit::SpectralBox {
                    start: b.start,
                    end: b.end,
                    low_hz: b.low_hz,
                    high_hz: b.high_hz,
                };
                daw_core::spectral_edit::check(&spectral, audio.len() as u64, audio.sample_rate)
                    .map_err(|e| ApiError(format!("{e:#}")))?;
                if let Some(c) = b.channel.filter(|&c| c >= audio.channels.len()) {
                    return Err(ApiError(format!(
                        "the file has {} channels; there is no channel {c}",
                        audio.channels.len()
                    )));
                }
            }
        }
        if let (Some(start), Some(end)) = (change.editor_start, change.editor_end) {
            if end <= start {
                return Err(ApiError("editor_end must be after editor_start".into()));
            }
        }
        if let Some(range) = &change.time_selection {
            if range.end <= range.start {
                return Err(ApiError(format!(
                    "the time selection must end after it starts ({}..{})",
                    range.start, range.end
                )));
            }
            if let Some(bad) = range.tracks.iter().find(|&&t| project.track(t).is_none()) {
                return Err(ApiError(format!("no track with id {bad}")));
            }
        }
        if let Some(track) = change.select_track {
            self.selection = Some(Selection::Track(track));
        }
        if let Some(clips) = &change.select_clips {
            self.select_clips(clips);
        }
        if change.clear_time_selection == Some(true) {
            self.time_selection = None;
        }
        if let Some(doc) = change.document {
            crate::waveform::show_document(self, doc);
        }
        if let Some(mode) = change.view_mode {
            self.view_mode = mode;
        }
        if let Some(mode) = change.spectral {
            self.editor.spectral = mode;
        }
        if let Some(range) = change.spectral_range_db {
            self.editor.range_db = range;
        }
        if let Some(time) = change.editor_time {
            self.editor.time = time;
        }
        if let Some(doc) = editing {
            let width = f64::from(self.editor.width.max(100.0));
            let view = self.editor.view(doc);
            if let Some(start) = change.editor_start {
                if let Some(end) = change.editor_end {
                    view.spp = ((end - start) as f64 / width).max(zoom::MIN_SPP);
                }
                view.start = start as f64;
            }
            if let Some(sel) = change.editor_selection {
                view.selection = Some((sel.start, sel.end));
                view.channel = None;
                view.playhead = sel.start;
                view.boxed = None;
            }
            if let Some(b) = change.editor_box {
                view.boxed = Some((
                    daw_core::spectral_edit::SpectralBox {
                        start: b.start,
                        end: b.end,
                        low_hz: b.low_hz,
                        high_hz: b.high_hz,
                    },
                    b.channel,
                ));
                view.selection = None;
                view.channel = None;
                view.playhead = b.start;
            }
            if change.clear_editor_selection == Some(true) {
                view.selection = None;
                view.boxed = None;
            }
            if let Some(at) = change.editor_playhead {
                view.playhead = at;
            }
        }
        if let Some(range) = change.time_selection {
            // Every track when none are named, as the range actions read an empty list.
            let tracks = if range.tracks.is_empty() {
                self.session.project.tracks.iter().map(|t| t.id).collect()
            } else {
                range.tracks
            };
            self.set_time_selection(TimeSelection { tracks, ..range }, false);
        }
        if change.select_master == Some(true) {
            self.selection = Some(Selection::Master);
        }
        if let (Some(clip), Some(track)) = (change.select_clip, clip_track) {
            self.select_clip(track, clip, false);
            self.selected_point = None;
            self.selection = Some(Selection::Track(track));
        }
        if let Some(start) = change.start {
            if let Some(end) = change.end {
                let rate = self.session.sample_rate();
                self.samples_per_pixel = ((end - start) as f64
                    / f64::from(self.lanes_width.max(1.0)))
                .clamp(zoom::MIN_SPP, zoom::max_spp(rate));
            }
            self.view_start = start as f64;
            // An explicit scroll would be undone at once by follow-playhead during playback.
            if self.session.is_playing() {
                self.follow_playhead = false;
            }
        }
        if let Some(show) = change.show_mixer {
            self.show_mixer = show;
        }
        if let Some(show) = change.show_inspector {
            self.show_inspector = show;
        }
        if let Some(show) = change.show_diagnostics {
            self.show_diagnostics = show;
        }
        if let Some(snap) = change.snap {
            self.settings.snap = snap;
            self.save_settings();
        }
        if let Some(follow) = change.follow_playhead {
            self.follow_playhead = follow;
        }
        match change.zoom {
            Some(Zoom::In) => self.zoom_step(true),
            Some(Zoom::Out) => self.zoom_step(false),
            Some(Zoom::Fit) => self.zoom_to_fit(),
            Some(Zoom::Clip) => {
                if let Some(clip) = zoom_clip {
                    self.zoom_to_clip(clip)?;
                }
            }
            None => {}
        }
        if let Some(height) = change.track_height {
            self.set_all_track_heights(height);
        }
        if let Some(scale) = change.waveform_scale {
            self.waveform_scale = scale;
        }
        if let Some(scale) = change.loudness_scale {
            self.settings.loudness_scale = scale;
            self.save_settings();
        }
        if let Some(relative) = change.loudness_relative {
            self.settings.loudness_relative = relative;
            self.save_settings();
        }
        match change.piano_roll {
            Some(0) => self.piano_roll = None,
            Some(clip) => piano_roll::open(self, clip).map_err(ApiError)?,
            None => {}
        }
        self.view()
    }

    fn screenshot(&mut self, reply: Reply) {
        // A minimised window renders rarely if at all; answer now instead of making the
        // agent wait for a frame that may take a minute.
        let minimized = self
            .egui_ctx
            .input(|i| i.viewport().minimized)
            .unwrap_or(false);
        if minimized {
            reply(Err(ApiError(
                "the DAW window is minimised; ask the user to restore it for a screenshot".into(),
            )));
            return;
        }
        // The image arrives once the next frame has rendered.
        self.egui_ctx.request_screenshot(move |image| {
            let result = encode_png(&image)
                .map(ActionOutput::Png)
                .map_err(|e| ApiError(format!("encoding the screenshot: {e:#}")));
            reply(result);
        });
    }
}

impl eframe::App for DawApp {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        crate::opening::frame_begins(self);
        crate::frame_check::frame_begins(self);
        self.housekeeping(ctx);
    }

    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        // The screenshot check's file drag from "Explorer": the file hovering over the track's
        // lane (the pointer set as Windows would report it), then, with `drop`, let go.
        if let Some((path, track, drop, step)) = self.demo_drop.as_mut() {
            let row = self.drop_rows.iter().find(|r| r.0 == *track).map(|r| r.2);
            if let Some(row) = row {
                *step += 1;
                let at = egui::pos2(row.left() + 420.0, row.center().y);
                self.drag_pointer = Some(at);
                raw_input.events.push(egui::Event::PointerMoved(at));
                if *drop && *step == 30 {
                    raw_input
                        .dropped_files
                        .push(std::sync::Arc::new(DemoFile(path.clone())));
                } else if !*drop || *step < 30 {
                    raw_input.hovered_files.push(egui::HoveredFile {
                        path: Some(path.clone()),
                        ..Default::default()
                    });
                }
            }
        }
        // The screenshot check's click on the piano roll's ruler, a little off a sixteenth.
        let aimed = self.demo_ruler.map(|(tick, _, _)| tick);
        if let Some(p) = aimed.and_then(|tick| crate::piano_roll::ruler_point(self, tick)) {
            if let Some((_, _, step)) = self.demo_ruler.as_mut() {
                *step += 1;
                let at = p + egui::vec2(3.0, 0.0);
                raw_input.events.push(egui::Event::PointerMoved(at));
                if *step == 5 {
                    for pressed in [true, false] {
                        raw_input.events.push(egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        });
                    }
                }
            }
        }
        // The screenshot check's right-click on the Multitrack's ruler, then the pointer on to
        // the menu's Grid button, which opens its list (`--show grid-menu`).
        if let (Some(step), Some(ruler)) = (self.demo_grid_menu.as_mut(), self.ruler_rect) {
            *step += 1;
            let at = ruler.left_center() + egui::vec2(120.0, 0.0);
            if *step == 5 {
                raw_input.events.push(egui::Event::PointerMoved(at));
                for pressed in [true, false] {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Secondary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            } else if *step > 10 {
                if let Some(button) = self.grid_menu_button {
                    raw_input
                        .events
                        .push(egui::Event::PointerMoved(button.center()));
                }
            }
        }
        // The screenshot check's S, pressed once the playhead has reached the cut
        // (`--show midi-split`; the engine moves it a block or so after the locate).
        if self
            .demo_split_at
            .is_some_and(|at| self.session.position() == at)
        {
            self.demo_split_at = None;
            for pressed in [true, false] {
                raw_input.events.push(egui::Event::Key {
                    key: egui::Key::S,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: Default::default(),
                });
            }
        }
        // The screenshot check's click on the effects rack's Presets menu.
        crate::rack_presets::demo_input(self, raw_input);
        // The typed names' and volumes' check: its clicks, keys and text.
        crate::edit_check::input(self, ctx, raw_input);
        // The screenshot check's pointer resting on the (inactive) Waveform tab.
        if self.demo_tab_hover {
            let tab = crate::dock::tab_rect(ctx, &self.dock, crate::dock::Pane::Waveform(None));
            if let Some(rect) = tab {
                raw_input
                    .events
                    .push(egui::Event::PointerMoved(rect.center()));
            }
        }
        // The screenshot check's pointer on a piano roll key (`--show piano-roll`).
        if let Some(p) = self.piano_roll.as_ref().and_then(|s| s.pointer) {
            raw_input.events.push(egui::Event::PointerMoved(p));
        }
        // The screenshot check's right-click on a rack slot (`--show plugin-paste`), once the
        // racks have been drawn a few times.
        if let Some((at, step)) = self.demo_right_click.as_mut() {
            let found = self
                .rack_slots
                .iter()
                .find(|s| (s.0, s.1) == *at)
                .map(|s| s.2.center());
            if let Some(p) = found {
                *step += 1;
                raw_input.events.push(egui::Event::PointerMoved(p));
                if *step == 5 {
                    for pressed in [true, false] {
                        raw_input.events.push(egui::Event::PointerButton {
                            pos: p,
                            button: egui::PointerButton::Secondary,
                            pressed,
                            modifiers: Default::default(),
                        });
                    }
                }
            }
        }
        // The screenshot check's clip drag (`--show clip-drag`): press on the clip, then down
        // to the target track's row, a little later in time, held there (not dropped).
        if let Some((clip, to, step, from)) = self.demo_clip_drag.as_mut() {
            if from.is_none() {
                *from = self
                    .clip_rects
                    .iter()
                    .find(|(c, _)| c == clip)
                    // Low in the clip: its middle holds the gain line.
                    .map(|(_, r)| egui::pos2(r.center().x, r.bottom() - 8.0));
            }
            let target = self
                .drop_rows
                .iter()
                .find(|(t, _, _)| t == to)
                .map(|(_, _, r)| r.center().y);
            if let (Some(a), Some(y)) = (*from, target) {
                let b = egui::pos2(a.x + 60.0, y);
                let t = (step.saturating_sub(2) as f32 / 16.0).min(1.0);
                raw_input
                    .events
                    .push(egui::Event::PointerMoved(a + (b - a) * t));
                if *step == 1 {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos: a,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                *step += 1;
            }
        }
        // The screenshot check's playhead drag: press on the playhead, then 80 points past the
        // lanes' left edge, held there: the view scrolls back to the start with it.
        if let Some((step, from)) = self.demo_playhead_drag.as_mut() {
            if from.is_none() {
                *from = self.playhead_grab.map(|(p, _)| p);
            }
            if let (Some(a), Some((_, left))) = (*from, self.playhead_grab) {
                let b = egui::pos2(left - 80.0, a.y);
                let t = (step.saturating_sub(2) as f32 / 10.0).min(1.0);
                raw_input
                    .events
                    .push(egui::Event::PointerMoved(a + (b - a) * t));
                if *step == 1 {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos: a,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                *step += 1;
            }
        }
        // The screenshot check's track drag: press on the track's colour stripe (or its mixer
        // strip's name), then over the other track's upper quarter (its strip's left part),
        // held there (not dropped).
        if let Some((track, to, mixer, step, from)) = self.demo_track_drag.as_mut() {
            let rows: &[(TrackId, TrackKind, egui::Rect)] = if *mixer {
                &self.strip_rects
            } else {
                &self.drop_rows
            };
            let rect = |id: TrackId| rows.iter().find(|r| r.0 == id).map(|r| r.2);
            if from.is_none() {
                *from = rect(*track).map(|r| {
                    if *mixer {
                        r.left_top() + egui::vec2(20.0, 14.0)
                    } else {
                        egui::pos2(r.left() + 4.0, r.center().y)
                    }
                });
            }
            if let (Some(a), Some(target)) = (*from, rect(*to)) {
                let b = if *mixer {
                    egui::pos2(target.left() + target.width() * 0.25, a.y)
                } else {
                    egui::pos2(a.x, target.top() + target.height() * 0.25)
                };
                let t = (step.saturating_sub(2) as f32 / 16.0).min(1.0);
                raw_input
                    .events
                    .push(egui::Event::PointerMoved(a + (b - a) * t));
                if *step == 1 {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos: a,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                *step += 1;
            }
        }
        // The screenshot check's drag from one rack slot to another (`--show rack-move`):
        // onto the handle, press, then across to the other slot, held there (not dropped).
        if let Some((from, to, step)) = self.demo_drag.as_mut() {
            let find = |at: &(InsertTarget, usize), handle: bool| {
                self.rack_slots
                    .iter()
                    .find(|s| (s.0, s.1) == *at)
                    .and_then(|s| if handle { s.3 } else { Some(s.2) })
                    .map(|r| r.center())
            };
            if let (Some(a), Some(b)) = (find(from, true), find(to, false)) {
                let t = (step.saturating_sub(2) as f32 / 12.0).min(1.0);
                let at = a + (b - a) * t;
                raw_input.events.push(egui::Event::PointerMoved(at));
                if *step == 1 {
                    raw_input.events.push(egui::Event::PointerButton {
                        pos: a,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    });
                }
                *step += 1;
            }
        }
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // The whole frame is one edit: whatever the UI changes in the project becomes an
        // undo step, named by the code that made the change (`Session::label_edit`), and
        // recorded only if something changed. Nothing the UI does can bypass the history.
        self.session.begin_edit("", None);
        // A file dialog's answer, acted on where the menu click used to act on it: in the
        // frame's edit, so what it changes is an undo step as before.
        crate::file_dialog::run_ready(self);
        crate::marks::begin_frame();
        self.help.begin_frame();
        self.rack_slots.clear();
        self.clip_rects.clear();
        let drawing = Instant::now();
        self.frame_ui(ui);
        crate::frame_check::drawn(self, drawing.elapsed());
        self.session.end_edit();
        crate::opening::frame_ends(self, &ui.ctx().clone());
        crate::frame_check::frame_ends(self, &ui.ctx().clone());
        crate::edit_check::frame_ends(self, &ui.ctx().clone());
    }

    fn on_exit(&mut self) {
        // A save still being written finishes first (15.3 asks before closing instead).
        if let Err(e) = self.session.finish_saving() {
            eprintln!("saving on exit: {e:#}");
        }
        // The effects rack as it is now, for the next run (nothing in a check run).
        if let Err(e) = self.session.save_rack_now() {
            eprintln!("keeping the effects rack on exit: {e:#}");
        }
        self.settings.audio = self.session.settings.clone();
        self.sync_dock();
        self.settings.layout = serde_json::to_value(crate::dock::for_saving(&self.dock.tree)).ok();
        self.save_settings();
    }
}

impl DawApp {
    /// Show a layout (a saved one, as the settings keep it): its panes where it has them,
    /// the mixer and inspector shown or hidden as it has them. A layout that doesn't read
    /// (from another version) leaves the window as it is.
    pub fn load_layout(&mut self, layout: serde_json::Value) -> bool {
        match serde_json::from_value::<crate::dock::Layout>(layout) {
            Ok(tree) => {
                crate::dock::apply_flags(self, &tree);
                self.dock = crate::dock::Dock::new(tree);
                true
            }
            Err(_) => false,
        }
    }

    /// View > Layouts: show the layout called `name` ("Default": the built-in one).
    pub fn use_layout(&mut self, name: &str) -> Result<(), String> {
        if name.eq_ignore_ascii_case(crate::dock::DEFAULT) {
            let tree = crate::dock::default_layout();
            crate::dock::apply_flags(self, &tree);
            self.show_inspector = true;
            self.dock = crate::dock::Dock::new(tree);
            return Ok(());
        }
        let layout = self
            .settings
            .layouts
            .get(name)
            .cloned()
            .ok_or_else(|| format!("no layout called \"{name}\""))?;
        if self.load_layout(layout) {
            Ok(())
        } else {
            Err(format!("the layout \"{name}\" can't be read"))
        }
    }

    /// Bring the panes up to the app's flags now, as painting the next frame would: a
    /// minimised window isn't painted, yet MCP can show the mixer and save the layout.
    pub fn sync_dock(&mut self) {
        let wanted = crate::dock::wanted(self);
        crate::dock::reconcile(&mut self.dock.tree, &wanted);
    }

    /// Take `path` off the recent projects (the start screen's Remove, and the action); its
    /// files stay.
    pub fn remove_recent(&mut self, path: &Path) {
        if self.settings.remove_recent(path) {
            self.save_settings();
            self.toast(format!(
                "Took {} off the recent projects",
                path.file_stem().map_or_else(
                    || path.display().to_string(),
                    |n| n.to_string_lossy().into_owned()
                )
            ));
        }
    }

    /// A check run's recent projects (`--demo`; the settings are never saved then): the demo,
    /// three copies of it saved yesterday, three days and six weeks ago in the demo's `Temp`
    /// (one made from the "Band setup" template), and one that isn't there.
    fn demo_recents(&mut self) {
        let Some(demo) = self.session.path.clone() else {
            return;
        };
        let Ok(temp) = self.session.project_folder(daw_core::folder::TEMP) else {
            return;
        };
        let day = Duration::from_secs(24 * 3600);
        let now = std::time::SystemTime::now();
        let mut recent = vec![demo.clone()];
        for (name, ago) in [
            ("Summer song", day),
            ("Riff ideas", day * 3),
            ("Live recording - final mix", day * 42),
        ] {
            let dir = temp.join("Recent").join(name);
            let file = dir.join(format!("{name}.{}", daw_core::folder::EXTENSION));
            let made = std::fs::create_dir_all(&dir)
                .and_then(|_| std::fs::copy(&demo, &file))
                .and_then(|_| std::fs::File::options().write(true).open(&file))
                .and_then(|f| f.set_modified(now - ago));
            if let Err(e) = made {
                self.toast(format!("{}: {e}", file.display()));
            }
            recent.push(file);
        }
        // Gone: as if moved, or on a drive that isn't connected.
        recent.insert(
            2,
            std::path::absolute("out/check/Moved away/Old demo/Old demo.oxide").unwrap_or_default(),
        );
        self.settings
            .project_templates
            .insert(recent[1].clone(), "Band setup".into());
        self.settings.recent = recent;
    }

    /// Keep the window's layout as `name` (replacing one of that name).
    pub fn save_layout(&mut self, name: &str) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case(crate::dock::DEFAULT) {
            return Err(format!("a layout can't be called \"{name}\""));
        }
        self.sync_dock();
        let saved = serde_json::to_value(crate::dock::for_saving(&self.dock.tree))
            .map_err(|e| e.to_string())?;
        self.settings.layouts.insert(name.to_string(), saved);
        self.save_settings();
        Ok(())
    }

    fn frame_ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // A project opening (17.1): an empty window and its loading bar, nothing else, until
        // everything is in. Other loads (the effects rack, an undo's plug-ins, a new sample
        // rate's) leave the window as it is, under the busy overlay.
        if self.session.loading().is_some_and(|l| l.opens()) {
            self.opening_frame(ui, &ctx);
            return;
        }
        self.handle_shortcuts(&ctx);

        let menu = egui::Panel::top("menu").show(ui, |ui| dialogs::menu_bar(self, ui));
        theme::set_menu_bottom(&ctx, menu.response.rect.bottom());
        crate::marks::put_rect("menu", menu.response.rect);
        crate::opening::maybe_ask(self);
        crate::frame_check::maybe_ask(self);
        // Opened from the menu just now: the window empties in this frame already.
        if self.session.loading().is_some_and(|l| l.opens()) {
            self.opening_frame(ui, &ctx);
            return;
        }
        let bar = egui::Panel::top("transport")
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(6))
            .show(ui, |ui| transport::show(self, ui));
        crate::marks::put_rect("transport", bar.response.rect);
        let status = egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin::symmetric(8, 3)),
            )
            .show(ui, |ui| status_bar(self, ui));
        crate::marks::put_rect("status", status.response.rect);
        // The panes, docked (`dock.rs`).
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG).inner_margin(2))
            .show(ui, |ui| crate::dock::show(self, ui));

        browser::show(self, &ctx);
        crate::native_editor::show(self, &ctx);
        crate::waveform::dialogs(self, &ctx);
        crate::rack_presets::dialogs(self, &ctx);
        crate::waveform::apply_progress(self, &ctx);
        crate::waveform::apply_question(self, &ctx);
        dialogs::tempo_question(self, &ctx);
        crate::settings_window::show(self, &ctx);
        dialogs::export_progress(self, &ctx);
        dialogs::export_options(self, &ctx);
        crate::quantize::slicer_window(self, &ctx);
        crate::quantize::quantize_window(self, &ctx);
        crate::cpu::window(self, &ctx);
        crate::backups::window(self, &ctx);
        crate::templates::window(self, &ctx);
        crate::templates::save_window(self, &ctx);
        dialogs::save_layout_dialog(self, &ctx);
        crate::multi_out::show(self, &ctx);
        crate::unsaved::show(self, &ctx);
        crate::render::dialog(self, &ctx);
        crate::export_format::document_dialog(self, &ctx);
        crate::metadata_panel::show(self, &ctx);
        // A finished mixdown: show it in the Waveform view.
        if let Some(doc) = self.session.take_opened_mixdown() {
            crate::waveform::show_document(self, doc);
        }
        crate::backups::question(self, &ctx);
        crate::start::show(self, &ctx);
        dialogs::about(self, &ctx);
        // Last of the modals, so an open asked for by any of them above (the start screen, a
        // backup) has its bar over everything in this frame already.
        crate::busy::show(self, &ctx);
        self.update_title(&ctx);
        dialogs::export_report(self, &ctx);
        show_toasts(self, &ctx);
        self.take_screenshot_when_ready(&ctx);
    }

    /// A frame while a project opens: the window's background and the loading bar over it.
    fn opening_frame(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::BG))
            .show(ui, |_| {});
        crate::busy::show(self, ctx);
        self.update_title(ctx);
        show_toasts(self, ctx);
        self.take_screenshot_when_ready(ctx);
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match &self.session.path {
            Some(_) => format!("Oxide — {}", self.session.project.name),
            None => "Oxide".into(),
        };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }
}

fn status_bar(app: &mut DawApp, ui: &mut egui::Ui) {
    let stats = app.session.stats();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 14.0;
        let dim = |s: String| RichText::new(s).small().color(theme::TEXT_DIM);
        match &app.shown_device() {
            Some(info) => {
                let shown = ui.label(dim(format!(
                    "{} · {}{}",
                    info.host,
                    info.output_name,
                    info.input_name
                        .as_ref()
                        .filter(|i| *i != &info.output_name)
                        .map(|i| format!(" / {i}"))
                        .unwrap_or_default()
                )));
                crate::marks::put(ui, "status.device", shown.rect);
                // A bursty driver (FlexASIO on shared WASAPI) gives the whole burst its time.
                let bursts = if stats.cadence > stats.device_block {
                    format!(" (in bursts of {})", stats.cadence)
                } else {
                    String::new()
                };
                // Device buffers gathered into larger blocks (Settings > Audio, 23.1).
                let gathered = if stats.block_size > stats.device_block && stats.device_block > 0 {
                    format!(", processed in {}", stats.block_size)
                } else {
                    String::new()
                };
                let format = ui.label(dim(format!(
                    "{} Hz · {} samples{bursts}{gathered} · {} in / {} out · {}",
                    info.sample_rate,
                    stats.device_block,
                    info.inputs,
                    info.outputs,
                    info.sample_format
                )));
                crate::marks::put(ui, "status.format", format.rect);
            }
            // A check's virtual device (`--show recording`): its input is a simulated signal.
            None if app.session.virtual_inputs().is_some() => {
                let inputs = app.session.virtual_inputs().unwrap_or(0);
                let shown = ui.label(dim(format!(
                    "Virtual device (a check's) · {} Hz · {inputs} simulated in · nothing heard",
                    app.session.sample_rate()
                )));
                crate::marks::put(ui, "status.device", shown.rect);
            }
            None => {
                let text = app
                    .session
                    .device_error
                    .clone()
                    .unwrap_or_else(|| "No audio device".into());
                let shown = ui.label(
                    RichText::new(format!("Audio offline: {text}"))
                        .small()
                        .color(theme::RECORD),
                );
                crate::marks::put(ui, "status.device", shown.rect);
            }
        }
        let rate = f64::from(app.session.sample_rate());
        let pdc = ui.label(dim(format!(
            "PDC {} samples ({:.1} ms)",
            stats.latency,
            stats.latency as f64 * 1000.0 / rate
        )));
        crate::marks::put(ui, "status.pdc", pdc.rect);
        let threads = ui.label(dim(format!("{} audio threads", stats.workers + 1)));
        crate::marks::put(ui, "status.threads", threads.rect);
        threads.on_hover_text(
            "Worker threads plus the driver's audio thread (Options → Settings → Audio)",
        );
        mcp_status(ui, app);
        let midi = app.shown_midi_ports();
        let midi = ui.label(dim(if midi.is_empty() {
            "No MIDI inputs".to_string()
        } else {
            format!("MIDI: {}", midi.join(", "))
        }));
        crate::marks::put(ui, "status.midi", midi.rect);
        if stats.plugin_misses > 0 || stats.skipped_blocks > 0 {
            let skips = ui.label(
                RichText::new(format!(
                    "plugin skips {} · engine skips {}",
                    stats.plugin_misses, stats.skipped_blocks
                ))
                .small()
                .color(theme::MUTE),
            );
            crate::marks::put(ui, "status.skips", skips.rect);
            skips.on_hover_text(
                "Plugin skips: blocks a plug-in was bypassed because its editor was busy.\n\
                 Engine skips: blocks output as silence while an export ran.",
            );
        }
        if stats.input_underruns > 0 {
            let gaps = ui.label(dim(format!("input gaps {}", stats.input_underruns)));
            crate::marks::put(ui, "status.input_gaps", gaps.rect);
            gaps.on_hover_text(
                "Blocks where the input device had not delivered audio in time. A few at start-up \
                 are normal; a rising count means input and output run on different clocks.",
            );
        }
        // The version at the far right (the window's own title bar can't carry a tooltip).
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let version = ui
                .add(
                    egui::Label::new(dim(format!("Oxide {}", crate::version::VERSION)))
                        .sense(egui::Sense::click()),
                )
                .on_hover_text(format!(
                    "Oxide {}. Click for Help → About.",
                    crate::version::text()
                ));
            crate::marks::put(ui, "status.version", version.rect);
            if version.clicked() {
                app.show_about = true;
            }
        });
    });
}

/// The status bar's MCP: the address, "starting…", or why it is off (in red when it failed to
/// start, with Retry). A click retries a failed start, or opens the settings.
fn mcp_status(ui: &mut egui::Ui, app: &mut DawApp) {
    let status = app.mcp.status().clone();
    // The manual's shots show the default port, whatever port the run serves on.
    let shown = if app.neutral {
        crate::neutral::mcp_text(&status.text(), app.mcp_port())
    } else {
        status.text()
    };
    let text = RichText::new(shown).small();
    let text = if status.failed() {
        text.color(theme::RECORD)
    } else {
        text.color(theme::TEXT_DIM)
    };
    let tip = match &status {
        crate::mcp::Status::Running { address, .. } => format!(
            "AI agents can control the DAW through this MCP server ({address}).\n\
             Click to connect Claude Code and Claude Desktop (Options → AI control)."
        ),
        crate::mcp::Status::Starting { port } => {
            format!("The MCP server for AI agents is starting on port {port}.")
        }
        crate::mcp::Status::Failed { .. } => "The MCP server for AI agents could not start. \
             Click to try again, or choose another port in Options → AI control."
            .to_string(),
        crate::mcp::Status::Off { .. } => "The MCP server for AI agents is off. Click for the \
             settings (Options → AI control)."
            .to_string(),
    };
    let shown = ui.add(egui::Label::new(text).sense(egui::Sense::click()));
    crate::marks::put(ui, "status.mcp", shown.rect);
    let clicked = shown.on_hover_text(&tip).clicked();
    // At the status text's size, so it doesn't stand taller than the bar's other items.
    let retry = status.retryable() && {
        let retry = ui.add(egui::Button::new(RichText::new("Retry").small()).small());
        crate::marks::put(ui, "status.mcp_retry", retry.rect);
        retry.on_hover_text(&tip).clicked()
    };
    if retry || (clicked && status.retryable()) {
        app.retry_mcp();
    } else if clicked {
        app.open_settings(crate::settings_window::Section::Mcp);
    }
}

fn show_toasts(app: &mut DawApp, ctx: &egui::Context) {
    app.toasts.retain(|t| t.at.elapsed() < t.lasts);
    if app.toasts.is_empty() {
        return;
    }
    egui::Area::new(egui::Id::unique("toasts"))
        .anchor(egui::Align2::RIGHT_BOTTOM, egui::vec2(-16.0, -40.0))
        .order(egui::Order::Foreground)
        .show(ctx, |ui| {
            for toast in app.toasts.iter().rev().take(5) {
                egui::Frame::popup(ui.style())
                    .fill(Color32::from_rgb(0x2e, 0x32, 0x3b))
                    .show(ui, |ui| {
                        ui.set_max_width(420.0);
                        ui.label(&toast.text);
                    });
            }
        });
}

/// The window's Win32 handle, if it is a Win32 window.
fn window_handle(cc: &eframe::CreationContext) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    match cc.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
        _ => None,
    }
}

/// A file "dropped" by the screenshot check (`--show drop-question`).
#[derive(Debug)]
struct DemoFile(PathBuf);

impl egui::DroppedFile for DemoFile {
    fn path(&self) -> &Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// A made room impulse for the Convolution Reverb's screenshot (stereo, 1.2 s at `rate`): white
/// noise decaying 60 dB in 0.8 s from 0.5, different on each side, with four early reflections
/// in its first 40 ms. From fixed seeds, so every run makes the same file.
fn room_impulse(rate: u32) -> Vec<Vec<f32>> {
    let noise = |n: usize, seed: u64| -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let unit = (x >> 11) as f64 / (1u64 << 53) as f64;
                (unit * 2.0 - 1.0) as f32
            })
            .collect()
    };
    let n = rate as usize * 12 / 10;
    let r = f64::from(rate);
    [51, 52]
        .iter()
        .enumerate()
        .map(|(c, &seed)| {
            let mut out: Vec<f32> = noise(n, seed)
                .iter()
                .enumerate()
                .map(|(i, x)| (0.5 * f64::from(*x) * (-(i as f64 / r) * 6.9 / 0.8).exp()) as f32)
                .collect();
            out[0] = 0.5;
            for (k, (ms, level)) in [(7.0, 0.35), (13.0, -0.3), (21.0, 0.25), (34.0, -0.2)]
                .iter()
                .enumerate()
            {
                let at = ((ms + 1.5 * (c + k) as f64) * r / 1000.0) as usize;
                out[at] += *level as f32;
            }
            out
        })
        .collect()
}
