//! Busy work shows (dev plan 15.2): one overlay for every job that can take more than a
//! moment, over a window that keeps painting and takes no clicks meanwhile.
//!
//! - **Opening a project** (17.1): over an empty window (`DawApp::frame_ui`), the last
//!   project's plug-ins closing, the files read on workers, then the plug-ins loading a few a
//!   frame (`Session::update`), with one bar over all of it and what it is on ("Reading
//!   Guitar DI.wav (3 of 12)", "Loading Amp Sim on Bass (2 of 6)"): the plug-in named is
//!   the one a frame may wait on (a VST3 plug-in must be created on the main thread). Cancel
//!   goes back to where it was (`opening::cancel`).
//! - **The effects rack coming back** (18.2): the last run's, or a preset loaded, its plug-ins
//!   loading the same way, one a frame, over the window as it is (dimmed) and with no Cancel
//!   (a few plug-ins; the project stays open).
//! - **An undo or redo bringing plug-ins back, and a new sample rate** (18.3): the same, over
//!   the window as it is, with no Cancel (the step is the project already; the device runs at
//!   the new rate already). "Undoing Remove insert", "Loading Amp Sim on Guitar L (1 of
//!   3)".
//! - **Saving** (20.1): "Saving Song", each plug-in's state read in a frame of its own once the
//!   bar has been painted (`opening::hold` holds them until then), the file written on a worker
//!   (`Session::save_in_background`), with a bar over the reads ("Reading Sampler on Piano (1
//!   of 5)", then "Writing Song.oxide"). The "Save the changes?" question's look for changes
//!   reads them the same way ("Looking for changes to Song"). An autosave reads silently.
//! - **A job of the app's** ([`run`]): adding, pasting or copying a plug-in. The overlay is
//!   painted first and the job runs two frames later, once it is on screen.

use crate::app::DawApp;
use crate::theme;
use egui::RichText;

/// A job waiting for the overlay to reach the screen.
pub struct Job {
    label: String,
    frames: u8,
    run: Box<dyn FnOnce(&mut DawApp)>,
}

/// Run `job` once "`label`…" is on screen.
pub fn run(app: &mut DawApp, label: impl Into<String>, job: impl FnOnce(&mut DawApp) + 'static) {
    app.busy = Some(Job {
        label: label.into(),
        frames: 0,
        run: Box::new(job),
    });
}

/// Whether something is busy: keys and other input wait meanwhile.
pub fn active(app: &DawApp) -> bool {
    app.busy.is_some() || app.session.loading().is_some() || app.session.save_progress().is_some()
}

/// A background save that ended (`saved`, from `Session::take_saved` this frame): said, and the
/// file remembered (housekeeping, every frame). Returns whether it was written, once it has
/// ended.
pub fn saved(app: &mut DawApp, saved: Option<&Result<std::path::PathBuf, String>>) -> Option<bool> {
    match saved? {
        Ok(path) => {
            app.toast(format!("Saved {}", path.display()));
            app.settings.add_recent(path.clone());
            app.save_settings();
            Some(true)
        }
        Err(e) => {
            app.toast_long(format!("SAVE FAILED, the project was not saved: {e}"));
            Some(false)
        }
    }
}

