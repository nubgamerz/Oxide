//! Automation lanes in the arrangement (dev plan 5.3): a row under the track for each lane,
//! showing its curve, its points and its value at the playhead.
//!
//! - Double-click a lane to add a point there; drag a point to move it (snapped to beats in
//!   ticks when snap is on); Delete or right-click to remove it, and right-click to curve the
//!   segment after it.
//! - Every edit goes through the same `Project` calls the MCP actions use, and `Session::update`
//!   rebuilds the graph when lanes change, so nothing here marks anything dirty.
//! - The value↔height maps are pure functions, tested below.
//!
//! Plug-in parameter names come from [`DawApp::param_names`], filled once per plug-in: reading
//! them locks the plug-in, which would silence its track if done every frame.

use crate::app::{DawApp, Selection};
use crate::arrange::{draw_grid, TimeAxis, HEADER_WIDTH, INDENT};
use crate::theme;
use daw_core::automation::{
    AutomationLane, AutomationMode, AutomationTarget, LaneId, LaneOwner, NewPoint, PointId,
};
use daw_core::grid::Rounding;
use daw_core::project::InsertId;
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, RichText, Sense, Stroke, UiBuilder};

/// Every lane row is this tall, in points.
pub const LANE_HEIGHT: f32 = 72.0;
/// Room left above and below the curve inside a row, so points at the ends stay grabbable.
const PAD: f32 = 6.0;
/// Shown on mixer controls a lane is driving.
pub const AUTOMATED_TIP: &str =
    "Automated: a lane plays this. Edit the lane under the track (write modes come in 5.4).";

/// Levels (volume, send) are drawn on a fader's scale: by the fourth root of the gain, so the
/// range that matters (−30..+6 dB) gets most of the lane and −60 dB still sits at the bottom.
fn fader_position(db: f64) -> f64 {
    let root = |db: f64| 10f64.powf(db / 80.0);
    let (bottom, top) = (root(-60.0), root(12.0));
    (root(db) - bottom) / (top - bottom)
}

/// The inverse of [`fader_position`].
fn fader_db(position: f64) -> f64 {
    let root = |db: f64| 10f64.powf(db / 80.0);
    let (bottom, top) = (root(-60.0), root(12.0));
    80.0 * (bottom + position * (top - bottom)).log10()
}

fn is_level(target: &AutomationTarget) -> bool {
    matches!(
        target,
        AutomationTarget::Volume | AutomationTarget::SendLevel { .. }
    )
}

/// Where `value` sits in a lane, from 0 (bottom) to 1 (top). Volume and send levels are on a
/// fader's scale over −60..+12 dB, pan has the centre in the middle, mute sits at the bottom
/// when playing and at the top when muted, and a parameter is its normalized value.
pub fn height_of(target: &AutomationTarget, value: f64) -> f32 {
    let (low, high) = target.range();
    let value = value.clamp(low, high);
    let h = if is_level(target) {
        fader_position(value)
    } else {
        (value - low) / (high - low)
    };
    (h as f32).clamp(0.0, 1.0)
}

/// The value at height `h` (0 bottom, 1 top) in a lane: the inverse of [`height_of`], clamped
/// to the target's range. Mute snaps to 0 or 1.
pub fn value_at_height(target: &AutomationTarget, h: f32) -> f64 {
    let (low, high) = target.range();
    let h = f64::from(h.clamp(0.0, 1.0));
    if target.steps() {
        return if h >= 0.5 { high } else { low };
    }
    let value = if is_level(target) {
        fader_db(h)
    } else {
        low + h * (high - low)
    };
    value.clamp(low, high)
}

/// The lanes toggle in a track's header: a little line chart, lit while the lanes show. Drawn
/// rather than an emoji glyph, which renders as a coloured tile. Returns whether it was
/// clicked (and flipped `on`).
pub fn lanes_toggle(ui: &mut egui::Ui, on: &mut bool, tip: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 18.0), Sense::click());
    let response = response.on_hover_text(tip);
    let fill = if *on {
        theme::AUTOMATION
    } else if response.hovered() {
        Color32::from_gray(0x44)
    } else {
        Color32::from_gray(0x33)
    };
    crate::marks::put(ui, "automation.toggle", rect);
    let ink = if *on { Color32::BLACK } else { theme::TEXT_DIM };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, fill);
    let area = rect.shrink2(vec2(4.0, 4.5));
    let points: Vec<egui::Pos2> = [(0.0, 0.85), (0.35, 0.3), (0.65, 0.6), (1.0, 0.1)]
        .iter()
        .map(|&(x, y)| {
            pos2(
                area.left() + x * area.width(),
                area.top() + y * area.height(),
            )
        })
        .collect();
    painter.add(egui::Shape::line(points.clone(), Stroke::new(1.5, ink)));
    for p in points {
        painter.circle_filled(p, 1.6, ink);
    }
    if response.clicked() {
        *on = !*on;
        true
    } else {
        false
    }
}

