//! The master's loudness panel (dev plan 6.2): the EBU R 128 meter of `daw_core::loudness` as
//! EBU Mode shows it (Tech 3341 §2.7–2.8).
//!
//! - Momentary and short-term bars on the EBU +9 or +18 scale, laid around the project's
//!   loudness target (Tech 3341 sets 0 LU at -23 LUFS; here 0 LU is whichever target the
//!   project has, -14 for streaming by default).
//! - Integrated loudness, LRA (flagged as settling for its first 60 s), true peak (red above
//!   R 128's -1 dBTP), the maxima and the time measured. Every number has one decimal and its
//!   unit, LU from the target or LUFS as chosen.
//! - Reset, the target (presets or a value), the scale and the units.
//!
//! The scale and units are the user's preferences (`AppSettings`, and `set_view` for agents);
//! the target is the project's (`set_loudness_target`).

use crate::app::{DawApp, Selection};
use crate::theme;
use daw_core::api::LoudnessScale;
use daw_core::loudness::{Loudness, SILENCE};
use daw_core::project::{LOUDNESS_PRESETS, LOUDNESS_TARGET_RANGE};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke};

/// R 128's maximum permitted true peak: louder is shown in red.
pub const PEAK_CEILING: f64 = -1.0;

/// Where `level` (LUFS) sits on `scale` around `target`: 0 at the bottom, 1 at the top.
pub fn fraction(level: f64, target: f64, scale: LoudnessScale) -> f32 {
    let (low, high) = scale.range();
    (((level - target - low) / (high - low)) as f32).clamp(0.0, 1.0)
}

/// A loudness level as the panel shows it: one decimal and its unit, LU from `target` (signed)
/// or LUFS. Nothing measured yet is "–"; silence is "−∞".
pub fn format_level(level: Option<f64>, relative: bool, target: f64) -> String {
    let unit = if relative { "LU" } else { "LUFS" };
    match level {
        None => "–".into(),
        Some(l) if l <= SILENCE => format!("−∞ {unit}"),
        Some(l) if relative => format!("{:+.1} {unit}", l - target),
        Some(l) => format!("{l:.1} {unit}"),
    }
}

/// The loudness range, in LU.
pub fn format_range(range: Option<f64>) -> String {
    range.map_or("–".into(), |r| format!("{r:.1} LU"))
}

/// A true peak, in dBTP.
pub fn format_peak(peak: Option<f64>) -> String {
    match peak {
        None => "–".into(),
        Some(p) if p <= SILENCE => "−∞ dBTP".into(),
        Some(p) => format!("{p:.1} dBTP"),
    }
}

/// How long the integrated measurement has run, as m:ss.
fn format_time(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// The scale's tick spacing, in LU.
fn tick_step(scale: LoudnessScale) -> f64 {
    match scale {
        LoudnessScale::Ebu9 => 3.0,
        LoudnessScale::Ebu18 => 6.0,
    }
}

/// The momentary and short-term bars with their scale and the target line.
fn bars(ui: &mut egui::Ui, l: &Loudness, target: f64, scale: LoudnessScale, relative: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 190.0), Sense::hover());
    let painter = ui.painter_at(rect);
    let labels = 52.0;
    let area = Rect::from_min_max(
        pos2(rect.left() + labels, rect.top() + 6.0),
        pos2(rect.right() - 6.0, rect.bottom() - 18.0),
    );
    let y_of = |level: f64| area.bottom() - fraction(level, target, scale) * area.height();
    // The scale: a tick every 3 (or 6) LU, labelled in the chosen units.
    let (low, high) = scale.range();
    let step = tick_step(scale);
    let mut lu = low;
    while lu <= high + 1e-9 {
        let y = y_of(target + lu);
        painter.hline(
            (area.left() - 4.0)..=area.right(),
            y,
            Stroke::new(1.0, theme::GRID_BEAT),
        );
        let label = if relative {
            format!("{lu:+.0}")
        } else {
            format!("{:.0}", target + lu)
        };
        painter.text(
            pos2(area.left() - 8.0, y),
            Align2::RIGHT_CENTER,
            label,
            FontId::monospace(10.0),
            theme::TEXT_DIM,
        );
        lu += step;
    }
    // Two bars, momentary and short-term.
    let width = ((area.width() - 12.0) / 2.0).min(80.0);
    for (k, (name, level)) in [("M", l.momentary), ("S", l.short_term)]
        .into_iter()
        .enumerate()
    {
        let x = area.left() + 6.0 + k as f32 * (width + 12.0);
        let bar = Rect::from_min_max(pos2(x, area.top()), pos2(x + width, area.bottom()));
        painter.rect_filled(bar, 2, theme::BG);
        if let Some(level) = level {
            let top = y_of(level);
            let target_y = y_of(target);
            // Up to the target in green, past it in amber.
            painter.rect_filled(
                Rect::from_min_max(pos2(bar.left(), top.max(target_y)), bar.max),
                2,
                theme::METER_LOW,
            );
            if top < target_y {
                painter.rect_filled(
                    Rect::from_min_max(pos2(bar.left(), top), pos2(bar.right(), target_y)),
                    2,
                    theme::METER_MID,
                );
            }
        }
        painter.text(
            pos2(bar.center().x, area.bottom() + 9.0),
            Align2::CENTER_CENTER,
            name,
            FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    }
    // The target, across both bars.
    let y = y_of(target);
    painter.hline(
        (area.left() + 6.0)..=(area.left() + 2.0 * width + 18.0),
        y,
        Stroke::new(1.5, theme::ACCENT),
    );
}

