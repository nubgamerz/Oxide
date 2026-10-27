//! The plug-in browser: pick an installed VST3 for an insert slot.

use crate::app::DawApp;
use crate::theme;
use daw_core::plugins::{display_name, vendor};
use daw_core::project::InsertId;
use daw_core::session::InsertTarget;
use egui::RichText;
use std::path::PathBuf;

#[derive(Default)]
pub struct Browser {
    target: Option<InsertTarget>,
    /// Where in the chain the plug-in goes (an fx rack slot); `None`: by its kind.
    position: Option<usize>,
    /// The insert the chosen plug-in replaces (an fx rack slot's "Replace…").
    replacing: Option<InsertId>,
    search: String,
}

impl Browser {
    pub fn open_for(&mut self, target: InsertTarget) {
        self.open_at(target, None, None);
    }

    /// Open for one place in `target`'s chain, replacing `replacing` if given.
    pub fn open_at(
        &mut self,
        target: InsertTarget,
        position: Option<usize>,
        replacing: Option<InsertId>,
    ) {
        self.target = Some(target);
        self.position = position;
        self.replacing = replacing;
        self.search.clear();
    }
}

pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    let Some(target) = app.browser.target else {
        return;
    };
    let mut open = true;
    let owner = match target {
        InsertTarget::Master => "Master".to_string(),
        InsertTarget::Editor => "the Waveform view's rack".to_string(),
        InsertTarget::Track(id) => app
            .session
            .project
            .track(id)
            .map_or("track".into(), |t| t.name.clone()),
    };
    let replaced = app.browser.replacing.and_then(|old| {
        app.session
            .project
            .all_inserts()
            .chain(app.session.editor_rack())
            .find(|s| s.id == old)
            .map(|s| s.name.clone())
    });
    let title = match (replaced, app.browser.position) {
        (Some(old), _) => format!("Replace {old} on {owner}"),
        (None, Some(at)) => format!("Add plug-in to {owner}, slot {}", at + 1),
        (None, None) => format!("Add plug-in to {owner}"),
    };
    let mut chosen = None;
    // A resizable tool window, under the menu bar like the others.
    let shown = theme::below_menu_window(egui::Window::new(title), ctx)
        .open(&mut open)
        .collapsible(false)
        .default_size([420.0, 520.0])
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut app.browser.search)
                        .hint_text("Search plug-ins")
                        .desired_width(300.0),
                );
                search.request_focus();
                crate::marks::put(ui, "browser.search", search.rect);
                let rescan = ui.button("Rescan");
                crate::marks::put(ui, "browser.rescan", rescan.rect);
                if rescan.clicked() {
                    app.session.rescan_plugins();
                }
            });
            let needle = app.browser.search.to_lowercase();
            // The built-in effects first, then the VST3 plug-ins by vendor.
            let native: Vec<PathBuf> = daw_core::native::KINDS
                .iter()
                .map(|k| daw_core::native::path_of(k))
                .collect();
            let mut plugins: Vec<(String, String, PathBuf)> = native
                .iter()
                .chain(app.session.installed_plugins())
                .map(|p| (vendor(p), display_name(p), p.clone()))
                .filter(|(v, n, _)| {
                    needle.is_empty()
                        || n.to_lowercase().contains(&needle)
                        || v.to_lowercase().contains(&needle)
                })
                .collect();
            plugins.sort_by_key(|(vendor, name, _)| {
                (
                    vendor != daw_core::plugins::BUILT_IN,
                    vendor.to_lowercase(),
                    name.to_lowercase(),
                )
            });
            ui.label(theme::hint(format!(
                "{} plug-ins · built in and VST3",
                plugins.len()
            )));
            ui.separator();
            egui::ScrollArea::vertical()
                .auto_shrink(false)
                .show(ui, |ui| {
                    let mut current_vendor = String::new();
                    for (vendor, name, path) in &plugins {
                        if *vendor != current_vendor {
                            current_vendor = vendor.clone();
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new(vendor)
                                    .small()
                                    .strong()
                                    .color(theme::TEXT_DIM),
                            );
                        }
                        if ui
                            .add(egui::Button::selectable(false, format!("   {name}")))
                            .on_hover_text(path.display().to_string())
                            .clicked()
                        {
                            chosen = Some(path.clone());
                        }
                    }
                });
            if ui.input(|i| i.key_pressed(egui::Key::Enter)) && plugins.len() == 1 {
                chosen = Some(plugins[0].2.clone());
            }
        });
    if let Some(shown) = shown {
        crate::marks::put_rect("browser", shown.response.rect);
    }
    if let Some(path) = chosen {
        let (position, replacing) = (app.browser.position, app.browser.replacing);
        app.browser.target = None;
        let label = format!("Loading {}", display_name(&path));
        crate::busy::run(app, label, move |app| {
            add(app, target, &path, position, replacing);
        });
    } else if !open || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.browser.target = None;
    }
}

/// Load `path` into `target`'s chain at `position` (a busy job: the overlay is up), replacing
/// `replacing` if given.
fn add(
    app: &mut DawApp,
    target: InsertTarget,
    path: &std::path::Path,
    position: Option<usize>,
    replacing: Option<InsertId>,
) {
    match app.session.add_insert_at(target, path, position) {
        // A replacement goes in first, at the old one's place, so a failed load keeps it.
        Ok(id) => {
            if let Some(old) = replacing {
                app.session.remove_insert(old);
            }
            // An instrument picked for a MIDI track is offered next time without a scan.
            let on_midi_track = match target {
                InsertTarget::Track(t) => app
                    .session
                    .project
                    .track(t)
                    .is_some_and(|t| t.kind == daw_core::project::TrackKind::Instrument),
                InsertTarget::Master | InsertTarget::Editor => false,
            };
            if on_midi_track && app.session.is_instrument(id) {
                app.remember_instrument(path);
            }
        }
        Err(e) => app.toast(format!("{}: {e:#}", display_name(path))),
    }
}
