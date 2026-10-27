//! Save before closing (dev plan 15.3): closing the window, File > Exit, opening another
//! project (or a backup) and starting a new one ask first when something would be lost:
//! Save, Don't save, Cancel. Something is lost when the project differs from its file
//! (`Session::has_unsaved_changes`) or a file in the Waveform view has unsaved edits.
//!
//! The check runs in `logic`, outside the frame's edit: it reads every plug-in's state,
//! which must not become an undo step, and a close request is answered there (cancelled
//! until the question is settled) even while the window is minimised. The states are read a
//! plug-in a frame behind the busy bar ("Looking for changes to Song", 20.1:
//! `Session::check_unsaved`), so a slow one never freezes the window before the question. Save goes through the
//! background save and its overlay (15.2), and what was asked for happens once the file is
//! written; a save that fails asks again.

use crate::app::DawApp;
use crate::theme;
use daw_core::session::DocumentId;
use egui::RichText;

/// Something that would close the project, waiting on the question.
pub struct Pending {
    /// What it is, for the question: "closing", "opening Song", "starting a new project".
    what: String,
    then: Box<dyn FnOnce(&mut DawApp)>,
    stage: Stage,
}

#[derive(PartialEq)]
enum Stage {
    /// Not looked at yet (next `tick`, once nothing is busy).
    Check,
    /// The question is up: the project has changes (`project`), and these files too.
    Ask {
        project: bool,
        files: Vec<(DocumentId, String)>,
    },
    /// Save was chosen: waiting for the background save.
    Saving,
}

/// Do `then` once nothing unsaved would be lost: at once if nothing would, or after the
/// question. A second request while one waits is dropped (the first is still being asked).
pub fn then(app: &mut DawApp, what: impl Into<String>, then: impl FnOnce(&mut DawApp) + 'static) {
    if app.unsaved.is_some() {
        return;
    }
    app.unsaved = Some(Pending {
        what: what.into(),
        then: Box::new(then),
        stage: Stage::Check,
    });
}

/// Close the app, asking first if something would be lost (File > Exit, the window's close).
pub fn exit(app: &mut DawApp, ctx: &egui::Context) {
    let ctx = ctx.clone();
    then(app, "closing", move |app| {
        app.closing = true;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    });
}

/// Housekeeping, every frame (in `logic`): a close request held for the question, the check,
/// and a chosen save that has finished (`saved`: how, from `busy::saved`).
pub fn tick(app: &mut DawApp, ctx: &egui::Context, saved: Option<bool>) {
    if !app.closing && ctx.input(|i| i.viewport().close_requested()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        exit(app, ctx);
    }
    let Some(pending) = app.unsaved.as_mut() else {
        return;
    };
    match pending.stage {
        Stage::Check => {
            // A project still opening or being saved is looked at once that is done; so is one
            // whose Save As dialog is up (Save on a project without a file): chosen, the save
            // starts and is waited for here, then nothing is left to ask; cancelled, the
            // question comes back.
            if crate::busy::active(app) || crate::file_dialog::is_open(app) {
                return;
            }
            // Begun behind the bar (which makes the app busy until it answers).
            let Some(project) = app.session.check_unsaved() else {
                return;
            };
            let files: Vec<(DocumentId, String)> = app
                .session
                .documents()
                .iter()
                .filter(|d| d.is_dirty())
                .map(|d| {
                    let name = d.path.file_name().map_or_else(
                        || d.path.display().to_string(),
                        |n| n.to_string_lossy().into_owned(),
                    );
                    (d.id, name)
                })
                .collect();
            if project || !files.is_empty() {
                if let Some(pending) = app.unsaved.as_mut() {
                    pending.stage = Stage::Ask { project, files };
                }
            } else {
                go_on(app);
            }
        }
        Stage::Saving => match saved {
            Some(true) => go_on(app),
            // Said by `busy::saved`; the question again.
            Some(false) => pending.stage = Stage::Check,
            None => {}
        },
        Stage::Ask { .. } => {}
    }
}

/// What was asked for, now.
fn go_on(app: &mut DawApp) {
    if let Some(pending) = app.unsaved.take() {
        (pending.then)(app);
    }
}

/// The question, while it is up. Call every frame, after the panes.
pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    let Some(Pending {
        what,
        stage: Stage::Ask { project, files },
        ..
    }) = app.unsaved.as_ref()
    else {
        return;
    };
    let (project, files) = (*project, files.clone());
    let name = app.session.project.name.clone();
    let heading = if project {
        format!("Save the changes to {name} before {what}?")
    } else {
        format!("Save the edited files before {what}?")
    };
    let mut answer = None;
    let modal = crate::theme::modal(ctx, "unsaved").show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(heading);
        ui.add_space(4.0);
        let lost = if project {
            "Your changes to the project will be lost if you don't save them."
        } else {
            "Their edits will be lost if you don't save them."
        };
        ui.label(lost);
        if !files.is_empty() {
            ui.add_space(4.0);
            ui.label(if project {
                "Edited files (Waveform view), saved too:"
            } else {
                "Edited files (Waveform view):"
            });
            for (_, file) in files.iter().take(8) {
                ui.label(RichText::new(format!("• {file}")).color(theme::TEXT_DIM));
            }
        }
        ui.add_space(8.0);
        // Save on the right, as the dialog's main button; Cancel furthest from it.
        theme::button_row(ui, |ui| {
            if ui.add(theme::primary_button("Save")).clicked() {
                answer = Some(Answer::Save);
            }
            if ui.button("Don't save").clicked() {
                answer = Some(Answer::Discard);
            }
            if ui.button("Cancel").clicked() {
                answer = Some(Answer::Cancel);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(Answer::Cancel);
    }
    // The check's answer (`--answer`), as if clicked.
    if let Some(given) = app.answer.take() {
        answer = match given.as_str() {
            "save" => Some(Answer::Save),
            "discard" => Some(Answer::Discard),
            _ => Some(Answer::Cancel),
        };
    }
    match answer {
        Some(Answer::Save) => save(app, project, &files),
        Some(Answer::Discard) => go_on(app),
        Some(Answer::Cancel) => app.unsaved = None,
        None => {}
    }
}

enum Answer {
    Save,
    Discard,
    Cancel,
}

/// Save what has changes: the files at once, the project in the background (Save As's
/// dialog first if it has no file yet). Anything refused keeps the question up.
fn save(app: &mut DawApp, project: bool, files: &[(DocumentId, String)]) {
    for (id, _) in files {
        if let Err(e) = app.session.save_document(*id) {
            app.toast_long(format!("{e:#}"));
            return;
        }
    }
    if !project {
        go_on(app);
        return;
    }
    app.save_project(false);
    // Saving, or Save As's dialog up (the check waits for it, then for the save it starts), or
    // the save refused (with a toast): then the question again.
    let saving = app.session.saving().is_some();
    if let Some(pending) = app.unsaved.as_mut() {
        pending.stage = if saving { Stage::Saving } else { Stage::Check };
    }
}
