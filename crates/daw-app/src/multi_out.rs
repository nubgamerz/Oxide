//! "Create multi-out rack in a submix" (a rack slot's menu): which of a plug-in's extra
//! outputs get tracks of their own. A sampler may have 31, a drum kit uses a handful, so they are
//! chosen here rather than all made at once. The work is `Session::create_multi_out_rack`.

use crate::app::DawApp;
use crate::theme;
use daw_core::project::InsertId;
use daw_core::session::PluginOutput;

/// Outputs ticked when the dialog opens: all of a plug-in with this many or fewer.
const TICK_ALL_UP_TO: usize = 8;

pub struct Dialog {
    insert: InsertId,
    name: String,
    /// The extra outputs and whether each is ticked.
    outputs: Vec<(PluginOutput, bool)>,
}

impl Dialog {
    /// For insert `insert`; `None` (with a message) when it has no extra outputs to route.
    pub fn open(app: &mut DawApp, insert: InsertId) -> Option<Self> {
        let outputs = match app.session.plugin_outputs(insert) {
            Ok(o) => o,
            Err(e) => {
                app.toast(format!("{e:#}"));
                return None;
            }
        };
        let free: Vec<PluginOutput> = outputs
            .into_iter()
            .skip(1)
            .filter(|o| o.track.is_none())
            .collect();
        if free.is_empty() {
            app.toast("Every extra output of this plug-in has a track already");
            return None;
        }
        let few = free.len() <= TICK_ALL_UP_TO;
        let name = app
            .session
            .find_insert(insert)
            .map_or_else(String::new, |s| s.name.clone());
        Some(Self {
            insert,
            name,
            outputs: free
                .into_iter()
                .enumerate()
                .map(|(k, o)| (o, few || k < TICK_ALL_UP_TO - 1))
                .collect(),
        })
    }
}

/// A plug-in's menu entries for its multi-out rack, in the mixer's and the inspector's insert
/// lists (a track header's rack has the same in its own menu): shown only for a plug-in
/// with extra outputs on a track.
pub fn menu_items(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    target: daw_core::session::InsertTarget,
    insert: InsertId,
) {
    let (outputs, routed) = app.session.multi_out(insert);
    if outputs == 0 || !matches!(target, daw_core::session::InsertTarget::Track(_)) {
        return;
    }
    ui.separator();
    if ui.button("Create multi-out rack in a submix…").clicked() {
        app.multi_out = Dialog::open(app, insert);
        ui.close();
    }
    if routed > 0 && ui.button("Remove multi-out rack").clicked() {
        let removed = app.session.remove_multi_out_rack(insert).map(|_| ());
        app.report(removed);
        ui.close();
    }
}

pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    let Some(dialog) = app.multi_out.as_mut() else {
        return;
    };
    let mut create = false;
    let mut cancel = false;
    crate::theme::modal(ctx, "multi-out").show(ctx, |ui| {
        ui.set_width(420.0);
        ui.heading(format!("Multi-out rack: {}", dialog.name));
        ui.add(
            egui::Label::new(theme::hint(
                "Each output ticked gets an audio track in a folder under this one, with its \
                 own fader, pan, plug-ins and sends. Route the sounds to those outputs inside \
                 the plug-in (a sampler: each instrument's or drum's output); the main pair stays \
                 on this track.",
            ))
            .wrap(),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if ui.small_button("All").clicked() {
                dialog.outputs.iter_mut().for_each(|o| o.1 = true);
            }
            if ui.small_button("None").clicked() {
                dialog.outputs.iter_mut().for_each(|o| o.1 = false);
            }
        });
        egui::ScrollArea::vertical()
            .max_height(320.0)
            .show(ui, |ui| {
                for (output, on) in &mut dialog.outputs {
                    let shape = match output.channels {
                        1 => "mono".to_string(),
                        2 => "stereo".to_string(),
                        n => format!("{n} channels"),
                    };
                    ui.add(crate::theme::checkbox(
                        on,
                        format!("{}  ({shape})", output.name),
                    ));
                }
            });
        ui.add_space(6.0);
        let count = dialog.outputs.iter().filter(|o| o.1).count();
        theme::button_row(ui, |ui| {
            let label = format!("Create {count} track{}", if count == 1 { "" } else { "s" });
            create = ui
                .add_enabled(count > 0, theme::primary_button(&label))
                .clicked();
            cancel = ui.button("Cancel").clicked();
        });
    });
    if cancel {
        app.multi_out = None;
    }
    if !create {
        return;
    }
    let Some(dialog) = app.multi_out.take() else {
        return;
    };
    let buses: Vec<usize> = dialog
        .outputs
        .iter()
        .filter(|o| o.1)
        .map(|o| o.0.bus)
        .collect();
    app.session.label_edit("Create multi-out rack", None);
    match app
        .session
        .create_multi_out_rack(dialog.insert, Some(&buses))
    {
        Ok((folder, tracks)) => {
            app.session.mark_dirty();
            app.selection = Some(crate::app::Selection::Track(folder));
            app.toast(format!(
                "{} output track{} in \"{}\"",
                tracks.len(),
                if tracks.len() == 1 { "" } else { "s" },
                app.session
                    .project
                    .track(folder)
                    .map_or("", |t| t.name.as_str())
            ));
        }
        Err(e) => app.toast(format!("{e:#}")),
    }
}