/// The panel, in the master's inspector.
pub fn panel(app: &mut DawApp, ui: &mut egui::Ui) {
    let l = app.session.loudness();
    let target = app.session.project.loudness_target;
    let (scale, relative) = (app.settings.loudness_scale, app.settings.loudness_relative);

    // Integrated, large, with how far it is from the target.
    let top = ui.cursor().min;
    let integrated = ui.horizontal(|ui| {
        ui.label(RichText::new("Integrated").color(theme::TEXT_DIM));
        ui.label(
            RichText::new(format_level(l.integrated, relative, target))
                .size(20.0)
                .strong()
                .monospace(),
        )
        .on_hover_text("Gated programme loudness (EBU R 128), measured while playing");
    });
    // The manual's marks (27.5).
    crate::marks::put(ui, "loudness.integrated", integrated.response.rect);
    let before = ui.cursor().min.y;
    bars(ui, &l, target, scale, relative);
    let meters = egui::Rect::from_x_y_ranges(ui.min_rect().x_range(), before..=ui.cursor().min.y);
    crate::marks::put(ui, "loudness.meters", meters);

    let readings = egui::Grid::new("loudness-readings")
        .num_columns(2)
        .spacing([10.0, 3.0])
        .show(ui, |ui| {
            let row = |ui: &mut egui::Ui, name: &str, value: String, color: Color32, tip: &str| {
                ui.label(RichText::new(name).color(theme::TEXT_DIM))
                    .on_hover_text(tip);
                ui.label(RichText::new(value).monospace().color(color));
                ui.end_row();
            };
            let text = Color32::from_gray(0xdd);
            row(
                ui,
                "Short-term",
                format_level(l.short_term, relative, target),
                text,
                "3 s window",
            );
            row(
                ui,
                "Momentary",
                format_level(l.momentary, relative, target),
                text,
                "400 ms window",
            );
            row(
                ui,
                "Max short-term",
                format_level(l.max_short_term, relative, target),
                text,
                "Since reset, while playing",
            );
            row(
                ui,
                "Max momentary",
                format_level(l.max_momentary, relative, target),
                text,
                "Since reset, while playing",
            );
            let settling = l.range.is_some() && l.seconds < 60.0;
            let range = if settling {
                format!("{} (settling)", format_range(l.range))
            } else {
                format_range(l.range)
            };
            row(
                ui,
                "Loudness range",
                range,
                if settling { theme::TEXT_DIM } else { text },
                "LRA (EBU Tech 3342): not yet stable in the first 60 s",
            );
            let hot = l.true_peak.is_some_and(|p| p > PEAK_CEILING);
            row(
                ui,
                "True peak",
                format_peak(l.true_peak),
                if hot { theme::METER_HIGH } else { text },
                "Highest true peak since reset (4x oversampled); red above R 128's -1 dBTP",
            );
            row(
                ui,
                "Measured",
                format_time(l.seconds),
                text,
                "Time measured since reset, while playing",
            );
        });

    crate::marks::put(ui, "loudness.readings", readings.response.rect);
    ui.add_space(6.0);
    let buttons = ui.horizontal(|ui| {
        if ui
            .button("Reset")
            .on_hover_text("Start a new measurement: integrated, range, peak and maxima")
            .clicked()
        {
            app.session.reset_loudness();
        }
        let mut relative = app.settings.loudness_relative;
        let units = if relative { "Show LUFS" } else { "Show LU" };
        if ui
            .button(units)
            .on_hover_text("Readings in LU from the target, or in LUFS")
            .clicked()
        {
            relative = !relative;
            app.settings.loudness_relative = relative;
            app.save_settings();
        }
        let mut scale = app.settings.loudness_scale;
        egui::ComboBox::from_id_salt("loudness-scale")
            .selected_text(match scale {
                LoudnessScale::Ebu9 => "+9 scale",
                LoudnessScale::Ebu18 => "+18 scale",
            })
            .width(84.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut scale, LoudnessScale::Ebu9, "EBU +9 (-18 to +9 LU)");
                ui.selectable_value(&mut scale, LoudnessScale::Ebu18, "EBU +18 (-36 to +18 LU)");
            });
        if scale != app.settings.loudness_scale {
            app.settings.loudness_scale = scale;
            app.save_settings();
        }
    });
    crate::marks::put(ui, "loudness.buttons", buttons.response.rect);
    let target_row = ui.horizontal(|ui| {
        ui.label(RichText::new("Target").color(theme::TEXT_DIM));
        let target = &mut app.session.project.loudness_target;
        let chosen = LOUDNESS_PRESETS
            .iter()
            .find(|(_, lufs)| lufs == target)
            .map_or("Custom", |(name, _)| *name);
        egui::ComboBox::from_id_salt("loudness-target")
            .selected_text(chosen)
            .width(150.0)
            .show_ui(ui, |ui| {
                for (name, lufs) in LOUDNESS_PRESETS {
                    ui.selectable_value(target, lufs, format!("{name} ({lufs} LUFS)"));
                }
            });
        let (low, high) = LOUDNESS_TARGET_RANGE;
        ui.add(
            egui::DragValue::new(target)
                .range(low..=high)
                .speed(0.1)
                .fixed_decimals(1)
                .suffix(" LUFS"),
        )
        .on_hover_text("The integrated loudness to aim at");
    });
    crate::marks::put(ui, "loudness.target", target_row.response.rect);
    let whole = egui::Rect::from_min_max(top, egui::pos2(ui.min_rect().right(), ui.cursor().min.y));
    crate::marks::put(ui, "loudness", whole);
}

