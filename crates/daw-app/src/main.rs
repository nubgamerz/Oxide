//! Oxide, the desktop app.
//!
//! `daw [project.oxide]` opens a project (an older `.dawproj` too); without one, the start
//! screen asks for a new or existing project. For automated checks,
//! `daw --demo <file.wav> [--plugin NAME]... --screenshot out.png` builds a demo project,
//! renders about 2.5 s of frames, saves a screenshot and exits. Other check flags:
//! `--mixer` (show the mixer), `--play` (start playback), `--mute` (mute the master),
//! `--silent` (send the device silence, while the master and its meters run as usual),
//! `--wait SECONDS` (take the screenshot after this long, instead of about 2.5 s),
//! `--marks PATH` (with `--screenshot`: the named parts' rects in the shot, for the manual,
//! `marks.rs`), `--size WxH` (the window's size in points),
//! `--editor` (open the first plug-in editor),
//! `--show browser|settings|piano-roll|tempo-question|automation|loudness|export|export-report|stretch|fades|selection|markers|quantize|about`
//! (open a dialog; `settings-audio`, `settings-midi`, `settings-projects` and `settings-mcp`
//! open the settings on that section, `start` the start screen over the demo with a few recent
//! projects, one of them missing (every `--demo` run lists those, never saved); the first MIDI clip in the piano roll, the tempo question of a dropped file,
//! automation lanes on the first guitar, the master's loudness panel, the export options, or
//! the report of a normalised export of the demo; `open-files` and `open-plugins` open the
//! saved demo as File > Open does and time every frame, printing `RESULT:`/`FAIL:` lines and
//! exiting 1 on a failure, as `undo-reload` (an undo bringing three plug-ins back),
//! `render-progress` (a track rendered, held halfway for the shot) and `save` (the demo saved
//! as Ctrl+S does, its plug-ins' states read a frame each behind the bar) do, and `rename` and
//! `volume-edit` (a track renamed, and volumes and a pan typed, clicked and typed as the user
//! would, `edit_check`) too, and `record-menu` and `count-in` (the Record button's count-in
//! picked from its right-click menu, and with `count-in` a take recorded through it from a
//! simulated input); `mcp-settings` and `mcp-port-in-use` open the settings, the second
//! with the MCP port held by the app itself so the server finds it taken; `mcp-connect` and
//! `mcp-connected` too, with Claude's configs seeded in `--claude-profile DIR`: not
//! configured, and configured in Claude Code and for another port in Claude Desktop),
//! `--mcp-port N` / `--no-mcp` (the AI-control server), `--exit-after SECONDS`,
//! `--claude-profile DIR` (Claude's configs read and written in DIR, never the user's).
//! `--version` prints "Oxide 0.2.0 (c56cc31)" and exits (the installer check reads it).
//!
//! While it runs, the app holds the named mutex [`RUNNING_MUTEX`], so the installer
//! (`installer/oxide.iss`'s `AppMutex`) asks for Oxide to be closed before it updates or
//! removes it.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod arrange;
mod automation;
mod backups;
mod browser;
mod busy;
mod clipboard;
mod cpu;
mod diagnostics;
mod dialogs;
mod dock;
mod drop;
mod edit_check;
mod export_format;
mod file_dialog;
mod frame_check;
mod help;
mod icon;
mod inspector;
mod loudness;
mod marks;
mod mcp;
mod mcp_settings;
mod metadata_panel;
mod mixer;
mod multi_out;
mod native_editor;
mod neutral;
mod opening;
mod piano_roll;
mod plugin_clipboard;
mod quantize;
mod rack_presets;
mod recent;
mod render;
mod sends;
mod settings;
mod settings_window;
mod start;
mod templates;
mod theme;
mod track_drag;
mod transport;
mod unsaved;
mod value_edit;
mod version;
mod wave;
mod waveform;
mod zoom;

use std::path::PathBuf;

/// Command-line options.
#[derive(Default)]
pub struct Options {
    pub open: Option<PathBuf>,
    pub demo: Option<PathBuf>,
    pub demo_plugins: Vec<String>,
    /// The VST3s a check's scenarios use, by role (`--vst3 ROLE=NAME`, repeated): `amp`,
    /// `compressor`, `maximizer`, `eq` (effects; built-in ones stand in without them) and
    /// `synth`, `multi-out` (instruments; scenarios go without one). See
    /// [`DawApp::demo_effect`](crate::app::DawApp::demo_effect).
    pub demo_vst3: Vec<(String, String)>,
    pub screenshot: Option<PathBuf>,
    /// Write the screenshot's marks (the named parts' rects, `marks.rs`) here, as JSON.
    pub marks: Option<PathBuf>,
    /// The window's inner size in points (`--size 1280x800`; default 1440x900).
    pub size: Option<[f32; 2]>,
    pub show_mixer: bool,
    /// Start playback on launch (for checks).
    pub play: bool,
    /// Mute the master on launch, so checks make no sound.
    pub mute: bool,
    /// Send the device silence: checks that need audio through the master (its meters) but
    /// must make no sound.
    pub silent: bool,
    /// Take the screenshot after this many seconds instead of about 2.5 s of frames.
    pub wait: Option<f64>,
    /// Open the selected track's first plug-in editor on launch.
    pub editor: bool,
    /// Open a dialog on launch, for screenshots: `browser` or `settings`.
    pub show: Option<String>,
    /// Serve MCP on this port instead of the configured one.
    pub mcp_port: Option<u16>,
    /// Require this MCP token for this run instead of the configured one (checks).
    pub mcp_token: Option<String>,
    pub no_mcp: bool,
    /// Close the app after this many seconds (for checks).
    pub exit_after: Option<f64>,
    /// Start minimised (checks that work keeps going while hidden).
    pub minimize: bool,
    /// Answer the "Save the changes?" question with `save`, `discard` or `cancel` as soon as
    /// it is up (the maintainer's save-question check).
    pub answer: Option<String>,
    /// Read and write Claude Code's and Claude Desktop's configs in a profile inside this
    /// folder instead of the user's own (checks and shots, dev plan 17.3).
    pub claude_profile: Option<PathBuf>,
    /// Read the manual (Help > Manual, F1, `search_manual`) from this folder instead of
    /// `manual\` beside the program (checks build one of their own).
    pub manual: Option<PathBuf>,
    /// The manual's shots (27.3, TD191): show no machine's details (`neutral.rs`): a virtual
    /// device with a neutral name (no audio device is opened), the demo at
    /// `Documents\Oxide Projects\Demo`, a MIDI keyboard for the MIDI ports, MCP at its default
    /// port.
    pub neutral: bool,
    /// Print the version and exit.
    pub version: bool,
}

