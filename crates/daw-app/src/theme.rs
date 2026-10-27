//! Colours and the egui style.

use egui::{Color32, CornerRadius, Stroke, Visuals};

pub const BG: Color32 = Color32::from_rgb(0x1b, 0x1d, 0x22);
pub const PANEL: Color32 = Color32::from_rgb(0x23, 0x26, 0x2d);
pub const LANE: Color32 = Color32::from_rgb(0x20, 0x22, 0x28);
pub const LANE_ALT: Color32 = Color32::from_rgb(0x24, 0x27, 0x2e);
pub const LANE_SELECTED: Color32 = Color32::from_rgb(0x2b, 0x30, 0x3a);
/// The grid's three levels sit well apart so bars, beats and divisions read as three, and
/// even the faintest stays clear of a selected lane (`LANE_SELECTED`).
pub const GRID_BAR: Color32 = Color32::from_rgb(0x4a, 0x4f, 0x5c);
pub const GRID_BEAT: Color32 = Color32::from_rgb(0x39, 0x3d, 0x48);
/// The grid's division between beats (17.4): fainter than a beat, still clear of the lanes.
pub const GRID_DIVISION: Color32 = Color32::from_rgb(0x2e, 0x31, 0x3a);
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x90, 0x9c);
pub const ACCENT: Color32 = Color32::from_rgb(0x4a, 0xa8, 0xff);
pub const PLAYHEAD: Color32 = Color32::from_rgb(0xff, 0xd0, 0x40);
/// The playhead lit while a dragged clip is held on it by the magnet (17.5): nearly white.
pub const PLAYHEAD_LIT: Color32 = Color32::from_rgb(0xff, 0xf4, 0xc8);
pub const LOOP: Color32 = Color32::from_rgb(0x50, 0xc8, 0xb4);
pub const RECORD: Color32 = Color32::from_rgb(0xe8, 0x40, 0x40);
pub const MUTE: Color32 = Color32::from_rgb(0xe0, 0x9a, 0x30);
pub const SOLO: Color32 = Color32::from_rgb(0x50, 0xc8, 0x70);
pub const MONITOR: Color32 = Color32::from_rgb(0x4a, 0xa8, 0xff);
/// Automation lanes: their curves, points and names.
/// Markers: their flags and the lines they draw across the lanes.
pub const MARKER: Color32 = Color32::from_rgb(0xb0, 0x8c, 0xf0);
/// The master's colour, where it shows as a row (its stripe and its lanes).
pub const MASTER: Color32 = Color32::from_rgb(0xd8, 0xd8, 0xd8);
pub const AUTOMATION: Color32 = Color32::from_rgb(0xe0, 0x6a, 0xd0);
pub const METER_LOW: Color32 = Color32::from_rgb(0x40, 0xc0, 0x60);
pub const METER_MID: Color32 = Color32::from_rgb(0xe0, 0xc0, 0x40);
pub const METER_HIGH: Color32 = Color32::from_rgb(0xe8, 0x40, 0x40);
/// A lane a dragged clip can't go onto: its outline and its wash.
pub const REFUSED: Color32 = Color32::from_rgb(0xe8, 0x40, 0x40);
pub const REFUSED_FILL: Color32 = Color32::from_rgba_premultiplied(0x3a, 0x10, 0x10, 0x40);

pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = PANEL;
    visuals.window_fill = PANEL;
    visuals.extreme_bg_color = BG;
    visuals.faint_bg_color = LANE_ALT;
    visuals.selection.bg_fill = ACCENT.linear_multiply(0.6);
    // A selected item's text (selectable buttons, the inspector's chosen insert, the rack's
    // row, toggles like Cubic) and a focused field's outline: white, readable on the blue fill.
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    visuals.widgets.noninteractive.corner_radius = CornerRadius::same(3);
    visuals.widgets.inactive.corner_radius = CornerRadius::same(3);
    visuals.widgets.hovered.corner_radius = CornerRadius::same(3);
    visuals.widgets.active.corner_radius = CornerRadius::same(3);
    // Buttons stand out from the panel, and a disabled one is drawn at 40 % (fill and text
    // alike: dim but readable), so it reads as off at a glance, not by its text colour alone.
    visuals.widgets.inactive.weak_bg_fill = Color32::from_gray(58);
    visuals.widgets.hovered.weak_bg_fill = Color32::from_gray(76);
    // A checkbox's box and a slider's rail: clear of the panel even when unticked.
    visuals.widgets.inactive.bg_fill = Color32::from_gray(80);
    visuals.disabled_alpha = 0.55;
    ctx.set_visuals(visuals);
    ctx.global_style_mut(|style| {
        style.spacing.item_spacing = egui::vec2(6.0, 4.0);
        style.spacing.button_padding = egui::vec2(6.0, 2.0);
    });
}

