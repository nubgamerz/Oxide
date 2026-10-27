//! The Diagnostics pane (dev plan 11.5b): Click Repair and
//! Clip Repair on the Waveform view's current file. Pick one and its settings, scan the
//! selection or the file, get a list of what was found; a row takes the Waveform view there,
//! and the chosen rows (or all) are repaired as one step of the file's history. The list is the
//! session's (`Session::find_audio_problems`, the same call as the action), so it goes stale on
//! its own when the file changes.

use crate::app::DawApp;
use crate::theme;
use daw_core::api::ViewMode;
use daw_core::diagnostics::{Problem, ProblemKind, Settings};
use daw_core::session::DocumentId;
use egui::{pos2, vec2, Align2, FontId, Sense};
use std::collections::BTreeSet;

/// A column to sort the list by.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Column {
    #[default]
    Time,
    Channel,
    Length,
    Level,
}

/// The pane's state: what the next scan looks for, and how the list is shown.
#[derive(Debug, Default)]
pub struct Panel {
    pub kind: ProblemKind,
    pub settings: Settings,
    /// The rows picked, by problem id, in `picked_in`'s list.
    pub picked: BTreeSet<u32>,
    picked_in: Option<DocumentId>,
    /// The row a Shift-click extends from.
    anchor: Option<u32>,
    sort: Column,
    descending: bool,
}

const ROW: f32 = 18.0;
/// The columns' left edges (points from the list's left).
const COLUMNS: [(Column, &str, f32); 4] = [
    (Column::Time, "Time", 4.0),
    (Column::Channel, "Ch", 92.0),
    (Column::Length, "Samples", 122.0),
    (Column::Level, "Level", 182.0),
];

/// "1:02.345": a position in the file at its own rate.
fn time(sample: u64, rate: u32) -> String {
    let seconds = sample as f64 / f64::from(rate.max(1));
    let minutes = (seconds / 60.0).floor();
    format!("{}:{:06.3}", minutes as u64, seconds - minutes * 60.0)
}

fn channel_name(channel: usize, channels: usize) -> String {
    match (channels, channel) {
        (2, 0) => "L".into(),
        (2, 1) => "R".into(),
        _ => (channel + 1).to_string(),
    }
}

/// Scan the current file's `range` (the whole file without one) with the pane's settings.
pub fn scan(app: &mut DawApp, id: DocumentId, range: Option<(u64, u64)>) {
    let (kind, settings) = (app.diagnostics.kind, app.diagnostics.settings);
    match app.session.find_audio_problems(id, kind, &settings, range) {
        Ok(found) => {
            app.diagnostics.picked.clear();
            app.diagnostics.picked_in = Some(id);
            app.diagnostics.anchor = None;
            let what = if range.is_some() {
                "the selection"
            } else {
                "the file"
            };
            app.toast(if found.is_empty() {
                format!("{}: nothing found in {what}", kind.name())
            } else {
                format!(
                    "{}: {} found in {what}",
                    kind.name(),
                    kind.noun(found.len())
                )
            });
        }
        Err(e) => app.toast(format!("{}: {e:#}", kind.name())),
    }
}

/// Repair `ids` (all of them without) in the current file.
fn repair(app: &mut DawApp, id: DocumentId, ids: Option<Vec<u32>>) {
    match app.session.repair_audio_problems(id, ids.as_deref()) {
        Ok(n) => {
            let kind = app.session.audio_problems(id).map(|(k, _)| k);
            app.diagnostics.picked.clear();
            app.diagnostics.anchor = None;
            if let Some(kind) = kind {
                app.toast(format!("Repaired {}", kind.noun(n)));
            }
        }
        Err(e) => app.toast(format!("Repair: {e:#}")),
    }
}

/// Take the Waveform view to `p`: selected (on its channel), the playhead at its start, zoomed
/// in far enough to see it (about 10 ms around it, a few points a sample) and centred.
pub fn go_to(app: &mut DawApp, id: DocumentId, p: &Problem) {
    let Some(doc) = app.session.document(id) else {
        return;
    };
    let (rate, len, channels) = (
        f64::from(doc.audio.sample_rate),
        doc.audio.len() as f64,
        doc.audio.channels.len(),
    );
    let width = f64::from(app.editor.width.max(100.0));
    app.view_mode = ViewMode::Waveform;
    app.editor.current = Some(id);
    let view = app.editor.view(id);
    view.selection = Some((p.start, p.start + p.length));
    view.channel = (channels > 1).then_some(p.channel);
    view.playhead = p.start;
    view.boxed = None;
    let around = (rate * 0.01).max(p.length as f64 * 8.0);
    if view.spp <= 0.0 || p.length as f64 / view.spp < 4.0 || view.spp * width > 4.0 * around {
        view.spp = (around / width).max(1.0 / 32.0);
    }
    let shown = width * view.spp;
    let centre = p.start as f64 + p.length as f64 / 2.0;
    view.start = (centre - shown / 2.0).clamp(0.0, (len - shown).max(0.0));
}

pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    let dim = |s: String| egui::RichText::new(s).small().color(theme::TEXT_DIM);
    // The kind and its settings.
    let kinds = ui.horizontal(|ui| {
        for kind in [ProblemKind::ClickRepair, ProblemKind::ClipRepair] {
            ui.selectable_value(&mut app.diagnostics.kind, kind, kind.name());
        }
    });
    // The manual's marks (27.5).
    crate::marks::put(ui, "diagnostics.kind", kinds.response.rect);
    let s = &mut app.diagnostics.settings;
    let settings = egui::Grid::new("diagnostics_settings")
        .num_columns(2)
        .spacing(vec2(8.0, 4.0))
        .show(ui, |ui| match app.diagnostics.kind {
            ProblemKind::ClickRepair => {
                ui.label("Threshold");
                ui.add(
                    egui::Slider::new(&mut s.threshold, 0.0..=100.0)
                        .max_decimals(1)
                        .suffix(" %"),
                )
                .on_hover_text("Higher finds fewer clicks: only the louder ones");
                ui.end_row();
                ui.label("Complexity");
                ui.add(
                    egui::Slider::new(&mut s.complexity, 0.0..=100.0)
                        .max_decimals(1)
                        .suffix(" %"),
                )
                .on_hover_text("Higher takes longer clicks, with a longer model");
                ui.end_row();
            }
            ProblemKind::ClipRepair => {
                ui.label("Tolerance");
                ui.add(
                    egui::Slider::new(&mut s.tolerance, 0.0..=50.0)
                        .max_decimals(1)
                        .suffix(" %"),
                )
                .on_hover_text(
                    "How far under the clip level (the scanned range's highest and lowest \
                         sample) a sample still counts as clipped",
                );
                ui.end_row();
                ui.label("Min. clip size");
                ui.add(
                    egui::Slider::new(&mut s.min_clip_size, 3.0..=20.0)
                        .step_by(1.0)
                        .suffix(" samples"),
                )
                .on_hover_text("The shortest run of clipped samples taken for clipping");
                ui.end_row();
            }
        });
    crate::marks::put(ui, "diagnostics.settings", settings.response.rect);

    let Some(id) = app
        .editor
        .current
        .filter(|id| app.session.document(*id).is_some())
    else {
        ui.add_space(6.0);
        ui.label(dim("Open a file in the Waveform view to scan it".into()));
        return;
    };
    let selection = app.editor.view(id).selection;
    let scans = ui.horizontal(|ui| {
        let by = ui
            .add_enabled(selection.is_some(), egui::Button::new("Scan selection"))
            .on_hover_text("Look for problems in the Waveform view's selection")
            .on_disabled_hover_text("Select part of the file first");
        if by.clicked() {
            let label = format!("Scanning the selection ({})", app.diagnostics.kind.name());
            crate::busy::run(app, label, move |app| scan(app, id, selection));
        }
        if ui
            .button("Scan file")
            .on_hover_text("Look for problems in the whole file")
            .clicked()
        {
            let label = format!("Scanning the file ({})", app.diagnostics.kind.name());
            crate::busy::run(app, label, move |app| scan(app, id, None));
        }
    });
    crate::marks::put(ui, "diagnostics.scan", scans.response.rect);

    // The list, the current file's.
    if app.diagnostics.picked_in != Some(id) {
        app.diagnostics.picked.clear();
        app.diagnostics.anchor = None;
        app.diagnostics.picked_in = Some(id);
    }
    let Some((kind, mut list)) = app.session.audio_problems(id) else {
        ui.add_space(4.0);
        ui.label(dim(
            "Scan to list what is found; an edit to the file clears the list".into(),
        ));
        return;
    };
    let doc = app.session.document(id).expect("checked");
    let (rate, channels) = (doc.audio.sample_rate, doc.audio.channels.len());
    let panel = &mut app.diagnostics;
    panel.picked.retain(|n| list.iter().any(|p| p.id == *n));
    let picked = panel.picked.len();
    ui.add_space(2.0);
    ui.label(if list.is_empty() {
        format!("{}: nothing left to repair", kind.name())
    } else if picked > 0 {
        format!("{} found, {picked} selected", kind.noun(list.len()))
    } else {
        format!("{} found", kind.noun(list.len()))
    });
    let mut chosen: Option<Option<Vec<u32>>> = None;
    let repairs = ui.horizontal(|ui| {
        if ui
            .add_enabled(picked > 0, egui::Button::new("Repair selected"))
            .on_hover_text("Rebuild the selected rows: one step of the file's history")
            .on_disabled_hover_text("Click rows to select them (Ctrl and Shift add more)")
            .clicked()
        {
            chosen = Some(Some(panel.picked.iter().copied().collect()));
        }
        if ui
            .add_enabled(!list.is_empty(), egui::Button::new("Repair all"))
            .on_hover_text("Rebuild every row: one step of the file's history")
            .clicked()
        {
            chosen = Some(None);
        }
    });
    crate::marks::put(ui, "diagnostics.repair", repairs.response.rect);

    // Sorted as asked (ties in time order).
    match panel.sort {
        Column::Time => {}
        Column::Channel => list.sort_by_key(|p| p.channel),
        Column::Length => list.sort_by_key(|p| p.length),
        Column::Level => list.sort_by(|a, b| a.level_db.total_cmp(&b.level_db)),
    }
    if panel.descending {
        list.reverse();
    }

    // The header: click a column to sort by it, again to reverse.
    ui.add_space(2.0);
    let (header, _) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::hover());
    ui.painter().rect_filled(header, 0.0, theme::BG);
    for (column, name, x) in COLUMNS {
        let at = pos2(header.left() + x, header.center().y);
        let mark = match (panel.sort == column, panel.descending) {
            (false, _) => "",
            (true, false) => " ▲",
            (true, true) => " ▼",
        };
        let text = format!("{name}{mark}");
        let galley_rect = ui.painter().text(
            at,
            Align2::LEFT_CENTER,
            text,
            FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
        let hit = ui.interact(
            galley_rect.expand(2.0),
            egui::Id::unique(("diagnostics_sort", name)),
            Sense::click(),
        );
        if hit.clicked() {
            if panel.sort == column {
                panel.descending = !panel.descending;
            } else {
                panel.sort = column;
                panel.descending = false;
            }
        }
    }

    let mut go: Option<Problem> = None;
    let text = ui.visuals().text_color();
    let rows = egui::ScrollArea::vertical()
        .auto_shrink([false, false])
        .show_rows(ui, ROW, list.len(), |ui, rows| {
            for k in rows {
                let p = &list[k];
                let (rect, response) =
                    ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
                let is_picked = panel.picked.contains(&p.id);
                let fill = if is_picked {
                    theme::ACCENT.gamma_multiply(0.35)
                } else if response.hovered() {
                    theme::LANE_SELECTED
                } else if k % 2 == 1 {
                    theme::LANE_ALT
                } else {
                    egui::Color32::TRANSPARENT
                };
                ui.painter().rect_filled(rect, 0.0, fill);
                let cells = [
                    time(p.start, rate),
                    channel_name(p.channel, channels),
                    p.length.to_string(),
                    format!("{:.1} dB", p.level_db),
                ];
                for ((_, _, x), cell) in COLUMNS.iter().zip(cells) {
                    ui.painter().text(
                        pos2(rect.left() + x, rect.center().y),
                        Align2::LEFT_CENTER,
                        cell,
                        FontId::monospace(12.0),
                        // White on a picked row's blue, as every selected item.
                        if is_picked {
                            egui::Color32::WHITE
                        } else {
                            text
                        },
                    );
                }
                if response.clicked() {
                    let modifiers = ui.input(|i| i.modifiers);
                    if modifiers.shift {
                        // From the anchor to here, in the order shown.
                        let from = panel
                            .anchor
                            .and_then(|a| list.iter().position(|q| q.id == a))
                            .unwrap_or(k);
                        let (lo, hi) = (from.min(k), from.max(k));
                        if !modifiers.command {
                            panel.picked.clear();
                        }
                        panel.picked.extend(list[lo..=hi].iter().map(|q| q.id));
                    } else if modifiers.command {
                        if !panel.picked.remove(&p.id) {
                            panel.picked.insert(p.id);
                        }
                        panel.anchor = Some(p.id);
                    } else {
                        panel.picked.clear();
                        panel.picked.insert(p.id);
                        panel.anchor = Some(p.id);
                    }
                    go = Some(p.clone());
                }
            }
        });
    crate::marks::put(ui, "diagnostics.list", rows.inner_rect);
    if let Some(p) = go {
        go_to(app, id, &p);
    }
    if let Some(ids) = chosen {
        repair(app, id, ids);
    }
}
