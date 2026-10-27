//! Timed check runs for work that must never freeze the window (dev plan 18.3), as 17.1's
//! `--show open-files` / `open-plugins` are for opening:
//!
//! - `--show undo-reload`: the demo with three plug-ins on each guitar, Guitar L's
//!   plug-ins removed in one edit, then undone as Ctrl+Z does. The plug-ins come back behind the
//!   busy bar, a plug-in a frame, the window painting.
//! - `--show render-progress`: Guitar L rendered as the render dialog's Render does, held halfway
//!   (`Session::hold_export`) for the shot of its progress bar, then let go; its file goes on a
//!   new track.
//! - `--show save` (20.1): the demo with three plug-ins on each guitar saved as Ctrl+S
//!   saves it: the bar up in the click's frame, each plug-in's state read in a frame of its own
//!   after the bar has been painted twice, the file written on a worker.
//! - `--show file-dialog` (18.6): File > Open's dialog shown as Ctrl+O shows it, then left up
//!   until something closes it (the maintainer's check calls `get_project` over MCP while it
//!   is up, then cancels it). The window must keep painting meanwhile, every frame
//!   within budget, agents' calls answered, and the cancel must open nothing.
//!
//! Every frame from the click on is timed (from `logic`'s start to `ui`'s end): the click's frame
//! and the frames until the bar has been painted twice must fit a frame's budget, the bar must be
//! painted in the click's frame, and (undo) each VST3 plug-in loads in a frame of its own or
//! (render) every frame of the render fits the budget. They print `RESULT: PASS` or `FAIL:`
//! lines, and the app exits 1 on a failure.

use crate::app::DawApp;
use daw_core::project::{InsertId, TrackId};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A frame's budget at 60 Hz.
const BUDGET: Duration = Duration::from_micros(16_667);

/// Frames the bar is painted before the main thread's share of the work may start (as
/// `opening::hold` holds a load).
const PAINTED_FIRST: usize = 2;

/// Frames after the check run starts before it clicks (the window up and settled).
const ASK_AT: u32 = 30;

/// Frames the render stays held after its shot is asked for.
const HELD_AFTER_SHOT: u32 = 5;

/// States a save reads before its shot holds the rest (`--show save`).
const SAVE_READ_BEFORE_SHOT: usize = 3;

/// How long the busy bar is up before the save's shot: the overlay's fade-in (about 0.1 s)
/// well over, so the shot is the bar as it stays.
const SAVE_SHOT_AFTER: Duration = Duration::from_millis(500);

/// Frames drawn after the work ends before the report (the result painted).
const AFTER: u32 = 5;

/// The longest the window may go without a frame while a file dialog is up. It asks for one
/// every 16 ms; as the dialog comes up Windows disables the window and links the two threads'
/// input, which once cost 48 ms. A dialog that blocks the window stops it for seconds.
const GAP: Duration = Duration::from_millis(250);

/// Frames the window must paint while a file dialog is up (a dialog closed sooner proves
/// nothing).
const FRAMES_WHILE_OPEN: usize = 30;

static FAILED: AtomicBool = AtomicBool::new(false);

/// The check run failed: the app exits 1 (`main`).
pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

/// What the check run clicks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Undo the removal of these plug-ins (Ctrl+Z).
    UndoReload,
    /// Render this track (the render dialog's Render).
    RenderProgress(TrackId),
    /// Show File > Open's dialog (Ctrl+O), closed by the check's script.
    FileDialog,
    /// Save the project (Ctrl+S).
    Save,
}

struct Frame {
    /// When it began.
    at: Instant,
    took: Duration,
    /// The work was under way at the frame's end.
    busy: bool,
    /// The bar (the busy overlay, or the render's progress) was painted in it.
    bar: bool,
    /// Plug-ins of the check's that came in during it.
    loaded: usize,
    /// Agents' calls run by its end (`DawApp::mcp_calls`).
    calls: u64,
}

