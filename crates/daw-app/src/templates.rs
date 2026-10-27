//! Project templates in the app: File > Save as template… (the project's setup under a name)
//! and File > Templates… (the saved ones: rename, delete). The start screen starts new
//! projects from them. The templates themselves are `daw_core::template`, in `Templates`
//! beside the program.

use crate::app::DawApp;
use crate::theme;
use egui::RichText;

/// File > Save as template…, while open.
pub struct SaveTemplate {
    name: String,
    /// The name is taken: asked whether to replace it.
    replacing: bool,
}

impl SaveTemplate {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            replacing: false,
        }
    }
}

/// File > Templates…, while open: the template being renamed and its new name.
#[derive(Default)]
pub struct TemplatesWindow {
    renaming: Option<(String, String)>,
}

/// File > Save as template…
pub fn save_window(app: &mut DawApp, ctx: &egui::Context) {
    let Some(dialog) = app.save_template.as_mut() else {
        return;
    };
    let taken = |name: &str, app: &DawApp| {
        let file = daw_core::folder::file_name(name);
        app.session
            .templates()
            .iter()
            .any(|t| t.name.eq_ignore_ascii_case(&file))
    };
    let mut save = None;
    let mut cancel = false;
    crate::theme::modal(ctx, "save-template").show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading("Save as template");
        ui.add(
            egui::Label::new(theme::hint(
                "The project's tracks, folders, buses, routing, sends, mixer and plug-ins with \
                 their settings, the tempo, time signature, loudness target and markers. No \
                 clips or recordings. New projects can start from it.",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        let edit = theme::setting_row(ui, 50.0, "Name", "", |ui| {
            ui.add(egui::TextEdit::singleline(&mut dialog.name).desired_width(360.0))
        });
        if edit.changed() {
            dialog.replacing = false;
        }
        let ready = !dialog.name.trim().is_empty();
        ui.add_space(4.0);
        if dialog.replacing {
            ui.label(
                RichText::new(format!(
                    "A template called \"{}\" exists already. Replace it?",
                    dialog.name.trim()
                ))
                .color(theme::RECORD),
            );
        }
        theme::button_row(ui, |ui| {
            let label = if dialog.replacing { "Replace" } else { "Save" };
            let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(ready, theme::primary_button(label))
                .clicked()
                || (ready && enter)
            {
                save = Some(dialog.replacing);
            }
            cancel = ui.button("Cancel").clicked();
        });
    });
    let Some(replace) = save else {
        if cancel {
            app.save_template = None;
        }
        return;
    };
    let name = app
        .save_template
        .as_ref()
        .map(|d| d.name.trim().to_string())
        .unwrap_or_default();
    if !replace && taken(&name, app) {
        if let Some(d) = app.save_template.as_mut() {
            d.replacing = true;
        }
        return;
    }
    match app.session.save_template(&name, replace) {
        Ok(path) => {
            app.toast(format!("Saved the template {}", path.display()));
            app.save_template = None;
        }
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// File > Templates…
pub fn window(app: &mut DawApp, ctx: &egui::Context) {
    let Some(state) = app.templates_window.as_mut() else {
        return;
    };
    let templates = app.session.templates();
    let mut open = true;
    let mut delete = None;
    let mut rename = None;
    let shown = theme::dialog_window("Templates", ctx)
        .open(&mut open)
        .default_width(420.0)
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.add(
                egui::Label::new(theme::hint(
                    "Start a new project from one on the start screen (File > New project). \
                     Save one with File > Save as template….",
                ))
                .wrap(),
            );
            ui.separator();
            if templates.is_empty() {
                ui.label(theme::hint("No templates yet."));
            }
            egui::Grid::new("templates")
                .num_columns(3)
                .striped(true)
                .spacing([12.0, 4.0])
                .show(ui, |ui| {
                    for t in &templates {
                        match &mut state.renaming {
                            Some((from, to)) if *from == t.name => {
                                let edit =
                                    ui.add(egui::TextEdit::singleline(to).desired_width(200.0));
                                let enter = edit.lost_focus()
                                    && ui.input(|i| i.key_pressed(egui::Key::Enter));
                                if ui.button("Rename").clicked() || enter {
                                    rename = Some((from.clone(), to.clone()));
                                }
                            }
                            _ => {
                                ui.label(&t.name)
                                    .on_hover_text(t.path.display().to_string());
                                if ui.button("Rename…").clicked() {
                                    state.renaming = Some((t.name.clone(), t.name.clone()));
                                }
                            }
                        }
                        if ui.button("Delete").clicked() {
                            delete = Some(t.name.clone());
                        }
                        ui.end_row();
                    }
                });
        });
    if let Some(shown) = &shown {
        crate::marks::put_rect("templates", shown.response.rect);
    }
    if !open {
        app.templates_window = None;
        return;
    }
    if let Some((from, to)) = rename {
        let done = app.session.rename_template(&from, &to).map(|_| ());
        if done.is_ok() {
            if let Some(w) = app.templates_window.as_mut() {
                w.renaming = None;
            }
        }
        app.report(done);
    }
    if let Some(name) = delete {
        let done = app.session.delete_template(&name);
        if done.is_ok() {
            app.toast(format!("Deleted the template \"{name}\""));
        }
        app.report(done);
    }
}