fn parse_args() -> Options {
    let mut options = Options::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--demo" => options.demo = args.next().map(PathBuf::from),
            "--plugin" => options.demo_plugins.extend(args.next()),
            "--vst3" => options.demo_vst3.extend(args.next().and_then(|a| {
                a.split_once('=')
                    .map(|(role, name)| (role.trim().to_string(), name.trim().to_string()))
            })),
            "--screenshot" => options.screenshot = args.next().map(PathBuf::from),
            "--marks" => options.marks = args.next().map(PathBuf::from),
            "--size" => options.size = args.next().as_deref().and_then(parse_size),
            "--mixer" => options.show_mixer = true,
            "--play" => options.play = true,
            "--editor" => options.editor = true,
            "--show" => options.show = args.next(),
            "--mcp-port" => options.mcp_port = args.next().and_then(|p| p.parse().ok()),
            "--mcp-token" => options.mcp_token = args.next(),
            "--no-mcp" => options.no_mcp = true,
            "--minimize" => options.minimize = true,
            "--answer" => options.answer = args.next(),
            "--exit-after" => options.exit_after = args.next().and_then(|s| s.parse().ok()),
            "--mute" => options.mute = true,
            "--silent" => options.silent = true,
            "--wait" => options.wait = args.next().and_then(|s| s.parse().ok()),
            "--claude-profile" => options.claude_profile = args.next().map(PathBuf::from),
            "--manual" => options.manual = args.next().map(PathBuf::from),
            "--neutral" => options.neutral = true,
            "--version" => options.version = true,
            _ => options.open = Some(PathBuf::from(arg)),
        }
    }
    options
}

/// `1280x800` → `[1280.0, 800.0]`; `None` for anything else (or a size under the window's
/// minimum).
fn parse_size(text: &str) -> Option<[f32; 2]> {
    let (w, h) = text.split_once(['x', 'X'])?;
    let size = [w.trim().parse().ok()?, h.trim().parse().ok()?];
    (size[0] >= MIN_SIZE[0] && size[1] >= MIN_SIZE[1]).then_some(size)
}

/// The window's smallest inner size, in points.
const MIN_SIZE: [f32; 2] = [900.0, 560.0];

/// The mutex the app holds while it runs; `installer/oxide.iss`'s `AppMutex` names it (the
/// name is case-sensitive: change both together).
pub const RUNNING_MUTEX: &str = "OxideDAW";

/// Create [`RUNNING_MUTEX`] for the rest of the process. The handle is never closed: Windows
/// closes it when the process ends, so the mutex lasts exactly as long as the app.
fn hold_running_mutex() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::CreateMutexW;
        let name: Vec<u16> = RUNNING_MUTEX.encode_utf16().chain([0]).collect();
        // SAFETY: `name` is a NUL-terminated UTF-16 string that outlives the call; null
        // security attributes are the default, and the mutex is only made, not taken (FALSE).
        // A failure leaves the installer unable to tell the app runs, nothing more.
        let _handle = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
    }
}

fn main() -> eframe::Result {
    let options = parse_args();
    if options.version {
        println!("Oxide {}", version::text());
        return Ok(());
    }
    hold_running_mutex();
    let native = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Oxide")
            .with_icon(std::sync::Arc::new(icon::window_icon()))
            .with_inner_size(options.size.unwrap_or([1440.0, 900.0]))
            .with_min_inner_size(MIN_SIZE)
            .with_drag_and_drop(true),
        ..Default::default()
    };
    let result = eframe::run_native(
        "Oxide",
        native,
        Box::new(move |cc| Ok(Box::new(app::DawApp::new(cc, options)))),
    );
    // A check run that failed (`--show open-files` / `open-plugins` / `undo-reload` /
    // `render-progress` / `file-dialog` / `rename` / `volume-edit` /
    // `record-menu` / `count-in`) says so in its exit code.
    if opening::failed() || frame_check::failed() || edit_check::failed() {
        std::process::exit(1);
    }
    result
}