/// A value as the lane's header shows it.
pub fn format_value(target: &AutomationTarget, value: f64) -> String {
    match target {
        AutomationTarget::Volume | AutomationTarget::SendLevel { .. } => format!("{value:.1} dB"),
        AutomationTarget::Pan => crate::inspector::pan_label(value as f32),
        AutomationTarget::Mute => if value >= 0.5 { "muted" } else { "plays" }.into(),
        AutomationTarget::Parameter { .. } => format!("{:.0}%", value * 100.0),
    }
}

/// The automatable parameters of the plug-in in `insert`, read once per plug-in and kept.
/// Kept by the plug-in's path, not the insert: insert ids start again in every project, and
/// two inserts of one plug-in share a list. A plug-in that isn't loaded gives none, and is
/// asked again next time.
fn parameters(app: &mut DawApp, insert: InsertId) -> &[(u32, String)] {
    let Some(path) = app
        .session
        .project
        .find_insert(insert)
        .map(|s| s.path.clone())
    else {
        return &[];
    };
    if !app.param_names.contains_key(&path) {
        match app.session.automatable_parameters(insert) {
            Ok(list) => {
                app.param_names.insert(path.clone(), list);
            }
            Err(_) => return &[],
        }
    }
    &app.param_names[&path]
}

/// What a lane is called: "Volume", "Send → Reverb", "Compressor: Threshold".
pub fn lane_name(app: &mut DawApp, target: &AutomationTarget) -> String {
    match *target {
        AutomationTarget::Volume => "Volume".into(),
        AutomationTarget::Pan => "Pan".into(),
        AutomationTarget::Mute => "Mute".into(),
        AutomationTarget::SendLevel { send } => {
            let project = &app.session.project;
            let to = project
                .find_send(send)
                .and_then(|(_, s)| project.track(s.target))
                .map_or("?".into(), |t| t.name.clone());
            format!("Send → {to}")
        }
        AutomationTarget::Parameter { insert, parameter } => {
            let plugin = app
                .session
                .project
                .find_insert(insert)
                .map_or("Plug-in".into(), |s| s.name.clone());
            let name = parameters(app, insert)
                .iter()
                .find(|(id, _)| *id == parameter)
                .map_or(format!("parameter {parameter}"), |(_, n)| n.clone());
            format!("{plugin}: {name}")
        }
    }
}

/// How one mixer control stands with its automation this frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct Drive<T> {
    /// A lane plays it: the lane's value at the playhead.
    pub lane: Option<T>,
    /// It is being written: the value it is written at.
    pub written: Option<T>,
    /// Playing in a write mode: moving it writes it.
    pub writable: bool,
}

/// The fader, pan and mute of `owner` and their automation.
#[derive(Debug, Clone, Copy, Default)]
pub struct Automated {
    pub volume: Drive<f32>,
    pub pan: Drive<f32>,
    pub mute: Drive<bool>,
}

pub fn automated(app: &DawApp, owner: LaneOwner) -> Automated {
    let session = &app.session;
    let writable = session.is_playing() && session.automation_mode(owner).writes();
    let lane = |target| session.playing_value(owner, target, &app.tempo_map);
    let written = |target| session.written_value(owner, target);
    let level = |target| Drive {
        lane: lane(target).map(|v| v as f32),
        written: written(target).map(|v| v as f32),
        writable,
    };
    Automated {
        volume: level(AutomationTarget::Volume),
        pan: level(AutomationTarget::Pan),
        mute: Drive {
            lane: lane(AutomationTarget::Mute).map(|v| v >= 0.5),
            written: written(AutomationTarget::Mute).map(|v| v >= 0.5),
            writable,
        },
    }
}

