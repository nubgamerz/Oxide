//! Backups in the app: File > Open backup… (the project's backups, newest first), and the
//! question when a project opens with a backup newer than its file (Oxide closed without
//! saving, or crashed). Autosave itself runs in the session (`Session::update`).

use crate::app::DawApp;
use crate::theme;
use egui::RichText;
use std::path::PathBuf;
use std::time::SystemTime;

/// "2026-10-21 15:04" in local time.
fn when(t: SystemTime) -> String {
    chrono::DateTime::<chrono::Local>::from(t)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// "3 min ago", "2 h ago", "4 days ago".
fn ago(t: SystemTime) -> String {
    let secs = SystemTime::now()
        .duration_since(t)
        .map_or(0, |d| d.as_secs());
    match secs {
        0..60 => "just now".into(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86_400),
    }
}

/// File > Open backup…
pub fn window(app: &mut DawApp, ctx: &egui::Context) {
    if !app.show_backups {
        return;
    }
    let backups = app.session.backups();
    let mut open = true;
    let mut pick: Option<PathBuf> = None;
    let shown = theme::dialog_window("Open backup", ctx)
        .open(&mut open)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.set_width(460.0);
            ui.add(
                egui::Label::new(theme::hint(format!(
                    "Copies of the project written every {} while it has unsaved changes and \
                     playback is stopped; the newest {} are kept. Opening one replaces the \
                     open project until you save it.",
                    match app.settings.autosave_minutes {
                        0 => "few minutes (autosave is off now)".to_string(),
                        1 => "minute".to_string(),
                        n => format!("{n} minutes"),
                    },
                    app.settings.keep_backups,
                )))
                .wrap(),
            );
            ui.separator();
            if backups.is_empty() {
                ui.label(theme::hint("No backups yet."));
            }
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    egui::Grid::new("backups")
                        .num_columns(4)
                        .striped(true)
                        .spacing([12.0, 4.0])
                        .show(ui, |ui| {
                            for backup in &backups {
                                ui.label(RichText::new(when(backup.modified)).monospace());
                                ui.label(
                                    RichText::new(ago(backup.modified)).color(theme::TEXT_DIM),
                                );
                                ui.label(theme::hint(format!("{} KB", backup.size.div_ceil(1024))));
                                if ui
                                    .button("Open")
                                    .on_hover_text(backup.path.display().to_string())
                                    .clicked()
                                {
                                    pick = Some(backup.path.clone());
                                }
                                ui.end_row();
                            }
                        });
                });
        });
    if let Some(shown) = &shown {
        crate::marks::put_rect("backups", shown.response.rect);
    }
    app.show_backups = open;
    if let Some(path) = pick {
        open_backup(app, &path);
        app.show_backups = false;
    }
}

fn open_backup(app: &mut DawApp, path: &std::path::Path) {
    let path = path.to_path_buf();
    crate::unsaved::then(app, "opening the backup", move |app| {
        match app.session.open_backup(&path) {
            Ok(()) => {
                app.project_opened();
                app.toast("Opened the backup: save to keep it as the project");
            }
            Err(e) => app.toast(format!("{e:#}")),
        }
    });
}

/// The question when the project just opened has a newer backup.
pub fn question(app: &mut DawApp, ctx: &egui::Context) {
    let Some(path) = app.backup_question.clone() else {
        return;
    };
    let modified = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
    let mut answer = None;
    crate::theme::modal(ctx, "backup-question").show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading("A newer backup");
        ui.add_space(4.0);
        ui.label(format!(
            "A backup from {} is newer than the saved project: Oxide may have closed without \
             saving, or crashed. Open the backup instead?",
            modified.map_or_else(|| "earlier".into(), when)
        ));
        ui.add_space(8.0);
        theme::button_row(ui, |ui| {
            if ui.add(theme::primary_button("Open the backup")).clicked() {
                answer = Some(true);
            }
            if ui.button("Keep the saved project").clicked() {
                answer = Some(false);
            }
        });
    });
    match answer {
        Some(true) => {
            app.backup_question = None;
            open_backup(app, &path);
        }
        Some(false) => app.backup_question = None,
        None => {}
    }
}
