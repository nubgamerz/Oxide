//! Copy and paste plug-ins (dev plan 15.1): a plug-in's menu (a track header's rack, the
//! mixer, the inspector) has Copy and Paste. The clipboard itself is the session's
//! (`Session::copy_plugin` / `paste_plugin`), so agents share it.

use crate::app::DawApp;
use daw_core::project::InsertId;
use daw_core::session::InsertTarget;

/// Copy insert `insert`, saying what was copied.
pub fn copy(app: &mut DawApp, insert: InsertId) {
    match app.session.copy_plugin(insert) {
        Ok(clip) => {
            let name = clip.name.clone();
            app.toast(format!("Copied {name}: right-click a slot to paste it"));
        }
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// Paste the copied plug-in into `target`'s chain at `position`, behind the busy overlay.
pub fn paste(app: &mut DawApp, target: InsertTarget, position: usize) {
    let name = app
        .session
        .plugin_clipboard()
        .map_or_else(String::new, |c| c.name.clone());
    crate::busy::run(app, format!("Pasting {name}"), move |app| {
        app.session.label_edit(&format!("Paste {name}"), None);
        let pasted = app.session.paste_plugin(target, position).map(|_| ());
        app.session.mark_dirty();
        app.report(pasted);
    });
}

/// Copy, and Paste after it, in a plug-in's menu (the mixer's and the inspector's lists).
pub fn menu_items(app: &mut DawApp, ui: &mut egui::Ui, target: InsertTarget, insert: InsertId) {
    if ui
        .button("Copy")
        .on_hover_text("Copy it with its settings, to paste on any track")
        .clicked()
    {
        copy(app, insert);
        ui.close();
    }
    let copied = app.session.plugin_clipboard().map(|c| c.name.clone());
    if let Some(name) = copied {
        if ui.button(format!("Paste {name} after")).clicked() {
            let after = app
                .session
                .insert_slots(target)
                .iter()
                .position(|s| s.id == insert)
                .map_or(usize::MAX, |i| i + 1);
            paste(app, target, after);
            ui.close();
        }
    }
}

/// Paste the copied plug-in at the end of `target`'s chain, from an "add plug-in" button's
/// right-click.
pub fn paste_menu(app: &mut DawApp, ui: &mut egui::Ui, target: InsertTarget) {
    let paste_name = app.session.plugin_clipboard().map(|c| c.name.clone());
    match paste_name {
        Some(name) => {
            if ui.button(format!("Paste {name}")).clicked() {
                paste(app, target, usize::MAX);
                ui.close();
            }
        }
        None => {
            ui.label("Nothing copied: right-click a plug-in > Copy");
        }
    }
}