/// What a control did this frame, for the automation writer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gesture {
    /// Grabbed or moved, to this value (in the target's units).
    Touched(f64),
    /// Let go.
    Released,
}

/// Hand `owner`'s control gestures to the session's writer. Called once the borrows of the
/// project made while drawing the controls are over.
pub fn apply_gestures(
    app: &mut DawApp,
    owner: LaneOwner,
    gestures: &[(AutomationTarget, Gesture)],
) {
    for &(target, gesture) in gestures {
        match gesture {
            Gesture::Touched(value) => app.session.touch_control(owner, target, value),
            Gesture::Released => app.session.release_control(owner, target),
        }
    }
}

const WRITING_TIP: &str = "Writing automation: moving this while playing writes its lane \
     (the track's mode: Touch, Latch or Write).";

/// Add a control for `value` (a fader or pan), as its automation stands (`drive`):
///
/// - playing in a write mode: live, showing what is written or what the lane plays, and
///   reporting grabs and moves as gestures for the writer instead of changing `value`;
/// - a lane playing it otherwise: greyed, showing the lane's value (edits to the fixed value
///   would be ignored, so the control doesn't take them);
/// - neither: the fixed value, as ever.
pub fn control(
    ui: &mut egui::Ui,
    drive: Drive<f32>,
    value: &mut f32,
    add: impl FnOnce(&mut egui::Ui, &mut f32) -> egui::Response,
) -> (egui::Response, Option<Gesture>) {
    if drive.writable {
        let mut shown = drive.written.or(drive.lane).unwrap_or(*value);
        let before = shown;
        let response = add(ui, &mut shown).on_hover_text(WRITING_TIP);
        mark_drive(
            ui,
            response.rect,
            drive.written.is_some(),
            drive.lane.is_some(),
        );
        let gesture = gesture(
            Hands {
                grabbed: response.drag_started(),
                held: response.dragged() || response.clicked() || response.has_focus(),
                let_go: response.drag_stopped(),
            },
            before,
            shown,
        );
        return (response, gesture);
    }
    match drive.lane {
        None => (add(ui, value), None),
        Some(shown) => {
            let mut shown = shown;
            let response = ui
                .add_enabled_ui(false, |ui| add(ui, &mut shown))
                .inner
                .on_disabled_hover_text(AUTOMATED_TIP);
            mark_drive(ui, response.rect, false, true);
            (response, None)
        }
    }
}

/// What a hand did to a control this frame.
#[derive(Debug, Clone, Copy, Default)]
struct Hands {
    grabbed: bool,
    /// Dragging, clicking or typing in it.
    held: bool,
    let_go: bool,
}

/// The gesture a writable control reports, from what the hand did and the value before and
/// after it was drawn. Only a hand on the control counts: a slider with fixed decimals rounds
/// the value it is given every frame, and that rounding is no touch (it would start writing a
/// control nobody holds and freeze it there).
fn gesture(hands: Hands, before: f32, after: f32) -> Option<Gesture> {
    if hands.let_go {
        Some(Gesture::Released)
    } else if hands.grabbed || (hands.held && after != before) {
        Some(Gesture::Touched(f64::from(after)))
    } else {
        None
    }
}

/// Outline a control its automation is driving: red while it is being written, the
/// automation colour while a lane plays it, so greyed reads as "automated", not "broken".
fn mark_drive(ui: &egui::Ui, rect: Rect, written: bool, lane: bool) {
    let color = if written {
        theme::RECORD
    } else if lane {
        theme::AUTOMATION
    } else {
        return;
    };
    ui.painter().rect_stroke(
        rect.expand(1.5),
        3,
        Stroke::new(1.0, color),
        egui::StrokeKind::Outside,
    );
}

