//! File dialogs that don't block the window (dev plan 18.6, TD20).
//!
//! Windows' file dialog runs a modal loop on the thread that shows it. Shown on the UI thread,
//! that froze the window and every MCP call (they run there) until it closed. Here the dialog is
//! built on the UI thread as before (filters, folder, file name, title), then shown on a thread
//! of its own; its answer comes back through a channel, picked up in `logic` ([`tick`]), and
//! what the caller wanted done with it runs at the start of the next frame's `ui`, inside the
//! frame's edit ([`run_ready`]), where the menu click used to run it, so a file imported or an
//! impulse loaded is still an undo step.
//!
//! The dialog is owned by Oxide's window (`set_parent`), so Windows keeps it in front of the
//! window and disables the window while it is up, as any modal dialog does: the window keeps
//! painting and answering agents, but takes no clicks or keys. That makes a second dialog
//! unreachable from the UI; one asked for anyway (from code) while one is up is refused with a
//! toast. Cancelling does nothing, as before: the caller's continuation only runs with a path.
//!
//! Agents keep working while a dialog is up, so the project may be replaced under it (an agent's
//! `open_project`). An answer given for the project that was open when the dialog came up is
//! dropped then, with a toast, rather than saving, importing into or exporting the other one.
//!
//! rfd's own async dialogs are, on Windows, exactly this (the blocking dialog on a spawned
//! thread) behind a future that would need an executor; the blocking dialog on our thread is the
//! same dialog without one. rfd initialises COM (apartment-threaded) on the thread that shows it.

use crate::app::DawApp;
use std::path::PathBuf;
use std::sync::mpsc;

/// What the dialog picks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pick {
    File,
    Files,
    Folder,
    Save,
}

/// What to do with the chosen paths (never called with none).
type Then<A> = Box<dyn FnOnce(&mut A, Vec<PathBuf>)>;

/// The dialog that is up, and what to do with its answer. Generic over the app so the
/// hand-over can be tested without a window.
pub struct Pending<A = DawApp> {
    answer: mpsc::Receiver<Option<Vec<PathBuf>>>,
    /// The answer, once it has come (`poll`); taken by `take_ready`.
    ready: Option<Option<Vec<PathBuf>>>,
    then: Then<A>,
    /// The project it was asked for (`Session::open_serial`).
    project: u64,
}

/// What became of an answered dialog.
enum Answered<A> {
    /// Paths chosen for the project still open: do this with them.
    Run(Then<A>, Vec<PathBuf>),
    /// Cancelled (or the dialog failed).
    Cancelled,
    /// Paths chosen, but another project was opened meanwhile.
    Stale,
}

/// Whether a file dialog is up (or its answer is waiting to be acted on).
pub fn is_open(app: &DawApp) -> bool {
    app.file_dialog.is_some()
}

/// Pick one file, then `then` with it (nothing on cancel).
pub fn pick_file(
    app: &mut DawApp,
    dialog: rfd::FileDialog,
    then: impl FnOnce(&mut DawApp, PathBuf) + 'static,
) {
    show(app, dialog, Pick::File, one(then));
}

/// Pick one or more files, then `then` with them (nothing on cancel).
pub fn pick_files(
    app: &mut DawApp,
    dialog: rfd::FileDialog,
    then: impl FnOnce(&mut DawApp, Vec<PathBuf>) + 'static,
) {
    show(app, dialog, Pick::Files, Box::new(then));
}

/// Pick a folder, then `then` with it (nothing on cancel).
pub fn pick_folder(
    app: &mut DawApp,
    dialog: rfd::FileDialog,
    then: impl FnOnce(&mut DawApp, PathBuf) + 'static,
) {
    show(app, dialog, Pick::Folder, one(then));
}

/// Choose a file to save, then `then` with it (nothing on cancel).
pub fn save_file(
    app: &mut DawApp,
    dialog: rfd::FileDialog,
    then: impl FnOnce(&mut DawApp, PathBuf) + 'static,
) {
    show(app, dialog, Pick::Save, one(then));
}

fn one<A: 'static>(then: impl FnOnce(&mut A, PathBuf) + 'static) -> Then<A> {
    Box::new(move |app, paths| {
        if let Some(path) = paths.into_iter().next() {
            then(app, path);
        }
    })
}

