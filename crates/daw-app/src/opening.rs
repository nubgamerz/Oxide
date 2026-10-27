//! A project opens behind its loading bar (dev plan 17.1): while `Session::loading` says it is
//! opening, the window is empty but for the bar (`DawApp::frame_ui`, `busy::show`), the
//! main thread's share of the open (destroying and loading plug-ins) waits until the bar has
//! been painted twice (`hold`), and Cancel goes back to where it was (`cancel`).
//!
//! The check runs `--show open-files` and `--show open-plugins` ([`Check`]) open the demo as
//! the menu does and time every frame: the frames up to the bar on screen and every frame of
//! the files under a frame's budget, a plug-in a frame after that, and no Multitrack drawn
//! until the end. They print `RESULT: PASS` or `FAIL:` lines and exit 1 on a failure.

use crate::app::DawApp;
use daw_core::api::{AfterOpen, ApiError, Reply};
use daw_core::session::{LoadStage, Session};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Frames the bar is painted before any plug-in is destroyed or loaded (one isn't always
/// enough on Windows for it to reach the screen before a frame blocks; as `busy::run`).
const PAINTED_FIRST: u8 = 2;

/// A frame's budget at 60 Hz.
const BUDGET: Duration = Duration::from_micros(16_667);

/// Frames after the check run starts before it asks to open (the window up and settled).
const ASK_AT: u32 = 20;

/// The files the files' shot lets read before it holds the rest.
const FILES_FIRST: usize = 2;

/// The check run failed: the app exits 1 (`main`).
static FAILED: AtomicBool = AtomicBool::new(false);

pub fn failed() -> bool {
    FAILED.load(Ordering::Relaxed)
}

/// What the busy bar is up for. Its frames painted count anew when that changes: the look for
/// unsaved changes before an open (20.1) puts the bar up for its reads, and the open that
/// follows in the same frame must still wait for its own bar to be painted twice before a
/// plug-in is closed or loaded (17.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarFor {
    /// An open, an undo's plug-ins, the rack, a new rate (`Session::loading`).
    Loading,
    /// A save, or the look for unsaved changes (`Session::save_progress`).
    Saving,
}

/// The frames the busy bar has been painted for what it is up for now.
#[derive(Debug, Default)]
pub struct BarPainted {
    what: Option<BarFor>,
    frames: u8,
}

impl BarPainted {
    /// The bar was painted this frame, for `what` (`busy::show`).
    pub fn painted(&mut self, what: BarFor) {
        if self.what != Some(what) {
            self.what = Some(what);
            self.frames = 0;
        }
        self.frames = self.frames.saturating_add(1);
    }

    /// Whether `what`'s work still waits: its bar not yet painted twice.
    pub fn holds(&self, what: BarFor) -> bool {
        self.what != Some(what) || self.frames < PAINTED_FIRST
    }

    /// Nothing busy: the next bar counts from nothing.
    fn idle(&mut self) {
        *self = Self::default();
    }
}

/// Housekeeping, before `Session::update`: hold the open's plug-ins back (and a save's reads of
/// their states, 20.1) until the bar has been painted. A minimised window paints nothing, so
/// nothing is held then (an agent's open of a project goes on while the window is minimised).
pub fn hold(app: &mut DawApp) {
    let minimized = app.minimized();
    let shot = app.hold_save_for_shot;
    hold_session(&mut app.bar_painted, &mut app.session, minimized, shot);
}

/// [`hold`]'s rule: an open's (or a save's) main-thread work waits until its own bar has been
/// painted twice, unless the window is minimised; `keep_save` holds a save's reads anyway (the
/// save check's shot).
pub fn hold_session(bar: &mut BarPainted, session: &mut Session, minimized: bool, keep_save: bool) {
    let opening = session.loading().is_some();
    let saving = session.save_progress().is_some();
    if !opening && !saving {
        bar.idle();
    }
    session.hold_loading(opening && !minimized && bar.holds(BarFor::Loading));
    session.hold_saving(saving && (keep_save || (!minimized && bar.holds(BarFor::Saving))));
}

/// Agents' saves (save_project, 20.1) run behind the same bar as Ctrl+S: the action begins the
/// save and hands its answer here (`Host::when_saved`), answered when the file is written (or
/// with why not).
#[derive(Default)]
pub struct AgentSaves {
    waiting: Vec<(Reply, AfterOpen)>,
}