/// A track's mute button, as its automation stands (see [`control`]). A click in a write mode
/// writes the new state and, having no "let go", keeps writing it until playback stops, as
/// latch does.
///
/// `mark` names it for the manual (`marks::put`).
pub fn mute_toggle(
    ui: &mut egui::Ui,
    drive: Drive<bool>,
    mute: &mut bool,
    mark: &'static str,
) -> Option<Gesture> {
    if drive.writable {
        let mut shown = drive.written.or(drive.lane).unwrap_or(*mute);
        let (clicked, rect) = theme::toggle_at(ui, &mut shown, "M", theme::MUTE, WRITING_TIP);
        crate::marks::put(ui, mark, rect);
        return clicked.then_some(Gesture::Touched(f64::from(u8::from(shown))));
    }
    match drive.lane {
        None => {
            let (_, rect) = theme::toggle_at(ui, mute, "M", theme::MUTE, "Mute");
            crate::marks::put(ui, mark, rect);
        }
        Some(shown) => {
            let mut shown = shown;
            let response = ui
                .add_enabled_ui(false, |ui| {
                    theme::toggle(ui, &mut shown, "M", theme::MUTE, AUTOMATED_TIP);
                })
                .response
                .on_disabled_hover_text(AUTOMATED_TIP);
            crate::marks::put(ui, mark, response.rect);
            mark_drive(ui, response.rect, false, true);
        }
    }
    None
}

/// The automation mode button: R, T, L or W, coloured unless Read; a click moves to the next.
/// Returns whether it changed `mode`. `mark` names it for the manual (`marks::put`).
pub fn mode_button(ui: &mut egui::Ui, mode: &mut AutomationMode, mark: &'static str) -> bool {
    use AutomationMode::{Latch, Read, Touch, Write};
    let (label, fill, next, what) = match mode {
        Read => ("R", Color32::from_gray(0x33), Touch, "Read: lanes play"),
        Touch => (
            "T",
            theme::PLAYHEAD,
            Latch,
            "Touch: holding a control writes it",
        ),
        Latch => (
            "L",
            theme::MUTE,
            Write,
            "Latch: a control moved keeps writing until stop",
        ),
        Write => (
            "W",
            theme::RECORD,
            Read,
            "Write: every lane is written while playing",
        ),
    };
    let text = RichText::new(label).strong().color(if *mode == Read {
        theme::TEXT_DIM
    } else {
        Color32::BLACK
    });
    let response = ui
        .add(
            egui::Button::new(text)
                .fill(fill)
                .min_size(vec2(20.0, 18.0)),
        )
        .on_hover_text(format!("Automation {what}. Click for the next mode."));
    crate::marks::put(ui, mark, response.rect);
    if response.clicked() {
        *mode = next;
        true
    } else {
        false
    }
}
/// Delete the selected point, unless a clip was selected after it. Returns whether it did.
pub fn delete_selected_point(app: &mut DawApp) -> bool {
    let Some((lane, point)) = app.selected_point else {
        return false;
    };
    if app.selected_clip.is_some() {
        return false;
    }
    app.selected_point = None;
    if app
        .session
        .project
        .remove_points(lane, &[point], None)
        .is_ok()
    {
        app.session.label_edit("Delete automation point", None);
    }
    true
}

/// The lanes of `track` as rows under it, when it shows them. Returns the rows' rect, so a
/// file dropped on a lane row lands on the track.
pub fn lane_rows(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    owner: LaneOwner,
    axis: TimeAxis,
) -> Option<Rect> {
    let project = &app.session.project;
    let (show, lanes, color, depth) = match owner {
        Some(track) => {
            let t = project.track(track)?;
            (
                t.show_automation,
                &t.automation,
                theme::track_color(t.color),
                project.depth(track) as f32,
            )
        }
        None => (
            project.master.show_automation,
            &project.master.automation,
            theme::MASTER,
            0.0,
        ),
    };
    if !show || lanes.is_empty() {
        return None;
    }
    let lanes = lanes.clone();
    let mut rows: Option<Rect> = None;
    for lane in &lanes {
        let rect = lane_row(app, ui, owner, lane, depth, color, axis);
        rows = Some(rows.map_or(rect, |r| r.union(rect)));
    }
    rows
}

/// What a lane row asks for once it is drawn: edits wait until nothing is borrowed.
enum Edit {
    Enable(bool),
    Remove,
    Add(NewPoint),
    Move(PointId, u64, f64),
    Delete(PointId),
    Curve(PointId, f32),
}