/// A compact readout for the transport bar: integrated loudness and true peak. Clicking it
/// shows the master's panel.
pub fn transport_readout(app: &mut DawApp, ui: &mut egui::Ui) {
    let l = app.session.loudness();
    let target = app.session.project.loudness_target;
    let relative = app.settings.loudness_relative;
    let hot = l.true_peak.is_some_and(|p| p > PEAK_CEILING);
    // Integrated and true peak only: the bar has no room for more (the panel has the rest).
    let text = format!(
        "I {}  TP {}",
        format_level(l.integrated, relative, target),
        format_peak(l.true_peak)
    );
    let response = ui
        .add(
            egui::Label::new(RichText::new(text).monospace().size(12.0).color(if hot {
                theme::METER_HIGH
            } else {
                Color32::from_gray(0xc8)
            }))
            .sense(Sense::click()),
        )
        .on_hover_text(
            "The master's loudness (EBU R 128), measured while playing.\n\
             I: integrated loudness, the whole programme's since the meter was reset, in LUFS \
             (or LU from the target, as the panel is set).\n\
             TP: true peak, the highest the signal reaches between samples, in dBTP. It turns \
             red above -1 dBTP: the mix may clip once converted or encoded (MP3, streaming).\n\
             Click for the loudness panel.",
        );
    crate::marks::put(ui, "transport.loudness", response.rect);
    if response.clicked() {
        app.selection = Some(Selection::Master);
        app.show_inspector = true;
    }
}

/// One line under the master strip: its integrated loudness (the value alone fits the strip;
/// the hover says what it is).
pub fn strip_line(app: &DawApp, ui: &mut egui::Ui) {
    let l = app.session.loudness();
    let target = app.session.project.loudness_target;
    // Two lines: the strip is too narrow for the label, value and unit on one.
    let tip = "Integrated loudness (EBU R 128); the master's inspector has the panel";
    ui.label(RichText::new("Integrated").small().color(theme::TEXT_DIM))
        .on_hover_text(tip);
    ui.label(
        RichText::new(format_level(
            l.integrated,
            app.settings.loudness_relative,
            target,
        ))
        .small()
        .monospace(),
    )
    .on_hover_text(tip);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scales_sit_around_the_target() {
        for scale in [LoudnessScale::Ebu9, LoudnessScale::Ebu18] {
            let (low, high) = scale.range();
            assert_eq!(fraction(-14.0 + low, -14.0, scale), 0.0);
            assert_eq!(fraction(-14.0 + high, -14.0, scale), 1.0);
            assert!(
                (fraction(-14.0, -14.0, scale) - 2.0 / 3.0).abs() < 1e-6,
                "target at 2/3"
            );
            assert_eq!(fraction(-200.0, -14.0, scale), 0.0, "clamped");
        }
        // EBU's own example: the +9 scale at a -23 target runs from -41 to -14 LUFS.
        assert_eq!(fraction(-41.0, -23.0, LoudnessScale::Ebu9), 0.0);
        assert_eq!(fraction(-14.0, -23.0, LoudnessScale::Ebu9), 1.0);
    }

    #[test]
    fn readings_have_one_decimal_and_their_unit() {
        assert_eq!(format_level(Some(-14.04), false, -14.0), "-14.0 LUFS");
        assert_eq!(format_level(Some(-12.76), true, -14.0), "+1.2 LU");
        assert_eq!(format_level(Some(-16.0), true, -14.0), "-2.0 LU");
        assert_eq!(format_level(None, false, -14.0), "–");
        assert_eq!(format_level(Some(SILENCE), false, -14.0), "−∞ LUFS");
        assert_eq!(format_range(Some(6.84)), "6.8 LU");
        assert_eq!(format_peak(Some(-0.94)), "-0.9 dBTP");
        assert_eq!(format_time(75.4), "1:15");
    }
}