/// The size of a dialog's explanations ([`hint`]): small but readable, unlike egui's 9-point
/// `small()`, which stays for the status bar and the lanes' labels.
pub const HINT_SIZE: f32 = 12.0;

/// A line explaining a setting or a dialog, in the dim text colour at [`HINT_SIZE`].
pub fn hint(text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).size(HINT_SIZE).color(TEXT_DIM)
}

/// A dialog's main button: the accent fill, dark text, rightmost ([`button_row`]). Disabled
/// (`add_enabled(false, …)`) it is drawn as an ordinary disabled button, since a faded accent
/// with dark text can't be read.
pub fn primary_button(text: &str) -> Primary {
    Primary(text.to_string())
}

/// [`primary_button`]'s widget.
pub struct Primary(String);

impl egui::Widget for Primary {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        if ui.is_enabled() {
            ui.add(
                egui::Button::new(egui::RichText::new(self.0).color(Color32::BLACK)).fill(ACCENT),
            )
        } else {
            ui.add(egui::Button::new(self.0))
        }
    }
}

/// A checkbox that shows its state at a glance: an outlined empty box when off, an accent box
/// with a white tick when on (egui's own differs only by a thin tick). Use it for every
/// checkbox (`ui.add(theme::checkbox(&mut on, "Label"))`).
pub fn checkbox<'a>(on: &'a mut bool, text: impl Into<egui::WidgetText>) -> Check<'a> {
    Check {
        on,
        text: text.into(),
    }
}

/// [`checkbox`]'s widget: egui's checkbox (its layout, clicks and accessibility) with its box
/// painted over.
pub struct Check<'a> {
    on: &'a mut bool,
    text: egui::WidgetText,
}

impl egui::Widget for Check<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let response = ui.add(egui::Checkbox::new(&mut *self.on, self.text));
        let size = ui.spacing().icon_width;
        let rect = egui::Rect::from_center_size(
            egui::pos2(response.rect.left() + size * 0.5, response.rect.center().y),
            egui::Vec2::splat(size),
        );
        let painter = ui.painter();
        if *self.on {
            painter.rect_filled(rect, 3.0, ACCENT);
            let inner = rect.shrink(3.0);
            painter.line(
                vec![
                    egui::pos2(inner.left(), inner.center().y),
                    egui::pos2(inner.left() + inner.width() * 0.4, inner.bottom()),
                    egui::pos2(inner.right(), inner.top()),
                ],
                Stroke::new(2.0, Color32::WHITE),
            );
        } else {
            let edge = if response.hovered() {
                Color32::from_gray(0xc0)
            } else {
                Color32::from_gray(0x8a)
            };
            painter.rect(
                rect,
                3.0,
                BG,
                Stroke::new(1.2, edge),
                egui::StrokeKind::Inside,
            );
        }
        response
    }
}

/// A question's modal (the kind [`dialog_window`] describes): centred, the backdrop dimming
/// everything behind it (which it also keeps from being clicked), and the same inner margin
/// as a tool window.
pub fn modal(ctx: &egui::Context, name: &str) -> egui::Modal {
    egui::Modal::new(egui::Id::unique(name))
        .frame(egui::Frame::popup(&ctx.global_style()).inner_margin(DIALOG_MARGIN))
        .backdrop_color(Color32::from_black_alpha(150))
}

/// Space inside a dialog's edge, questions and tool windows alike.
pub const DIALOG_MARGIN: i8 = 12;