/// One lane's row: a header with its name, on/off, value at the playhead and remove button;
/// then the curve and points across the timeline.
fn lane_row(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    owner: LaneOwner,
    lane: &AutomationLane,
    depth: f32,
    color: Color32,
    axis: TimeAxis,
) -> Rect {
    let rect = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), LANE_HEIGHT));
    let header = Rect::from_min_size(rect.min, vec2(HEADER_WIDTH, LANE_HEIGHT));
    let area = Rect::from_min_max(pos2(header.right(), rect.top()), rect.max);
    let plot = area.shrink2(vec2(0.0, PAD));
    // The manual's marks: the first lane drawn (27.5).
    crate::marks::put(ui, "automation.lane", rect);
    crate::marks::put(ui, "automation.lane.header", header);
    let target = lane.target;
    let map = app.tempo_map.clone();
    let snapping = app.settings.snap;
    let y_of = |value: f64| plot.bottom() - height_of(&target, value) * plot.height();
    let value_of = |y: f32| value_at_height(&target, (plot.bottom() - y) / plot.height());
    let grid = app.session.project.grid;
    let spp = app.samples_per_pixel;
    // On the song's grid, as everything else the Multitrack snaps (`arrange::snap`).
    let snap_tick = |sample: f64| {
        let tick = map.tick_at(sample.max(0.0)).round().max(0.0) as u64;
        if snapping {
            let x = crate::arrange::grid_x(&map, spp);
            map.snap_grid_tick(tick, grid, &x, Rounding::Nearest)
        } else {
            tick
        }
    };
    let mut edits: Vec<Edit> = Vec::new();

    // The row's background, under everything.
    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0, theme::BG);
    draw_grid(app, &painter, area, axis);
    painter.hline(
        area.x_range(),
        area.bottom() - 0.5,
        Stroke::new(1.0, theme::PANEL),
    );

    // The lane itself first, so the points (added after) win the pointer.
    let response = ui.interact(
        area,
        ui.scope_id().with(("auto-lane", lane.id)),
        Sense::click(),
    );
    if response.clicked() {
        app.selection = Some(owner.map_or(Selection::Master, Selection::Track));
        app.selected_point = None;
        if let Some(p) = response.interact_pointer_pos() {
            crate::arrange::locate_click(app, axis, p.x);
        }
    }
    if response.double_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            edits.push(Edit::Add(NewPoint {
                tick: snap_tick(axis.sample(p.x)),
                value: value_of(p.y),
                curve: 0.0,
            }));
        }
    }
    let menu_at = ui.scope_id().with(("auto-lane-menu", lane.id));
    if response.secondary_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            ui.ctx().data_mut(|d| d.insert_temp(menu_at, p));
        }
    }
    response.context_menu(|ui| {
        if ui.button("Add point here").clicked() {
            if let Some(p) = ui.ctx().data(|d| d.get_temp::<egui::Pos2>(menu_at)) {
                edits.push(Edit::Add(NewPoint {
                    tick: snap_tick(axis.sample(p.x)),
                    value: value_of(p.y),
                    curve: 0.0,
                }));
            }
            ui.close();
        }
        let label = if lane.enabled {
            "Switch lane off"
        } else {
            "Switch lane on"
        };
        if ui.button(label).clicked() {
            edits.push(Edit::Enable(!lane.enabled));
            ui.close();
        }
        if ui.button("Remove lane").clicked() {
            edits.push(Edit::Remove);
            ui.close();
        }
    });

    // The curve: one sample per pixel column, which also draws mute's steps and the holds.
    let line = if lane.enabled {
        theme::AUTOMATION
    } else {
        theme::TEXT_DIM.gamma_multiply(0.6)
    };
    if lane.points.is_empty() {
        painter.text(
            plot.left_center() + vec2(8.0, 0.0),
            Align2::LEFT_CENTER,
            "No points: double-click to add one",
            FontId::proportional(11.0),
            theme::TEXT_DIM,
        );
    } else {
        let curve: Vec<egui::Pos2> = (area.left() as i32..=area.right() as i32)
            .map(|x| {
                let x = x as f32;
                let tick = map.tick_at(axis.sample(x).max(0.0));
                pos2(x, y_of(lane.value_at(tick).unwrap_or(0.0)))
            })
            .collect();
        painter.add(egui::Shape::line(curve, Stroke::new(1.5, line)));
        // A jump (two points on one tick) gets its vertical, which a line sampled per pixel
        // column can miss.
        for pair in lane.points.windows(2).filter(|w| w[0].tick == w[1].tick) {
            let x = axis.x(map.sample_at_tick(pair[0].tick as f64));
            let (a, b) = (y_of(pair[0].value), y_of(pair[1].value));
            painter.vline(x, a.min(b)..=a.max(b), Stroke::new(1.5, line));
        }
    }

    // The points.
    for point in &lane.points {
        let at = pos2(
            axis.x(map.sample_at_tick(point.tick as f64)),
            y_of(point.value),
        );
        if at.x < area.left() - 6.0 || at.x > area.right() + 6.0 {
            continue;
        }
        let id = ui.scope_id().with(("auto-point", point.id));
        let handle = ui
            .interact(
                Rect::from_center_size(at, vec2(12.0, 12.0)),
                id,
                Sense::click_and_drag(),
            )
            // A pointing hand: Windows draws `Grab` as the four-way move cross.
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(format_value(&target, point.value));
        let selected = app.selected_point == Some((lane.id, point.id));
        if selected {
            crate::marks::put(ui, "automation.point", handle.rect);
        }
        if handle.clicked() || handle.drag_started() {
            app.selected_point = Some((lane.id, point.id));
            app.clear_clip_selection();
            app.selection = Some(owner.map_or(Selection::Master, Selection::Track));
        }
        // Moves keep where the point was grabbed, so it doesn't jump to the pointer.
        if handle.drag_started() {
            if let Some(p) = handle.interact_pointer_pos() {
                ui.ctx().data_mut(|d| d.insert_temp(id, at - p));
            }
        }
        if handle.dragged() {
            if let Some(p) = handle.interact_pointer_pos() {
                let grab: egui::Vec2 = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_default();
                let to = p + grab;
                let tick = snap_tick(axis.sample(to.x));
                let value = value_of(to.y.clamp(plot.top(), plot.bottom()));
                edits.push(Edit::Move(point.id, tick, value));
                painter.text(
                    to + vec2(8.0, -8.0),
                    Align2::LEFT_BOTTOM,
                    format_value(&target, value),
                    FontId::monospace(11.0),
                    Color32::WHITE,
                );
            }
        }
        if handle.drag_stopped() {
            ui.ctx().data_mut(|d| d.remove::<egui::Vec2>(id));
        }
        handle.context_menu(|ui| {
            if ui.button("Delete point (Del)").clicked() {
                edits.push(Edit::Delete(point.id));
                ui.close();
            }
            ui.separator();
            ui.label(RichText::new("Segment to the next point").weak());
            for (label, curve) in [
                ("Straight", 0.0),
                ("Ease in (slow start)", 0.6),
                ("Ease out (fast start)", -0.6),
            ] {
                if ui.radio(point.curve == curve, label).clicked() {
                    edits.push(Edit::Curve(point.id, curve));
                    ui.close();
                }
            }
        });
        let (radius, fill) = if selected {
            (4.5, Color32::WHITE)
        } else if handle.hovered() {
            (4.5, line)
        } else {
            (3.5, line)
        };
        painter.circle(at, radius, fill, Stroke::new(1.0, theme::BG));
    }

    // The header, drawn last over its own rect.
    let readout = lane
        .value_at(map.tick_at(app.session.position() as f64))
        .map_or("—".into(), |v| format_value(&target, v));
    let name = lane_name(app, &target);
    let hp = ui.painter_at(header);
    hp.rect_filled(header, 0, theme::LANE);
    hp.vline(
        header.right() - 0.5,
        header.y_range(),
        Stroke::new(1.0, theme::BG),
    );
    hp.hline(
        header.x_range(),
        header.bottom() - 0.5,
        Stroke::new(1.0, theme::BG),
    );
    let indent = depth * INDENT + 14.0;
    let stripe = Rect::from_min_size(
        pos2(header.left() + indent, header.top() + 4.0),
        vec2(3.0, header.height() - 8.0),
    );
    hp.rect_filled(stripe, 1, color.gamma_multiply(0.6));
    let head = ui.interact(
        header,
        ui.scope_id().with(("auto-head", lane.id)),
        Sense::click(),
    );
    if head.clicked() {
        app.selection = Some(owner.map_or(Selection::Master, Selection::Track));
    }
    let content = Rect::from_min_max(
        pos2(stripe.right() + 6.0, header.top() + 4.0),
        pos2(header.right() - 6.0, header.bottom() - 4.0),
    );
    ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 3.0);
        ui.add(
            egui::Label::new(RichText::new(&name).strong().color(if lane.enabled {
                theme::AUTOMATION
            } else {
                theme::TEXT_DIM
            }))
            .truncate(),
        )
        .on_hover_text(&name);
        ui.horizontal(|ui| {
            let mut on = lane.enabled;
            if theme::toggle(
                ui,
                &mut on,
                "On",
                theme::AUTOMATION,
                "Play this lane (off: kept, but moves nothing)",
            ) {
                edits.push(Edit::Enable(on));
            }
            ui.label(
                RichText::new(readout)
                    .small()
                    .monospace()
                    .color(theme::TEXT_DIM),
            )
            .on_hover_text("At the playhead");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("×").on_hover_text("Remove lane").clicked() {
                    edits.push(Edit::Remove);
                }
            });
        });
    });
    // Moves the cursor to the row's bottom, whatever the header left it at.
    ui.allocate_rect(rect, Sense::hover());

    apply(app, lane.id, edits);
    rect
}