fn show(app: &mut DawApp, dialog: rfd::FileDialog, pick: Pick, then: Then<DawApp>) {
    let dialog = owned_by(dialog, app.hwnd);
    let wake = app.egui_ctx.clone();
    let ask = move || match pick {
        Pick::File => dialog.pick_file().map(|p| vec![p]),
        Pick::Files => dialog.pick_files(),
        Pick::Folder => dialog.pick_folder().map(|p| vec![p]),
        Pick::Save => dialog.save_file().map(|p| vec![p]),
    };
    let project = app.session.open_serial();
    if let Err(e) = start(&mut app.file_dialog, project, then, ask, move || {
        wake.request_repaint()
    }) {
        app.toast(e);
    }
}

/// Run `ask` (the dialog) on a thread of its own, its answer to come back into `slot`; `wake`
/// runs once it has answered. Refused while `slot` holds a dialog.
fn start<A>(
    slot: &mut Option<Pending<A>>,
    project: u64,
    then: Then<A>,
    ask: impl FnOnce() -> Option<Vec<PathBuf>> + Send + 'static,
    wake: impl FnOnce() + Send + 'static,
) -> Result<(), String> {
    if slot.is_some() {
        return Err("A file dialog is already open".into());
    }
    let (send, answer) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("file-dialog".into())
        .spawn(move || {
            let _ = send.send(ask());
            wake();
        })
        .map_err(|e| format!("Couldn't show the file dialog: {e}"))?;
    *slot = Some(Pending {
        answer,
        ready: None,
        then,
        project,
    });
    Ok(())
}

/// The dialog's answer, if it has come.
fn poll<A>(slot: &mut Option<Pending<A>>) {
    let Some(pending) = slot.as_mut() else {
        return;
    };
    if pending.ready.is_some() {
        return;
    }
    match pending.answer.try_recv() {
        Ok(paths) => pending.ready = Some(paths),
        Err(mpsc::TryRecvError::Empty) => {}
        // The dialog's thread ended without an answer (it panicked): as if cancelled.
        Err(mpsc::TryRecvError::Disconnected) => pending.ready = Some(None),
    }
}

/// An answered dialog leaves `slot` (`project`: the one open now); `None` while it is up.
fn take_ready<A>(slot: &mut Option<Pending<A>>, project: u64) -> Option<Answered<A>> {
    if slot.as_ref().is_none_or(|p| p.ready.is_none()) {
        return None;
    }
    let pending = slot.take()?;
    let paths = pending.ready.flatten().unwrap_or_default();
    Some(if paths.is_empty() {
        Answered::Cancelled
    } else if pending.project != project {
        Answered::Stale
    } else {
        Answered::Run(pending.then, paths)
    })
}

/// Housekeeping, every frame (in `logic`): the dialog's answer, once it has come.
pub fn tick(app: &mut DawApp) {
    poll(&mut app.file_dialog);
}

/// Act on an answer that has come (in `ui`, inside the frame's edit). Cancelled: nothing.
pub fn run_ready(app: &mut DawApp) {
    let project = app.session.open_serial();
    match take_ready(&mut app.file_dialog, project) {
        Some(Answered::Run(then, paths)) => then(app, paths),
        Some(Answered::Stale) => app.toast(
            "Another project was opened while the file dialog was up: nothing was done with \
             the file chosen",
        ),
        Some(Answered::Cancelled) | None => {}
    }
}

/// The dialog owned by Oxide's window, so it stays in front of it and the window waits for it.
#[cfg(windows)]
fn owned_by(dialog: rfd::FileDialog, hwnd: Option<isize>) -> rfd::FileDialog {
    match hwnd.and_then(std::num::NonZeroIsize::new) {
        Some(hwnd) => dialog.set_parent(&Owner(hwnd)),
        None => dialog,
    }
}

#[cfg(not(windows))]
fn owned_by(dialog: rfd::FileDialog, _hwnd: Option<isize>) -> rfd::FileDialog {
    dialog
}

/// Oxide's window as rfd takes an owner.
#[cfg(windows)]
struct Owner(std::num::NonZeroIsize);

#[cfg(windows)]
impl raw_window_handle::HasWindowHandle for Owner {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        let raw = raw_window_handle::Win32WindowHandle::new(self.0);
        // SAFETY: the handle is Oxide's main window (from eframe at start), which lives as long
        // as the app does; rfd only reads it, to pass to the dialog as its owner.
        Ok(unsafe { raw_window_handle::WindowHandle::borrow_raw(raw.into()) })
    }
}