pub struct Check {
    pub kind: Kind,
    frame: u32,
    asked: bool,
    began: Option<(Instant, usize)>,
    /// The bar was painted this frame (`busy::show`, `dialogs::export_progress`).
    pub bar: bool,
    frames: Vec<Frame>,
    /// The plug-ins the undo must bring back.
    expected: Vec<InsertId>,
    /// The tracks before the render (its new one comes after).
    tracks_before: usize,
    /// The states a save reads, and when it was asked for.
    states: usize,
    asked_at: Option<std::time::SystemTime>,
    /// Agents' calls run, and the project's file, before the click.
    calls_before: u64,
    path_before: Option<std::path::PathBuf>,
    shot: Option<u32>,
    /// When the save's bar first went up.
    bar_since: Option<Instant>,
    /// Frames since the work ended.
    after: u32,
    reported: bool,
}

impl Check {
    /// A check of `kind`; `expected` are the plug-ins an undo brings back.
    pub fn new(kind: Kind, expected: Vec<InsertId>) -> Self {
        Self {
            kind,
            frame: 0,
            asked: false,
            began: None,
            bar: false,
            frames: Vec::new(),
            expected,
            tracks_before: 0,
            states: 0,
            asked_at: None,
            calls_before: 0,
            path_before: None,
            shot: None,
            bar_since: None,
            after: 0,
            reported: false,
        }
    }

    /// The screenshot: the bar on screen partway (a plug-in loading with more to come; the
    /// render held halfway).
    pub fn shot_due(&self, app: &DawApp) -> bool {
        let painted = self.frames.iter().rev().take_while(|f| f.busy).count();
        match self.kind {
            Kind::UndoReload => app.session.loading().is_some_and(|l| {
                l.stage == daw_core::session::LoadStage::Loading
                    && l.stage_done + 1 < l.stage_total
                    && painted >= PAINTED_FIRST
            }),
            // Held halfway: its bar partway.
            Kind::RenderProgress(_) => {
                let progress = match app.session.export_status() {
                    daw_core::session::ExportStatus::Running { progress, .. } => progress,
                    _ => 0.0,
                };
                progress >= 0.4 && painted >= PAINTED_FIRST
            }
            // The window painting behind the dialog (which egui's shot doesn't hold).
            Kind::FileDialog => painted >= PAINTED_FIRST,
            // Partway through the reads (held there for it), the overlay long faded in.
            Kind::Save => {
                app.hold_save_for_shot
                    && self
                        .bar_since
                        .is_some_and(|at| at.elapsed() >= SAVE_SHOT_AFTER)
            }
        }
    }
}

/// Whether the check's work is under way.
fn busy(app: &DawApp, kind: Kind) -> bool {
    match kind {
        Kind::UndoReload => app.session.loading().is_some(),
        Kind::RenderProgress(_) => matches!(
            app.session.export_status(),
            daw_core::session::ExportStatus::Running { .. }
        ),
        Kind::FileDialog => crate::file_dialog::is_open(app),
        Kind::Save => app.session.saving().is_some(),
    }
}

/// The check's plug-ins loaded now.
fn loaded(app: &DawApp, expected: &[InsertId]) -> usize {
    expected
        .iter()
        .filter(|id| app.session.insert_info(**id).is_some())
        .count()
}

/// What the check counts each frame: plug-ins loaded (undo), or a save's states read so far
/// (all of them once it writes or is done).
fn counted(app: &DawApp, check: &Check) -> usize {
    match check.kind {
        Kind::Save => match app.session.save_progress() {
            Some(p) if !p.writing => p.done,
            _ if check.asked => check.states,
            _ => 0,
        },
        _ => loaded(app, &check.expected),
    }
}

/// The project's inserts with a plug-in: the states a save reads.
fn states(app: &DawApp) -> usize {
    app.session
        .project
        .all_inserts()
        .filter(|s| app.session.insert_info(s.id).is_some())
        .count()
}

/// Start of a frame (`logic`).
pub fn frame_begins(app: &mut DawApp) {
    let Some(check) = &app.frame_check else {
        return;
    };
    let live = counted(app, check);
    if let Some(check) = &mut app.frame_check {
        check.began = Some((Instant::now(), live));
        check.bar = false;
    }
}