impl AgentSaves {
    /// Answer `reply` with `then`'s answer when the save now running is written (at once if
    /// none is).
    pub fn wait(&mut self, session: &mut Session, reply: Reply, then: AfterOpen) {
        if session.saving().is_none() {
            reply(then(session));
            return;
        }
        self.waiting.push((reply, then));
    }

    /// Each frame, after `Session::update`, with how a save ended this frame (`saved`, from
    /// `Session::take_saved`): answer the calls waiting on it.
    pub fn tick(&mut self, session: &mut Session, saved: Option<&Result<PathBuf, String>>) {
        if self.waiting.is_empty() || (saved.is_none() && session.saving().is_some()) {
            return;
        }
        for (reply, then) in std::mem::take(&mut self.waiting) {
            match saved {
                Some(Err(why)) => reply(Err(ApiError(format!("the save failed: {why}")))),
                _ => reply(then(session)),
            }
        }
    }
}

/// Agents' opens (open_project, open_backup, new_project from a template, cancel_open) run
/// behind the same bar as File > Open: the action begins the open and hands its answer here
/// (`Host::when_opened`), and the call is answered when the open ends, made then (the newer
/// backup is only known at the end). A cancel (the bar's, or cancel_open) or another open over
/// it moves the session's `open_serial`, and the call is answered with an error at once.
#[derive(Default)]
pub struct AgentOpens {
    waiting: Vec<Waiting>,
}

struct Waiting {
    /// The session's `open_serial` when the open began.
    serial: u64,
    reply: Reply,
    then: AfterOpen,
    /// A project was opening (not the effects rack, an undo's plug-ins or a new rate's).
    opens: bool,
}

impl AgentOpens {
    /// Answer `reply` with `then`'s answer when the open now running ends (at once if none is).
    pub fn wait(&mut self, session: &mut Session, reply: Reply, then: AfterOpen) {
        if session.loading().is_none() {
            reply(then(session));
            return;
        }
        self.waiting.push(Waiting {
            serial: session.open_serial(),
            reply,
            then,
            opens: session.loading().is_some_and(|l| l.opens()),
        });
    }

    /// Each frame, after `Session::update`: answer the calls whose open has ended or been
    /// cancelled. True when one of them opened a project (the app clears what was selected;
    /// a rack preset, an undo or a sample-rate change opens none).
    pub fn tick(&mut self, session: &mut Session) -> bool {
        let mut opened = false;
        for waiting in std::mem::take(&mut self.waiting) {
            if waiting.serial != session.open_serial() {
                let now = session
                    .path
                    .as_ref()
                    .map_or("no project".to_string(), |p| p.display().to_string());
                (waiting.reply)(Err(ApiError(format!(
                    "the open was cancelled (or another open replaced it) before it finished; \
                     open now: {now}"
                ))));
            } else if session.loading().is_none() {
                (waiting.reply)((waiting.then)(session));
                opened |= waiting.opens;
            } else {
                self.waiting.push(waiting);
            }
        }
        opened
    }
}

