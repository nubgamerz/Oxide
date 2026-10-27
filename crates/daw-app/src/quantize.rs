//! Audio quantize (dev plan 8.6): slice a clip at its transients, then pull the slices to the
//! grid. Both windows call the session methods the `detect_transients`, `split_clip_at` and
//! `quantize_clips` actions call.

use crate::app::DawApp;
use crate::arrange::TimeAxis;
use crate::theme;
use daw_core::grid::Division;
use daw_core::project::ClipId;
use egui::{Rect, RichText, Stroke};

/// The grid choices offered, by label (as `Division` parses them).
pub const DIVISIONS: [&str; 8] = ["bar", "beat", "1/4", "1/8", "1/16", "1/32", "1/8t", "1/16t"];

/// The slicer window: a clip and the transients its settings find, shown on the clip.
pub struct Slicer {
    pub clip: ClipId,
    pub sensitivity: f32,
    pub min_gap_ms: f32,
    /// Timeline positions, for the settings in `found_for`.
    pub transients: Vec<u64>,
    found_for: Option<(f32, f32)>,
}

/// The quantize window's settings, kept between uses.
pub struct QuantizeOptions {
    pub open: bool,
    pub division: &'static str,
    /// Percent: 100 puts each slice on the grid, 50 halfway.
    pub strength: f32,
    pub fill_gaps: bool,
    /// Milliseconds of crossfade between neighbouring slices (0: hard cuts).
    pub crossfade_ms: f32,
}

impl Default for QuantizeOptions {
    fn default() -> Self {
        Self {
            open: false,
            division: "1/16",
            strength: 100.0,
            fill_gaps: true,
            crossfade_ms: 5.0,
        }
    }
}

/// Open the slicer on an audio clip.
pub fn open_slicer(app: &mut DawApp, clip: ClipId) {
    app.slicer = Some(Slicer {
        clip,
        sensitivity: 0.5,
        min_gap_ms: 50.0,
        transients: Vec::new(),
        found_for: None,
    });
}

/// The selected audio clips (quantize moves audio clips; MIDI has quantize_notes).
fn selected_audio(app: &DawApp) -> Vec<ClipId> {
    app.selected_clip_ids()
        .into_iter()
        .filter(|&c| app.session.project.find_clip(c).is_some())
        .collect()
}

/// Open the quantize window for the selected audio clips.
pub fn open_quantize(app: &mut DawApp) {
    if selected_audio(app).is_empty() {
        app.toast("Select the audio clips to quantize (slice one at its transients first)");
        return;
    }
    app.quantize.open = true;
}