fn apply(app: &mut DawApp, lane: LaneId, edits: Vec<Edit>) {
    for edit in edits {
        let label = match edit {
            Edit::Enable(on) => {
                if let Some(l) = app.session.project.find_lane_mut(lane) {
                    l.enabled = on;
                }
                None
            }
            Edit::Remove => {
                app.session.project.remove_lane(lane);
                if app.selected_point.is_some_and(|(l, _)| l == lane) {
                    app.selected_point = None;
                }
                None
            }
            Edit::Add(point) => {
                if let Ok(ids) = app.session.project.add_points(lane, &[point]) {
                    app.selected_point = ids.first().map(|p| (lane, *p));
                    app.clear_clip_selection();
                }
                Some("Add automation point")
            }
            // Drags stay "Automation", so a drag's frames merge into one step.
            Edit::Move(point, tick, value) => {
                let _ = app
                    .session
                    .project
                    .set_point(point, Some(tick), Some(value), None);
                None
            }
            Edit::Delete(point) => {
                let _ = app.session.project.remove_points(lane, &[point], None);
                if app.selected_point == Some((lane, point)) {
                    app.selected_point = None;
                }
                Some("Delete automation point")
            }
            Edit::Curve(point, curve) => {
                let _ = app
                    .session
                    .project
                    .set_point(point, None, None, Some(curve));
                Some("Curve automation")
            }
        };
        if let Some(label) = label {
            app.session.label_edit(label, None);
        }
    }
}