/// The overlay, and the app's job once it has been seen. Call every frame, after the panes.
pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    let saving = app.session.save_progress();

    let opening = app.session.loading();
    let opens = opening.as_ref().is_some_and(|l| l.opens());
    let (title, detail, progress) = if let Some(loading) = &opening {
        (
            title(app, loading),
            describe(loading),
            Some(loading.fraction()),
        )
    } else if let Some(save) = &saving {
        let name = &app.session.project.name;
        let title = match &save.path {
            Some(_) => format!("Saving {name}"),
            None => format!("Looking for changes to {name}"),
        };
        (title, describe_save(save), Some(save.fraction()))
    } else if let Some(job) = &app.busy {
        (job.label.clone(), String::new(), None)
    } else {
        return;
    };
    // Opening, the window behind is empty: nothing of the project shows until it is all in.
    let backdrop = if opens {
        theme::BG
    } else {
        egui::Color32::from_black_alpha(140)
    };
    let mut cancel = false;
    crate::theme::modal(ctx, "busy")
        .backdrop_color(backdrop)
        .show(ctx, |ui| {
            ui.set_width(360.0);
            ui.horizontal(|ui| {
                ui.add(egui::Spinner::new().size(22.0));
                ui.vertical(|ui| {
                    ui.label(RichText::new(&title).strong());
                    if !detail.is_empty() {
                        ui.label(RichText::new(&detail).color(theme::TEXT_DIM));
                    }
                });
            });
            if let Some(p) = progress {
                ui.add_space(6.0);
                theme::progress(ui, p, false);
            }
            if opens {
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    cancel = ui.button("Cancel").clicked();
                });
            }
        });
    ctx.request_repaint();
    app.busy_frames += 1;
    if opening.is_some() || saving.is_some() {
        // Counted for what the bar is up for (`opening::hold`).
        let what = if opening.is_some() {
            crate::opening::BarFor::Loading
        } else {
            crate::opening::BarFor::Saving
        };
        app.bar_painted.painted(what);
        if let Some(check) = &mut app.open_check {
            check.bar = true;
        }
    }
    crate::frame_check::bar_painted(app);
    if cancel {
        crate::opening::cancel(app);
    }
    // The app's job, once the overlay has been painted twice (one frame isn't always enough
    // on Windows for it to reach the screen before the job blocks).
    if let Some(mut job) = app.busy.take() {
        if job.frames < 2 {
            job.frames += 1;
            app.busy = Some(job);
        } else {
            (job.run)(app);
        }
    }
}

/// What the plug-ins load for: "Opening Song", "Loading the effects rack", "Undoing Remove
/// insert", "Changing the sample rate to 44.1 kHz".
fn title(app: &DawApp, loading: &daw_core::session::LoadProgress) -> String {
    use daw_core::session::LoadKind;
    let step = loading.label.as_deref().unwrap_or("the last edit");
    match loading.kind {
        LoadKind::Open => format!("Opening {}", app.session.project.name),
        LoadKind::Rack => "Loading the effects rack".to_string(),
        LoadKind::Undo => format!("Undoing {step}"),
        LoadKind::Redo => format!("Redoing {step}"),
        LoadKind::SampleRate => format!(
            "Changing the sample rate to {} kHz",
            f64::from(app.session.sample_rate()) / 1000.0
        ),
    }
}

/// What a save is on: "Reading Sampler on Piano (1 of 5)", "Writing Song.oxide".
fn describe_save(save: &daw_core::session::SaveProgress) -> String {
    if save.writing {
        return match &save.path {
            Some(path) => format!(
                "Writing {}",
                path.file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
            ),
            None => "Comparing with the file".to_string(),
        };
    }
    let count = format!("({} of {})", (save.done + 1).min(save.total), save.total);
    match (&save.next, &save.track) {
        (Some(next), Some(track)) if !track.is_empty() => {
            format!("Reading {next} on {track} {count}")
        }
        (Some(next), _) => format!("Reading {next} {count}"),
        (None, _) => String::new(),
    }
}

/// What an open is on: "Closing Amp Sim (1 of 3)", "Reading Guitar DI.wav (3 of 12)",
/// "Loading Compressor on Bass (2 of 6)".
fn describe(loading: &daw_core::session::LoadProgress) -> String {
    use daw_core::session::LoadStage;
    let next = loading.next.clone().unwrap_or_default();
    let count = format!(
        "({} of {})",
        (loading.stage_done + 1).min(loading.stage_total),
        loading.stage_total
    );
    match (loading.stage, &loading.track) {
        (LoadStage::Loading, _) if loading.kind == daw_core::session::LoadKind::Rack => {
            format!("{next} {count}")
        }
        (LoadStage::Closing, _) => format!("Closing {next} {count}"),
        (LoadStage::Reading, _) => format!("Reading {next} {count}"),
        (LoadStage::Loading, Some(track)) => format!("Loading {next} on {track} {count}"),
        (LoadStage::Loading, None) => format!("Loading {next} {count}"),
    }
}