/// Where the user would click (in `ui`, before the dialogs and the overlay are drawn).
pub fn maybe_ask(app: &mut DawApp) {
    let count = states(app);
    let Some(check) = &mut app.frame_check else {
        return;
    };
    if check.asked || check.frame < ASK_AT {
        return;
    }
    check.asked = true;
    let kind = check.kind;
    check.tracks_before = app.session.project.tracks.len();
    check.calls_before = app.mcp_calls;
    check.path_before = app.session.path.clone();
    check.states = count;
    check.asked_at = Some(std::time::SystemTime::now());
    match kind {
        Kind::UndoReload => app.undo(),
        Kind::RenderProgress(track) => {
            app.session.hold_export(true);
            crate::render::start(app, track, 0.5, true);
        }
        Kind::FileDialog => crate::dialogs::open_dialog(app),
        Kind::Save => app.save_project(false),
    }
}

/// The bar was painted.
pub fn bar_painted(app: &mut DawApp) {
    if let Some(check) = &mut app.frame_check {
        check.bar = true;
    }
}

/// The screenshot was asked for this frame.
pub fn shot_taken(app: &mut DawApp) {
    if let Some(check) = &mut app.frame_check {
        check.shot = Some(check.frame);
    }
}

/// End of a frame (`ui`): time it; once the work is over and its result drawn, report and close.
pub fn frame_ends(app: &mut DawApp, ctx: &egui::Context) {
    let Some(check) = &app.frame_check else {
        return;
    };
    let kind = check.kind;
    let busy_now = busy(app, kind);
    let live = counted(app, check);
    let calls = app.mcp_calls;
    let Some(check) = &mut app.frame_check else {
        return;
    };
    check.frame += 1;
    if let Some((began, before)) = check.began.take() {
        if check.asked {
            check.frames.push(Frame {
                at: began,
                took: began.elapsed(),
                busy: busy_now,
                bar: check.bar,
                loaded: live.saturating_sub(before),
                calls,
            });
        }
    }
    // The save: held partway through its reads for the shot (once the overlay has faded in),
    // let go once it is in. The held frames read nothing, so the check's rules still hold.
    if kind == Kind::Save && busy_now {
        check.bar_since.get_or_insert_with(Instant::now);
        let read = app.session.save_progress().map_or(0, |p| p.done);
        match check.shot {
            None if read >= SAVE_READ_BEFORE_SHOT => app.hold_save_for_shot = true,
            Some(at) if check.frame >= at + HELD_AFTER_SHOT => app.hold_save_for_shot = false,
            _ => {}
        }
    }
    let Some(check) = &mut app.frame_check else {
        return;
    };
    // The render's shot is in: let it go.
    if matches!(kind, Kind::RenderProgress(_))
        && check
            .shot
            .is_some_and(|at| check.frame >= at + HELD_AFTER_SHOT)
    {
        app.session.hold_export(false);
    }
    let Some(check) = &mut app.frame_check else {
        return;
    };
    let started = check.frames.iter().any(|f| f.busy);
    if started && !busy_now {
        check.after += 1;
    }
    let done = check.after >= AFTER;
    let timed_out = check.asked && check.frames.len() > 3000;
    if (done || timed_out) && !check.reported {
        check.reported = true;
        let failures = report(app, timed_out);
        if !failures.is_empty() {
            FAILED.store(true, Ordering::Relaxed);
        }
        app.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Print the frames and the verdict; returns the failures.
fn report(app: &DawApp, timed_out: bool) -> Vec<String> {
    let Some(check) = &app.frame_check else {
        return Vec::new();
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mut failures = Vec::new();
    if timed_out {
        failures.push("the work never finished (3000 frames)".to_string());
    }
    let Some(first) = check.frames.first() else {
        failures.push("the check never clicked".into());
        print(&failures);
        return failures;
    };
    let what = match check.kind {
        Kind::UndoReload => "undo",
        Kind::RenderProgress(_) => "render",
        Kind::FileDialog => "file dialog",
        Kind::Save => "save",
    };
    for (k, f) in check.frames.iter().enumerate() {
        println!(
            "frame {k:>3}: {:>8.2} ms{}{}",
            ms(f.took),
            if f.busy { "  busy" } else { "" },
            if f.bar { "  (bar)" } else { "" }
        );
        if f.loaded > 0 {
            let what = if check.kind == Kind::Save {
                "state(s) read"
            } else {
                "plug-in(s) loaded"
            };
            println!("           {} {what}", f.loaded);
        }
    }
    if check.kind != Kind::FileDialog {
        // The click's frame and the ones up to the bar painted twice.
        let held = check
            .frames
            .iter()
            .take(PAINTED_FIRST)
            .map(|f| f.took)
            .max()
            .unwrap_or_default();
        println!(
            "the {what}'s first frame {:.2} ms; up to the bar on screen, worst {:.2} ms",
            ms(first.took),
            ms(held)
        );
        if held > BUDGET {
            failures.push(format!(
                "the frames up to the bar on screen took up to {:.1} ms (budget {:.1} ms)",
                ms(held),
                ms(BUDGET)
            ));
        }
        if !first.busy {
            failures.push(format!("the {what} wasn't under way after its click"));
        } else if !first.bar {
            failures.push(format!(
                "the bar wasn't painted in the {what}'s first frame"
            ));
        }
    }
    let worst = check
        .frames
        .iter()
        .map(|f| f.took)
        .max()
        .unwrap_or_default();
    let busy_frames = check.frames.iter().filter(|f| f.busy).count();
    println!(
        "{} frames in all, {busy_frames} with the {what} under way, worst {:.1} ms",
        check.frames.len(),
        ms(worst)
    );
    match check.kind {
        Kind::UndoReload => {
            let vst3 = check
                .expected
                .iter()
                .filter(|id| {
                    app.session
                        .find_insert(**id)
                        .is_some_and(|s| !s.path.to_string_lossy().starts_with("native:"))
                })
                .count();
            let loading = check.frames.iter().filter(|f| f.loaded > 0).count();
            println!("{vst3} VST3 plug-ins came back in {loading} frames");
            if vst3 == 0 {
                failures.push("the undo brought back no VST3 plug-in (none installed?)".into());
            } else if loading < vst3 {
                failures.push(format!(
                    "{vst3} VST3 plug-ins loaded in {loading} frames: the bar couldn't paint \
                     between them"
                ));
            }
            let missing = check.expected.len() - loaded(app, &check.expected);
            if missing > 0 {
                failures.push(format!("{missing} plug-ins didn't come back"));
            }
        }
        Kind::FileDialog => dialog_failures(app, check, &mut failures),
        Kind::Save => save_failures(app, check, &mut failures),
        Kind::RenderProgress(track) => {
            if worst > BUDGET {
                failures.push(format!(
                    "a frame of the render took {:.1} ms (budget {:.1} ms)",
                    ms(worst),
                    ms(BUDGET)
                ));
            }
            let made = app.session.project.tracks.len() > check.tracks_before
                && app
                    .session
                    .project
                    .tracks
                    .iter()
                    .any(|t| t.name.ends_with("(rendered)") && !t.clips.is_empty());
            if !made {
                failures.push("the render's new track wasn't made".into());
            }
            if app.session.project.track(track).is_some_and(|t| t.enabled) {
                failures.push("the rendered track wasn't switched off".into());
            }
        }
    }
    print(&failures);
    failures
}

/// The save check's verdict: every state read, never two VST3s' in one frame (the demo's are
/// all VST3s), none before the bar was painted twice, and the file written.
fn save_failures(app: &DawApp, check: &Check, failures: &mut Vec<String>) {
    let read: usize = check.frames.iter().map(|f| f.loaded).sum();
    let most = check.frames.iter().map(|f| f.loaded).max().unwrap_or(0);
    let early: usize = check
        .frames
        .iter()
        .take(PAINTED_FIRST)
        .map(|f| f.loaded)
        .sum();
    let frames = check.frames.iter().filter(|f| f.loaded > 0).count();
    println!(
        "{read} plug-in states read in {frames} frames, at most {most} in a frame, {early} before \
         the bar had been painted twice"
    );
    if check.states == 0 {
        failures.push("the demo has no plug-ins (none installed?)".into());
    }
    if read != check.states {
        failures.push(format!("{read} states read of {}", check.states));
    }
    if most > 1 {
        failures.push(format!(
            "{most} plug-in states read in one frame: the bar couldn't paint between them"
        ));
    }
    if early > 0 {
        failures.push(format!(
            "{early} states read before the bar had been painted twice"
        ));
    }
    let written = app
        .session
        .path
        .as_ref()
        .and_then(|p| std::fs::metadata(p).ok())
        .and_then(|m| m.modified().ok())
        .zip(check.asked_at)
        .is_some_and(|(modified, asked)| modified >= asked);
    if !written {
        failures.push("the project file wasn't written".into());
    }
}

/// The file dialog check's verdict: the window painted on, in budget, answering agents, and
/// the cancel opened nothing.
fn dialog_failures(app: &DawApp, check: &Check, failures: &mut Vec<String>) {
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let Some(first) = check.frames.first() else {
        return;
    };
    if !first.busy {
        failures.push("the file dialog wasn't up after its click".into());
        return;
    }
    if first.took > BUDGET {
        failures.push(format!(
            "the frame that showed the dialog took {:.1} ms (budget {:.1} ms): it waited for it",
            ms(first.took),
            ms(BUDGET)
        ));
    }
    let open: Vec<&Frame> = check.frames.iter().take_while(|f| f.busy).collect();
    let seconds = open.last().map_or(0.0, |f| (f.at - first.at).as_secs_f64());
    let worst = open.iter().map(|f| f.took).max().unwrap_or_default();
    // Between frame starts, up to the first frame after the dialog closed.
    let gap = check
        .frames
        .windows(2)
        .take(open.len())
        .map(|w| w[1].at - w[0].at)
        .max()
        .unwrap_or_default();
    let calls = open
        .last()
        .map_or(0, |f| f.calls.saturating_sub(check.calls_before));
    println!(
        "the dialog was up {seconds:.1} s: {} frames painted, worst {:.2} ms, longest gap between \
         frames {:.1} ms, {calls} agent call(s) answered",
        open.len(),
        ms(worst),
        ms(gap)
    );
    if open.len() < FRAMES_WHILE_OPEN {
        failures.push(format!(
            "only {} frames painted while the dialog was up (at least {FRAMES_WHILE_OPEN})",
            open.len()
        ));
    }
    if worst > BUDGET {
        failures.push(format!(
            "a frame took {:.1} ms while the dialog was up (budget {:.1} ms)",
            ms(worst),
            ms(BUDGET)
        ));
    }
    if gap > GAP {
        failures.push(format!(
            "the window went {:.1} ms without a frame while the dialog was up (at most {:.0} ms)",
            ms(gap),
            ms(GAP)
        ));
    }
    if calls == 0 {
        failures.push("no agent call was answered while the dialog was up".into());
    }
    if app.session.loading().is_some() || app.session.path != check.path_before {
        failures.push("cancelling the dialog opened a project".into());
    }
}

fn print(failures: &[String]) {
    if failures.is_empty() {
        println!("RESULT: PASS");
    } else {
        for f in failures {
            println!("FAIL: {f}");
        }
    }
}

/// Frames drawn before the draw timing starts: the window up, fonts made, the demo's files in.
const DRAWN_FROM: u32 = 60;

/// The waveform zoom shots' timing (23.2: `--show clip-overview`, `clip-close`,
/// `clip-samples`, `waveform-samples`): every frame's drawing (`ui`, the Multitrack and the
/// Waveform view painted at that zoom) from [`DRAWN_FROM`] until the shot, the worst of them
/// reported with the shot and failing the run over a frame's budget.
#[derive(Debug, Default)]
pub struct DrawTimer {
    frames: u32,
    timed: u32,
    worst: Duration,
    total: Duration,
}

/// A frame's drawing took `took`.
pub fn drawn(app: &mut DawApp, took: Duration) {
    if let Some(timer) = &mut app.draw_timer {
        timer.frames += 1;
        if timer.frames > DRAWN_FROM {
            timer.timed += 1;
            timer.worst = timer.worst.max(took);
            timer.total += took;
        }
    }
}

/// The shot is being taken: report the drawing's worst and mean frame.
pub fn report_drawing(app: &mut DawApp) {
    let Some(timer) = app.draw_timer.take() else {
        return;
    };
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let mean = timer.total / timer.timed.max(1);
    println!(
        "drawing: worst frame {:.2} ms, mean {:.2} ms over {} frames",
        ms(timer.worst),
        ms(mean),
        timer.timed
    );
    let mut failures = Vec::new();
    if timer.timed == 0 {
        failures.push("no frame was timed before the shot".to_string());
    }
    if timer.worst > BUDGET {
        failures.push(format!(
            "a frame took {:.1} ms to draw (budget {:.1} ms)",
            ms(timer.worst),
            ms(BUDGET)
        ));
    }
    if !failures.is_empty() {
        FAILED.store(true, Ordering::Relaxed);
    }
    print(&failures);
}
