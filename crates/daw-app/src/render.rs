//! Rendering a track, exporting its MIDI and switching tracks off, in the app: the track
//! menus' "Render track…", "Export MIDI…" and "Disable track" / "Enable track", and the
//! render's small dialog. The work is the session's (`Session::render_track`,
//! `export_midi_track`, `Track::enabled`).

use crate::app::{DawApp, Selection};
use crate::theme;
use daw_core::project::TrackId;

/// The render dialog's choices, while it is open.
pub struct RenderDialog {
    track: TrackId,
    tail_seconds: f64,
    disable: bool,
}

impl RenderDialog {
    /// The dialog for `track`, as the menu opens it.
    pub fn open(track: TrackId) -> Self {
        Self {
            track,
            tail_seconds: 2.0,
            disable: true,
        }
    }
}

/// "Render track…" and "Disable track" / "Enable track" for `id`, in a menu.
pub fn menu_items(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId) {
    let Some(track) = app.session.project.track(id) else {
        return;
    };
    let enabled = track.enabled;
    let has_midi = !track.midi_clips.is_empty();
    let name = track.name.clone();
    if has_midi
        && ui
            .button("Export MIDI…")
            .on_hover_text(
                "Save the track's MIDI clips as one MIDI file, with the project's tempo, for another program (the notes as programmed, nothing from plug-ins)",
            )
            .clicked()
    {
        ui.close();
        let dialog = app
            .dialog_in(Some(daw_core::folder::EXPORTED))
            .add_filter("MIDI file", &["mid"])
            .set_file_name(format!("{}.mid", daw_core::folder::file_name(&name)));
        crate::file_dialog::save_file(app, dialog, move |app, path| {
            match app.session.export_midi_track(id, &path) {
                Ok(()) => app.toast(format!("Saved {}", path.display())),
                Err(e) => app.toast(format!("{e:#}")),
            }
        });
    }
    if ui
        .button("Render track…")
        .on_hover_text(
            "Render the track with its plug-ins to a new audio track under it (and switch \
             this one off to save its CPU)",
        )
        .clicked()
    {
        app.render_dialog = Some(RenderDialog::open(id));
        ui.close();
    }
    let label = if enabled {
        "Disable track"
    } else {
        "Enable track"
    };
    if ui
        .button(label)
        .on_hover_text(
            "Off, the track and its plug-ins don't run at all (no CPU, unlike mute); its \
             settings are kept",
        )
        .clicked()
    {
        set_enabled(app, id, !enabled);
        ui.close();
    }
}

pub fn set_enabled(app: &mut DawApp, id: TrackId, enabled: bool) {
    if let Some(track) = app.session.project.track_mut(id) {
        track.enabled = enabled;
        app.session.mark_dirty();
    }
}

/// The render's dialog: the tail, and whether to switch the track off afterwards.
pub fn dialog(app: &mut DawApp, ctx: &egui::Context) {
    // The track a finished render made: select it.
    if let Some(new) = app.session.take_rendered_track() {
        app.selection = Some(Selection::Track(new));
    }
    let Some(state) = app.render_dialog.as_mut() else {
        return;
    };
    let Some(track) = app.session.project.track(state.track) else {
        app.render_dialog = None;
        return;
    };
    let name = track.name.clone();
    let muted = track.mixer.mute;
    let range = app.session.track_range(state.track);
    let mut open = true;
    let mut render = false;
    let mut cancel = false;
    let shown = theme::dialog_window(format!("Render {name}"), ctx)
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.add(
                egui::Label::new(theme::hint(
                    "Renders the track as you hear it (its clips or MIDI through its \
                     instrument, its plug-ins, fader, pan and automation, and a folder's \
                     tracks; not its sends, which go on the new track) into the project's \
                     Rendered folder, and puts it on a new track right under it.",
                ))
                .wrap(),
            );
            ui.add_space(6.0);
            theme::setting_row(
                ui,
                70.0,
                "Tail",
                "For reverb tails and note releases.",
                |ui| {
                    ui.horizontal(|ui| {
                        ui.add(
                            egui::DragValue::new(&mut state.tail_seconds)
                                .range(0.0..=30.0)
                                .speed(0.1)
                                .suffix(" s"),
                        );
                        ui.label("after the last clip");
                    });
                },
            );
            theme::setting_row(ui, 70.0, "Afterwards", "", |ui| {
                ui.add(crate::theme::checkbox(
                    &mut state.disable,
                    "Switch this track off (it then costs no CPU)",
                ));
            });
            if muted {
                ui.colored_label(theme::MUTE, "The track is muted: unmute it to render it.");
            } else if range.is_none() {
                ui.colored_label(theme::MUTE, "The track has no clips to render.");
            }
            ui.add_space(4.0);
            theme::button_row(ui, |ui| {
                render = ui
                    .add_enabled(!muted && range.is_some(), theme::primary_button("Render"))
                    .clicked();
                cancel = ui.button("Cancel").clicked();
            });
        });
    if let Some(shown) = &shown {
        crate::marks::put_rect("render", shown.response.rect);
    }
    if render {
        let (track, tail, disable) = (state.track, state.tail_seconds, state.disable);
        app.render_dialog = None;
        start(app, track, tail, disable);
    } else if !open || cancel {
        app.render_dialog = None;
    }
}

/// The dialog's Render: the render starts on export's thread (`Session::render_track`) and its
/// progress bar, with Cancel, shows from this frame on (`dialogs::export_progress`).
pub fn start(app: &mut DawApp, track: TrackId, tail_seconds: f64, disable: bool) {
    match app.session.render_track(track, tail_seconds, disable) {
        Ok(path) => app.toast(format!("Rendering to {}", path.display())),
        Err(e) => app.toast(format!("{e:#}")),
    }
}