/// The bar's Cancel: back to the project open before (reopened behind the bar), or to the
/// start screen.
pub fn cancel(app: &mut DawApp) {
    match app.session.cancel_loading() {
        Ok(Some(path)) => {
            let name = path
                .file_stem()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
            app.toast(format!("Opening cancelled: back to {name}"));
            app.project_opened();
        }
        Ok(None) => {
            app.toast("Opening cancelled");
            app.project_opened();
        }
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// One frame of the check run.
struct Frame {
    took: Duration,
    /// The open's stage at the frame's start and end (None: not opening).
    from: Option<LoadStage>,
    to: Option<LoadStage>,
    /// The bar was painted in it.
    bar: bool,
    /// Plug-ins loaded in it.
    loaded: usize,
}

/// The check run's open of the demo (`--show open-files` / `open-plugins`).
pub struct Check {
    path: PathBuf,
    /// Hold the files back until the shot of the bar on them is taken.
    files: bool,
    frame: u32,
    asked: bool,
    began: Option<(Instant, Option<LoadStage>, usize)>,
    /// The bar was painted this frame (`busy::show`).
    pub bar: bool,
    /// Frames the Multitrack was drawn while the project was opening.
    multitrack_while_opening: u32,
    /// Frames the Multitrack was drawn after the open.
    multitrack_after: u32,
    frames: Vec<Frame>,
    /// The frame the screenshot was asked for.
    shot: Option<u32>,
    reported: bool,
}

impl Check {
    pub fn new(path: PathBuf, files: bool) -> Self {
        Self {
            path,
            files,
            frame: 0,
            asked: false,
            began: None,
            bar: false,
            multitrack_while_opening: 0,
            multitrack_after: 0,
            frames: Vec::new(),
            shot: None,
            reported: false,
        }
    }

    /// Whether the screenshot is due: the bar on screen at its stage (the files held, or a
    /// plug-in loading).
    pub fn shot_due(&self, app: &DawApp) -> bool {
        let painted = self
            .frames
            .iter()
            .rev()
            .take_while(|f| f.to.is_some())
            .count();
        let Some(l) = app.session.loading() else {
            return false;
        };
        if self.files {
            // The files let through in, the rest held; the bar faded in.
            l.stage == LoadStage::Reading && l.stage_done >= FILES_FIRST && painted >= 20
        } else {
            l.stage == LoadStage::Loading && l.stage_done + 1 < l.stage_total && painted >= 3
        }
    }
}

/// Start of a frame (`logic`).
pub fn frame_begins(app: &mut DawApp) {
    let stage = app.session.loading().map(|l| l.stage);
    let live = live_plugins(&app.session);
    if let Some(check) = &mut app.open_check {
        check.began = Some((Instant::now(), stage, live));
        check.bar = false;
    }
}

/// Where the menu would ask: the check run asks to open the demo, as File > Open does.
pub fn maybe_ask(app: &mut DawApp) {
    let Some(check) = &mut app.open_check else {
        return;
    };
    if check.asked || check.frame < ASK_AT {
        return;
    }
    check.asked = true;
    let (path, files) = (check.path.clone(), check.files);
    if files {
        app.session.hold_file_reads(Some(FILES_FIRST));
    }
    app.open_project_asking(&path);
}

/// The Multitrack is being drawn.
pub fn multitrack_drawn(app: &mut DawApp) {
    let opening = app.session.loading().is_some();
    if let Some(check) = &mut app.open_check {
        if opening {
            check.multitrack_while_opening += 1;
        } else if check.asked {
            check.multitrack_after += 1;
        }
    }
}

/// The screenshot was asked for this frame.
pub fn shot_taken(app: &mut DawApp) {
    if let Some(check) = &mut app.open_check {
        check.shot = Some(check.frame);
    }
}

/// End of a frame (`ui`): time it; once the open is over and the Multitrack has been drawn,
/// report and close.
pub fn frame_ends(app: &mut DawApp, ctx: &egui::Context) {
    let stage = app.session.loading().map(|l| l.stage);
    let live = live_plugins(&app.session);
    let Some(check) = &mut app.open_check else {
        return;
    };
    check.frame += 1;
    if let Some((began, from, before)) = check.began.take() {
        if check.asked {
            check.frames.push(Frame {
                took: began.elapsed(),
                from,
                to: stage,
                bar: check.bar,
                loaded: live.saturating_sub(before),
            });
        }
    }
    // The files' shot is in: let them go.
    if check.files && check.shot.is_some_and(|at| check.frame >= at + 5) {
        app.session.hold_file_reads(None);
    }
    let opened = check.frames.iter().any(|f| f.to.is_some());
    let done = opened && stage.is_none() && check.multitrack_after >= 3;
    let timed_out = check.asked && check.frames.len() > 3000;
    if (done || timed_out) && !check.reported {
        check.reported = true;
        let failures = report(check, timed_out, &app.session);
        if !failures.is_empty() {
            FAILED.store(true, Ordering::Relaxed);
        }
        app.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}

/// Print the frames and the verdict; returns the failures.
fn report(check: &Check, timed_out: bool, session: &daw_core::session::Session) -> Vec<String> {
    let mut failures = Vec::new();
    if timed_out {
        failures.push("the open never finished (3000 frames)".to_string());
    }
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    // From the frame the open began (the bar's first) to the Multitrack's.
    let first = check.frames.iter().position(|f| f.to.is_some());
    let Some(first) = first else {
        failures.push("the project never started opening".into());
        print(&failures);
        return failures;
    };
    let opening = &check.frames[first..];
    for (k, f) in opening.iter().enumerate() {
        println!(
            "frame {k:>3}: {:>8.2} ms  {:?} -> {:?}{}",
            ms(f.took),
            f.from,
            f.to,
            if f.bar { "  (bar)" } else { "" }
        );
        if f.loaded > 0 {
            println!("           {} plug-in(s) loaded", f.loaded);
        }
    }
    // The frame that asked (the menu's click) and the one that opened.
    let asked = check.frames[..=first].iter().map(|f| f.took).max();
    println!(
        "the click's frames: worst {:.2} ms; the open's first frame {:.2} ms",
        ms(asked.unwrap_or_default()),
        ms(opening[0].took)
    );
    let held = opening
        .iter()
        .take(PAINTED_FIRST as usize)
        .map(|f| f.took)
        .max()
        .unwrap_or_default();
    if asked.unwrap_or_default() > BUDGET || held > BUDGET {
        failures.push(format!(
            "the frames up to the bar on screen took up to {:.1} ms (budget {:.1} ms)",
            ms(asked.unwrap_or_default().max(held)),
            ms(BUDGET)
        ));
    }
    if !opening[0].bar {
        failures.push("the bar wasn't painted in the open's first frame".into());
    }
    let reading: Vec<Duration> = opening
        .iter()
        .filter(|f| f.from == Some(LoadStage::Reading) && f.to == Some(LoadStage::Reading))
        .map(|f| f.took)
        .collect();
    let worst_reading = reading.iter().max().copied().unwrap_or_default();
    println!(
        "{} frames reading files, worst {:.2} ms",
        reading.len(),
        ms(worst_reading)
    );
    if worst_reading > BUDGET {
        failures.push(format!(
            "a frame reading files took {:.1} ms (budget {:.1} ms)",
            ms(worst_reading),
            ms(BUDGET)
        ));
    }
    let closing = opening
        .iter()
        .filter(|f| f.from == Some(LoadStage::Closing))
        .count();
    let loading = opening.iter().filter(|f| f.loaded > 0).count();
    let worst = opening.iter().map(|f| f.took).max().unwrap_or_default();
    println!(
        "{} frames in all, {closing} closing plug-ins, {loading} loading them, worst {:.1} ms",
        opening.len(),
        ms(worst)
    );
    if check.multitrack_while_opening > 0 {
        failures.push(format!(
            "the Multitrack was drawn in {} frames while the project opened",
            check.multitrack_while_opening
        ));
    }
    let vst3 = session
        .project
        .all_inserts()
        .filter(|s| !s.path.to_string_lossy().starts_with("native:"))
        .count();
    // Each heavy VST3 loaded is a frame of its own (the demo's three all are).
    if loading < vst3 {
        failures.push(format!(
            "{vst3} VST3 plug-ins loaded in {loading} frames: the bar couldn't paint between them"
        ));
    }
    let missing = session
        .project
        .all_inserts()
        .filter(|s| session.insert_info(s.id).is_none())
        .count();
    if missing > 0 {
        failures.push(format!("{missing} plug-ins didn't load"));
    }
    print(&failures);
    failures
}

/// The project's plug-ins loaded so far.
fn live_plugins(session: &daw_core::session::Session) -> usize {
    session
        .project
        .all_inserts()
        .filter(|s| session.insert_info(s.id).is_some())
        .count()
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

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::api::{ActionOutput, ApiResult, Host, Registry, SessionHost};
    use serde_json::{json, Value};
    use std::sync::mpsc::{channel, Receiver};

    /// The app's side of an agent's open, without a window: a staged session and its waiting
    /// calls, as `DawApp` has them.
    struct App {
        session: Session,
        opens: AgentOpens,
        saves: AgentSaves,
    }

    impl Host for App {
        fn session(&mut self) -> &mut Session {
            &mut self.session
        }

        fn when_opened(&mut self, reply: Reply, then: AfterOpen) {
            self.opens.wait(&mut self.session, reply, then);
        }

        fn when_saved(&mut self, reply: Reply, then: AfterOpen) {
            self.saves.wait(&mut self.session, reply, then);
        }
    }

    fn headless() -> Session {
        Session::new(daw_core::device::AudioSettings {
            output_device: Some("<none>".into()),
            threads: Some(0),
            ..Default::default()
        })
    }

    /// Call `name` as MCP does; the answer arrives on the receiver, now or later.
    fn ask(r: &Registry, app: &mut App, name: &str, args: Value) -> Receiver<ApiResult<Value>> {
        let (tx, rx) = channel();
        r.call(
            app,
            name,
            args,
            Box::new(move |answer| {
                let _ = tx.send(answer.map(|output| match output {
                    ActionOutput::Json(v) => v,
                    ActionOutput::Png(_) => Value::Null,
                }));
            }),
        );
        rx
    }

    /// One frame of the app's housekeeping; how long it took. (A few milliseconds apart, as
    /// frames are, for the workers' reads.)
    fn frame(app: &mut App) -> Duration {
        std::thread::sleep(Duration::from_millis(2));
        let started = Instant::now();
        app.session.update();
        app.opens.tick(&mut app.session);
        let ended = app.session.take_saved();
        app.saves.tick(&mut app.session, ended.as_ref());
        started.elapsed()
    }

    #[test]
    fn an_agents_open_runs_behind_the_bar_and_answers_when_in_or_cancelled() {
        let r = Registry::new();
        // A project of three tracks, each a clip and a plug-in, saved (headless: at once).
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../out/check/test-agent-open");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut maker = headless();
        let mut host = SessionHost(&mut maker);
        let made = r
            .call_now(
                &mut host,
                "new_project",
                json!({"name": "Agent", "folder": dir}),
            )
            .unwrap();
        let ActionOutput::Json(made) = made else {
            panic!()
        };
        let path = std::path::PathBuf::from(made["path"].as_str().unwrap());
        for k in 0..3 {
            let file = dir.join(format!("take {k}.wav"));
            let samples = (0..24_000).map(|i| 0.1 * (i as f32 * 0.01).sin()).collect();
            let audio = daw_core::wav::Audio {
                sample_rate: 48_000,
                channels: vec![samples],
            };
            daw_core::wav::write_f32(&file, &audio).unwrap();
            let Ok(ActionOutput::Json(track)) =
                r.call_now(&mut host, "add_track", json!({"kind": "audio"}))
            else {
                panic!()
            };
            let track = track["track"].clone();
            let import = json!({"track": track, "path": file, "position": 0});
            r.call_now(&mut host, "import_audio", import).unwrap();
            let insert = json!({"track": track, "plugin": "amplify"});
            r.call_now(&mut host, "add_insert", insert).unwrap();
        }
        r.call_now(&mut host, "save_project", Value::Null).unwrap();

        // The app: opens are staged. The agent's open_project begins it and isn't answered.
        let mut session = headless();
        session.stage_loads = true;
        let mut app = App {
            session,
            opens: AgentOpens::default(),
            saves: AgentSaves::default(),
        };
        app.session.hold_loading(true);
        let answer = ask(&r, &mut app, "open_project", json!({"path": path}));
        assert!(app.session.loading().is_some(), "the open is staged");
        assert!(
            answer.try_recv().is_err(),
            "answered before anything loaded"
        );
        // The bar's first frames (held, as `hold` does until it is painted): no answer.
        for _ in 0..3 {
            frame(&mut app);
            assert!(answer.try_recv().is_err());
        }
        app.session.hold_loading(false);
        let mut frames = Vec::new();
        let opened = loop {
            assert!(
                frames.len() < 10_000,
                "the open never finished: {:?}",
                app.session.loading()
            );
            let took = frame(&mut app);
            frames.push(took);
            match answer.try_recv() {
                Ok(opened) => break opened.unwrap(),
                Err(_) => assert!(app.session.loading().is_some(), "done but unanswered"),
            }
        };
        // Answered as the last plug-in came in, with what the action returns.
        assert!(app.session.loading().is_none());
        let inserts: Vec<_> = app.session.project.all_inserts().map(|s| s.id).collect();
        assert_eq!(inserts.len(), 3);
        assert!(inserts
            .iter()
            .all(|id| app.session.insert_info(*id).is_some()));
        assert_eq!(
            opened["path"].as_str().map(std::path::PathBuf::from),
            Some(path.clone())
        );
        assert!(opened.get("newer_backup").is_some(), "{opened}");
        // The frames between never blocked on the whole open.
        let worst = frames.iter().max().copied().unwrap_or_default();
        assert!(worst < Duration::from_millis(500), "a frame took {worst:?}");

        // Cancelled from the bar while the agent waits: answered with an error at once.
        let answer = ask(&r, &mut app, "open_project", json!({"path": path}));
        assert!(app.session.loading().is_some());
        app.session.cancel_loading().unwrap();
        frame(&mut app);
        let cancelled = answer.try_recv().expect("answered at the cancel");
        let why = cancelled.unwrap_err().0;
        assert!(why.contains("cancelled"), "{why}");
        // Cancelled by the agent: the open's call errs, cancel_open answers once the project
        // open before is back.
        while app.session.loading().is_some() {
            frame(&mut app);
        }
        let answer = ask(&r, &mut app, "open_project", json!({"path": path}));
        let cancel = ask(&r, &mut app, "cancel_open", Value::Null);
        frame(&mut app);
        assert!(answer.try_recv().unwrap().is_err());
        let reopened = loop {
            if let Ok(reopened) = cancel.try_recv() {
                break reopened.unwrap();
            }
            assert!(app.session.loading().is_some(), "reopened but unanswered");
            frame(&mut app);
        };
        assert_eq!(
            reopened["reopened"].as_str().map(std::path::PathBuf::from),
            Some(path)
        );
        assert!(app.session.loading().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 20.1: an agent's save_project runs as Ctrl+S does, behind the bar (its reads held until
    /// the bar is painted, other calls held meanwhile by `Mcp::take_calls`), and is answered once
    /// the file is written, with the path; a save that fails is answered with why.
    #[test]
    fn an_agents_save_answers_once_the_file_is_written() {
        let r = Registry::new();
        let dir = std::env::temp_dir().join(format!("oxide-agent-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App {
            session: headless(),
            opens: AgentOpens::default(),
            saves: AgentSaves::default(),
        };
        let made = ask(
            &r,
            &mut app,
            "new_project",
            json!({"name": "Saved", "folder": dir}),
        );
        let path = made.try_recv().unwrap().unwrap()["path"].clone();
        let added = ask(&r, &mut app, "add_track", json!({"kind": "audio"}));
        let track = added.try_recv().unwrap().unwrap()["track"].clone();
        let args = json!({"track": track, "plugin": "amplify"});
        ask(&r, &mut app, "add_insert", args)
            .try_recv()
            .unwrap()
            .unwrap();

        app.session.hold_saving(true);
        let answer = ask(&r, &mut app, "save_project", Value::Null);
        assert!(app.session.saving().is_some(), "the save is staged");
        for _ in 0..3 {
            frame(&mut app);
            assert!(answer.try_recv().is_err(), "answered before it was written");
        }
        assert_eq!(app.session.save_progress().map(|p| p.done), Some(0));
        app.session.hold_saving(false);
        let saved = answered(&mut app, &answer);
        assert_eq!(saved["path"], path);
        assert!(app.session.saving().is_none());
        assert!(!app.session.has_unsaved_changes());

        // Somewhere it can't be written (a folder in the way): answered with why.
        let blocked = dir.join("Blocked.oxide");
        std::fs::create_dir_all(&blocked).unwrap();
        let answer = ask(&r, &mut app, "save_project", json!({"path": blocked}));
        let failed = (0..10_000)
            .find_map(|_| {
                frame(&mut app);
                answer.try_recv().ok()
            })
            .expect("never answered");
        let why = failed.unwrap_err().0;
        assert!(why.contains("save failed"), "{why}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 17.1's guarantee kept through 20.1: an open that begins as the look for unsaved changes
    /// (or a save) ends, its bar already painted many times for that, still closes no plug-in of
    /// the last project and loads none until its own bar has been painted twice.
    #[test]
    fn an_open_after_a_save_waits_for_its_own_bar() {
        let r = Registry::new();
        let dir = std::env::temp_dir().join(format!("oxide-bar-after-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut app = App {
            session: headless(),
            opens: AgentOpens::default(),
            saves: AgentSaves::default(),
        };
        let made = ask(
            &r,
            &mut app,
            "new_project",
            json!({"name": "Bar", "folder": dir}),
        );
        let path = made.try_recv().unwrap().unwrap()["path"].clone();
        let path = std::path::PathBuf::from(path.as_str().unwrap());
        let added = ask(&r, &mut app, "add_track", json!({"kind": "audio"}));
        let track = added.try_recv().unwrap().unwrap()["track"].clone();
        for _ in 0..3 {
            let args = json!({"track": track, "plugin": "amplify"});
            ask(&r, &mut app, "add_insert", args)
                .try_recv()
                .unwrap()
                .unwrap();
        }
        app.session.stage_loads = true;
        let mut bar = BarPainted::default();
        // One frame as the app runs it: hold, update, then the bar painted if anything is busy.
        let frame = |app: &mut App, bar: &mut BarPainted| {
            hold_session(bar, &mut app.session, false, false);
            app.session.update();
            if app.session.loading().is_some() {
                bar.painted(BarFor::Loading);
            } else if app.session.save_progress().is_some() {
                bar.painted(BarFor::Saving);
            }
        };
        // A save behind the bar, painted many times.
        app.session.save_in_background(&path).unwrap();
        bar.painted(BarFor::Saving);
        let mut frames = 0;
        while app.session.saving().is_some() {
            frame(&mut app, &mut bar);
            frames += 1;
            assert!(frames < 10_000, "the save never finished");
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(frames >= 3, "{frames}");
        // The open begins in the frame the save ended, and its bar is painted in that frame.
        app.session.open(&path).unwrap();
        bar.painted(BarFor::Loading);
        let closing = app.session.loading().expect("staged");
        assert_eq!(closing.stage, LoadStage::Closing);
        assert_eq!(closing.stage_total, 3);
        // Painted once: the next frame holds every plug-in (none closed or loaded).
        frame(&mut app, &mut bar);
        let held = app.session.loading().expect("held");
        assert_eq!((held.stage, held.stage_done), (LoadStage::Closing, 0));
        // Painted twice: then they go.
        while app.session.loading().is_some() {
            frame(&mut app, &mut bar);
        }
        assert_eq!(app.session.project.all_inserts().count(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The answer to `answer`, waiting frames for it (no later than 10 000 frames).
    fn answered(app: &mut App, answer: &Receiver<ApiResult<Value>>) -> Value {
        for _ in 0..10_000 {
            if let Ok(done) = answer.try_recv() {
                return done.unwrap();
            }
            frame(app);
        }
        panic!("never answered: {:?}", app.session.loading());
    }

    /// 18.3: an agent's undo that brings a plug-in back, and its new sample rate, are answered
    /// once the plug-ins are in (staged behind the bar as an open is, held until it is painted),
    /// not before; neither is an open (the window stays as it is, under the bar).
    #[test]
    fn an_agents_undo_and_rate_change_answer_once_the_plug_ins_are_in() {
        use daw_core::session::LoadKind;
        let r = Registry::new();
        let mut session = headless();
        session.stage_loads = true;
        let mut app = App {
            session,
            opens: AgentOpens::default(),
            saves: AgentSaves::default(),
        };
        let added = ask(&r, &mut app, "add_track", json!({"kind": "audio"}));
        let track = added.try_recv().unwrap().unwrap()["track"].clone();
        let args = json!({"track": track, "plugin": "amplify"});
        let added = ask(&r, &mut app, "add_insert", args);
        let insert = added.try_recv().unwrap().unwrap()["insert"]
            .as_u64()
            .unwrap() as u32;
        let removed = ask(&r, &mut app, "remove_insert", json!({"insert": insert}));
        removed.try_recv().unwrap().unwrap();
        frame(&mut app);

        app.session.hold_loading(true);
        let answer = ask(&r, &mut app, "undo", Value::Null);
        let loading = app.session.loading().expect("staged");
        assert_eq!(loading.kind, LoadKind::Undo);
        assert!(!loading.opens());
        for _ in 0..3 {
            frame(&mut app);
            assert!(answer.try_recv().is_err(), "answered before it loaded");
        }
        app.session.hold_loading(false);
        let undone = answered(&mut app, &answer);
        assert_eq!(undone["done"], "Remove insert", "{undone}");
        assert!(app.session.insert_info(insert).is_some());

        app.session.hold_loading(true);
        let answer = ask(
            &r,
            &mut app,
            "set_audio_settings",
            json!({"sample_rate": 44_100}),
        );
        let loading = app.session.loading().expect("staged");
        assert_eq!(loading.kind, LoadKind::SampleRate);
        for _ in 0..3 {
            frame(&mut app);
            assert!(answer.try_recv().is_err(), "answered before it loaded");
        }
        app.session.hold_loading(false);
        let settings = answered(&mut app, &answer);
        assert!(settings.is_object(), "{settings}");
        assert_eq!(app.session.sample_rate(), 44_100);
        assert!(app.session.insert_info(insert).is_some());
        assert!(app.session.loading().is_none());
    }
}