/// A dialog's buttons, on the right: add the primary one first (it lands rightmost), then the
/// others (Cancel last, leftmost).
pub fn button_row<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    // One row high: a right-to-left layout given the whole column would grow to its height.
    let size = egui::vec2(ui.available_width(), ui.spacing().interact_size.y);
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            add(ui)
        },
    )
    .inner
}

/// A progress bar with its percentage centred on it in white, readable on the accent fill and
/// on the empty track alike (egui puts it at the left in the selection's colour). `animate`
/// keeps the window repainting while the work runs; egui's own animation would draw a spinner
/// at the fill's end, over the centred percentage, so it is not used.
pub fn progress(ui: &mut egui::Ui, fraction: f32, animate: bool) -> egui::Response {
    let response = ui.add(egui::ProgressBar::new(fraction));
    if animate {
        ui.ctx().request_repaint();
    }
    let percent = (fraction.clamp(0.0, 1.0) * 100.0) as u32;
    ui.painter().text(
        response.rect.center(),
        egui::Align2::CENTER_CENTER,
        format!("{percent}%"),
        egui::FontId::proportional(12.0),
        Color32::WHITE,
    );
    response
}

/// A dialog's row: `label` in a column `label_width` wide, then the control `add` makes, and
/// under it `hint` (if any) saying what it does. Rows of one dialog share the width, so their
/// labels and controls line up.
pub fn setting_row<R>(
    ui: &mut egui::Ui,
    label_width: f32,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    setting_row_at(ui, label_width, label, hint, add).0
}

// The labels of the settings rows drawn on this thread, for the test that the manual's settings
// page names every one (`settings_window`'s `the_settings_page_names_every_row`).
#[cfg(test)]
thread_local! {
    pub static ROWS_DRAWN: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// [`setting_row`], and the row's rect (its label, control and line; the manual's marks).
pub fn setting_row_at<R>(
    ui: &mut egui::Ui,
    label_width: f32,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, egui::Rect) {
    #[cfg(test)]
    ROWS_DRAWN.with(|rows| rows.borrow_mut().push(label.to_string()));
    let inner = ui.horizontal_top(|ui| {
        let height = ui.spacing().interact_size.y;
        ui.allocate_ui_with_layout(
            egui::vec2(label_width, height),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.set_min_size(egui::vec2(label_width, height));
                ui.label(label);
            },
        );
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            let made = add(ui);
            if !hint.is_empty() {
                ui.add(egui::Label::new(self::hint(hint)).wrap());
            }
            made
        })
        .inner
    });
    ui.add_space(6.0);
    (inner.inner, inner.response.rect)
}

/// A dialog's heading and the line under it.
pub fn dialog_heading(ui: &mut egui::Ui, title: &str, line: &str) {
    ui.label(egui::RichText::new(title).size(17.0).strong());
    if !line.is_empty() {
        ui.add(egui::Label::new(hint(line)).wrap());
    }
    ui.add_space(8.0);
}

/// Where the menu bar ends, kept by the frame (`DawApp::frame_ui`) for [`below_menu`].
fn menu_bottom_id() -> egui::Id {
    egui::Id::unique("oxide-menu-bottom")
}

/// Remember where the menu bar ends this frame.
pub fn set_menu_bottom(ctx: &egui::Context, y: f32) {
    ctx.data_mut(|d| d.insert_temp(menu_bottom_id(), y));
}

/// The window's area under the menu bar: dialogs open there, centred, and can't be dragged
/// over the menus.
pub fn below_menu(ctx: &egui::Context) -> egui::Rect {
    let screen = ctx.content_rect();
    let top = ctx
        .data(|d| d.get_temp::<f32>(menu_bottom_id()))
        .unwrap_or(screen.top() + 26.0);
    egui::Rect::from_min_max(egui::pos2(screen.left(), top), screen.max)
}