pub fn slicer_window(app: &mut DawApp, ctx: &egui::Context) {
    let Some(mut slicer) = app.slicer.take() else {
        return;
    };
    let Some(name) = app
        .session
        .project
        .find_clip(slicer.clip)
        .map(|(_, c)| c.name.clone())
    else {
        return;
    };
    // Found again when the settings change, but not mid-drag: a long clip takes a moment.
    let settled = !ctx.input(|i| i.pointer.any_down());
    let wanted = (slicer.sensitivity, slicer.min_gap_ms);
    if settled && slicer.found_for != Some(wanted) {
        match app
            .session
            .detect_transients(slicer.clip, wanted.0, wanted.1)
        {
            Ok(found) => slicer.transients = found.into_iter().map(|t| t.0).collect(),
            Err(e) => app.toast(format!("{e:#}")),
        }
        slicer.found_for = Some(wanted);
    }
    let (mut slice, mut close) = (false, false);
    let mut open = true;
    // A tool window (it stays open while the sensitivity is tried), under the menu bar.
    theme::dialog_window("Slice at transients", ctx)
        .open(&mut open)
        .show(ctx, |ui| {
            // A width of its own: the buttons' row goes to the right edge of it.
            ui.set_width(300.0);
            ui.label(RichText::new(&name).strong());
            egui::Grid::new("slicer").num_columns(2).show(ui, |ui| {
                ui.label("Sensitivity");
                ui.add(egui::Slider::new(&mut slicer.sensitivity, 0.0..=1.0).fixed_decimals(2))
                    .on_hover_text("Higher finds softer attacks");
                ui.end_row();
                ui.label("Shortest gap");
                ui.add(
                    egui::Slider::new(&mut slicer.min_gap_ms, 10.0..=500.0)
                        .suffix(" ms")
                        .fixed_decimals(0),
                )
                .on_hover_text("Two cuts are never closer than this");
                ui.end_row();
            });
            ui.label(theme::hint(format!(
                "{} transients, shown on the clip",
                slicer.transients.len()
            )));
            ui.add_space(6.0);
            theme::button_row(ui, |ui| {
                if ui
                    .add_enabled(
                        !slicer.transients.is_empty(),
                        theme::primary_button("Slice"),
                    )
                    .on_hover_text("Cut there, select the slices and open Quantize")
                    .clicked()
                {
                    slice = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
    if slice {
        let cut = app
            .session
            .project
            .split_clip_at(slicer.clip, &slicer.transients);
        if let Some((pieces, _)) = cut {
            app.session.mark_dirty();
            app.session.label_edit("Slice at transients", None);
            app.select_clips(&pieces);
            app.quantize.open = true;
        }
        return;
    }
    if !close && open {
        app.slicer = Some(slicer);
    }
}

pub fn quantize_window(app: &mut DawApp, ctx: &egui::Context) {
    if !app.quantize.open {
        return;
    }
    let clips = selected_audio(app);
    let (mut apply, mut close) = (false, false);
    let mut open = true;
    theme::dialog_window("Quantize", ctx)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.set_width(320.0);
            ui.label(format!(
                "{} selected audio clip{}",
                clips.len(),
                if clips.len() == 1 { "" } else { "s" }
            ));
            egui::Grid::new("quantize").num_columns(2).show(ui, |ui| {
                ui.label("Grid");
                egui::ComboBox::from_id_salt("quantize-grid")
                    .selected_text(app.quantize.division)
                    .show_ui(ui, |ui| {
                        for division in DIVISIONS {
                            ui.selectable_value(&mut app.quantize.division, division, division);
                        }
                    });
                ui.end_row();
                ui.label("Strength");
                ui.add(
                    egui::Slider::new(&mut app.quantize.strength, 0.0..=100.0)
                        .suffix(" %")
                        .fixed_decimals(0),
                )
                .on_hover_text("100 puts each clip's start on the grid; 50 moves it halfway");
                ui.end_row();
                ui.label("Crossfade");
                ui.add(
                    egui::Slider::new(&mut app.quantize.crossfade_ms, 0.0..=50.0)
                        .suffix(" ms")
                        .fixed_decimals(1),
                )
                .on_hover_text(
                    "Joins neighbouring slices over the quiet just before each hit, so moved \
                     slices don't click; 0 for hard cuts",
                );
                ui.end_row();
            });
            ui.add(crate::theme::checkbox(
                &mut app.quantize.fill_gaps,
                "Fill gaps",
            ))
            .on_hover_text(
                "A clip with a gap before it starts earlier, playing the audio before its \
                     hit, so no silence opens up",
            );
            ui.label(theme::hint("Set the tempo to the performance's first."));
            ui.add_space(6.0);
            theme::button_row(ui, |ui| {
                if ui
                    .add_enabled(!clips.is_empty(), theme::primary_button("Quantize"))
                    .clicked()
                {
                    apply = true;
                }
                if ui.button("Close").clicked() {
                    close = true;
                }
            });
        });
    if apply {
        let division: Result<Division, _> = app.quantize.division.parse();
        let done = division
            .map_err(|e| anyhow::anyhow!(e))
            .and_then(|division| {
                app.session.quantize_clips(
                    &clips,
                    division,
                    f64::from(app.quantize.strength) / 100.0,
                    app.quantize.fill_gaps,
                    f64::from(app.quantize.crossfade_ms),
                )
            });
        match done {
            Ok(moves) => {
                app.session.label_edit("Quantize clips", None);
                app.toast(format!(
                    "Quantized {} clip{} to {}",
                    moves.len(),
                    if moves.len() == 1 { "" } else { "s" },
                    app.quantize.division
                ));
            }
            Err(e) => app.toast(format!("{e:#}")),
        }
    }
    if close || !open {
        app.quantize.open = false;
    }
}

/// The slicer's transients as lines over its clip, so the cuts are seen before they're made.
pub fn draw_transients(
    app: &DawApp,
    painter: &egui::Painter,
    clip: ClipId,
    rect: Rect,
    axis: TimeAxis,
) {
    let Some(slicer) = app.slicer.as_ref().filter(|s| s.clip == clip) else {
        return;
    };
    // Below the clip's title strip, so its name stays readable.
    let below = (rect.top() + 14.0)..=rect.bottom();
    for &at in &slicer.transients {
        let x = axis.x(at as f64);
        if rect.x_range().contains(x) {
            painter.vline(x, below.clone(), Stroke::new(1.5, theme::PLAYHEAD));
        }
    }
}