#[cfg(windows)]
impl raw_window_handle::HasDisplayHandle for Owner {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Ok(raw_window_handle::DisplayHandle::windows())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Barrier};
    use std::time::{Duration, Instant};

    #[derive(Default)]
    struct App {
        got: Vec<PathBuf>,
        calls: usize,
    }

    fn record() -> Then<App> {
        Box::new(|app: &mut App, paths| {
            app.calls += 1;
            app.got = paths;
        })
    }

    fn run(answered: Option<Answered<App>>, app: &mut App) -> &'static str {
        match answered {
            Some(Answered::Run(then, paths)) => {
                then(app, paths);
                "run"
            }
            Some(Answered::Cancelled) => "cancelled",
            Some(Answered::Stale) => "stale",
            None => "up",
        }
    }

    /// Poll until the dialog's thread has answered (or a few seconds pass).
    fn answered(slot: &mut Option<Pending<App>>) {
        let until = Instant::now() + Duration::from_secs(5);
        while slot.as_ref().is_some_and(|p| p.ready.is_none()) && Instant::now() < until {
            poll(slot);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    #[test]
    fn a_dialog_answers_on_its_own_thread_and_its_paths_reach_the_continuation() {
        let mut slot = None;
        let mut app = App::default();
        // The "dialog" waits until the test says so: nothing is ready meanwhile, the slot is
        // held, and a second request is refused.
        let gate = Arc::new(Barrier::new(2));
        let held = gate.clone();
        let ui_thread = std::thread::current().id();
        let woken = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let wake = woken.clone();
        start(
            &mut slot,
            7,
            record(),
            move || {
                assert_ne!(std::thread::current().id(), ui_thread);
                held.wait();
                Some(vec![PathBuf::from("a.wav"), PathBuf::from("b.wav")])
            },
            move || wake.store(true, std::sync::atomic::Ordering::Relaxed),
        )
        .unwrap();
        poll(&mut slot);
        assert_eq!(run(take_ready(&mut slot, 7), &mut app), "up");
        assert!(slot.is_some(), "the dialog is still up");
        let second = start(&mut slot, 7, record(), || None, || {});
        assert_eq!(second.unwrap_err(), "A file dialog is already open");

        gate.wait();
        answered(&mut slot);
        assert_eq!(run(take_ready(&mut slot, 7), &mut app), "run");
        assert_eq!(app.calls, 1);
        assert_eq!(app.got, [PathBuf::from("a.wav"), PathBuf::from("b.wav")]);
        assert!(slot.is_none(), "the next dialog may open");
        assert!(woken.load(std::sync::atomic::Ordering::Relaxed));
    }

    #[test]
    fn a_cancelled_or_failed_dialog_runs_nothing_and_frees_the_slot() {
        // Cancelled, an empty pick, and a dialog thread that died.
        let asks: [Box<dyn FnOnce() -> Option<Vec<PathBuf>> + Send>; 3] = [
            Box::new(|| None),
            Box::new(|| Some(Vec::new())),
            Box::new(|| panic!("the dialog failed")),
        ];
        for ask in asks {
            let mut slot = None;
            let mut app = App::default();
            start(&mut slot, 1, record(), ask, || {}).unwrap();
            answered(&mut slot);
            assert_eq!(run(take_ready(&mut slot, 1), &mut app), "cancelled");
            assert_eq!(app.calls, 0);
            assert!(slot.is_none(), "the next dialog may open");
        }
    }

    #[test]
    fn an_answer_for_a_project_replaced_meanwhile_is_dropped() {
        let mut slot = None;
        let mut app = App::default();
        start(
            &mut slot,
            1,
            record(),
            || Some(vec!["a.oxide".into()]),
            || {},
        )
        .unwrap();
        answered(&mut slot);
        // An agent opened another project while the dialog was up.
        assert_eq!(run(take_ready(&mut slot, 2), &mut app), "stale");
        assert_eq!(app.calls, 0);
        assert!(slot.is_none(), "the next dialog may open");
    }

    #[test]
    fn one_path_continuations_get_the_first_path() {
        let mut app = App::default();
        let then = one(|app: &mut App, path| {
            app.calls += 1;
            app.got = vec![path];
        });
        then(&mut app, vec![PathBuf::from("x.oxide")]);
        assert_eq!(app.got, [PathBuf::from("x.oxide")]);
        assert_eq!(app.calls, 1);
    }
}