/// The app's two kinds of dialog (17.7):
/// - a **question** that needs an answer before anything else (`egui::Modal`: the save
///   question, the export options, a dropped file's questions…): no title bar, a heading
///   (`ui.heading`) at its top left, what it asks, `hint` lines, and `button_row` with the
///   answer to go ahead rightmost (`primary_button`), Cancel leftmost; Escape or a click
///   outside cancels;
/// - a **tool window** that stays open beside the work (Settings, Open backup, Templates,
///   Render, the plug-in browser, Metadata, Slice and Quantize, the CPU panel): this, a
///   titled window with a close button, placed under the menu bar ([`below_menu_window`]),
///   sized to its content.
pub fn dialog_window<'a>(
    title: impl Into<egui::WidgetText>,
    ctx: &egui::Context,
) -> egui::Window<'a> {
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(DIALOG_MARGIN);
    below_menu_window(
        egui::Window::new(title)
            .collapsible(false)
            .auto_sized()
            .frame(frame),
        ctx,
    )
}

/// `window` opened under the menu bar, centred, and kept below it (a resizable tool window
/// such as the plug-in browser; [`dialog_window`] for the rest).
pub fn below_menu_window<'a>(window: egui::Window<'a>, ctx: &egui::Context) -> egui::Window<'a> {
    let area = below_menu(ctx);
    window
        .pivot(egui::Align2::CENTER_TOP)
        .default_pos(egui::pos2(area.center().x, area.top() + 48.0))
        .constrain_to(area)
}

pub fn track_color(rgb: [u8; 3]) -> Color32 {
    Color32::from_rgb(rgb[0], rgb[1], rgb[2])
}

/// A small toggle button that lights up in `color` when on.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool, label: &str, color: Color32, tip: &str) -> bool {
    toggle_at(ui, on, label, color, tip).0
}

/// [`toggle`], and where it was drawn (for the manual's marks, `marks::put`).
pub fn toggle_at(
    ui: &mut egui::Ui,
    on: &mut bool,
    label: &str,
    color: Color32,
    tip: &str,
) -> (bool, egui::Rect) {
    let text =
        egui::RichText::new(label)
            .strong()
            .color(if *on { Color32::BLACK } else { TEXT_DIM });
    let button = egui::Button::new(text)
        .fill(if *on { color } else { Color32::from_gray(0x33) })
        .min_size(egui::vec2(20.0, 18.0));
    let response = ui.add(button).on_hover_text(tip);
    if response.clicked() {
        *on = !*on;
    }
    (response.clicked(), response.rect)
}

/// How far a knob turns over its whole range: 270°, from 7:30 to 4:30 on a clock face.
const KNOB_SWEEP: f32 = 1.5 * std::f32::consts::PI;
/// Points of vertical drag that cover a knob's whole range (ten times more with Shift).
const KNOB_TRAVEL: f32 = 160.0;

/// Where `value` sits on a knob, 0 to 1, with `centre` at the top (0.5): each side of the
/// centre is spread over its own half, so a volume knob points straight up at 0 dB,
/// whatever the range below and above it.
pub fn knob_position(value: f32, range: &std::ops::RangeInclusive<f32>, centre: f32) -> f32 {
    let (low, high) = (*range.start(), *range.end());
    let value = value.clamp(low, high);
    if value < centre {
        0.5 * (value - low) / (centre - low).max(f32::EPSILON)
    } else {
        0.5 + 0.5 * (value - centre) / (high - centre).max(f32::EPSILON)
    }
}

/// The value at knob position `t` (the inverse of [`knob_position`]).
pub fn knob_value(t: f32, range: &std::ops::RangeInclusive<f32>, centre: f32) -> f32 {
    let (low, high) = (*range.start(), *range.end());
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        low + (centre - low) * t * 2.0
    } else {
        centre + (high - centre) * (t - 0.5) * 2.0
    }
}

/// A knob's angle at position `t`, in radians from straight up (negative to the left).
pub fn knob_angle(t: f32) -> f32 {
    (t.clamp(0.0, 1.0) - 0.5) * KNOB_SWEEP
}

/// The value after dragging a knob up by `up` points (down is negative); `fine` is Shift.
/// The drag moves the knob's position, so each side of the centre turns at its own rate.
pub fn knob_drag(
    value: f32,
    up: f32,
    range: &std::ops::RangeInclusive<f32>,
    centre: f32,
    fine: bool,
) -> f32 {
    let travel = if fine {
        KNOB_TRAVEL * 10.0
    } else {
        KNOB_TRAVEL
    };
    let t = knob_position(value, range, centre) + up / travel;
    knob_value(t, range, centre)
}