/// The "Add automation lane" menu for `owner`: its mixer controls, its sends and each
/// plug-in's automatable parameters (filtered, since some plug-ins have thousands). A target
/// that already has a lane is greyed out.
pub fn add_lane_menu(app: &mut DawApp, ui: &mut egui::Ui, owner: LaneOwner) {
    let Some(lanes) = app.session.project.lanes(owner) else {
        return;
    };
    let taken: Vec<AutomationTarget> = lanes.iter().map(|l| l.target).collect();
    let mut chosen: Option<AutomationTarget> = None;
    let mut offer = |ui: &mut egui::Ui, label: &str, target: AutomationTarget| {
        if ui
            .add_enabled(!taken.contains(&target), egui::Button::new(label))
            .clicked()
        {
            chosen = Some(target);
            ui.close();
        }
    };
    offer(ui, "Volume", AutomationTarget::Volume);
    offer(ui, "Pan", AutomationTarget::Pan);
    offer(ui, "Mute", AutomationTarget::Mute);
    let project = &app.session.project;
    let (sends, slots) = match owner {
        Some(track) => {
            let t = project.track(track);
            let sends: Vec<(u32, String)> = t
                .map(|t| {
                    t.sends
                        .iter()
                        .map(|s| {
                            let to = project
                                .track(s.target)
                                .map_or("?".into(), |t| t.name.clone());
                            (s.id, to)
                        })
                        .collect()
                })
                .unwrap_or_default();
            let slots = t.map(|t| t.inserts.clone()).unwrap_or_default();
            (sends, slots)
        }
        None => (Vec::new(), project.master.inserts.clone()),
    };
    if !sends.is_empty() {
        ui.separator();
        for (send, to) in &sends {
            offer(
                ui,
                &format!("Send → {to}"),
                AutomationTarget::SendLevel { send: *send },
            );
        }
    }
    if !slots.is_empty() {
        ui.separator();
    }
    for slot in &slots {
        ui.menu_button(&slot.name, |ui| {
            if !app.session.is_loaded(slot.id) {
                ui.label(RichText::new("Not loaded").weak());
                return;
            }
            ui.add(
                egui::TextEdit::singleline(&mut app.param_filter)
                    .hint_text("Filter parameters")
                    .desired_width(180.0),
            );
            let needle = app.param_filter.to_lowercase();
            let matches: Vec<(u32, String)> = parameters(app, slot.id)
                .iter()
                .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
                .cloned()
                .collect();
            egui::ScrollArea::vertical()
                .max_height(320.0)
                .show(ui, |ui| {
                    for (parameter, name) in matches.iter().take(60) {
                        offer(
                            ui,
                            name,
                            AutomationTarget::Parameter {
                                insert: slot.id,
                                parameter: *parameter,
                            },
                        );
                    }
                });
            if matches.len() > 60 {
                ui.label(
                    RichText::new(format!(
                        "{} more: type to narrow them down",
                        matches.len() - 60
                    ))
                    .weak(),
                );
            } else if matches.is_empty() {
                ui.label(RichText::new("No parameters match").weak());
            }
        });
    }
    if let Some(target) = chosen {
        if let Err(e) = app.session.add_automation_lane(owner, target, None) {
            app.toast(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::automation::AutomationTarget::{Mute, Pan, Parameter, SendLevel, Volume};

    #[test]
    fn heights_and_values_map_both_ways() {
        for target in [
            Volume,
            Pan,
            SendLevel { send: 1 },
            Parameter {
                insert: 1,
                parameter: 2,
            },
        ] {
            let (low, high) = target.range();
            assert_eq!(height_of(&target, low), 0.0);
            assert_eq!(height_of(&target, high), 1.0);
            for h in [0.0f32, 0.1, 0.25, 0.5, 0.9, 1.0] {
                let back = height_of(&target, value_at_height(&target, h));
                assert!((back - h).abs() < 1e-6, "{target:?} at {h}");
            }
        }
        assert_eq!(height_of(&Pan, 0.0), 0.5, "the centre in the middle");
        // A fader's scale: the range that matters gets most of the lane.
        assert!(height_of(&Volume, 0.0) - height_of(&Volume, -24.0) > 0.35);
        assert!(height_of(&Volume, -30.0) > 0.15 && height_of(&Volume, 0.0) < 0.7);
        assert_eq!(value_at_height(&Volume, 2.0), 12.0, "clamped above");
        assert_eq!(value_at_height(&Volume, -1.0), -60.0, "and below");
    }

    #[test]
    fn only_a_hand_on_a_control_touches_it() {
        let none = Hands::default();
        assert_eq!(gesture(none, -23.98, -24.0), None, "rounding is no touch");
        let grabbed = Hands {
            grabbed: true,
            ..none
        };
        assert_eq!(
            gesture(grabbed, -24.0, -24.0),
            Some(Gesture::Touched(-24.0))
        );
        let held = Hands { held: true, ..none };
        assert_eq!(gesture(held, -24.0, -20.0), Some(Gesture::Touched(-20.0)));
        assert_eq!(gesture(held, -24.0, -24.0), None, "held still: nothing new");
        let let_go = Hands {
            let_go: true,
            ..none
        };
        assert_eq!(gesture(let_go, -20.0, -20.0), Some(Gesture::Released));
    }

    #[test]
    fn mute_snaps_to_its_two_states() {
        assert_eq!(value_at_height(&Mute, 0.49), 0.0);
        assert_eq!(value_at_height(&Mute, 0.5), 1.0);
        assert_eq!(value_at_height(&Mute, 0.9), 1.0);
        assert_eq!((height_of(&Mute, 0.0), height_of(&Mute, 1.0)), (0.0, 1.0));
    }

    #[test]
    fn values_read_in_their_units() {
        assert_eq!(format_value(&Volume, -6.04), "-6.0 dB");
        assert_eq!(format_value(&Pan, -0.3), "L30");
        assert_eq!(format_value(&Mute, 1.0), "muted");
        assert_eq!(
            format_value(
                &Parameter {
                    insert: 1,
                    parameter: 2
                },
                0.426
            ),
            "43%"
        );
    }
}