/// A small rotary knob, like a track header's volume and pan: drag up or down to turn
/// it (Shift for fine). `centre` points straight up (0 dB, or centre pan), and the arc lights
/// from it to the value. Resetting on double-click is the caller's (see `mixer::fader_section`:
/// an automated control must not take it).
pub fn knob(
    ui: &mut egui::Ui,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    centre: f32,
    color: Color32,
) -> egui::Response {
    let size = egui::vec2(20.0, 20.0);
    let (rect, mut response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());
    if response.dragged() {
        let fine = ui.input(|i| i.modifiers.shift);
        let turned = knob_drag(*value, -response.drag_delta().y, &range, centre, fine);
        if turned != *value {
            *value = turned;
            response.mark_changed();
        }
    }
    let painter = ui.painter_at(rect.expand(1.0));
    let c = rect.center();
    let r = rect.width() / 2.0 - 1.5;
    let visuals = ui.style().interact(&response);
    painter.circle_filled(c, r, Color32::from_gray(0x30));
    // The track behind the arc, then the lit arc from `centre` to the value.
    let arc = |from: f32, to: f32, stroke: Stroke| {
        let steps = ((to - from).abs() / 0.15).ceil().max(1.0) as usize;
        let points: Vec<_> = (0..=steps)
            .map(|k| {
                let a = from + (to - from) * k as f32 / steps as f32;
                c + egui::vec2(a.sin(), -a.cos()) * r
            })
            .collect();
        painter.add(egui::Shape::line(points, stroke));
    };
    arc(
        -KNOB_SWEEP / 2.0,
        KNOB_SWEEP / 2.0,
        Stroke::new(2.0, Color32::from_gray(0x45)),
    );
    let at = knob_angle(knob_position(*value, &range, centre));
    let enabled = ui.is_enabled();
    arc(
        0.0,
        at,
        Stroke::new(2.0, if enabled { color } else { TEXT_DIM }),
    );
    let tip = c + egui::vec2(at.sin(), -at.cos()) * (r - 3.0);
    painter.line_segment([c, tip], Stroke::new(2.0, visuals.fg_stroke.color));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_knob_points_up_at_its_centre_and_spans_270_degrees() {
        use std::f32::consts::PI;
        let range = -60.0..=12.0;
        let angle = |v: f32| knob_angle(knob_position(v, &range, 0.0));
        assert!(angle(0.0).abs() < 1e-6, "0 dB points straight up");
        assert!((angle(-60.0) + 0.75 * PI).abs() < 1e-6);
        assert!((angle(12.0) - 0.75 * PI).abs() < 1e-6);
        assert!((angle(-30.0) + 0.375 * PI).abs() < 1e-5);
        assert_eq!(angle(40.0), angle(12.0), "out of range clamps");
        for v in [-60.0, -42.5, -3.0, 0.0, 5.5, 12.0] {
            let back = knob_value(knob_position(v, &range, 0.0), &range, 0.0);
            assert!((back - v).abs() < 1e-4, "{v} -> {back}");
        }
    }

    #[test]
    fn dragging_a_knob_turns_it_within_its_range() {
        let range = -1.0..=1.0;
        // The whole travel covers the whole range; Shift is ten times finer.
        assert!((knob_drag(0.0, KNOB_TRAVEL / 2.0, &range, 0.0, false) - 1.0).abs() < 1e-6);
        assert!((knob_drag(0.0, KNOB_TRAVEL / 2.0, &range, 0.0, true) - 0.1).abs() < 1e-6);
        assert_eq!(knob_drag(0.9, 100.0, &range, 0.0, false), 1.0);
        assert_eq!(knob_drag(-0.9, -100.0, &range, 0.0, false), -1.0);
        // Volume: half the travel takes 0 dB down to -60, the other half up to +12.
        let volume = -60.0..=12.0;
        assert!((knob_drag(0.0, -KNOB_TRAVEL / 2.0, &volume, 0.0, false) + 60.0).abs() < 1e-4);
        assert!((knob_drag(0.0, KNOB_TRAVEL / 4.0, &volume, 0.0, false) - 6.0).abs() < 1e-4);
    }
}
