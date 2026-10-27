//! The arrangement: track headers on the left, the timeline with clips on the right.

use crate::app::{DawApp, Meter, Selection};
use crate::theme;
use crate::value_edit;
use daw_core::api::{TimeSelection, MAX_WAVEFORM_SCALE};
use daw_core::automation::LaneOwner;
use daw_core::grid::{Division, Grid, Rounding, GRID_NOTES};
use daw_core::project::{
    Clip, ClipId, InsertId, Monitor, TempoChange, TrackId, TrackKind, MAX_TRACK_HEIGHT,
    MIN_TRACK_HEIGHT,
};
use daw_core::session::InsertTarget;
use daw_core::tempo::{LineKind, TempoMap};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke, StrokeKind, UiBuilder};

pub(crate) const HEADER_WIDTH: f32 = 250.0;
const RULER_HEIGHT: f32 = 26.0;
/// The marker strip under the ruler.
const MARKER_HEIGHT: f32 = 18.0;
/// The horizontal scroll bar and zoom buttons along the bottom.
const FOOTER_HEIGHT: f32 = 16.0;
pub(crate) const INDENT: f32 = 14.0;
/// The track menu's height presets, in points.
pub const TRACK_HEIGHT_PRESETS: [(&str, f32); 3] =
    [("Small", 40.0), ("Medium", 80.0), ("Large", 160.0)];

/// Maps between timeline samples and screen x within the lanes.
#[derive(Clone, Copy)]
pub(crate) struct TimeAxis {
    left: f32,
    start: f64,
    spp: f64,
}

impl TimeAxis {
    pub(crate) fn x(&self, sample: f64) -> f32 {
        self.left + ((sample - self.start) / self.spp) as f32
    }

    /// [`TimeAxis::x`] in f64, for positions far off screen (a file's start zoomed in).
    pub(crate) fn x64(&self, sample: f64) -> f64 {
        f64::from(self.left) + (sample - self.start) / self.spp
    }

    pub(crate) fn sample(&self, x: f32) -> f64 {
        self.start + f64::from(x - self.left) * self.spp
    }

    /// The sample under the pointer at `x`: the one whose span of the axis it is in, however
    /// far in or out the view is zoomed.
    pub(crate) fn sample_under(&self, x: f32) -> u64 {
        self.sample(x).floor().max(0.0) as u64
    }
}

/// Snap to the nearest line of the song's grid (17.4) when snapping is on, following the
/// tempo map; off, to the sample. Every snapped gesture in the Multitrack comes through here
/// (the ruler, the playhead's drag, clips, edges, selections, markers, automation points);
/// clip moves and edge trims first try the playhead's magnet ([`snap_clip`]).
pub(crate) fn snap(app: &DawApp, sample: f64) -> f64 {
    snap_to(
        &app.tempo_map,
        app.session.project.grid,
        app.samples_per_pixel,
        app.settings.snap,
        sample,
    )
}

/// [`snap`] without the app: on `map`'s grid `grid` at `spp` samples a point (which Auto's
/// division follows), or the sample itself (never before 0) when `on` is false.
pub(crate) fn snap_to(map: &TempoMap, grid: Grid, spp: f64, on: bool, sample: f64) -> f64 {
    let sample = sample.max(0.0);
    if !on {
        return sample;
    }
    map.snap_grid(sample as u64, grid, &grid_x(map, spp), Rounding::Nearest) as f64
}

/// The sample a click at `x` places the playhead on (25.2): the sample under it on `axis`,
/// snapped to `map`'s grid `grid` when `on` (Snap). The one rule for every click that moves
/// the playhead in the Multitrack: an empty part of a lane, a clip's body (audio or MIDI), the
/// ruler, the master's lane and an automation lane. No magnet: it pulls clips onto the
/// playhead, never the playhead somewhere.
pub(crate) fn clicked_sample(map: &TempoMap, grid: Grid, on: bool, axis: TimeAxis, x: f32) -> u64 {
    snap_to(map, grid, axis.spp, on, axis.sample(x)) as u64
}

/// A press at `pointer` at egui time `now` could be the second click of a double-click (26.2):
/// the last click (`last`: where and when) was under `delay` seconds ago (egui's
/// `max_double_click_delay`) and within `reach` points (`max_click_dist`), as egui counts one.
/// While it could, the playhead's handle takes no presses, so the second press goes to what
/// the first click hit (a clip opens, the ruler adds a marker) and not to the playhead that
/// click put under the pointer. A pointer egui doesn't know (it left the window) is far.
pub(crate) fn double_click_pending(
    last: Option<(egui::Pos2, f64)>,
    now: f64,
    pointer: Option<egui::Pos2>,
    delay: f64,
    reach: f32,
) -> bool {
    let (Some((at, when)), Some(pointer)) = (last, pointer) else {
        return false;
    };
    now - when < delay && at.distance(pointer) < reach
}

/// This frame's click noted in `DawApp::last_click` (any button, anywhere: egui's pointer is
/// the window's), and whether a press now could still be a double-click's second
/// ([`double_click_pending`] with egui's own limits). Every view whose click puts the playhead
/// under the pointer asks this before it lets the playhead's drag-only line take presses (the
/// Multitrack's handle, 26.2; the piano roll's line, 26.4). Each asks itself, since either may
/// be the only one shown; asking twice in a frame notes the same click.
pub(crate) fn double_click_waits(app: &mut DawApp, ui: &egui::Ui) -> bool {
    let (clicked, now, pointer) = ui.input(|i| {
        let clicked = i.pointer.any_click().then(|| i.pointer.interact_pos());
        (clicked.flatten(), i.time, i.pointer.latest_pos())
    });
    if let Some(at) = clicked {
        app.last_click = Some((at, now));
    }
    let (delay, reach) = ui.ctx().options(|o| {
        let input = &o.input_options;
        (input.max_double_click_delay, input.max_click_dist)
    });
    double_click_pending(app.last_click, now, pointer, delay, reach)
}

/// A drag of the Multitrack's playhead under way (26.3): where it has put the playhead, and
/// where the pointer was last seen. Kept on the app because neither may come from the engine
/// or from egui's response: the session's position follows a locate only once the audio
/// thread has run (a buffer or a FlexASIO burst later), and egui forgets the press's widget
/// once it loses the pointer (`PointerGone`), leaving the drag with no pointer position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PlayheadDrag {
    pub target: u64,
    pub pointer_x: f32,
}

/// One frame of a playhead drag (26.3) on `axis` (the view as this frame began) whose lanes
/// end at `right`, the pointer at `pointer_x`, `dt` seconds since the last frame: the view's
/// new start (scrolled while the pointer is near or past an edge, [`crate::zoom::edge_scroll`],
/// never before 0) and where the playhead goes in it ([`drag_target`]). Nothing here reads
/// the engine's position, so a playhead the engine has yet to move changes nothing.
pub(crate) fn drag_step(
    map: &TempoMap,
    grid: Grid,
    on: bool,
    axis: TimeAxis,
    right: f32,
    pointer_x: f32,
    dt: f32,
) -> (f64, u64) {
    let speed = crate::zoom::edge_scroll(pointer_x, axis.left, right);
    let start = (axis.start + f64::from(speed * dt) * axis.spp).max(0.0);
    let axis = TimeAxis { start, ..axis };
    (start, drag_target(map, grid, on, axis, right, pointer_x))
}

/// Where a playhead drag with the pointer at `pointer_x` puts the playhead on `axis` (the
/// view after this frame's scroll), the lanes ending at `right`: always in the lanes, at the
/// pointer or the edge it went past (the very start once the view is there and the pointer
/// is past the left edge). Snapped (`on`) to `map`'s grid line nearest it if that line is in
/// the lanes; else to the line on the inside of the edge it is past; else (zoomed in so far
/// that no line is near) the sample itself. Before 26.3 the nearest line could lie past the
/// edge, and the next frame's handle, drawn there, was off screen and never dragged again.
pub(crate) fn drag_target(
    map: &TempoMap,
    grid: Grid,
    on: bool,
    axis: TimeAxis,
    right: f32,
    pointer_x: f32,
) -> u64 {
    if axis.start <= 0.0 && pointer_x < axis.left {
        return 0;
    }
    let plain = axis.sample_under(pointer_x.clamp(axis.left + 1.0, right - 1.0));
    if !on {
        return plain;
    }
    let grid_x = grid_x(map, axis.spp);
    kept_on_screen(
        |rounding| map.snap_grid(plain, grid, &grid_x, rounding),
        plain,
        |sample| axis.x(sample as f64),
        axis.left,
        right,
    )
}

/// How far from the pointer (or the edge it went past) a playhead drag's grid line on the
/// inside may be, in points, when the nearest line is off screen: twice [`daw_core::grid::AUTO_GAP`], the
/// furthest apart Auto's note lines are, so at Auto's zooms the playhead stays on the grid by
/// the edge; zoomed in past the grid it stays at the edge instead of drifting off with a line.
const DRAG_LINE_REACH: f32 = 2.0 * daw_core::grid::AUTO_GAP as f32;

/// The snapped place a playhead drag puts down, kept in the lanes `left..=right` (26.3; the
/// Multitrack's and the piano roll's): `snapped(Nearest)` if it's there; else the line on the
/// inside of the edge it is past, if it's there and within [`DRAG_LINE_REACH`] of `plain`;
/// else `plain` (the unsnapped sample, which the caller keeps in the lanes). `x_of` places a
/// sample on screen.
pub(crate) fn kept_on_screen(
    snapped: impl Fn(Rounding) -> u64,
    plain: u64,
    x_of: impl Fn(u64) -> f32,
    left: f32,
    right: f32,
) -> u64 {
    let inside = |sample: u64| (left..=right).contains(&x_of(sample));
    let nearest = snapped(Rounding::Nearest);
    if inside(nearest) {
        return nearest;
    }
    let inward = snapped(if x_of(nearest) > right {
        Rounding::Previous
    } else {
        Rounding::Next
    });
    if inside(inward) && (x_of(inward) - x_of(plain)).abs() <= DRAG_LINE_REACH {
        inward
    } else {
        plain
    }
}

/// The playhead to a click at `x` ([`clicked_sample`] with the app's grid and Snap). A
/// refusal (recording) is reported as a lane click's always was.
pub(crate) fn locate_click(app: &mut DawApp, axis: TimeAxis, x: f32) {
    let at = clicked_sample(
        &app.tempo_map,
        app.session.project.grid,
        app.settings.snap,
        axis,
        x,
    );
    let located = app.session.locate(at);
    app.report(located);
}

/// Where song tick `tick` is in the Multitrack at `spp` samples a point, in points from the
/// timeline's start: the zoom the grid's lines are thinned by and Auto picks its division by.
pub(crate) fn grid_x(map: &TempoMap, spp: f64) -> impl Fn(f64) -> f64 + '_ {
    move |tick| map.sample_at_tick(tick) / spp
}

/// How near, in points, a dragged clip's start or end must come to the playhead for the
/// magnet to put it there (17.5). In points, not samples, so it reaches as far on screen at
/// every zoom.
pub(crate) const MAGNET_REACH: f64 = 8.0;

/// A dragged clip's length, for the magnet: an audio clip's in samples; a MIDI clip's in
/// ticks, since where its end falls follows the tempo map from wherever it lands.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Span {
    Samples(f64),
    Ticks(f64),
}

impl Span {
    /// The length of `clip` as dragged.
    pub(crate) fn of(clip: &daw_core::project::AnyClip) -> Span {
        match clip {
            daw_core::project::AnyClip::Audio(c) => Span::Samples(c.length as f64),
            daw_core::project::AnyClip::Midi(c) => Span::Ticks(c.length as f64),
        }
    }
}

/// Where a clip dragged to start at `start` (the pointer's place, unsnapped) goes: onto the
/// playhead when its start or end is within [`MAGNET_REACH`] points of it (the nearer edge
/// wins), else to the grid by [`snap_to`]. The magnet works whether Snap is on or off, and
/// wins over the grid within its reach; `ctrl` (held this frame) turns it off. Returns the
/// start and whether the magnet holds it. The reach is judged on the unsnapped position, so a
/// clip hopping from grid line to grid line can't jump over the playhead. An end that would
/// need the clip to start before 0 is no candidate. A trimmed edge is a clip of length 0.
#[allow(clippy::too_many_arguments)]
pub(crate) fn snap_clip_to(
    map: &TempoMap,
    grid: Grid,
    spp: f64,
    on: bool,
    playhead: f64,
    start: f64,
    span: Span,
    ctrl: bool,
) -> (f64, bool) {
    let start = start.max(0.0);
    if !ctrl {
        if let Some(at) = magnet(map, spp, playhead, start, span) {
            return (at, true);
        }
    }
    (snap_to(map, grid, spp, on, start), false)
}

/// The magnet alone: the start that puts the nearer edge of a clip at `start` long `span` on
/// `playhead`, if that edge is within reach and the clip would not start before 0.
fn magnet(map: &TempoMap, spp: f64, playhead: f64, start: f64, span: Span) -> Option<f64> {
    let reach = MAGNET_REACH * spp;
    // Each edge: how far it is from the playhead (samples) and the start that puts it there.
    let (end, start_for_end) = match span {
        Span::Samples(length) => (start + length, playhead - length),
        Span::Ticks(ticks) => {
            let end = map.sample_at_tick(map.tick_at(start) + ticks);
            let tick = map.tick_at(playhead) - ticks;
            (
                end,
                if tick < 0.0 {
                    -1.0
                } else {
                    map.sample_at_tick(tick)
                },
            )
        }
    };
    [
        ((start - playhead).abs(), playhead),
        ((end - playhead).abs(), start_for_end),
    ]
    .into_iter()
    .filter(|&(away, at)| away <= reach && at >= 0.0)
    .min_by(|a, b| a.0.total_cmp(&b.0))
    .map(|(_, at)| at)
}

/// [`snap_clip_to`] in the app: its tempo map, grid, zoom, Snap and playhead.
pub(crate) fn snap_clip(app: &DawApp, start: f64, span: Span, ctrl: bool) -> (f64, bool) {
    snap_clip_to(
        &app.tempo_map,
        app.session.project.grid,
        app.samples_per_pixel,
        app.settings.snap,
        app.session.position() as f64,
        start,
        span,
        ctrl,
    )
}

/// The division the Multitrack's grid shows where the view starts (Auto's follows the zoom).
/// From the session's map, not the frame's copy, so an agent's tempo change is seen before
/// the Multitrack next draws.
pub(crate) fn grid_division(app: &DawApp) -> Division {
    grid_division_at(
        &app.session.tempo_map(),
        app.session.project.grid,
        app.view_start,
        app.samples_per_pixel,
    )
}

/// [`grid_division`] for a view from sample `start` at `spp` samples a point.
pub(crate) fn grid_division_at(map: &TempoMap, grid: Grid, start: f64, spp: f64) -> Division {
    let bar = map.bar_at(start.max(0.0)).0;
    map.division_in(grid, bar, &grid_x(map, spp))
}

/// The song's grid choices, for the ruler's and the piano roll's menus: Auto, then 1/4 to
/// 1/64, and Triplets (for the fixed ones). Returns the grid picked, if one was.
pub(crate) fn grid_choices(ui: &mut egui::Ui, grid: Grid, shown: Division) -> Option<Grid> {
    let mut picked = None;
    let auto = grid == Grid::Auto;
    let label = if auto {
        format!("Auto (now {shown})")
    } else {
        "Auto".to_string()
    };
    if ui
        .add(egui::Button::selectable(auto, label))
        .on_hover_text("The division follows the zoom: finer as you zoom in, coarser out")
        .clicked()
    {
        picked = Some(Grid::Auto);
    }
    let triplet = matches!(grid, Grid::Fixed(Division::Note { triplet: true, .. }));
    for n in GRID_NOTES {
        let this = Grid::Fixed(Division::Note { n, triplet });
        let on = grid == this;
        let text = format!("1/{n}");
        if ui.add(egui::Button::selectable(on, text)).clicked() {
            picked = Some(this);
        }
    }
    let mut thirds = triplet;
    if ui
        .add_enabled(!auto, crate::theme::checkbox(&mut thirds, "Triplets"))
        .on_hover_text("Three in the time of two of the division")
        .changed()
    {
        if let Grid::Fixed(Division::Note { n, .. }) = grid {
            picked = Some(Grid::Fixed(Division::Note { n, triplet: thirds }));
        }
    }
    picked
}

pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    crate::opening::multitrack_drawn(app);
    // Built once a frame: the ruler, the grid and snapping all read it.
    app.tempo_map = app.session.tempo_map();
    let whole = ui.available_rect_before_wrap();
    let footer = Rect::from_min_max(
        pos2(whole.left(), whole.bottom() - FOOTER_HEIGHT),
        whole.max,
    );
    let full = Rect::from_min_max(whole.min, pos2(whole.right(), footer.top()));
    let axis = TimeAxis {
        left: full.left() + HEADER_WIDTH,
        start: app.view_start,
        spp: app.samples_per_pixel,
    };
    let lanes_rect = Rect::from_min_max(pos2(axis.left, full.top()), full.max);
    app.lanes_width = lanes_rect.width();
    // Set again by the lane whose edge the pointer is on (`clip_edges`).
    app.edge_lit = None;

    handle_zoom_and_scroll(app, ui, whole, axis.left);
    footer_ui(app, ui, footer, axis.left);
    let axis = TimeAxis {
        start: app.view_start,
        spp: app.samples_per_pixel,
        ..axis
    };

    // Ruler with the add-track buttons beside it.
    let ruler = Rect::from_min_size(
        pos2(axis.left, full.top()),
        vec2(full.width() - HEADER_WIDTH, RULER_HEIGHT),
    );
    let corner = Rect::from_min_size(full.min, vec2(HEADER_WIDTH, RULER_HEIGHT));
    ui.scope_builder(
        UiBuilder::new().max_rect(corner.shrink2(vec2(6.0, 2.0))),
        |ui| {
            ui.horizontal_centered(|ui| {
                let add = ui.menu_button("+ Track", |ui| {
                    if ui.button("Audio track").clicked() {
                        app.add_track(TrackKind::Audio);
                    }
                    if ui.button("Instrument track").clicked() {
                        app.add_track(TrackKind::Instrument);
                    }
                    if ui.button("Folder (submix)").clicked() {
                        app.add_track(TrackKind::Folder);
                    }
                    if ui.button("Bus (return)").clicked() {
                        app.add_track(TrackKind::Bus);
                    }
                });
                crate::marks::put(ui, "arrange.add_track", add.response.rect);
                let master_selected = app.selection == Some(Selection::Master);
                if ui.selectable_label(master_selected, "Master").clicked() {
                    app.selection = Some(Selection::Master);
                }
            });
        },
    );
    draw_ruler(app, ui, ruler, axis);
    let strip_top = full.top() + RULER_HEIGHT;
    let strip_corner = Rect::from_min_size(
        pos2(full.left(), strip_top),
        vec2(HEADER_WIDTH, MARKER_HEIGHT),
    );
    ui.painter().rect_filled(strip_corner, 0, theme::BG);
    ui.painter().text(
        pos2(strip_corner.left() + 8.0, strip_corner.center().y),
        Align2::LEFT_CENTER,
        "Markers",
        FontId::proportional(10.0),
        theme::TEXT_DIM,
    );
    let strip = Rect::from_min_size(
        pos2(axis.left, strip_top),
        vec2(full.width() - HEADER_WIDTH, MARKER_HEIGHT),
    );
    draw_markers(app, ui, strip, axis);

    // Tracks.
    let body = Rect::from_min_max(pos2(full.left(), strip_top + MARKER_HEIGHT), full.max);
    let mut body_ui = ui.new_child(UiBuilder::new().max_rect(body));
    // No springing back past an end: an offset set below (rows zoomed in place) is drawn as
    // set, and a mouse wheel never rubber-bands anyway.
    body_ui.spacing_mut().scroll.kinetic.rubber_band = false;
    let mut rows_area = egui::ScrollArea::vertical().auto_shrink(false);
    // Heights changed this frame around the pointer: drawn in place at once.
    if let Some(offset) = app.track_scroll_to.take() {
        rows_area = rows_area.vertical_scroll_offset(offset);
    }
    let rows_area = rows_area.show(&mut body_ui, |ui| {
        ui.spacing_mut().item_spacing.y = 0.0;
        let ids: Vec<TrackId> = app
            .session
            .project
            .tracks
            .iter()
            .filter(|t| !app.session.project.is_hidden(t.id))
            .map(|t| t.id)
            .collect();
        // The master first, as a row of its own (no clips), then the tracks.
        let mut master = master_row(app, ui, axis);
        if let Some(rows) = crate::automation::lane_rows(app, ui, None, axis) {
            master = master.union(rows);
        }
        app.master_row = Some(master);
        // Where each row is this frame: a file dropped on one lands on that track.
        app.drop_rows.clear();
        for (row, id) in ids.into_iter().enumerate() {
            track_row(app, ui, id, row, axis);
            // A file dropped on a track's lane rows lands on the track.
            if let Some(rows) = crate::automation::lane_rows(app, ui, Some(id), axis) {
                if let Some(last) = app.drop_rows.last_mut() {
                    last.2 = last.2.union(rows);
                }
            }
        }
        apply_clip_drag(app, ui);
        let rows = app.drop_rows.clone();
        crate::track_drag::apply(app, ui, crate::track_drag::Surface::Arrange, &rows);
        time_selection_drag(app, ui, axis);
        draw_time_selection(app, ui, axis);
        // The empty space below the tracks.
        let rest = ui.available_rect_before_wrap();
        let rest = Rect::from_min_size(
            rest.min,
            vec2(rest.width(), rest.height().max(crate::zoom::ROOM_BELOW)),
        );
        let response = ui.allocate_rect(rest, Sense::click());
        if response.double_clicked() {
            app.add_track(TrackKind::Audio);
        } else if response.clicked() {
            app.selection = None;
            app.clear_clip_selection();
        }
        if app.session.project.tracks.is_empty() {
            ui.painter().text(
                rest.center_top() + vec2(0.0, 40.0),
                Align2::CENTER_CENTER,
                "Double-click to add an audio track, or drop audio or MIDI files here",
                FontId::proportional(14.0),
                theme::TEXT_DIM,
            );
        }
        response.context_menu(|ui| {
            if ui.button("Add audio track").clicked() {
                app.add_track(TrackKind::Audio);
            }
            if ui.button("Add instrument track").clicked() {
                app.add_track(TrackKind::Instrument);
            }
            if ui.button("Add folder").clicked() {
                app.add_track(TrackKind::Folder);
            }
            if ui.button("Add bus (return)").clicked() {
                app.add_track(TrackKind::Bus);
            }
        });
    });
    app.track_view = Some((rows_area.inner_rect, rows_area.state.unclamped_offset().y));

    // A faint tint over the lanes inside an active loop.
    if let Some((start, end)) = app.session.project.loop_region.active() {
        let lanes = Rect::from_min_max(pos2(axis.left, body.top()), full.max);
        let band =
            Rect::from_x_y_ranges(axis.x(start as f64)..=axis.x(end as f64), lanes.y_range());
        ui.painter_at(lanes)
            .rect_filled(band, 0, theme::LOOP.gamma_multiply(0.06));
    }
    // And a red one inside an active punch range.
    if let Some((start, end)) = app.session.project.punch.active() {
        let lanes = Rect::from_min_max(pos2(axis.left, body.top()), full.max);
        let band =
            Rect::from_x_y_ranges(axis.x(start as f64)..=axis.x(end as f64), lanes.y_range());
        ui.painter_at(lanes)
            .rect_filled(band, 0, theme::RECORD.gamma_multiply(0.05));
    }

    // Playhead over ruler and lanes. Drag it (anywhere along it) to move it: with snap on, to
    // the grid's line nearest the pointer, as the ruler's click; off, to the sample under it.
    // Added after the clips and the ruler, so it wins where they meet; but not while a press
    // could be a double-click's second (26.2): a click on a clip, a lane or the ruler puts the
    // playhead right under the pointer, and egui gives a press on a drag-only widget drawn over
    // a clickable one to nobody's click, so the clip would never see its double-click.
    let waiting = double_click_waits(app, ui);
    let handle_id = ui.scope_id().with("playhead");
    app.playhead_handle = Some(handle_id);
    let held = ui.ctx().is_being_dragged(handle_id);
    if !held {
        // Let go (or Escape): the playhead is where the drag last put it.
        app.playhead_drag = None;
    }
    let takes_presses = held || !waiting;
    // While a drag is under way the playhead is drawn, and its handle placed, where the drag
    // put it, and the handle is interacted wherever that is (26.3): the session's position
    // lags a locate by a buffer or more, and a handle placed there could be off screen and so
    // never dragged again while egui still held the drag.
    let shown = app
        .playhead_drag
        .map_or_else(|| app.session.position(), |d| d.target);
    let x = axis.x(shown as f64);
    app.playhead_grab = None;
    if held || (x >= axis.left - 1.0 && x <= full.right()) {
        let x = x.clamp(axis.left - 1.0, full.right());
        let grab = Rect::from_x_y_ranges(x - 3.0..=x + 3.0, full.y_range());
        let handle = takes_presses.then(|| ui.interact(grab, handle_id, Sense::drag()));
        let dragging = handle.as_ref().is_some_and(|h| h.dragged());
        if handle.as_ref().is_some_and(|h| h.hovered()) || dragging {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
        }
        let mut x = x;
        if dragging {
            // egui's pointer, not the handle's `interact_pointer_pos`: that is gone for the
            // rest of the drag once egui loses the pointer (a lone `PointerGone`). With no
            // pointer at all (out of the window), the last one seen.
            let seen = ui.input(|i| i.pointer.latest_pos()).map(|p| p.x);
            let drag = app.playhead_drag.unwrap_or(PlayheadDrag {
                target: app.session.position(),
                pointer_x: seen.unwrap_or(x),
            });
            let pointer_x = seen.unwrap_or(drag.pointer_x);
            // Near or past either edge the view scrolls with it (stopping at the start), and
            // the playhead stays in the lanes, at the pointer or the edge it went past.
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            let axis = TimeAxis {
                start: app.view_start,
                ..axis
            };
            let (start, at) = drag_step(
                &app.tempo_map,
                app.session.project.grid,
                app.settings.snap,
                axis,
                full.right(),
                pointer_x,
                dt,
            );
            if start != app.view_start {
                app.view_start = start;
                ui.ctx().request_repaint();
            }
            let mut target = drag.target;
            if at != target {
                let located = app.session.locate(at);
                if located.is_ok() {
                    target = at;
                }
                app.report(located);
            }
            app.playhead_drag = Some(PlayheadDrag { target, pointer_x });
            // Drawn in the view as it now is (next frame's), where the drag put it.
            x = TimeAxis { start, ..axis }
                .x(target as f64)
                .clamp(axis.left - 1.0, full.right());
        }
        app.playhead_grab = Some((pos2(x, full.center().y), axis.left));
        if let Some(handle) = handle {
            handle.on_hover_text(if app.settings.snap {
                "Drag to move the playhead along the grid (Snap is on)"
            } else {
                "Drag to move the playhead, sample by sample (Snap is off)"
            });
        }
        let painter = ui.painter_at(Rect::from_min_max(pos2(axis.left, full.top()), full.max));
        // A dragged clip (or a trimmed edge) held on it by the magnet (17.5): lit, so the
        // jump is seen to come from it.
        let magnet = app.group_drag.as_ref().is_some_and(|d| d.magnet)
            || (app.edge_magnet && app.edge_drag.is_some());
        if magnet {
            painter.vline(
                x,
                full.y_range(),
                Stroke::new(7.0, theme::PLAYHEAD.gamma_multiply(0.35)),
            );
            painter.vline(x, full.y_range(), Stroke::new(2.5, theme::PLAYHEAD_LIT));
        } else {
            painter.vline(
                x,
                full.y_range(),
                Stroke::new(if dragging { 2.5 } else { 1.5 }, theme::PLAYHEAD),
            );
        }
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x - 6.0, full.top()),
                pos2(x + 6.0, full.top()),
                pos2(x, full.top() + 8.0),
            ],
            theme::PLAYHEAD,
            Stroke::NONE,
        ));
    }
    // Following holds off while the playhead is dragged: the drag moves the view itself.
    if app.follow_playhead && app.session.is_playing() && app.playhead_drag.is_none() {
        let width = f64::from(full.width() - HEADER_WIDTH);
        let pos = app.session.position() as f64;
        let visible = width * app.samples_per_pixel;
        if pos > app.view_start + visible * 0.9 || pos < app.view_start {
            app.view_start = (pos - visible * 0.1).max(0.0);
        }
    }
    // Moved in the piano roll (16.3): shown here too, scrolled to if it is off screen.
    if std::mem::take(&mut app.reveal_playhead) {
        let width = f64::from(full.width() - HEADER_WIDTH);
        let pos = app.session.position() as f64;
        let visible = width * app.samples_per_pixel;
        if pos > app.view_start + visible || pos < app.view_start {
            app.view_start = (pos - visible * 0.1).max(0.0);
        }
    }

    // An edge drag whose clip left the screen (or the project) never hears its release.
    if app.edge_drag.is_some() && !ui.input(|i| i.pointer.any_down()) {
        app.edge_drag = None;
        app.edge_magnet = false;
    }

    draw_drop_ghost(app, ui, lanes_rect, axis);
    handle_dropped_files(app, ui, lanes_rect, axis);
}

/// Along the bottom: zoom buttons under the headers, a scroll bar for the timeline under the
/// lanes.
fn footer_ui(app: &mut DawApp, ui: &mut egui::Ui, rect: Rect, lanes_left: f32) {
    ui.painter().rect_filled(rect, 0, theme::PANEL);
    let corner = Rect::from_min_max(rect.min, pos2(lanes_left, rect.bottom()));
    ui.scope_builder(
        UiBuilder::new().max_rect(corner.shrink2(vec2(6.0, 0.0))),
        |ui| {
            let zoom = ui.horizontal_centered(|ui| {
                ui.spacing_mut().button_padding = vec2(5.0, 0.0);
                if ui
                    .small_button("−")
                    .on_hover_text("Zoom out (-, Ctrl+wheel)")
                    .clicked()
                {
                    app.zoom_step(false);
                }
                if ui
                    .small_button("+")
                    .on_hover_text("Zoom in (=, Ctrl+wheel)")
                    .clicked()
                {
                    app.zoom_step(true);
                }
                if ui
                    .small_button("Fit")
                    .on_hover_text("Show the whole project (F)")
                    .clicked()
                {
                    app.zoom_to_fit();
                }
                if app.waveform_scale > 1.0 {
                    let label = format!("Wave ×{:.1}", app.waveform_scale);
                    if ui
                        .small_button(label)
                        .on_hover_text("Waveform display gain (Alt+Shift+wheel). Click to reset.")
                        .clicked()
                    {
                        app.waveform_scale = 1.0;
                    }
                }
            });
            crate::marks::put(ui, "arrange.zoom", zoom.response.rect);
        },
    );

    // The scroll bar spans the project plus a little room, or the view if that reaches further.
    let bar = Rect::from_min_max(pos2(lanes_left, rect.top()), rect.max).shrink2(vec2(2.0, 4.0));
    let visible = f64::from(app.lanes_width.max(1.0)) * app.samples_per_pixel;
    let total = (app.session.content_end() as f64 * 1.1)
        .max(app.view_start + visible)
        .max(1.0);
    let scale = f64::from(bar.width()) / total;
    let thumb_left = bar.left() + (app.view_start * scale) as f32;
    let thumb_width = ((visible * scale) as f32).max(16.0).min(bar.width());
    // Zoomed far in the thumb is wider than the view's share: keep it inside the bar.
    let thumb_left = thumb_left.min(bar.right() - thumb_width);
    let thumb = Rect::from_min_size(pos2(thumb_left, bar.top()), vec2(thumb_width, bar.height()));
    let response = ui
        .interact(
            bar,
            ui.scope_id().with("timeline_scroll"),
            Sense::click_and_drag(),
        )
        .on_hover_text("Drag to scroll the timeline (Shift+wheel also scrolls)");
    if response.dragged() {
        let view = app
            .timeline()
            .scroll(-f64::from(response.drag_delta().x) / scale / app.samples_per_pixel);
        app.set_timeline(view);
    } else if response.clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            if !thumb.contains(pos) {
                // Jump so the clicked point sits in the middle.
                let centre = f64::from(pos.x - bar.left()) / scale;
                app.view_start = (centre - visible / 2.0).max(0.0);
            }
        }
    }
    let painter = ui.painter_at(rect);
    painter.rect_filled(bar, 4, theme::BG);
    let active = response.hovered() || response.dragged();
    let colour = if active {
        theme::TEXT_DIM
    } else {
        theme::GRID_BAR
    };
    painter.rect_filled(thumb, 4, colour);
}

/// The mouse wheel anywhere over the arrangement (headers included):
///
/// - wheel: scroll the tracks up and down (the scroll area below does this);
/// - Ctrl+wheel (or a pinch): zoom the timeline around the pointer;
/// - Shift+wheel (or a sideways swipe): scroll the timeline;
/// - Alt+wheel: every track taller or shorter;
/// - Alt+Shift+wheel: waveform display gain.
///
/// egui turns Ctrl+wheel into a zoom delta and Shift+wheel into a sideways scroll itself. The
/// Alt gestures take the scroll delta away from the scroll area, so the rows don't also move;
/// Alt+wheel sets the rows' offset in the same frame so they grow and shrink around the pointer
/// (26.1). egui spreads a notch over several frames: what a notch does follows the modifiers its
/// own wheel event carried (`alt_wheel`), so letting go of Alt partway never scrolls the rest.
fn handle_zoom_and_scroll(app: &mut DawApp, ui: &egui::Ui, area: Rect, lanes_left: f32) {
    let (scroll, zoom_delta, modifiers, pointer, notch, scrolling) = ui.input(|i| {
        (
            i.smooth_scroll_delta,
            i.zoom_delta(),
            i.modifiers,
            i.pointer.hover_pos(),
            i.events.iter().rev().find_map(|e| match e {
                egui::Event::MouseWheel { modifiers, .. } => Some(*modifiers),
                _ => None,
            }),
            i.is_scrolling(),
        )
    });
    if let Some(held) = notch {
        app.alt_wheel = held.alt.then_some(held.shift);
    } else if !scrolling {
        app.alt_wheel = None;
    }
    if !ui.rect_contains_pointer(area) {
        return;
    }
    // A notch begun with Alt, or Alt pressed during one (as before 26.1).
    let alt = app
        .alt_wheel
        .or_else(|| modifiers.alt.then_some(modifiers.shift));
    let rate = app.session.sample_rate();
    if zoom_delta != 1.0 {
        // Over the headers, zoom around the lanes' left edge.
        let x = pointer.map_or(0.0, |p| (p.x - lanes_left).max(0.0));
        let view = app
            .timeline()
            .zoom_around(1.0 / f64::from(zoom_delta), f64::from(x), rate);
        app.set_timeline(view);
    } else if let Some(shift) = alt.filter(|_| scroll != egui::Vec2::ZERO) {
        // Alt may arrive as either axis (Alt+Shift especially); take whichever moved.
        let wheel = scroll.x + scroll.y;
        if shift {
            // No heights change: the rows stay as they are.
            let factor = (1.0 + wheel / 200.0).clamp(0.5, 2.0);
            app.waveform_scale = (app.waveform_scale * factor).clamp(1.0, MAX_WAVEFORM_SCALE);
        } else {
            let before = drawn_heights(app);
            scale_track_heights(app, wheel);
            if let Some(y) = pointer.map(|p| p.y) {
                app.track_scroll_to = rows_in_place(app, &before, y);
            }
        }
        ui.ctx()
            .input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
    } else if scroll.x != 0.0 {
        let view = app.timeline().scroll(f64::from(scroll.x));
        app.set_timeline(view);
    }
}

/// A range drawn in the ruler (20.4): the loop's (or with `punch`, the punch range's) start and
/// end, in samples, and nothing else: whether it loops (or punches) stays as it was, for the Loop
/// (Punch) button to switch, as `set_loop` with a range and no `enabled` does. The time
/// selection's Loop (L) is the one gesture that also turns it on.
pub(crate) fn drawn_range(
    session: &mut daw_core::session::Session,
    start: u64,
    end: u64,
    punch: bool,
) -> anyhow::Result<()> {
    if punch {
        session.set_punch_range(start, end)
    } else {
        session.set_loop_region(start, end)
    }
}

fn draw_ruler(app: &mut DawApp, ui: &mut egui::Ui, rect: Rect, axis: TimeAxis) {
    app.ruler_rect = Some(rect);
    let response = ui
        .allocate_rect(rect, Sense::click_and_drag())
        .on_hover_text(
            "Click to move the playhead, drag to set the loop range, Shift+drag to set the punch \
             range (Loop and Punch turn them on)",
        );
    crate::marks::put(ui, "arrange.ruler", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0, theme::BG);
    // The loop region: a band across the ruler, bright while looping is on.
    let region = app.session.project.loop_region;
    if region.end > region.start {
        let color = if region.enabled {
            theme::LOOP
        } else {
            theme::TEXT_DIM
        };
        let (x0, x1) = (axis.x(region.start as f64), axis.x(region.end as f64));
        let band = Rect::from_x_y_ranges(x0..=x1, rect.y_range());
        crate::marks::put(ui, "arrange.loop_range", band.intersect(rect));
        painter.rect_filled(band, 0, color.gamma_multiply(0.22));
        for x in [x0, x1] {
            painter.vline(x, rect.y_range(), Stroke::new(1.5, color));
        }
    }
    // The punch range: a strip along the bottom of the ruler, red while punch is on.
    let punch = app.session.project.punch;
    if punch.end > punch.start {
        let color = if punch.enabled {
            theme::RECORD
        } else {
            theme::TEXT_DIM
        };
        let strip = Rect::from_x_y_ranges(
            axis.x(punch.start as f64)..=axis.x(punch.end as f64),
            (rect.bottom() - 6.0)..=rect.bottom(),
        );
        painter.rect_filled(strip, 0, color.gamma_multiply(0.8));
    }
    let map = &app.tempo_map;
    // The division's lines as short ticks along the bottom, under the beats'.
    for (x, kind) in grid_xs(map, app.session.project.grid, axis, rect) {
        if kind == LineKind::Division {
            painter.vline(
                x,
                (rect.bottom() - 3.0)..=rect.bottom(),
                Stroke::new(1.0, theme::GRID_BAR),
            );
        }
    }
    let view = (axis.start.max(0.0), axis.sample(rect.right()));
    // Label every bar, or every 2/4/8/... bars when zoomed out (judged by the bar in view).
    let (first, _) = map.bar_at(view.0);
    let bar_px = (map.bar_start(first + 1) - map.bar_start(first)) / axis.spp;
    let mut every = 1i64;
    while (bar_px * every as f64) < 48.0 {
        every *= 2;
    }
    // From the bar the view starts in, so a view inside one bar still gets its beats.
    for (bar, at) in map.bars_between(map.bar_start(first), view.1) {
        let x = axis.x(at);
        if bar.rem_euclid(every) == 0 {
            painter.vline(x, rect.y_range(), Stroke::new(1.0, theme::GRID_BAR));
            painter.text(
                pos2(x + 4.0, rect.top() + 3.0),
                Align2::LEFT_TOP,
                (bar + 1).to_string(),
                FontId::monospace(11.0),
                theme::TEXT_DIM,
            );
        }
        let width = (map.bar_start(bar + 1) - at) / axis.spp;
        if width >= 60.0 {
            let first_beat = map.beats_at(at).round();
            let beats = map.meter_at(at.round() as i64).numerator;
            // Zoomed in far enough, beats are numbered too ("2.3" is bar 2, beat 3).
            let label_beats = width / f64::from(beats) >= 50.0;
            for k in 1..beats {
                let bx = axis.x(map.sample_of_beat(first_beat + f64::from(k)));
                painter.vline(
                    bx,
                    (rect.bottom() - 6.0)..=rect.bottom(),
                    Stroke::new(1.0, theme::GRID_BAR),
                );
                if label_beats {
                    painter.text(
                        pos2(bx + 3.0, rect.top() + 5.0),
                        Align2::LEFT_TOP,
                        format!("{}.{}", bar + 1, k + 1),
                        FontId::monospace(9.0),
                        theme::TEXT_DIM.gamma_multiply(0.7),
                    );
                }
            }
        }
    }
    // A marker where the tempo or meter changes, saying what to.
    for change in &app.session.project.tempo_changes {
        let bar = map.bar_start(i64::from(change.bar));
        let at = map.sample_of_beat(map.beats_at(bar) + f64::from(change.beat));
        let x = axis.x(at);
        if x < rect.left() - 80.0 || x > rect.right() {
            continue;
        }
        let mut label = Vec::new();
        if let Some((n, d)) = change.time_signature {
            label.push(format!("{n}/{d}"));
        }
        if let Some(tempo) = change.tempo {
            label.push(format!("{tempo:.0}"));
        }
        painter.add(egui::Shape::convex_polygon(
            vec![
                pos2(x, rect.bottom() - 9.0),
                pos2(x + 6.0, rect.bottom() - 9.0),
                pos2(x, rect.bottom() - 1.0),
            ],
            theme::MUTE,
            Stroke::NONE,
        ));
        // On a bar line, after the bar number.
        let digits = (change.bar + 1).to_string().len() as f32;
        let gap = if change.beat == 0 {
            10.0 + 7.0 * digits
        } else {
            7.0
        };
        painter.text(
            pos2(x + gap, rect.top() + 4.0),
            Align2::LEFT_TOP,
            label.join(" · "),
            FontId::proportional(10.0),
            theme::MUTE,
        );
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::GRID_BAR),
    );
    tempo_menu(app, &response, axis);
    ruler_selection(app, ui, rect, axis);
    // Click to locate; drag to draw the loop range (Shift: the punch range), leaving it on or
    // off as it was (20.4): the Loop and Punch buttons switch them.
    if response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            locate_click(app, axis, p.x);
        }
    }
    if response.drag_started() {
        let (origin, shift) = ui.input(|i| (i.pointer.press_origin(), i.modifiers.shift));
        app.ruler_drag = origin.map(|p| (snap(app, axis.sample(p.x)), shift));
    }
    if response.dragged() {
        if let (Some((anchor, punch)), Some(p)) = (app.ruler_drag, response.interact_pointer_pos())
        {
            let here = snap(app, axis.sample(p.x));
            let (start, end) = (anchor.min(here) as u64, anchor.max(here) as u64);
            // An empty range (no movement yet) is not worth reporting.
            if end > start {
                let done = drawn_range(&mut app.session, start, end, punch);
                if done.is_err() {
                    // Refused (recording): say why once and drop the drag.
                    app.ruler_drag = None;
                    app.report(done);
                }
            }
        }
    }
    if response.drag_stopped() {
        app.ruler_drag = None;
    }
}

/// The time selection on the ruler: a band along its top whose edges drag (snapped). Added
/// after the ruler itself, so the edges win over its playhead and loop drags.
fn ruler_selection(app: &mut DawApp, ui: &mut egui::Ui, rect: Rect, axis: TimeAxis) {
    let Some(range) = app.time_selection.clone() else {
        return;
    };
    let (x0, x1) = (axis.x(range.start as f64), axis.x(range.end as f64));
    let band = Rect::from_x_y_ranges(
        x0.max(rect.left())..=x1.min(rect.right()),
        rect.top()..=rect.top() + 5.0,
    );
    if band.width() > 0.0 {
        ui.painter_at(rect)
            .rect_filled(band, 0, Color32::from_white_alpha(170));
    }
    for (k, x) in [x0, x1].into_iter().enumerate() {
        if x < rect.left() - 4.0 || x > rect.right() + 4.0 {
            continue;
        }
        let handle = Rect::from_x_y_ranges(x - 4.0..=x + 4.0, rect.y_range());
        let response = ui
            .interact(
                handle,
                ui.scope_id().with(("selection-edge", k)),
                Sense::drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            .on_hover_text("Drag to move the time selection's edge");
        if response.dragged() {
            if let Some(p) = response.interact_pointer_pos() {
                let at = snap(app, axis.sample(p.x)).max(0.0) as u64;
                let (start, end) = if k == 0 {
                    (at.min(range.end.saturating_sub(1)), range.end)
                } else {
                    (range.start, at.max(range.start + 1))
                };
                app.set_time_selection(
                    TimeSelection {
                        start,
                        end,
                        ..range.clone()
                    },
                    false,
                );
            }
        }
    }
}

/// Right-click on the ruler: the song's grid (17.4), and add, edit or remove the tempo or
/// meter change at that bar.
fn tempo_menu(app: &mut DawApp, response: &egui::Response, axis: TimeAxis) {
    if response.secondary_clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            app.ruler_menu_bar = app.tempo_map.bar_at(axis.sample(p.x).max(0.0)).0.max(0);
        }
    }
    response.context_menu(|ui| {
        let grid = app.session.project.grid;
        let shown = grid_division(app);
        let label = match grid {
            Grid::Auto => format!("Grid: Auto ({shown})"),
            Grid::Fixed(d) => format!("Grid: {d}"),
        };
        let menu = ui.menu_button(label, |ui| {
            if let Some(grid) = grid_choices(ui, grid, shown) {
                let done = app.session.set_grid(grid);
                app.report(done);
                ui.close();
            }
        });
        app.grid_menu_button = Some(menu.response.rect);
        ui.separator();
        let bar = app.ruler_menu_bar as u32;
        ui.label(egui::RichText::new(format!("Bar {}", bar + 1)).strong());
        if bar == 0 {
            ui.label("The opening tempo and meter are set in the transport bar.");
            return;
        }
        let existing = app
            .session
            .project
            .tempo_changes
            .iter()
            .find(|c| c.bar == bar && c.beat == 0)
            .copied();
        let here = app
            .tempo_map
            .meter_at(app.tempo_map.bar_start(i64::from(bar)) as i64);
        let mut change = existing.unwrap_or(TempoChange {
            bar,
            beat: 0,
            tempo: None,
            time_signature: None,
        });
        let before = change;
        ui.horizontal(|ui| {
            let mut on = change.tempo.is_some();
            ui.add(crate::theme::checkbox(&mut on, "Tempo"));
            let mut tempo = change.tempo.unwrap_or(here.tempo);
            ui.add_enabled(
                on,
                egui::DragValue::new(&mut tempo)
                    .range(20.0..=300.0)
                    .speed(0.2)
                    .fixed_decimals(1)
                    .suffix(" BPM"),
            );
            change.tempo = on.then_some(tempo);
        });
        ui.horizontal(|ui| {
            let mut on = change.time_signature.is_some();
            ui.add(crate::theme::checkbox(&mut on, "Time signature"));
            let (mut n, mut d) = change
                .time_signature
                .unwrap_or((here.numerator, here.denominator));
            ui.add_enabled(on, egui::DragValue::new(&mut n).range(1..=16));
            ui.label("/");
            ui.add_enabled_ui(on, |ui| {
                egui::ComboBox::from_id_salt("change-den")
                    .width(40.0)
                    .selected_text(d.to_string())
                    .show_ui(ui, |ui| {
                        for v in [2u32, 4, 8, 16] {
                            ui.selectable_value(&mut d, v, v.to_string());
                        }
                    });
            });
            change.time_signature = on.then_some((n, d));
        });
        if change != before {
            // Unticking both removes the change.
            let done = if change.tempo.is_none() && change.time_signature.is_none() {
                app.session.remove_tempo_change(bar, 0)
            } else {
                app.session.remove_tempo_change(bar, 0).ok();
                app.session.add_tempo_change(change)
            };
            app.report(done);
        }
        if existing.is_some() && ui.button("Remove change").clicked() {
            let done = app.session.remove_tempo_change(bar, 0);
            app.report(done);
            ui.close();
        }
    });
}

/// A grid line's colour: bars brightest, then beats, then the division between them.
pub(crate) fn line_color(kind: LineKind) -> Color32 {
    match kind {
        LineKind::Bar => theme::GRID_BAR,
        LineKind::Beat => theme::GRID_BEAT,
        LineKind::Division => theme::GRID_DIVISION,
    }
}

/// The grid's lines across `rect` on the timeline `axis` (x and kind).
fn grid_xs(map: &TempoMap, grid: Grid, axis: TimeAxis, rect: Rect) -> Vec<(f32, LineKind)> {
    let from = map
        .tick_at(axis.sample(rect.left()).max(0.0))
        .floor()
        .max(0.0) as u64;
    let to = map
        .tick_at(axis.sample(rect.right()).max(0.0))
        .ceil()
        .max(0.0) as u64;
    map.grid_lines(from, to, grid, &grid_x(map, axis.spp))
        .into_iter()
        .map(|l| (axis.x(map.sample_at_tick(l.tick as f64)), l.kind))
        .collect()
}

pub(crate) fn draw_grid(app: &DawApp, painter: &egui::Painter, rect: Rect, axis: TimeAxis) {
    let map = &app.tempo_map;
    // Bars, beats and the song's division (17.4), each level fainter, thinned where they
    // would crowd (`TempoMap::grid_lines`).
    for (x, kind) in grid_xs(map, app.session.project.grid, axis, rect) {
        painter.vline(x, rect.y_range(), Stroke::new(1.0, line_color(kind)));
    }
    // A faint line down from each marker.
    for marker in &app.session.project.markers {
        let x = axis.x(map.sample_at_tick(marker.tick as f64));
        if rect.x_range().contains(x) {
            painter.vline(
                x,
                rect.y_range(),
                Stroke::new(1.0, theme::MARKER.gamma_multiply(0.35)),
            );
        }
    }
}

/// The marker strip under the ruler: a flag per marker, numbered as the 1-9 keys reach them.
/// Click a flag to go there, drag it to move it (snapped), double-click to rename;
/// right-click to rename or delete. Double-click the strip to add a marker there, or
/// right-click it.
fn draw_markers(app: &mut DawApp, ui: &mut egui::Ui, rect: Rect, axis: TimeAxis) {
    let strip = ui
        .interact(rect, ui.scope_id().with("marker-strip"), Sense::click())
        .on_hover_text("Double-click to add a marker; Insert adds one at the playhead");
    crate::marks::put(ui, "arrange.markers", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0, theme::BG);
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::GRID_BAR),
    );
    let map = app.tempo_map.clone();
    let markers = app.session.project.markers.clone();
    let xs: Vec<f32> = markers
        .iter()
        .map(|m| axis.x(map.sample_at_tick(m.tick as f64)))
        .collect();
    for (i, marker) in markers.iter().enumerate() {
        let x = xs[i];
        // A flag reaches at most to the next marker, so close ones don't paint over it.
        let next = xs.get(i + 1).copied().unwrap_or(f32::MAX);
        if x > rect.right() || next < rect.left() {
            continue;
        }
        let width = (next - x - 2.0).clamp(12.0, 220.0);
        let flag = Rect::from_min_size(pos2(x, rect.top() + 1.0), vec2(width, rect.height() - 3.0));
        painter.vline(x, rect.y_range(), Stroke::new(1.5, theme::MARKER));
        if let Some((_, name)) = app
            .renaming_marker
            .as_mut()
            .filter(|(id, _)| *id == marker.id)
        {
            let edit = ui.put(
                flag.with_max_x(flag.left() + width.max(120.0)),
                egui::TextEdit::singleline(name).font(FontId::proportional(11.0)),
            );
            edit.request_focus();
            if edit.lost_focus() {
                let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
                let name = name.trim().to_string();
                if !escaped && !name.is_empty() {
                    let renamed = app.session.set_marker(marker.id, Some(name), None);
                    app.report(renamed);
                }
                app.renaming_marker = None;
            }
            continue;
        }
        let label = format!("{}  {}", i + 1, marker.name);
        // Cut with an ellipsis where the next marker begins.
        let mut job = egui::text::LayoutJob::simple_singleline(
            label,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((width - 8.0).max(4.0));
        let text = painter.layout_job(job);
        let tag = Rect::from_min_size(
            flag.min,
            vec2((text.size().x + 8.0).min(width), flag.height()),
        );
        let response = ui
            .interact(
                tag,
                ui.scope_id().with(("marker", marker.id)),
                Sense::click_and_drag(),
            )
            .on_hover_text(format!(
                "{}: click to go there, drag to move, double-click to rename",
                marker.name
            ));
        let hot = response.hovered() || response.dragged();
        painter.rect_filled(
            tag,
            2,
            theme::MARKER.gamma_multiply(if hot { 0.6 } else { 0.35 }),
        );
        painter.with_clip_rect(tag.intersect(rect)).galley(
            pos2(tag.left() + 4.0, tag.center().y - text.size().y / 2.0),
            text,
            Color32::WHITE,
        );
        if response.clicked() {
            let went = app.session.go_to_marker(marker.id).map(|_| ());
            app.report(went);
        }
        if response.double_clicked() {
            app.renaming_marker = Some((marker.id, marker.name.clone()));
        }
        if response.dragged() {
            if let Some(p) = response.interact_pointer_pos() {
                // From the pointer each frame: nothing to drift.
                let at = snap(app, axis.sample(p.x)).max(0.0) as u64;
                let moved = app.session.set_marker(marker.id, None, Some(at));
                app.report(moved);
            }
        }
        response.context_menu(|ui| {
            if ui.button("Rename…").clicked() {
                app.renaming_marker = Some((marker.id, marker.name.clone()));
                ui.close();
            }
            if ui.button("Delete marker").clicked() {
                let removed = app.session.project.remove_marker(marker.id);
                if let Err(e) = removed {
                    app.toast(e);
                }
                ui.close();
            }
        });
    }
    if strip.double_clicked() {
        if let Some(p) = strip.interact_pointer_pos() {
            let at = snap(app, axis.sample(p.x)).max(0.0) as u64;
            app.session.add_marker(at, None);
        }
    }
    if strip.secondary_clicked() {
        if let Some(p) = strip.interact_pointer_pos() {
            app.marker_menu_at = snap(app, axis.sample(p.x)).max(0.0) as u64;
        }
    }
    strip.context_menu(|ui| {
        if ui.button("Add marker here").clicked() {
            let at = app.marker_menu_at;
            app.session.add_marker(at, None);
            ui.close();
        }
        if ui.button("Add marker at playhead (Ins)").clicked() {
            app.add_marker_at_playhead();
            ui.close();
        }
    });
}

fn track_row(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, row: usize, axis: TimeAxis) {
    let Some((kind, height)) = app.session.project.track(id).map(|t| (t.kind, t.height())) else {
        return;
    };
    let width = ui.available_width();
    // Allocated at the end: the header's controls are laid out in a scope, which leaves the
    // cursor under them rather than under the row.
    let rect = Rect::from_min_size(ui.cursor().min, vec2(width, height));
    let header = Rect::from_min_size(rect.min, vec2(HEADER_WIDTH, height));
    let lane = Rect::from_min_max(pos2(rect.left() + HEADER_WIDTH, rect.top()), rect.max);
    let selected = app.selection == Some(Selection::Track(id));

    // Lane background and grid.
    let painter = ui.painter_at(lane);
    let bg = if selected {
        theme::LANE_SELECTED
    } else if row.is_multiple_of(2) {
        theme::LANE
    } else {
        theme::LANE_ALT
    };
    painter.rect_filled(lane, 0, bg);
    draw_grid(app, &painter, lane, axis);
    painter.hline(
        lane.x_range(),
        lane.bottom() - 0.5,
        Stroke::new(1.0, theme::BG),
    );

    let lane_response = ui.interact(
        lane,
        ui.scope_id().with(("lane", id)),
        Sense::click_and_drag(),
    );
    if lane_response.drag_started() {
        // A time selection, from the empty lane: clips take their own drags first.
        if let Some(at) = ui.input(|i| i.pointer.press_origin()) {
            let extend = ui.input(|i| i.modifiers.shift);
            app.range_drag = Some(RangeDrag {
                anchor: snap(app, axis.sample(at.x)).max(0.0),
                anchor_y: at.y,
                base: app.time_selection.clone().filter(|_| extend),
            });
        }
    }
    if lane_response.secondary_clicked() {
        if let Some(p) = lane_response.interact_pointer_pos() {
            app.lane_menu_at = Some((id, axis.sample(p.x).max(0.0) as u64));
        }
    }
    lane_response.context_menu(|ui| lane_menu(app, ui, id, kind));
    if lane_response.clicked() {
        app.selection = Some(Selection::Track(id));
        app.clear_clip_selection();
        app.time_selection = None;
        if let Some(p) = lane_response.interact_pointer_pos() {
            locate_click(app, axis, p.x);
        }
    }

    if kind == TrackKind::Folder {
        draw_folder_summary(app, &painter, id, lane, axis);
    } else {
        let drawn = clips(app, ui, id, lane, axis);
        midi_clips(app, ui, id, lane, axis);
        // After every body in the lane, so the edge the pointer is on sits on top of them.
        clip_edges(app, ui, id, &drawn, lane, axis);
    }
    header_ui(app, ui, id, header, selected);
    resize_handle(app, ui, id, header);
    // Moves the cursor to the row's bottom, whatever the header left it at.
    ui.allocate_rect(rect, Sense::hover());
    app.drop_rows.push((id, kind, rect));
}

/// The master as a row at the top of the arrangement, laid out like a track's (name, mute, the
/// volume and pan knobs, its fx rack, the automation mode and lanes) but with a lane that holds
/// no clips. It is the project's master, as the mixer and inspector show it. Returns its rect.
fn master_row(app: &mut DawApp, ui: &mut egui::Ui, axis: TimeAxis) -> Rect {
    let height = app.session.project.master.height();
    let width = ui.available_width();
    let rect = Rect::from_min_size(ui.cursor().min, vec2(width, height));
    let header = Rect::from_min_size(rect.min, vec2(HEADER_WIDTH, height));
    let lane = Rect::from_min_max(pos2(rect.left() + HEADER_WIDTH, rect.top()), rect.max);
    let selected = app.selection == Some(Selection::Master);

    // The lane: the grid, and a word on why it is empty.
    let painter = ui.painter_at(lane);
    painter.rect_filled(
        lane,
        0,
        if selected {
            theme::LANE_SELECTED
        } else {
            theme::BG
        },
    );
    draw_grid(app, &painter, lane, axis);
    painter.text(
        lane.left_center() + vec2(12.0, 0.0),
        Align2::LEFT_CENTER,
        "Master: the mix of every track. It holds no clips.",
        FontId::proportional(11.0),
        theme::TEXT_DIM,
    );
    let response = ui.interact(lane, ui.scope_id().with("master-lane"), Sense::click());
    if response.clicked() {
        app.selection = Some(Selection::Master);
        if let Some(p) = response.interact_pointer_pos() {
            locate_click(app, axis, p.x);
        }
    }
    response.context_menu(|ui| {
        ui.label(egui::RichText::new("Automation").strong());
        crate::automation::add_lane_menu(app, ui, None);
    });

    // The header.
    let hp = ui.painter_at(header);
    hp.rect_filled(
        header,
        0,
        if selected {
            theme::LANE_SELECTED
        } else {
            theme::PANEL
        },
    );
    hp.vline(
        header.right() - 0.5,
        header.y_range(),
        Stroke::new(1.0, theme::BG),
    );
    let head = ui.interact(header, ui.scope_id().with("master-header"), Sense::click());
    if head.clicked() {
        app.selection = Some(Selection::Master);
    }
    let stripe = Rect::from_min_size(
        pos2(header.left() + 2.0, header.top() + 3.0),
        vec2(4.0, height - 6.0),
    );
    hp.rect_filled(stripe, 2, theme::MASTER);
    let meter_rect = Rect::from_min_size(
        pos2(header.right() - 14.0, header.top() + 6.0),
        vec2(8.0, height - 12.0),
    );
    draw_vertical_meter(&hp, meter_rect, &app.master_meter);
    let content = Rect::from_min_max(
        pos2(stripe.right() + 6.0, header.top() + 4.0),
        pos2(meter_rect.left() - 4.0, header.bottom() - 4.0),
    );
    let automated = crate::automation::automated(app, None);
    let mut gestures = Vec::new();
    let mut typed = value_edit::Typed::for_owner(None);
    // Fixed bands, each clipped to itself, so the rack can never push the footer out: the
    // name line, the knobs, the footer at the bottom, and the rack between.
    let knobs = height >= HEADER_KNOBS_FROM;
    let footer = height >= HEADER_FOOTER_FROM;
    let band = |top: f32, h: f32| {
        Rect::from_min_max(
            pos2(content.left(), top),
            pos2(content.right(), (top + h).min(content.bottom())),
        )
    };
    let name_line = band(content.top(), HEADER_LINE);
    let knob_line = band(name_line.bottom() + 3.0, HEADER_LINE + 2.0);
    let footer_line = band(content.bottom() - HEADER_LINE, HEADER_LINE);
    let rack_room = Rect::from_min_max(
        pos2(content.left(), knob_line.bottom() + 3.0),
        pos2(content.right(), footer_line.top() - 3.0),
    );
    let rack = footer && rack_room.height() >= RACK_SLOT * 3.0;
    let scoped = |ui: &mut egui::Ui, rect: Rect, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.scope_builder(UiBuilder::new().max_rect(rect), |ui| {
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
            add(ui);
        });
    };
    scoped(ui, name_line, &mut |ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let mute = &mut app.session.project.master.mixer.mute;
            if let Some(g) = crate::automation::mute_toggle(ui, automated.mute, mute, "master.mute")
            {
                gestures.push((daw_core::automation::AutomationTarget::Mute, g));
            }
            if !knobs {
                ui.add_space(3.0);
                header_extras(app, ui, None, true);
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.label(egui::RichText::new("Master").strong())
                    .on_hover_text("The master: every track's mix, out to the device");
            });
        });
    });
    if knobs {
        scoped(ui, knob_line, &mut |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                gestures.extend(header_knobs(app, ui, None, automated, &mut typed));
                if !footer {
                    header_automation_menu(app, ui, None);
                    header_extras(app, ui, None, true);
                }
            });
        });
    }
    if rack {
        scoped(ui, rack_room, &mut |ui| {
            fx_rack(app, ui, InsertTarget::Master, rack_room.height());
        });
    }
    if footer {
        scoped(ui, footer_line, &mut |ui| {
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                header_automation_menu(app, ui, None);
                header_extras(app, ui, None, !rack);
            });
        });
    }
    crate::automation::apply_gestures(app, None, &gestures);
    typed.apply(app);

    // Its height, by the bottom edge, as a track's.
    let edge = Rect::from_x_y_ranges(header.x_range(), (header.bottom() - 4.0)..=header.bottom());
    let resize = ui
        .interact(edge, ui.scope_id().with("master-resize"), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize the master's row");
    if resize.dragged() {
        let master = &mut app.session.project.master;
        master.height = Some((master.height() + resize.drag_delta().y).clamp(
            daw_core::project::MIN_TRACK_HEIGHT,
            daw_core::project::MAX_TRACK_HEIGHT,
        ));
    }
    // A firmer line under it: the tracks start below.
    ui.painter().hline(
        rect.x_range(),
        rect.bottom() - 1.0,
        Stroke::new(2.0, theme::BG),
    );
    ui.allocate_rect(rect, Sense::hover());
    rect
}

/// A track's MIDI clips: a block each with a miniature of its notes. They sit in musical time,
/// so the tempo map places them. Click to select (the playhead to the click, 25.2), drag to
/// move (snapped in ticks), double-click to open the piano roll.
fn midi_clips(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, lane: Rect, axis: TimeAxis) {
    let Some(track) = app.session.project.track(id) else {
        return;
    };
    let clips = track.midi_clips.clone();
    let color = played(app, id, theme::track_color(track.color));
    let map = app.tempo_map.clone();
    let x_of = |tick: u64| axis.x(map.sample_at_tick(tick as f64));
    let painter = ui.painter_at(lane);
    for clip in &clips {
        let (x0, x1) = (x_of(clip.start), x_of(clip.end()));
        if x1 < lane.left() || x0 > lane.right() {
            continue;
        }
        let rect = Rect::from_x_y_ranges(
            x0..=x1.max(x0 + 2.0),
            (lane.top() + 2.0)..=(lane.bottom() - 2.0),
        );
        let response = ui.interact(
            rect.intersect(lane),
            ui.scope_id().with(("midi-clip", clip.id)),
            Sense::click_and_drag(),
        );
        app.clip_rects.push((clip.id, response.rect));
        crate::marks::put(ui, "clip.midi", response.rect);
        let selected = app.is_clip_selected(clip.id)
            || app.piano_roll.as_ref().is_some_and(|p| p.clip == clip.id);
        if response.clicked() {
            clip_clicked(app, ui, &response, id, clip.id, axis);
        }
        if response.double_clicked() {
            let _ = crate::piano_roll::open(app, clip.id);
        }
        let start = map.sample_at_tick(clip.start as f64);
        drag_clips(app, &response, id, clip.id, start, axis);
        response.context_menu(|ui| {
            if ui.button("Open in piano roll").clicked() {
                let _ = crate::piano_roll::open(app, clip.id);
                ui.close();
            }
            if ui.button("Split at playhead (S)").clicked() {
                app.select_clip(id, clip.id, false);
                app.selected_point = None;
                split_selected_clip(app);
                ui.close();
            }
            if ui.button("Export as MIDI file…").clicked() {
                ui.close();
                let dialog = app
                    .dialog_in(Some(daw_core::folder::EXPORTED))
                    .add_filter("MIDI file", &["mid"])
                    .set_file_name(format!("{}.mid", clip.name));
                let clip = clip.id;
                crate::file_dialog::save_file(app, dialog, move |app, path| {
                    match app.session.export_midi_clip(clip, &path) {
                        Ok(()) => app.toast(format!("Saved {}", path.display())),
                        Err(e) => app.toast(format!("{e:#}")),
                    }
                });
            }
            if ui.button("Delete (Del)").clicked() {
                app.select_clip(id, clip.id, false);
                app.selected_point = None;
                delete_selected_clip(app);
                ui.close();
            }
        });

        painter.rect_filled(rect, 3, color.gamma_multiply(0.25));
        painter.rect_stroke(
            rect,
            3,
            Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected { Color32::WHITE } else { color },
            ),
            StrokeKind::Inside,
        );
        let title = Rect::from_min_max(rect.min, pos2(rect.right(), rect.top() + 14.0));
        painter.rect_filled(title, 3, color.gamma_multiply(0.8));
        clip_title(&painter, title, rect.intersect(lane), clip.name.clone());
        // The notes, scaled to the clip's pitch range (at least an octave).
        let area = Rect::from_min_max(
            pos2(rect.left(), title.bottom() + 4.0),
            rect.max - vec2(0.0, 3.0),
        );
        let notes: Vec<_> = clip.audible_notes().collect();
        let (Some(low), Some(high)) = (
            notes.iter().map(|n| n.pitch).min(),
            notes.iter().map(|n| n.pitch).max(),
        ) else {
            continue;
        };
        let span = f32::from(high - low).max(12.0) + 1.0;
        let row = (area.height() / span).clamp(1.0, 4.0);
        let painter = painter.with_clip_rect(area.intersect(lane));
        for note in notes {
            let y = area.bottom() - (f32::from(note.pitch - low) + 0.5) / span * area.height();
            let end = (note.tick + note.length).min(clip.length);
            let xs = x_of(clip.start + note.tick)
                ..=x_of(clip.start + end).max(x_of(clip.start + note.tick) + 1.0);
            painter.hline(xs, y, Stroke::new(row, color));
        }
    }
}

/// The bottom edge of a track's header: drag it to change the row's height, Ctrl+drag to set
/// every track to that height. Heights are view state, never undo steps.
fn resize_handle(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, header: Rect) {
    let edge = Rect::from_x_y_ranges(header.x_range(), (header.bottom() - 4.0)..=header.bottom());
    let response = ui
        .interact(edge, ui.scope_id().with(("resize", id)), Sense::drag())
        .on_hover_cursor(egui::CursorIcon::ResizeVertical)
        .on_hover_text("Drag to resize the track (Ctrl: all tracks; Alt+wheel also resizes)");
    if response.hovered() || response.dragged() {
        ui.painter().hline(
            edge.x_range(),
            header.bottom() - 1.0,
            Stroke::new(2.0, theme::ACCENT),
        );
    }
    if response.dragged() {
        let dy = response.drag_delta().y;
        let all = ui.input(|i| i.modifiers.ctrl);
        let project = &mut app.session.project;
        let Some(current) = project.track(id).map(|t| t.height()) else {
            return;
        };
        let height = (current + dy).clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT);
        for track in &mut project.tracks {
            if all || track.id == id {
                track.height = Some(height);
            }
        }
    }
}

/// Alt+wheel: every track taller or shorter together, keeping their proportions.
fn scale_track_heights(app: &mut DawApp, wheel: f32) {
    let factor = (1.0 + wheel / 300.0).clamp(0.5, 2.0);
    for track in &mut app.session.project.tracks {
        let height = (track.height() * factor).clamp(MIN_TRACK_HEIGHT, MAX_TRACK_HEIGHT);
        track.height = Some(height);
    }
}

/// The heights of the tracks drawn last frame (`drop_rows`), in their order.
fn drawn_heights(app: &DawApp) -> Vec<Option<f32>> {
    let project = &app.session.project;
    app.drop_rows
        .iter()
        .map(|(id, _, _)| project.track(*id).map(|t| t.height()))
        .collect()
}

/// The rows' scroll offset that keeps what is under the pointer (at `y`) in place now the
/// tracks' heights have changed from `before` (`drawn_heights`, in `drop_rows`' order) to
/// what they are: from where every row was drawn last frame, so the master's row, folders'
/// hidden tracks and automation lanes count as drawn. `None` before the rows were first drawn.
///
/// The rows are placed at the scroll area's own offset rather than where they were drawn:
/// egui draws them rounded to a pixel, and a notch's frames anchored each on the last one's
/// rounding would drift a point or more by its end.
fn rows_in_place(app: &DawApp, before: &[Option<f32>], y: f32) -> Option<f32> {
    let (view, offset) = app.track_view?;
    let master = app.master_row?;
    let shift = view.top() - offset - master.top();
    let project = &app.session.project;
    let rows: Vec<crate::zoom::Row> = app
        .drop_rows
        .iter()
        .zip(before)
        .map(|((id, _, rect), old)| {
            let new = project.track(*id).map(|t| t.height());
            // A track gone since it was drawn keeps the room it had.
            let (old, new) = match (*old, new) {
                (Some(old), Some(new)) => (old, new),
                _ => (rect.height(), rect.height()),
            };
            crate::zoom::Row {
                top: rect.top() + shift,
                bottom: rect.bottom() + shift,
                old,
                new,
            }
        })
        .collect();
    let bottom = rows.last().map_or(master.bottom() + shift, |r| r.bottom);
    let content = egui::Rangef::new(master.top() + shift, bottom);
    Some(crate::zoom::offset_in_place(
        content,
        &rows,
        view.y_range(),
        y,
    ))
}

/// A folder lane shows where its sub-tracks have clips.
fn draw_folder_summary(
    app: &DawApp,
    painter: &egui::Painter,
    id: TrackId,
    lane: Rect,
    axis: TimeAxis,
) {
    let project = &app.session.project;
    let Some(folder) = project.track(id) else {
        return;
    };
    let color = theme::track_color(folder.color).linear_multiply(0.35);
    for track in project
        .tracks
        .iter()
        .filter(|t| project.is_descendant(t.id, id))
    {
        for clip in &track.clips {
            let x0 = axis.x(clip.start as f64).max(lane.left());
            let x1 = axis.x(clip.end() as f64).min(lane.right());
            if x1 > x0 {
                let r =
                    Rect::from_min_max(pos2(x0, lane.top() + 10.0), pos2(x1, lane.bottom() - 10.0));
                painter.rect_filled(r, 2, color);
            }
        }
    }
}

/// A track's audio clips: a block each with its waveform, fades and gain line. Returns the
/// clips drawn, in drawing order with their rects (and the clip whose edge is being dragged,
/// on screen or not), for [`clip_edges`] to offer their edges once the lane's bodies are in.
fn clips(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    id: TrackId,
    lane: Rect,
    axis: TimeAxis,
) -> Vec<(Clip, Rect)> {
    let mut drawn = Vec::new();
    let Some(track) = app.session.project.track(id) else {
        return drawn;
    };
    let color = played(app, id, theme::track_color(track.color));
    let clips: Vec<Clip> = track.clips.clone();
    let painter = ui.painter_at(lane);
    for clip in clips {
        let x0 = axis.x(clip.start as f64);
        let x1 = axis.x(clip.end() as f64);
        let rect = Rect::from_min_max(
            pos2(x0, lane.top() + 2.0),
            pos2(x1.max(x0 + 2.0), lane.bottom() - 3.0),
        );
        let edge_dragged = app.edge_drag.as_ref().is_some_and(|d| d.clip == clip.id);
        if x1 < lane.left() || x0 > lane.right() {
            if edge_dragged {
                drawn.push((clip, rect));
            }
            continue;
        }
        drawn.push((clip.clone(), rect));
        let response = ui.interact(
            rect.intersect(lane),
            ui.scope_id().with(("clip", clip.id)),
            Sense::click_and_drag(),
        );
        app.clip_rects.push((clip.id, response.rect));
        let selected = app.is_clip_selected(clip.id);
        crate::marks::put(ui, "clip", response.rect);

        painter.rect_filled(rect, 3, color.linear_multiply(0.28));
        let title = Rect::from_min_size(rect.min, vec2(rect.width(), 14.0));
        painter.rect_filled(title, 3, color.linear_multiply(0.7));
        let visible = rect.intersect(lane);
        // A clip with several takes (loop recording) says which one plays, and a stretched
        // one how, and whether its render is still on the way.
        let mut label = if clip.takes.len() > 1 {
            format!(
                "{}  · take {}/{}",
                clip.name,
                clip.active + 1,
                clip.takes.len()
            )
        } else {
            clip.name.clone()
        };
        if let Some(follow) = &clip.follow {
            label += &format!("  · follows {:.0} BPM", follow.tempo);
        } else if clip.stretch != 1.0 {
            label += &format!("  · ×{:.2}", clip.stretch);
        }
        if clip.pitch != 0.0 {
            label += &format!("  · {:+.1} st", clip.pitch);
        }
        if app.session.clip_rendering(&clip) {
            label += "  · rendering…";
        }
        clip_title(&painter, title, visible, label);
        draw_waveform(
            app,
            &painter,
            &clip,
            Rect::from_min_max(pos2(rect.left(), title.bottom()), rect.max),
            lane,
            axis,
            color,
        );
        painter.rect_stroke(
            rect,
            3,
            Stroke::new(
                if selected { 2.0 } else { 1.0 },
                if selected { Color32::WHITE } else { color },
            ),
            StrokeKind::Inside,
        );

        if response.clicked() {
            clip_clicked(app, ui, &response, id, clip.id, axis);
        }
        // Double-click an audio clip to open its file in the Waveform view.
        if response.double_clicked() {
            crate::waveform::open_clip(app, clip.id);
        }
        drag_clips(app, &response, id, clip.id, clip.start as f64, axis);
        crate::quantize::draw_transients(app, &painter, clip.id, rect, axis);
        clip_handles(app, ui, &painter, &clip, rect, lane, axis);
        response.context_menu(|ui| {
            if clip.takes.len() > 1 {
                ui.label(egui::RichText::new("Takes").strong());
                for (k, take) in clip.takes.iter().enumerate() {
                    let label = format!("{}  {}", k + 1, take.name);
                    if ui.radio(k == clip.active, label).clicked() {
                        let _ = app.session.set_active_take(clip.id, k);
                        ui.close();
                    }
                }
                if ui
                    .button(format!("Delete take {}", clip.active + 1))
                    .on_hover_text("The file on disk is kept")
                    .clicked()
                {
                    let _ = app.session.delete_take(clip.id, clip.active);
                    ui.close();
                }
                ui.separator();
            }
            if ui.button("Split at playhead (S)").clicked() {
                app.select_clip(id, clip.id, false);
                app.selected_point = None;
                split_selected_clip(app);
                ui.close();
            }
            let touching = app.session.project.track(id).is_some_and(|t| {
                t.clips
                    .iter()
                    .filter(|c| c.id != clip.id && c.start < clip.start)
                    .max_by_key(|c| c.start)
                    .is_some_and(|c| c.end() >= clip.start)
            });
            if ui
                .add_enabled(touching, egui::Button::new("Crossfade with previous clip"))
                .on_hover_text(
                    "Fade the clip before into this one over their overlap (20 ms if they only \
                     touch). Equal power; make it linear for two pieces of one recording with \
                     the crossfade_clips action.",
                )
                .on_disabled_hover_text("Needs a clip before it that touches or overlaps it")
                .clicked()
            {
                let shape = daw_core::project::FadeShape::EqualPower;
                if let Err(e) = app.session.crossfade_clips(clip.id, None, shape) {
                    app.toast(format!("{e:#}"));
                }
                ui.close();
            }
            if ui
                .button("Slice at transients…")
                .on_hover_text("Cut the clip at its attacks, then quantize the slices")
                .clicked()
            {
                app.select_clip(id, clip.id, false);
                crate::quantize::open_slicer(app, clip.id);
                ui.close();
            }
            if ui.button("Quantize… (Q)").clicked() {
                if !app.is_clip_selected(clip.id) {
                    app.select_clip(id, clip.id, false);
                }
                crate::quantize::open_quantize(app);
                ui.close();
            }
            if ui.button("Zoom to clip").clicked() {
                app.select_clip(id, clip.id, false);
                app.selected_point = None;
                let _ = app.zoom_to_clip(clip.id);
                ui.close();
            }
            if ui.button("Delete (Del)").clicked() {
                app.select_clip(id, clip.id, false);
                app.selected_point = None;
                delete_selected_clip(app);
                ui.close();
            }
        });
        response.on_hover_text(format!(
            "{}\n{}",
            clip.name,
            crate::transport::format_time(clip.length, app.session.sample_rate())
        ));
    }
    // Over the clips, as a new take records over what was there.
    recording_takes(app, &painter, id, lane, axis);
    drawn
}

/// A clip's fade gain `into` samples from its start, as `mix_clips` plays it.
fn fade_at(clip: &Clip, into: f64) -> f32 {
    let fade_in = clip.fade_in.length.min(clip.length);
    let fade_out = clip.fade_out.length.min(clip.length - fade_in);
    if into < fade_in as f64 {
        clip.fade_in.shape.gain((into / fade_in as f64) as f32)
    } else if into >= (clip.length - fade_out) as f64 {
        let left = (clip.length as f64 - into).max(0.0);
        clip.fade_out.shape.gain((left / fade_out as f64) as f32)
    } else {
        1.0
    }
}

/// Where clip gain sits on a clip, from its waveform's top (0) to bottom (1): -60 dB at the
/// bottom, +24 at the top, 0 dB a little above the middle.
fn gain_fraction(db: f32) -> f32 {
    1.0 - ((db + 60.0) / 84.0).clamp(0.0, 1.0)
}

/// A fade's length as its handle is dragged (25.1): the length it had when grabbed at sample
/// `press`, moved by as far as the pointer (now over `pointer`) has gone since; a fade-in
/// grows to the right, a fade-out to the left, within the clip's `length`. The pointer still
/// on the press point leaves it exactly as it was.
fn dragged_fade(grabbed: u64, press: f64, pointer: f64, from_start: bool, length: u64) -> u64 {
    let moved = if from_start {
        pointer - press
    } else {
        press - pointer
    };
    (grabbed as f64 + moved).round().clamp(0.0, length as f64) as u64
}

/// A clip's fades and gain: the fade curves drawn over it, a handle at each fade's inner end
/// on the top edge (drag to set its length), and the gain line (drag up or down;
/// double-click for 0 dB). Plain drags; Alt+edge stays the stretch.
#[allow(clippy::too_many_arguments)]
fn clip_handles(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    painter: &egui::Painter,
    clip: &Clip,
    rect: Rect,
    lane: Rect,
    axis: TimeAxis,
) {
    use daw_core::project::Fade;
    let wave = Rect::from_min_max(pos2(rect.left(), rect.top() + 14.0), rect.max);
    if wave.height() < 8.0 {
        return;
    }
    let curve_stroke = Stroke::new(1.0, Color32::from_white_alpha(170));
    // The fade curves: gain from silence at the edge to full at the handle.
    for (fade, from_start) in [(clip.fade_in, true), (clip.fade_out, false)] {
        if fade.length == 0 {
            continue;
        }
        let steps = 24;
        let points: Vec<egui::Pos2> = (0..=steps)
            .map(|k| {
                let t = k as f32 / steps as f32;
                let sample = if from_start {
                    clip.start as f64 + fade.length as f64 * f64::from(t)
                } else {
                    clip.end() as f64 - fade.length as f64 * f64::from(t)
                };
                let gain = fade.shape.gain(t);
                pos2(axis.x(sample), wave.bottom() - gain * wave.height())
            })
            .collect();
        painter.add(egui::Shape::line(points, curve_stroke));
    }
    let wide_enough = rect.width() > 24.0;
    let handle_id = |what: &str| {
        ui.scope_id()
            .with(("clip-handle", clip.id, what.to_string()))
    };
    for from_start in [true, false] {
        if !wide_enough {
            break;
        }
        let fade = if from_start {
            clip.fade_in
        } else {
            clip.fade_out
        };
        let x = if from_start {
            axis.x((clip.start + fade.length) as f64)
        } else {
            axis.x((clip.end() - fade.length) as f64)
        };
        let handle = Rect::from_center_size(pos2(x, wave.top() + 4.0), vec2(9.0, 9.0));
        if !lane.contains(handle.center()) {
            continue;
        }
        let what = if from_start { "fade in" } else { "fade out" };
        crate::marks::put(
            ui,
            if from_start {
                "clip.fade_in"
            } else {
                "clip.fade_out"
            },
            handle,
        );
        let response = ui.interact(handle, handle_id(what), Sense::drag());
        let hot = response.hovered() || response.dragged();
        painter.rect_filled(
            handle.shrink(1.0),
            1,
            if hot {
                Color32::WHITE
            } else {
                Color32::from_gray(200)
            },
        );
        painter.rect_stroke(
            handle.shrink(1.0),
            1,
            Stroke::new(1.0, Color32::BLACK),
            StrokeKind::Inside,
        );
        if hot {
            ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        // The fade as the drag began and the sample it was grabbed at (25.1): the handle
        // follows the pointer's movement from there, so a press changes nothing.
        let origin_id = handle_id(if from_start {
            "fade in origin"
        } else {
            "fade out origin"
        });
        if response.drag_started() {
            let press = ui.input(|i| i.pointer.press_origin()).map_or(x, |p| p.x);
            ui.ctx()
                .data_mut(|d| d.insert_temp(origin_id, (fade.length, axis.sample(press))));
        }
        if response.dragged() {
            let origin: Option<(u64, f64)> = ui.ctx().data(|d| d.get_temp(origin_id));
            if let (Some((grabbed, press)), Some(pointer)) =
                (origin, response.interact_pointer_pos())
            {
                // From the drag's start each frame: nothing to drift.
                let pointer = axis.sample(pointer.x);
                let length = dragged_fade(grabbed, press, pointer, from_start, clip.length);
                // Held still: nothing to change, and no graph to rebuild.
                let current = if from_start {
                    clip.fade_in.length
                } else {
                    clip.fade_out.length
                };
                let project = &mut app.session.project;
                if let Some(c) = project.clip_mut(clip.id).filter(|_| length != current) {
                    let (fade_in, fade_out) = (c.fade_in, c.fade_out);
                    if from_start {
                        c.set_fades(Fade { length, ..fade_in }, fade_out);
                    } else {
                        // The fade-out keeps what it's dragged to; the fade-in gives way.
                        let fade_in = Fade {
                            length: fade_in.length.min(c.length - length.min(c.length)),
                            ..fade_in
                        };
                        c.set_fades(fade_in, Fade { length, ..fade_out });
                    }
                    app.session.mark_dirty();
                }
            }
        }
        if response.drag_stopped() {
            ui.ctx().data_mut(|d| d.remove::<(u64, f64)>(origin_id));
        }
        response.on_hover_text(format!(
            "{} {}: drag to change",
            if from_start { "Fade in" } else { "Fade out" },
            crate::transport::format_time(fade.length, app.session.sample_rate())
        ));
    }

    // The gain line, across the clip between the fade handles.
    let y = wave.top() + gain_fraction(clip.gain_db) * wave.height();
    let span = rect.intersect(lane).x_range();
    if span.max - span.min < 24.0 {
        return;
    }
    let line = Rect::from_x_y_ranges(span.min + 8.0..=span.max - 8.0, y - 3.0..=y + 3.0);
    crate::marks::put(ui, "clip.gain", line);
    let response = ui.interact(line, handle_id("gain"), Sense::click_and_drag());
    let hot = response.hovered() || response.dragged();
    painter.hline(
        line.x_range(),
        y,
        Stroke::new(
            if hot { 2.0 } else { 1.0 },
            Color32::from_rgba_unmultiplied(255, 220, 120, if hot { 255 } else { 140 }),
        ),
    );
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeVertical);
    }
    let origin_id = handle_id("gain-origin");
    if response.drag_started() {
        // From where it was pressed, not where the drag was decided: the line follows the
        // pointer's whole movement, and a long press without one changes nothing.
        let pointer = ui
            .input(|i| i.pointer.press_origin())
            .or(response.interact_pointer_pos())
            .map_or(y, |p| p.y);
        ui.ctx()
            .data_mut(|d| d.insert_temp(origin_id, (clip.gain_db, pointer)));
    }
    let mut set = None;
    if response.dragged() {
        let origin: Option<(f32, f32)> = ui.ctx().data(|d| d.get_temp(origin_id));
        if let (Some((db, from)), Some(pointer)) = (origin, response.interact_pointer_pos()) {
            // The whole -60 to +24 over the waveform's height.
            set = Some(db - (pointer.y - from) * 84.0 / wave.height());
        }
    }
    if response.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<(f32, f32)>(origin_id));
    }
    if response.double_clicked() {
        set = Some(0.0);
    }
    if let Some(db) = set.map(|db| db.clamp(-60.0, 24.0)) {
        // Held still: nothing to change, and no graph to rebuild.
        if let Some(c) = app
            .session
            .project
            .clip_mut(clip.id)
            .filter(|c| c.gain_db != db)
        {
            c.gain_db = db;
            app.session.mark_dirty();
        }
    }
    response.on_hover_text(format!(
        "Clip gain {:+.1} dB: drag; double-click for 0",
        clip.gain_db
    ));
}

/// A clip's name in its title bar, cut short with an ellipsis to the part of the clip on
/// screen, so it never runs past the clip however it is split, trimmed, stretched or zoomed.
pub(crate) fn clip_title(painter: &egui::Painter, title: Rect, visible: Rect, text: String) {
    let width = visible.width() - 8.0;
    if width < 6.0 {
        return;
    }
    let mut job = egui::text::LayoutJob::single_section(
        text,
        egui::TextFormat::simple(FontId::proportional(11.0), Color32::BLACK),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    painter.with_clip_rect(title.intersect(visible)).galley(
        pos2(visible.left() + 4.0, title.top() + 1.0),
        galley,
        Color32::BLACK,
    );
}

/// How far either side of a clip's edge, in points, the pointer grabs it.
pub(crate) const EDGE_REACH: f32 = 4.0;
/// How far below a clip's top its edges can be grabbed: the title bar's fade handles are above.
const EDGE_TOP: f32 = 24.0;

/// A clip's edge in a lane, as the pointer could grab it (25.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct EdgeSpot {
    pub clip: ClipId,
    /// The clip's start (else its end).
    pub left: bool,
    /// Where it is on screen, and the height it can be grabbed over.
    pub x: f32,
    pub ys: egui::Rangef,
    /// Its clip is selected: its edges win over others in reach.
    pub selected: bool,
    /// A clip drawn after its own covers it.
    pub hidden: bool,
}

/// The edge lit this frame: the pointer on it, or it being dragged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LitEdge {
    pub clip: ClipId,
    pub left: bool,
    /// Under another clip, lit across it.
    pub hidden: bool,
}

/// The edges a lane's clips offer (25.1): `clips` in drawing order (later ones on top), each
/// with its rect and whether it is selected. An edge off the lane isn't offered (a clip
/// running past the view has no edge to grab there); one is hidden when a clip drawn after its
/// own covers it.
pub(crate) fn edge_spots(clips: &[(ClipId, Rect, bool)], lane: Rect) -> Vec<EdgeSpot> {
    let mut spots = Vec::new();
    for (k, &(clip, rect, selected)) in clips.iter().enumerate() {
        let top = (rect.top() + EDGE_TOP).min(rect.bottom() - 6.0);
        let ys = egui::Rangef::new(top.max(lane.top()), rect.bottom().min(lane.bottom()));
        for left in [true, false] {
            let x = if left { rect.left() } else { rect.right() };
            if !lane.x_range().contains(x) {
                continue;
            }
            let hidden = clips[k + 1..]
                .iter()
                .any(|(_, over, _)| over.left() < x && x < over.right());
            spots.push(EdgeSpot {
                clip,
                left,
                x,
                ys,
                selected,
                hidden,
            });
        }
    }
    spots
}

/// The edge the pointer at `p` is on, of `spots` in drawing order (25.1): of the edges within
/// [`EDGE_REACH`], a selected clip's wins, then the nearest, then the topmost (drawn last).
/// `None` when no edge is in reach: the pointer is on whatever body is there. Edges are chosen
/// before bodies, so one under another clip can still be grabbed.
pub(crate) fn edge_under(spots: &[EdgeSpot], p: egui::Pos2) -> Option<usize> {
    spots
        .iter()
        .enumerate()
        .filter(|(_, e)| (e.x - p.x).abs() <= EDGE_REACH && e.ys.contains(p.y))
        .max_by(|(i, a), (j, b)| {
            a.selected
                .cmp(&b.selected)
                .then_with(|| (b.x - p.x).abs().total_cmp(&(a.x - p.x).abs()))
                .then_with(|| i.cmp(j))
        })
        .map(|(i, _)| i)
}

/// A drag on a clip's edge under way (25.1): the clip as the drag began, whether it stretches
/// (Alt as it began) or trims, and where it was grabbed. The edge follows the pointer's
/// movement from that point, keeping the press point's distance from the edge, so a press
/// changes nothing and the first move snaps.
#[derive(Clone, Debug)]
pub struct EdgeDrag {
    pub clip: ClipId,
    pub left: bool,
    pub origin: Clip,
    pub stretching: bool,
    /// The sample under the pointer when it was pressed.
    pub press: f64,
    /// The pointer has left the press point: from then on the edge follows it.
    pub moving: bool,
}

impl EdgeDrag {
    pub(crate) fn new(clip: &Clip, left: bool, stretching: bool, press: f64) -> EdgeDrag {
        EdgeDrag {
            clip: clip.id,
            left,
            origin: clip.clone(),
            stretching,
            press,
            moving: false,
        }
    }

    /// The edge as the drag began, in samples.
    fn edge(&self) -> f64 {
        if self.left {
            self.origin.start as f64
        } else {
            self.origin.end() as f64
        }
    }

    /// Where the edge goes, before snapping, with the pointer over sample `pointer`: `None`
    /// until the pointer first leaves the press point, then the edge as it began moved as far
    /// as the pointer has.
    pub(crate) fn follow(&mut self, pointer: f64) -> Option<f64> {
        self.moving |= pointer != self.press;
        self.moving.then(|| self.edge() + (pointer - self.press))
    }
}

/// One frame of an edge drag, the pointer over sample `pointer` (25.1): nothing until the
/// pointer leaves the press point ([`EdgeDrag::follow`]); then the edge goes to `place` of
/// where the pointer's movement takes it ([`set_edge`]). The Multitrack does the same with
/// [`snap_clip`] (a trim) or [`snap`] (a stretch) as `place`; here for the tests.
#[cfg(test)]
pub(crate) fn drag_edge(
    session: &mut daw_core::session::Session,
    drag: &mut EdgeDrag,
    pointer: f64,
    place: impl Fn(f64) -> (f64, bool),
) -> anyhow::Result<bool> {
    let Some(target) = drag.follow(pointer) else {
        return Ok(false);
    };
    let (at, held) = place(target);
    set_edge(session, drag, at, held)
}

/// A dragged edge to sample `at` (snapped; `held`: the magnet put it there), from the clip as
/// the drag began, so nothing drifts (25.1): a trim stays within the audio and the other edge,
/// a stretch's ratio within its range. Each drag is one undo step. Returns whether the magnet
/// holds the edge (only while the edge is really on it: the audio may end short of it).
pub(crate) fn set_edge(
    session: &mut daw_core::session::Session,
    drag: &EdgeDrag,
    at: f64,
    held: bool,
) -> anyhow::Result<bool> {
    let (origin, left) = (&drag.origin, drag.left);
    if !drag.stretching {
        let at = at.max(0.0) as u64;
        let (earliest, latest) = session.clip_trim_range(origin);
        let latest = latest.unwrap_or(origin.end());
        let (start, end) = if left {
            (at.clamp(earliest, origin.end() - 1), origin.end())
        } else {
            (origin.start, at.clamp(origin.start + 1, latest))
        };
        let trimmed = session.trim_clip(drag.clip, Some(start), Some(end));
        let key = format!("Trim clip:{}", drag.clip);
        session.label_edit("Trim clip", Some(key));
        trimmed?;
        return Ok(held && (if left { start } else { end }) == at);
    }
    let length = if left {
        origin.end() as f64 - at
    } else {
        at - origin.start as f64
    };
    // The ratio first, clamped; the length then follows from it.
    let (low, high) = daw_core::stretch::RATIO_RANGE;
    let source = origin.length as f64 / origin.stretch;
    let ratio = (length.max(1.0) / source).clamp(low, high);
    let stretched = session.restretch_clip(drag.clip, origin, ratio, left);
    // A left-edge drag moves the start too: name it a stretch, merged per drag.
    let key = format!("Stretch clip:{}", drag.clip);
    session.label_edit("Stretch clip", Some(key));
    stretched?;
    Ok(false)
}

/// A lane's clip edges (25.1), after every clip's body in it: drag one to trim the clip there
/// (in hides the audio, back out restores it, as far as the audio goes; onto the playhead near
/// it, as clip moves, unless Ctrl is held; else snapped unless snap is off), Alt-drag to
/// stretch it (the left edge keeps its end). `drawn` is the lane's clips in drawing order. The
/// edge the pointer is on comes from [`edge_under`] and only that one is interacted, after the
/// bodies, so it sits on top of them: a selected clip's edge beats its neighbour's, and an
/// edge under another clip can still be grabbed, lit across the clip covering it. The edges
/// stop short of the title bar's fade handles.
fn clip_edges(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    track: TrackId,
    drawn: &[(Clip, Rect)],
    lane: Rect,
    axis: TimeAxis,
) {
    let rows: Vec<(ClipId, Rect, bool)> = drawn
        .iter()
        .map(|(c, r)| (c.id, *r, app.is_clip_selected(c.id)))
        .collect();
    let spots = edge_spots(&rows, lane);
    let scope = ui.scope_id();
    let id_of = |clip: ClipId, left: bool| scope.with(("clip-edge", clip, left));
    // The edge being dragged keeps the pointer wherever it goes; else the one it's on.
    let dragging = app
        .edge_drag
        .as_ref()
        .filter(|d| drawn.iter().any(|(c, _)| c.id == d.clip))
        .map(|d| (d.clip, d.left));
    let chosen = dragging.or_else(|| {
        let p = ui.ctx().pointer_hover_pos()?;
        if !ui.rect_contains_pointer(lane) {
            return None;
        }
        let spot = spots[edge_under(&spots, p)?];
        // Something else being dragged (a clip, the playhead, a selection) keeps the pointer.
        let busy = ui
            .ctx()
            .dragged_id()
            .is_some_and(|d| d != id_of(spot.clip, spot.left));
        (!busy).then_some((spot.clip, spot.left))
    });
    let Some((id, left)) = chosen else {
        return;
    };
    let Some((clip, rect)) = drawn.iter().find(|(c, _)| c.id == id) else {
        return;
    };
    let x = if left { rect.left() } else { rect.right() };
    let hidden = spots
        .iter()
        .any(|s| s.clip == id && s.left == left && s.hidden);
    let top = (rect.top() + EDGE_TOP).min(rect.bottom() - 6.0);
    let strip = Rect::from_x_y_ranges(x - EDGE_REACH..=x + EDGE_REACH, top..=rect.bottom());
    let edge = ui.interact(strip, id_of(id, left), Sense::drag());
    if edge.hovered() || dragging.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
    }
    let edge = edge.on_hover_text(if clip.follow.is_some() {
        "Drag to trim (it follows the tempo, so it can't be stretched)"
    } else {
        "Drag to trim; Alt-drag to stretch"
    });
    if edge.drag_started() {
        let alt = ui.input(|i| i.modifiers.alt);
        let stretching = alt && clip.follow.is_none();
        // Where it was pressed, not where the pointer is now: the edge keeps its distance.
        let press = ui
            .input(|i| i.pointer.press_origin())
            .or(edge.interact_pointer_pos())
            .map_or(x, |p| p.x);
        app.edge_drag = Some(EdgeDrag::new(clip, left, stretching, axis.sample(press)));
        app.selection = Some(Selection::Track(track));
        app.select_clip(track, id, false);
        app.selected_point = None;
    }
    if edge.dragged() {
        if let (Some(mut drag), Some(pointer)) = (app.edge_drag.take(), edge.interact_pointer_pos())
        {
            // Nothing until the pointer leaves the press point; then where it takes the edge,
            // onto the playhead or the grid (a stretch: the grid).
            if let Some(target) = drag.follow(axis.sample(pointer.x)) {
                let ctrl = ui.input(|i| i.modifiers.ctrl);
                let (at, held) = if drag.stretching {
                    (snap(app, target), false)
                } else {
                    snap_clip(app, target, Span::Samples(0.0), ctrl)
                };
                match set_edge(&mut app.session, &drag, at, held) {
                    Ok(held) => app.edge_magnet = held,
                    Err(e) => app.toast(format!("{e:#}")),
                }
            }
            app.edge_drag = Some(drag);
        }
    }
    if edge.drag_stopped() {
        app.edge_drag = None;
        app.edge_magnet = false;
    }
    // Lit, so the user sees what they'll drag: across the clip covering it when hidden.
    let painter = ui.painter_at(lane);
    if hidden {
        painter.vline(
            x,
            rect.y_range(),
            Stroke::new(7.0, theme::ACCENT.gamma_multiply(0.3)),
        );
    }
    painter.vline(x, rect.y_range(), Stroke::new(2.0, theme::ACCENT));
    app.edge_lit = Some(LitEdge {
        clip: id,
        left,
        hidden,
    });
}

#[allow(clippy::too_many_arguments)]
fn draw_waveform(
    app: &DawApp,
    painter: &egui::Painter,
    clip: &Clip,
    rect: Rect,
    lane: Rect,
    axis: TimeAxis,
    color: Color32,
) {
    // What the clip plays: a stretched clip's render once it's ready.
    let Some((data, offset)) = app.session.clip_audio(clip) else {
        painter.text(
            rect.center(),
            Align2::CENTER_CENTER,
            "missing file",
            FontId::proportional(11.0),
            theme::RECORD,
        );
        return;
    };
    let clip_gain = daw_core::util::db_to_gain(clip.gain_db);
    // The clip's own gain and fades show as they play.
    let gain = |into: f64| app.waveform_scale * clip_gain * fade_at(clip, into);
    let span = (clip.start, offset, clip.length);
    let waves = [data.wave(0), data.wave(1)];
    let channels = data.channels.len().clamp(1, 2);
    paint_peaks(
        painter,
        rect,
        lane,
        axis,
        color,
        span,
        &waves[..channels],
        gain,
    );
}

/// Draw a file's waveform (1 or 2 channels, [`crate::wave`]: columns from its peaks, or its
/// samples zoomed right in) over `span`: `(start on the timeline, offset into the file,
/// length)`, in samples. A clip's file and a take still recording (21.1) are drawn by this one
/// function, the take from its 256-sample peaks alone ([`daw_core::peaks::Wave::of_peaks`]),
/// which is what the file shows at 256 samples a point and wider, so the clip a take becomes
/// looks exactly as it did; closer in, the take shows blocks until its file is in. `gain` is
/// the display gain `into` samples from the start.
#[allow(clippy::too_many_arguments)]
fn paint_peaks(
    painter: &egui::Painter,
    rect: Rect,
    lane: Rect,
    axis: TimeAxis,
    color: Color32,
    (start, offset, length): (u64, u64, u64),
    waves: &[daw_core::peaks::Wave],
    gain: impl Fn(f64) -> f32,
) {
    let channels = waves.len().clamp(1, 2);
    let lane_h = rect.height() / channels as f32;
    let left = rect.left().max(lane.left());
    let right = rect.right().min(lane.right());
    let place = daw_core::peaks::Place {
        // Kept in f64: zoomed right in, the file's start may be millions of points away.
        origin: axis.x64(start as f64 - offset as f64),
        spp: axis.spp,
        from: offset,
        to: offset + length,
        left,
        right,
    };
    for c in 0..channels {
        let wave = waves.get(c).copied().unwrap_or_default();
        let look = crate::wave::Look {
            mid: rect.top() + lane_h * (c as f32 + 0.5),
            half: lane_h * 0.45,
            color: color.linear_multiply(0.95),
        };
        crate::wave::paint(painter, &wave, &place, look, |at| {
            gain(at as f64 - offset as f64)
        });
    }
}

/// The takes recording on this track (21.1): each drawn as the clip it becomes, as far as it
/// has come, its waveform from the peaks the audio thread sends. The record colour marks its
/// title and frame; on stop the clip takes its place with the same peaks.
fn recording_takes(app: &DawApp, painter: &egui::Painter, id: TrackId, lane: Rect, axis: TimeAxis) {
    let Some(track) = app.session.project.track(id) else {
        return;
    };
    let color = played(app, id, theme::track_color(track.color));
    for take in app.session.recording_takes() {
        if take.track != id {
            continue;
        }
        let x0 = axis.x(take.start as f64);
        let x1 = axis.x((take.start + take.length) as f64);
        if x1 < lane.left() || x0 > lane.right() {
            continue;
        }
        let rect = Rect::from_min_max(
            pos2(x0, lane.top() + 2.0),
            pos2(x1.max(x0 + 2.0), lane.bottom() - 3.0),
        );
        painter.rect_filled(rect, 3, color.linear_multiply(0.28));
        let title = Rect::from_min_size(rect.min, vec2(rect.width(), 14.0));
        painter.rect_filled(title, 3, theme::RECORD.linear_multiply(0.7));
        let visible = rect.intersect(lane);
        clip_title(painter, title, visible, format!("● {}", take.name));
        let span = (take.start, take.offset, take.length);
        let body = Rect::from_min_max(pos2(rect.left(), title.bottom()), rect.max);
        let mut waves = [daw_core::peaks::Wave::default(); 2];
        for (wave, peaks) in waves.iter_mut().zip(take.peaks) {
            *wave = daw_core::peaks::Wave::of_peaks(peaks);
        }
        let channels = take.peaks.len().clamp(1, 2);
        paint_peaks(
            painter,
            body,
            lane,
            axis,
            color,
            span,
            &waves[..channels],
            |_| app.waveform_scale,
        );
        painter.rect_stroke(rect, 3, Stroke::new(1.0, theme::RECORD), StrokeKind::Inside);
    }
}

fn header_ui(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, rect: Rect, selected: bool) {
    // The manual's `track.*` marks are the selected track's.
    let _quiet = crate::marks::quiet(!selected);
    let depth = app.session.project.depth(id) as f32;
    let meter = app.meters.get(&id).copied().unwrap_or_default();
    let painter = ui.painter_at(rect);
    painter.rect_filled(
        rect,
        0,
        if selected {
            theme::LANE_SELECTED
        } else {
            theme::PANEL
        },
    );
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::BG),
    );
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        Stroke::new(1.0, theme::BG),
    );

    // Dragging the header's background (or its name, or the colour stripe) moves the track.
    let response = ui.interact(
        rect,
        ui.scope_id().with(("header", id)),
        Sense::click_and_drag(),
    );
    if response.clicked() {
        app.selection = Some(Selection::Track(id));
    }
    crate::track_drag::respond(app, &response, id, crate::track_drag::Surface::Arrange);

    let Some(track) = app.session.project.track(id) else {
        return;
    };
    let color = theme::track_color(track.color);
    let indent = depth * INDENT;
    let stripe = Rect::from_min_size(
        pos2(rect.left() + indent + 2.0, rect.top() + 3.0),
        vec2(4.0, rect.height() - 6.0),
    );
    painter.rect_filled(stripe, 2, color);
    // The stripe is the grip: it shows a hand, and a few dots when hovered. (A pointing hand:
    // Windows draws `Grab` as the four-way move cross.)
    let grip = stripe.expand2(vec2(3.0, 0.0));
    if ui.rect_contains_pointer(grip) && app.track_drag.is_none() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        for k in 0..3 {
            let y = stripe.center().y + (k as f32 - 1.0) * 5.0;
            painter.circle_filled(pos2(stripe.center().x, y), 1.2, Color32::BLACK);
        }
    }

    // Meter on the right edge.
    let meter_rect = Rect::from_min_size(
        pos2(rect.right() - 14.0, rect.top() + 6.0),
        vec2(8.0, rect.height() - 12.0),
    );
    draw_vertical_meter(&painter, meter_rect, &meter);

    let content = Rect::from_min_max(
        pos2(stripe.right() + 6.0, rect.top() + 4.0),
        pos2(meter_rect.left() - 4.0, rect.bottom() - 4.0),
    );
    ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
        ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
        header_rows(app, ui, id, rect.height());
    });
    // Off (or inside a folder that is): the header greyed out, its controls still there.
    if !app.session.project.plays(id) {
        ui.painter()
            .rect_filled(rect, 0, Color32::from_black_alpha(120));
    }
    crate::marks::put(ui, "track.header", rect);
    header_menu(app, id, &response);
}

/// A track's colour as its clips are drawn: faded when it doesn't play (off, or in a folder
/// that is), as its header is.
fn played(app: &DawApp, id: TrackId, color: Color32) -> Color32 {
    if app.session.project.plays(id) {
        color
    } else {
        color.gamma_multiply(0.35)
    }
}

/// From this row height the header shows the volume and pan knobs under the name.
pub(crate) const HEADER_KNOBS_FROM: f32 = 52.0;
/// From this row height the automation menu gets its own line at the bottom.
pub(crate) const HEADER_FOOTER_FROM: f32 = 84.0;
/// One header line: the buttons are 18 points tall.
const HEADER_LINE: f32 = 20.0;

/// The header's contents, as much as `height` fits:
/// - always: the name, then M, S, R (record arm) and the monitor button on the right;
/// - from [`HEADER_KNOBS_FROM`]: the volume and pan knobs;
/// - from [`HEADER_FOOTER_FROM`]: the automation mode ("Read ▾") at the bottom; on shorter
///   rows it sits after the knobs, or is left to the inspector on a one-line row.
fn header_rows(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, height: f32) {
    let automated = crate::automation::automated(app, Some(id));
    let mut gestures = Vec::new();
    let mut typed = value_edit::Typed::for_owner(Some(id));
    let width = ui.available_width();
    let knobs = height >= HEADER_KNOBS_FROM;
    let footer = height >= HEADER_FOOTER_FROM;
    ui.allocate_ui_with_layout(
        vec2(width, HEADER_LINE),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            // Right to left: the buttons, then (one-line rows) the lanes and FX, then the name.
            // While the name is typed, its field has the whole line.
            let field = value_edit::name_id(value_edit::Place::Header, Some(id));
            if !value_edit::is_open(ui.ctx(), field) {
                gestures.extend(header_buttons(app, ui, id, automated));
                if !knobs {
                    ui.add_space(3.0);
                    header_extras(app, ui, Some(id), true);
                }
            }
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                header_name(app, ui, id, f32::INFINITY);
            });
        },
    );
    if knobs {
        ui.allocate_ui_with_layout(
            vec2(width, HEADER_LINE + 2.0),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                gestures.extend(header_knobs(app, ui, Some(id), automated, &mut typed));
                if !footer {
                    header_automation_menu(app, ui, Some(id));
                    header_extras(app, ui, Some(id), true);
                }
            },
        );
    }
    let mut rack = false;
    if footer {
        // Between the knobs and the bottom line: the fx rack, once a title and two slots fit.
        let room = ui.available_height() - HEADER_LINE - 3.0;
        if room >= RACK_SLOT * 3.0 {
            rack = true;
            ui.allocate_ui(vec2(width, room), |ui| {
                fx_rack(app, ui, InsertTarget::Track(id), room)
            });
        }
        let rest = ui.available_height() - HEADER_LINE;
        if rest > 0.0 {
            ui.add_space(rest);
        }
        ui.allocate_ui_with_layout(
            vec2(width, HEADER_LINE),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                header_automation_menu(app, ui, Some(id));
                header_extras(app, ui, Some(id), !rack);
            },
        );
    }
    // Applied once the track's borrows are over.
    crate::automation::apply_gestures(app, Some(id), &gestures);
    typed.apply(app);
}

/// M, S, R (record arm) and the monitor button, added right to left. Folders and returns
/// hold no clips, so they have no R or monitor. Returns the mute's automation gesture.
fn header_buttons(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    id: TrackId,
    automated: crate::automation::Automated,
) -> Option<(
    daw_core::automation::AutomationTarget,
    crate::automation::Gesture,
)> {
    let track = app.session.project.track_mut(id)?;
    // Off, the track leaves the graph (no CPU); a render can stand in for it.
    let mut on = track.enabled;
    let tip = if on {
        "On: click to switch the track off (it and its plug-ins stop running; settings kept)"
    } else {
        "Off: it costs no CPU. Click to switch it back on"
    };
    let (switched, power) = power_button_at(ui, &mut on, tip);
    crate::marks::put(ui, "track.power", power);
    if switched {
        track.enabled = on;
        app.session.mark_dirty();
    }
    let track = app.session.project.track_mut(id)?;
    if track.kind.holds_clips() {
        let monitor = monitor_button(ui, &mut track.monitor);
        crate::marks::put(ui, "track.monitor", monitor);
        let arm_tip = if track.kind == TrackKind::Instrument {
            "Record: arm to record MIDI, and play this track's instrument from the keyboard"
        } else {
            "Record: arm for recording"
        };
        let (_, arm) = theme::toggle_at(ui, &mut track.arm, "R", theme::RECORD, arm_tip);
        crate::marks::put(ui, "track.arm", arm);
    }
    let (_, solo) = theme::toggle_at(ui, &mut track.mixer.solo, "S", theme::SOLO, "Solo");
    crate::marks::put(ui, "track.solo", solo);
    crate::automation::mute_toggle(ui, automated.mute, &mut track.mixer.mute, "track.mute")
        .map(|g| (daw_core::automation::AutomationTarget::Mute, g))
}

/// Input monitoring, as headphones: blue on Auto (monitor while armed), lit when On, struck
/// through when Off. A click moves to the next. Returns where it was drawn.
fn monitor_button(ui: &mut egui::Ui, monitor: &mut Monitor) -> Rect {
    let (fill, text, tip) = match monitor {
        Monitor::Auto => (
            Color32::from_gray(0x33),
            egui::RichText::new("🎧").color(theme::MONITOR),
            "Input monitoring: Auto (while armed). Click for On.",
        ),
        Monitor::On => (
            theme::MONITOR,
            egui::RichText::new("🎧").color(Color32::BLACK),
            "Input monitoring: On. Click for Off.",
        ),
        Monitor::Off => (
            Color32::from_gray(0x33),
            egui::RichText::new("🎧")
                .color(theme::TEXT_DIM)
                .strikethrough(),
            "Input monitoring: Off. Click for Auto.",
        ),
    };
    let button = ui
        .add(
            egui::Button::new(text)
                .fill(fill)
                .min_size(vec2(20.0, 18.0)),
        )
        .on_hover_text(tip);
    if button.clicked() {
        *monitor = match monitor {
            Monitor::Auto => Monitor::On,
            Monitor::On => Monitor::Off,
            Monitor::Off => Monitor::Auto,
        };
    }
    button.rect
}

/// The volume and pan knobs with their values, as the mixer has them (automation included).
/// A value clicked turns into a field to type one (22.2, `value_edit`); typed values come
/// back in `typed`, applied once the project's borrows are over.
fn header_knobs(
    app: &mut DawApp,
    ui: &mut egui::Ui,
    owner: LaneOwner,
    automated: crate::automation::Automated,
    typed: &mut value_edit::Typed,
) -> Vec<(
    daw_core::automation::AutomationTarget,
    crate::automation::Gesture,
)> {
    use crate::automation::control;
    use daw_core::automation::AutomationTarget;
    use value_edit::{level_id, Level, Place};
    let mut gestures = Vec::new();
    let project = &mut app.session.project;
    let mixer = match owner {
        Some(id) => match project.track_mut(id) {
            Some(track) => &mut track.mixer,
            None => return gestures,
        },
        None => &mut project.master.mixer,
    };
    let target = value_edit::target_of(owner);
    // Room either side of a value, so an automated knob's outline doesn't touch it.
    let value = |ui: &mut egui::Ui, level: Level, shown: f32, editable: bool| {
        ui.add_space(2.0);
        let font = FontId::monospace(egui::TextStyle::Small.resolve(ui.style()).size);
        let id = level_id(Place::Header, owner, level);
        let size = vec2(34.0, HEADER_LINE);
        value_edit::level(ui, id, level, shown, size, font, editable, target)
    };

    ui.add_space(2.0);
    let (volume, gesture) = control(ui, automated.volume, &mut mixer.gain_db, |ui, gain| {
        let tip =
            format!("Volume {gain:+.1} dB: drag up or down (Shift: fine), double-click for 0 dB");
        theme::knob(ui, gain, -60.0..=12.0, 0.0, theme::ACCENT).on_hover_text(tip)
    });
    if owner.is_some() {
        crate::marks::put(ui, "track.volume", volume.rect);
    }
    gestures.extend(gesture.map(|g| (AutomationTarget::Volume, g)));
    let fixed = automated.volume.lane.is_none() && !automated.volume.writable;
    if volume.double_clicked() && fixed {
        mixer.gain_db = 0.0;
    }
    let shown = automated
        .volume
        .written
        .or(automated.volume.lane)
        .unwrap_or(mixer.gain_db);
    typed.gain_db = value(ui, Level::Volume, shown, fixed);
    ui.add_space(2.0);

    let (pan, gesture) = control(ui, automated.pan, &mut mixer.pan, |ui, pan| {
        let tip = format!(
            "Pan {}: drag up or down (Shift: fine), double-click to centre",
            crate::inspector::pan_label(*pan)
        );
        theme::knob(ui, pan, -1.0..=1.0, 0.0, theme::ACCENT).on_hover_text(tip)
    });
    if owner.is_some() {
        crate::marks::put(ui, "track.pan", pan.rect);
    }
    gestures.extend(gesture.map(|g| (AutomationTarget::Pan, g)));
    let fixed = automated.pan.lane.is_none() && !automated.pan.writable;
    if pan.double_clicked() && fixed {
        mixer.pan = 0.0;
    }
    let shown = automated
        .pan
        .written
        .or(automated.pan.lane)
        .unwrap_or(mixer.pan);
    typed.pan = value(ui, Level::Pan, shown, fixed);
    gestures
}

/// The automation mode as a menu ("Read ▾").
fn header_automation_menu(app: &mut DawApp, ui: &mut egui::Ui, owner: LaneOwner) {
    use daw_core::automation::AutomationMode;
    let project = &mut app.session.project;
    let mode = match owner {
        Some(id) => match project.track_mut(id) {
            Some(track) => &mut track.automation_mode,
            None => return,
        },
        None => &mut project.master.automation_mode,
    };
    let color = match mode {
        AutomationMode::Read => theme::TEXT_DIM,
        AutomationMode::Touch => theme::PLAYHEAD,
        AutomationMode::Latch => theme::MUTE,
        AutomationMode::Write => theme::RECORD,
    };
    let menu = egui::ComboBox::from_id_salt(("automation-mode", owner))
        .width(58.0)
        .selected_text(
            egui::RichText::new(format!("{mode:?}"))
                .small()
                .color(color),
        )
        .show_ui(ui, |ui| {
            for (m, what) in [
                (AutomationMode::Read, "lanes play"),
                (AutomationMode::Touch, "holding a control writes it"),
                (
                    AutomationMode::Latch,
                    "a control moved keeps writing until stop",
                ),
                (AutomationMode::Write, "every lane is written while playing"),
            ] {
                ui.selectable_value(mode, m, format!("{m:?}"))
                    .on_hover_text(what);
            }
        })
        .response;
    if owner.is_some() {
        crate::marks::put(ui, "track.automation_mode", menu.rect);
    }
    menu.on_hover_text("Automation mode: what moving this track's controls while playing does");
}

/// The automation lanes toggle and (unless the fx rack shows them) the insert count, when the
/// track has them.
fn header_extras(app: &mut DawApp, ui: &mut egui::Ui, owner: LaneOwner, fx_count: bool) {
    let project = &mut app.session.project;
    let (automation, show, inserts) = match owner {
        Some(id) => match project.track_mut(id) {
            Some(t) => (&t.automation, &mut t.show_automation, &t.inserts),
            None => return,
        },
        None => {
            let m = &mut project.master;
            (&m.automation, &mut m.show_automation, &m.inserts)
        }
    };
    if !automation.is_empty() {
        let tip = format!("Automation lanes ({}): show or hide them", automation.len());
        crate::automation::lanes_toggle(ui, show, &tip);
    }
    if fx_count && !inserts.is_empty() {
        ui.label(
            egui::RichText::new(format!("FX {}", inserts.len()))
                .small()
                .color(theme::TEXT_DIM),
        )
        .on_hover_text(
            inserts
                .iter()
                .map(|i| i.name.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
}

/// One fx rack slot's height.
const RACK_SLOT: f32 = 17.0;
/// The width of a rack slot's menu arrow.
const RACK_MENU: f32 = 14.0;
/// Slots the rack always offers (one more than the inserts when full).
const RACK_SLOTS: usize = 5;

/// What the fx rack was asked to do this frame, applied once its borrows are over.
enum RackAction {
    Bypass(InsertId, bool),
    BypassAll(bool),
    Editor(InsertId),
    /// Open the browser for this place in the chain, replacing an insert if given.
    Browse(usize, Option<InsertId>),
    Remove(InsertId),
    /// A slot dropped at this place: moved there (from this rack or another), or copied.
    Drop(InsertId, usize, bool),
    /// Put the plug-in on the plug-in clipboard; paste the copied one at this place.
    Copy(InsertId),
    Paste(usize),
    /// Choose which of the plug-in's extra outputs get tracks (`multi_out::Dialog`).
    MultiOut(InsertId),
    RemoveMultiOut(InsertId),
}

/// A filled rack slot: the insert, its name, whether it's bypassed, what
/// `Session::insert_info` says (has an editor, latency, editor open; `None`: not loaded),
/// `Session::multi_out` (extra outputs, how many have tracks), and why it isn't loaded.
type RackSlot = (
    InsertId,
    String,
    bool,
    Option<(bool, usize, bool)>,
    (usize, usize),
    NotLoaded,
);

/// Why a slot has no plug-in (`None` once it has one).
#[derive(Clone, Debug, PartialEq)]
enum NotLoaded {
    No,
    /// Still loading (a project or the kept rack coming in, a plug-in a frame).
    Loading,
    /// It failed to: why, when known. Its slot keeps its settings.
    Failed(Option<String>),
}

/// A rack slot being dragged to another place.
#[derive(Clone, Copy)]
struct RackDrag(InsertId);

/// The fx rack: a title line with a power button for the whole rack, then
/// numbered slots, each with its own power button (bypass), the plug-in's name (click to open
/// its editor) and a menu. An empty slot opens the browser for that place. The signal runs
/// top to bottom (slot 1 first). Drag a slot's number onto another slot to move it there, in
/// this rack or another track's (with its settings and lanes), or Ctrl-drag to copy it.
/// Scrolls when it doesn't fit `height`. A track header shows its track's; the Waveform view
/// shows its own (`InsertTarget::Editor`).
pub fn fx_rack(app: &mut DawApp, ui: &mut egui::Ui, target: InsertTarget, height: f32) {
    let loading = app.session.loading().is_some();
    let slots: Vec<RackSlot> = app
        .session
        .insert_slots(target)
        .iter()
        .map(|s| {
            let info = app.session.insert_info(s.id);
            let not_loaded = match (info, app.session.load_failure(s.id)) {
                (Some(_), _) => NotLoaded::No,
                (None, Some(why)) => NotLoaded::Failed(Some(why.to_string())),
                (None, None) if loading => NotLoaded::Loading,
                (None, None) => NotLoaded::Failed(None),
            };
            (
                s.id,
                s.name.clone(),
                s.bypass,
                info,
                app.session.multi_out(s.id),
                not_loaded,
            )
        })
        .collect();
    let paste = app.session.plugin_clipboard().map(|c| c.name.clone());
    let width = ui.available_width();
    let mut action = None;
    // The manual's names for this rack (`track.rack`, `master.rack`, `waveform.rack`).
    let mark = match target {
        InsertTarget::Track(_) => "track.rack",
        InsertTarget::Master => "master.rack",
        InsertTarget::Editor => "waveform.rack",
    };

    let title = ui.allocate_ui_with_layout(
        vec2(width, RACK_SLOT),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.label(
                egui::RichText::new("fx")
                    .small()
                    .italics()
                    .color(theme::TEXT_DIM),
            );
            ui.label(egui::RichText::new("↓").small().color(theme::TEXT_DIM))
                .on_hover_text(
                    "The signal runs top to bottom: slot 1 hears the input first, the last \
                     slot feeds the fader",
                );
            let any_on = slots.iter().any(|s| !s.2);
            let mut on = any_on;
            let tip = if slots.is_empty() {
                "The rack's power: no plug-ins yet"
            } else if any_on {
                "The rack's power: click to bypass every plug-in"
            } else {
                "The rack's power: click to turn every plug-in back on"
            };
            let power = ui.add_enabled_ui(!slots.is_empty(), |ui| power_button(ui, &mut on, tip));
            crate::marks::put_with(ui, || format!("{mark}.power"), power.response.rect);
            if power.inner {
                action = Some(RackAction::BypassAll(any_on));
            }
        },
    );

    let count = slots.len().max(RACK_SLOTS - 1) + 1;
    let list = egui::ScrollArea::vertical()
        .id_salt(("fx-rack", target))
        // What the title line and its spacing leave, so the bottom line stays in the header.
        .max_height((height - RACK_SLOT - 4.0).max(RACK_SLOT))
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            // Every row the same width, clear of the floating scroll bar, so the menu arrows
            // line up and nothing reaches the meter beside the header.
            let row = (ui.available_width() - 6.0).max(40.0);
            for k in 0..count {
                let slot = slots.get(k);
                let frame = egui::Frame::new()
                    .fill(Color32::from_gray(0x2a))
                    .corner_radius(2)
                    .inner_margin(egui::Margin::symmetric(2, 0));
                let (zone, dropped) = ui.dnd_drop_zone::<RackDrag, _>(frame, |ui| {
                    ui.set_width(row - 4.0);
                    ui.set_max_width(row - 4.0);
                    ui.set_height(RACK_SLOT - 2.0);
                    ui.horizontal_centered(|ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        rack_slot(ui, target, k, slot, paste.as_deref(), &mut action)
                    })
                    .inner
                });
                app.rack_slots
                    .push((target, k, zone.response.rect, zone.inner));
                crate::marks::put_with(ui, || format!("{mark}.slot.{}", k + 1), zone.response.rect);
                if let Some(drag) = dropped {
                    let copy = ui.input(|i| i.modifiers.command);
                    action = Some(RackAction::Drop(drag.0, k, copy));
                }
            }
        });
    // The title and the slots shown (not the room left under them).
    let shown = Rect::from_min_size(
        list.inner_rect.min,
        vec2(
            list.inner_rect.width(),
            list.content_size.y.min(list.inner_rect.height()),
        ),
    );
    crate::marks::put(ui, mark, title.response.rect.union(shown));

    let Some(action) = action else {
        return;
    };
    match action {
        RackAction::Bypass(insert, bypass) => {
            if let Some(slot) = app.session.insert_mut(insert) {
                slot.bypass = bypass;
            }
        }
        RackAction::BypassAll(bypass) => {
            let set = app.session.set_inserts_bypassed(target, bypass).map(|_| ());
            app.report(set);
        }
        RackAction::Editor(insert) => {
            if let Err(e) = app.session.toggle_editor(insert) {
                app.toast(format!("{e:#}"));
            }
        }
        RackAction::Browse(position, replacing) => {
            app.browser.open_at(target, Some(position), replacing);
        }
        RackAction::Remove(insert) => app.session.remove_insert(insert),
        RackAction::Copy(insert) => crate::plugin_clipboard::copy(app, insert),
        RackAction::Paste(position) => crate::plugin_clipboard::paste(app, target, position),
        RackAction::MultiOut(insert) => {
            app.multi_out = crate::multi_out::Dialog::open(app, insert);
        }
        RackAction::RemoveMultiOut(insert) => {
            let removed = app.session.remove_multi_out_rack(insert).map(|_| ());
            app.report(removed);
        }
        RackAction::Drop(insert, position, true) => {
            // A copy loads a new instance: behind the busy overlay.
            let name = app
                .session
                .find_insert(insert)
                .map_or_else(String::new, |s| s.name.clone());
            crate::busy::run(app, format!("Copying {name}"), move |app| {
                let done = app
                    .session
                    .copy_insert(insert, target, position)
                    .map(|_| ());
                app.session.mark_dirty();
                app.report(done);
            });
        }
        RackAction::Drop(insert, position, false) => {
            let done = if app
                .session
                .insert_slots(target)
                .iter()
                .any(|s| s.id == insert)
            {
                app.session.move_insert_to(insert, position);
                Ok(())
            } else {
                app.session
                    .move_insert_into(insert, target, position)
                    .map(|_| ())
            };
            app.report(done);
        }
    }
}

/// One rack slot's contents: its number (the drag handle), and for a plug-in its power button,
/// name and menu. Returns where the drag handle is, for a filled slot.
fn rack_slot(
    ui: &mut egui::Ui,
    target: InsertTarget,
    k: usize,
    slot: Option<&RackSlot>,
    paste: Option<&str>,
    action: &mut Option<RackAction>,
) -> Option<Rect> {
    let number = egui::RichText::new(format!("{}", k + 1))
        .small()
        .monospace()
        .color(theme::TEXT_DIM);
    let Some(&(insert, ref name, bypass, info, (outputs, routed), ref not_loaded)) = slot else {
        ui.label(number);
        // The rest of the empty row is the button.
        let rest = ui
            .allocate_response(ui.available_size(), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Add a plug-in here");
        if rest.clicked() {
            *action = Some(RackAction::Browse(k, None));
        }
        // Right-click: paste the copied plug-in here.
        rest.context_menu(|ui| {
            match paste {
                Some(name) => {
                    if ui.button(format!("Paste {name}")).clicked() {
                        *action = Some(RackAction::Paste(k));
                        ui.close();
                    }
                }
                None => {
                    ui.label(egui::RichText::new("Nothing copied").color(theme::TEXT_DIM));
                }
            }
            if ui.button("Add plug-in…").clicked() {
                *action = Some(RackAction::Browse(k, None));
                ui.close();
            }
        });
        return None;
    };
    let handle = ui
        .dnd_drag_source(
            ui.scope_id().with(("fx-rack-drag", target, insert)),
            RackDrag(insert),
            |ui| ui.label(number),
        )
        .response
        .on_hover_text(
            "Drag onto another slot to move it there, in this rack or another track's; \
             Ctrl-drag to copy it",
        )
        .rect;
    let mut on = !bypass;
    // A slot whose plug-in didn't load plays nothing either way, so its switch is greyed
    // rather than lit as if it were running.
    let failed = matches!(not_loaded, NotLoaded::Failed(_));
    let switched = ui
        .add_enabled_ui(!failed, |ui| {
            power_button(
                ui,
                &mut on,
                if bypass {
                    "Bypassed: click to turn on"
                } else {
                    "On: click to bypass"
                },
            )
        })
        .inner;
    if switched {
        *action = Some(RackAction::Bypass(insert, !on));
    }
    let loaded = info.is_some();
    let open = info.is_some_and(|(_, _, open)| open);
    let has_editor = info.is_some_and(|(editor, _, _)| editor);
    let text = egui::RichText::new(name).small();
    let text = match not_loaded {
        // Greyed: it is coming, or it kept its settings when its plug-in didn't load.
        NotLoaded::Loading => text.color(theme::TEXT_DIM).italics(),
        NotLoaded::Failed(Some(_)) => text.color(theme::TEXT_DIM).strikethrough(),
        NotLoaded::Failed(None) => text.color(theme::RECORD).strikethrough(),
        NotLoaded::No => text,
    };
    let text = if !loaded {
        text
    } else if bypass {
        text.color(theme::TEXT_DIM)
    } else {
        text
    };
    let name_width = (ui.available_width() - RACK_MENU - 3.0).max(10.0);
    // The name, left-aligned, lit while its editor is open.
    let (rect, label) = ui.allocate_exact_size(vec2(name_width, RACK_SLOT - 4.0), Sense::click());
    if open || label.hovered() {
        let fill = if open {
            ui.visuals().selection.bg_fill
        } else {
            Color32::from_gray(0x38)
        };
        ui.painter().rect_filled(rect, 2, fill);
    }
    // Painted into the slot (truncated with an ellipsis), so the row keeps its layout.
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        rect.width() - 6.0,
        egui::TextStyle::Small,
    );
    let at = pos2(rect.left() + 3.0, rect.center().y - galley.size().y / 2.0);
    ui.painter_at(rect)
        .galley(at, galley, ui.visuals().text_color());
    let label = label.on_hover_text(match info {
        Some((true, latency, _)) => {
            format!("{name}: click to open its editor\nLatency {latency} samples")
        }
        Some((false, latency, _)) => format!("{name}: no editor\nLatency {latency} samples"),
        None => match not_loaded {
            NotLoaded::Loading => format!("{name}: loading…"),
            NotLoaded::Failed(Some(why)) => format!(
                "{name} didn't load: {why}\nIts settings are kept with the slot; remove it to \
                 forget them"
            ),
            _ => format!("{name} failed to load"),
        },
    });
    if label.clicked() && has_editor {
        *action = Some(RackAction::Editor(insert));
    }
    let menu = |ui: &mut egui::Ui, action: &mut Option<RackAction>| {
        if ui
            .add_enabled(
                has_editor,
                egui::Button::new(if open { "Close editor" } else { "Open editor" }),
            )
            .clicked()
        {
            *action = Some(RackAction::Editor(insert));
            ui.close();
        }
        if ui
            .button(if bypass { "Turn on" } else { "Bypass" })
            .clicked()
        {
            *action = Some(RackAction::Bypass(insert, !bypass));
            ui.close();
        }
        if ui.button("Replace…").clicked() {
            *action = Some(RackAction::Browse(k, Some(insert)));
            ui.close();
        }
        ui.separator();
        if ui
            .button("Copy")
            .on_hover_text("Copy it with its settings, to paste on any track")
            .clicked()
        {
            *action = Some(RackAction::Copy(insert));
            ui.close();
        }
        if let Some(name) = paste {
            if ui.button(format!("Paste {name} after")).clicked() {
                *action = Some(RackAction::Paste(k + 1));
                ui.close();
            }
        }
        // A multi-out instrument's extra outputs, on tracks of their own (a track's chain).
        if outputs > 0 && matches!(target, InsertTarget::Track(_)) {
            ui.separator();
            if ui
                .button("Create multi-out rack in a submix…")
                .on_hover_text(format!(
                    "Route its {outputs} extra outputs (a drum kit's kick, snare...) to tracks \
                     of their own, in a folder under this track"
                ))
                .clicked()
            {
                *action = Some(RackAction::MultiOut(insert));
                ui.close();
            }
            if routed > 0 && ui.button("Remove multi-out rack").clicked() {
                *action = Some(RackAction::RemoveMultiOut(insert));
                ui.close();
            }
        }
        if ui.button("Remove").clicked() {
            *action = Some(RackAction::Remove(insert));
            ui.close();
        }
    };
    label.context_menu(|ui| menu(ui, action));
    egui::containers::menu::MenuButton::from_button(
        egui::Button::new(egui::RichText::new("▸").color(theme::TEXT_DIM))
            .frame(false)
            .min_size(vec2(RACK_MENU, RACK_SLOT - 4.0)),
    )
    .ui(ui, |ui| menu(ui, action))
    .0
    .on_hover_text("Open, bypass, replace or remove");
    Some(handle)
}

/// A small power button, lit when `on`: the power symbol (a broken ring and a stroke), drawn
/// rather than typed, since the app's fonts have no glyph for it. Returns whether it was
/// clicked (and flips `on`).
fn power_button(ui: &mut egui::Ui, on: &mut bool, tip: &str) -> bool {
    power_button_at(ui, on, tip).0
}

/// [`power_button`], and where it was drawn (the manual's marks).
fn power_button_at(ui: &mut egui::Ui, on: &mut bool, tip: &str) -> (bool, Rect) {
    let (rect, response) = ui.allocate_exact_size(vec2(16.0, 14.0), Sense::click());
    let response = response.on_hover_text(tip);
    let enabled = ui.is_enabled();
    let fill = if *on && enabled {
        theme::SOLO
    } else if response.hovered() {
        Color32::from_gray(0x40)
    } else {
        Color32::from_gray(0x33)
    };
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2, fill);
    let ink = if *on && enabled {
        Color32::BLACK
    } else {
        theme::TEXT_DIM
    };
    let c = rect.center();
    let r = 4.0;
    // The ring, open at the top (about 60 degrees either side of straight up).
    let ring: Vec<_> = (0..=20)
        .map(|k| {
            let a = 0.55 + (std::f32::consts::TAU - 1.1) * k as f32 / 20.0;
            c + vec2(a.sin(), -a.cos()) * r
        })
        .collect();
    painter.add(egui::Shape::line(ring, Stroke::new(1.3, ink)));
    painter.line_segment(
        [c + vec2(0.0, -r - 1.0), c + vec2(0.0, -0.5)],
        Stroke::new(1.3, ink),
    );
    let clicked = response.clicked();
    if clicked {
        *on = !*on;
    }
    (clicked, rect)
}

/// The folder arrow, the kind icon and the name (double-click to rename), at most
/// `max_width` wide.
fn header_name(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, max_width: f32) {
    let Some(track) = app.session.project.track_mut(id) else {
        return;
    };
    if track.kind == TrackKind::Folder {
        let arrow = if track.collapsed { "▶" } else { "▼" };
        if ui
            .small_button(arrow)
            .on_hover_text("Show or hide sub-tracks")
            .clicked()
        {
            track.collapsed = !track.collapsed;
        }
    }
    let kind_icon = match track.kind {
        TrackKind::Audio => "〰",
        TrackKind::Instrument => "🎹",
        TrackKind::Folder => "📁",
        TrackKind::Bus => "↩",
    };
    ui.label(egui::RichText::new(kind_icon).color(theme::TEXT_DIM));
    // Renaming (22.1): Enter or a click away keeps the name, Escape the old one
    // (`value_edit`); through the session, as `set_track` renames.
    let field = value_edit::name_id(value_edit::Place::Header, Some(id));
    if value_edit::is_open(ui.ctx(), field) {
        let width = ui.available_width().min(max_width).max(60.0);
        let font = egui::TextStyle::Body.resolve(ui.style());
        if let Some(name) = value_edit::name_field(ui, field, vec2(width, HEADER_LINE), font) {
            if let Err(e) = app.session.rename_track(id, &name) {
                app.toast(format!("{e:#}"));
            }
        }
    } else {
        let name = ui
            .scope(|ui| {
                // Never wider than the header, so long names still truncate; the part that
                // tells similar tracks apart ("(bass)", "2") stays.
                let width = max_width.min(ui.available_width());
                ui.set_max_width(width);
                let font = egui::TextStyle::Body.resolve(ui.style());
                let shown = keep_ending(ui, &track.name, width, font);
                let label =
                    ui.add(egui::Label::new(egui::RichText::new(&shown).strong()).truncate());
                // An id of the track's own: the header's scope is numbered by its row, and a
                // name being dragged moves to another row.
                let name = ui.interact(
                    label.rect,
                    egui::Id::unique(("track-name", id)),
                    Sense::click_and_drag(),
                );
                value_edit::remember(ui, field, label.rect, &shown);
                crate::marks::put(ui, "track.name", label.rect);
                let name = name.on_hover_text("Double-click to rename");
                if shown != track.name {
                    name.on_hover_text(&track.name)
                } else {
                    name
                }
            })
            .inner;
        if name.double_clicked() {
            value_edit::open(ui.ctx(), field, track.name.clone(), id);
        } else if name.clicked() {
            app.selection = Some(Selection::Track(id));
        }
        crate::track_drag::respond(app, &name, id, crate::track_drag::Surface::Arrange);
    }
}

/// `name` shortened to fit `width` in `font`, keeping its ending when that is what tells
/// tracks apart (`ending`): "Acoustic Piano (bass)" becomes "Acoustic…(bass)", not
/// "Acoustic Pi…". Other names, and those whose ending alone barely fits, come back whole,
/// to be cut at the end. Track headers and mixer strips use it.
pub fn keep_ending(ui: &egui::Ui, name: &str, width: f32, font: egui::FontId) -> String {
    let Some(tail) = ending(name) else {
        return name.to_string();
    };
    let wide = |text: &str| {
        ui.painter()
            .layout_no_wrap(text.to_string(), font.clone(), egui::Color32::WHITE)
            .size()
            .x
    };
    if wide(name) <= width {
        return name.to_string();
    }
    let head: Vec<char> = name[..name.len() - tail.len()].trim_end().chars().collect();
    (1..head.len())
        .rev()
        .map(|n| format!("{}…{tail}", head[..n].iter().collect::<String>().trim_end()))
        .find(|t| wide(t) <= width)
        .unwrap_or_else(|| name.to_string())
}

/// The end of a name that tells it from similar ones: a part in brackets, "(treble)", or a
/// number with its space, "Gtr 2".
fn ending(name: &str) -> Option<&str> {
    let name = name.trim_end();
    if name.ends_with(')') {
        let open = name.rfind('(').filter(|&i| i > 0)?;
        return Some(&name[open..]);
    }
    let space = name.rfind(' ')?;
    let last = &name[space + 1..];
    (!last.is_empty() && last.chars().all(|c| c.is_ascii_digit())).then_some(&name[space..])
}

/// A track header's right-click menu.
fn header_menu(app: &mut DawApp, id: TrackId, response: &egui::Response) {
    let mut changed_structure = false;
    response.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            let name = app
                .session
                .project
                .track(id)
                .map(|t| t.name.clone())
                .unwrap_or_default();
            let field = value_edit::name_id(value_edit::Place::Header, Some(id));
            value_edit::open(ui.ctx(), field, name, id);
            ui.close();
        }
        let is_folder = app
            .session
            .project
            .track(id)
            .is_some_and(|t| t.kind == TrackKind::Folder);
        if is_folder {
            if ui.button("Add audio track inside").clicked() {
                let new = app.session.add_track(TrackKind::Audio, Some(id));
                app.selection = Some(Selection::Track(new));
                ui.close();
            }
            if ui.button("Add instrument track inside").clicked() {
                let new = app.session.add_track(TrackKind::Instrument, Some(id));
                app.selection = Some(Selection::Track(new));
                ui.close();
            }
        }
        ui.menu_button("Add send", |ui| crate::sends::add_send_menu(app, ui, id));
        ui.menu_button("Add automation lane", |ui| {
            crate::automation::add_lane_menu(app, ui, Some(id));
        });
        if let Some(track) = app.session.project.track_mut(id) {
            if !track.automation.is_empty() {
                ui.add(crate::theme::checkbox(
                    &mut track.show_automation,
                    "Show automation lanes",
                ));
            }
        }
        ui.menu_button("Height", |ui| {
            for (name, height) in TRACK_HEIGHT_PRESETS {
                if ui.button(name).clicked() {
                    if let Some(track) = app.session.project.track_mut(id) {
                        track.height = Some(height);
                    }
                    ui.close();
                }
            }
            if ui.button("Default").clicked() {
                if let Some(track) = app.session.project.track_mut(id) {
                    track.height = None;
                }
                ui.close();
            }
            ui.separator();
            ui.label(egui::RichText::new("All tracks").weak());
            for (name, height) in TRACK_HEIGHT_PRESETS {
                if ui.button(format!("All {}", name.to_lowercase())).clicked() {
                    app.set_all_track_heights(height);
                    ui.close();
                }
            }
        });
        ui.separator();
        if ui.button("Move up").clicked() {
            app.session.project.move_track(id, true);
            changed_structure = true;
            ui.close();
        }
        if ui.button("Move down").clicked() {
            app.session.project.move_track(id, false);
            changed_structure = true;
            ui.close();
        }
        let folders: Vec<(TrackId, String)> = app
            .session
            .project
            .valid_buses(id)
            .iter()
            .filter(|t| t.kind == TrackKind::Folder)
            .map(|t| (t.id, t.name.clone()))
            .collect();
        let parent = app.session.project.track(id).and_then(|t| t.parent);
        ui.menu_button("Move into folder", |ui| {
            for (folder, name) in &folders {
                if Some(*folder) != parent && ui.button(name).clicked() {
                    match app.session.project.set_parent(id, Some(*folder)) {
                        Ok(()) => changed_structure = true,
                        Err(e) => app.toast(e),
                    }
                    ui.close();
                }
            }
            if folders.is_empty() {
                ui.label("No folders");
            }
        });
        if parent.is_some() && ui.button("Move out of folder").clicked() {
            let grandparent = parent
                .and_then(|p| app.session.project.track(p))
                .and_then(|f| f.parent);
            match app.session.project.set_parent(id, grandparent) {
                Ok(()) => changed_structure = true,
                Err(e) => app.toast(e),
            }
            ui.close();
        }
        ui.separator();
        crate::render::menu_items(app, ui, id);
        ui.separator();
        if ui.button("Delete track").clicked() {
            app.session.remove_track(id);
            if app.selection == Some(Selection::Track(id)) {
                app.selection = None;
            }
            ui.close();
        }
    });
    if changed_structure {
        app.session.mark_dirty();
    }
}

#[allow(clippy::needless_range_loop)] // two channels, indexed in parallel
pub fn draw_vertical_meter(painter: &egui::Painter, rect: Rect, meter: &Meter) {
    painter.rect_filled(rect, 1, Color32::from_gray(0x15));
    let w = rect.width() / 2.0;
    let db = meter.db();
    for c in 0..2 {
        let column = Rect::from_min_size(
            pos2(rect.left() + w * c as f32, rect.top()),
            vec2(w - 1.0, rect.height()),
        );
        let frac = db_to_fraction(db[c]);
        let top = column.bottom() - column.height() * frac;
        let color = if db[c] > -0.1 {
            theme::METER_HIGH
        } else if db[c] > -9.0 {
            theme::METER_MID
        } else {
            theme::METER_LOW
        };
        painter.rect_filled(
            Rect::from_min_max(pos2(column.left(), top), column.max),
            0,
            color,
        );
        let hold = db_to_fraction(daw_core::util::gain_to_db(meter.hold[c]));
        if hold > 0.0 {
            let y = column.bottom() - column.height() * hold;
            painter.hline(column.x_range(), y, Stroke::new(1.0, Color32::WHITE));
        }
    }
}

/// Meter scale: -60 dB at the bottom, 0 dB near the top, with headroom to +6.
pub fn db_to_fraction(db: f32) -> f32 {
    ((db + 60.0) / 66.0).clamp(0.0, 1.0)
}

/// While files hover from Explorer: a ghost of each clip they'd make, at the (snapped) time
/// under the pointer, on the row it would land on, or on a "new track" row below the tracks
/// where none can take it (the same plan the drop follows, `drop::plan`).
fn draw_drop_ghost(app: &mut DawApp, ui: &egui::Ui, lanes: Rect, axis: TimeAxis) {
    let hovered: Vec<std::path::PathBuf> = ui.input(|i| {
        i.raw
            .hovered_files
            .iter()
            .filter_map(|f| f.path.clone())
            .collect()
    });
    if hovered.is_empty() {
        app.drop_ghost = None;
        return;
    }
    if app.drop_ghost.as_ref().is_none_or(|g| g.paths != hovered) {
        let rate = app.session.sample_rate();
        app.drop_ghost = Some(crate::drop::Ghost::new(hovered, rate));
    }
    let Some(p) = app.drag_pointer.filter(|p| p.y >= lanes.top()) else {
        return;
    };
    ui.ctx().request_repaint();
    let ghost = app.drop_ghost.as_ref().expect("made");
    let at = snap(app, axis.sample(p.x.max(lanes.left()))).max(0.0) as u64;
    let rows: Vec<(TrackId, TrackKind)> = app.drop_rows.iter().map(|r| (r.0, r.1)).collect();
    let start = app
        .drop_rows
        .iter()
        .position(|r| r.2.y_range().contains(p.y));
    let kinds: Vec<TrackKind> = ghost.clips.iter().map(|c| c.kind).collect();
    let targets = crate::drop::plan(&rows, start, &kinds);
    let map = app.session.tempo_map();
    let painter = ui.painter_at(lanes);
    let below = app.drop_rows.last().map_or(lanes.top(), |r| r.2.bottom());
    let mut new_rows = 0.0;
    for (k, clip) in ghost.clips.iter().enumerate() {
        let row = match targets[k].and_then(|t| app.drop_rows.iter().find(|r| r.0 == t)) {
            Some(r) => Rect::from_min_max(
                pos2(lanes.left(), r.2.top()),
                pos2(lanes.right(), r.2.bottom()),
            ),
            None => {
                // A new track's row, below the tracks.
                let top = below + new_rows * 60.0;
                new_rows += 1.0;
                let slot =
                    Rect::from_min_max(pos2(lanes.left(), top), pos2(lanes.right(), top + 56.0));
                painter.rect_stroke(
                    slot,
                    3,
                    Stroke::new(1.0, theme::TEXT_DIM.gamma_multiply(0.5)),
                    egui::StrokeKind::Inside,
                );
                painter.text(
                    slot.left_top() + vec2(8.0, 6.0),
                    Align2::LEFT_TOP,
                    "New track",
                    FontId::proportional(11.0),
                    theme::TEXT_DIM,
                );
                slot
            }
        };
        // Until an audio file's length is read, a bar's worth.
        let length = ghost.length(k, at, &map).unwrap_or_else(|| {
            (map.sample_at_tick(map.tick_at(at as f64) + 4.0 * 960.0) - at as f64) as u64
        });
        let x0 = axis.x(at as f64);
        let x1 = axis.x((at + length) as f64).max(x0 + 8.0);
        let rect = Rect::from_min_max(pos2(x0, row.top() + 2.0), pos2(x1, row.bottom() - 3.0));
        painter.rect_filled(rect, 3, theme::ACCENT.gamma_multiply(0.18));
        painter.rect_stroke(
            rect,
            3,
            Stroke::new(1.5, theme::ACCENT),
            egui::StrokeKind::Inside,
        );
        painter.text(
            rect.left_top() + vec2(4.0, 2.0),
            Align2::LEFT_TOP,
            &clip.name,
            FontId::proportional(11.0),
            Color32::WHITE,
        );
    }
}

fn handle_dropped_files(app: &mut DawApp, ui: &egui::Ui, lanes: Rect, axis: TimeAxis) {
    let dropped = ui.input(|i| i.raw.dropped_files.clone());
    if dropped.is_empty() {
        return;
    }
    // Where Windows says the pointer is (winit sends no position during a file drag), else
    // where egui last saw it.
    let pointer = app
        .drag_pointer
        .or_else(|| ui.input(|i| i.pointer.latest_pos()));
    // The time under the pointer (snapped), or the playhead if it's off the lanes.
    let at = pointer
        .filter(|p| lanes.contains(*p))
        .map_or(app.session.position() as f64, |p| {
            snap(app, axis.sample(p.x))
        });
    // The master holds no clips: a drop on its row is refused, saying where to drop instead.
    if pointer.is_some_and(|p| app.master_row.is_some_and(|r| r.contains(p))) {
        app.toast(
            "The master holds no clips: drop files on a track, or below the tracks for new ones"
                .to_string(),
        );
        return;
    }
    // The row under the pointer; below the last one, none (new tracks at the end).
    let rows: Vec<(TrackId, TrackKind)> = app.drop_rows.iter().map(|r| (r.0, r.1)).collect();
    let start = pointer.and_then(|p| {
        app.drop_rows
            .iter()
            .position(|r| r.2.y_range().contains(p.y))
    });
    let paths = dropped.iter().map(|f| f.path().to_path_buf()).collect();
    crate::drop::drop_files(app, paths, at.max(0.0) as u64, rows, start);
}

/// Ctrl (Cmd) or Shift held: a click adds to the selection or takes away from it.
fn extend_selection(ui: &egui::Ui) -> bool {
    ui.input(|i| i.modifiers.command || i.modifiers.shift)
}

/// A click on a clip's body (audio or MIDI; 25.2): the clip selected (alone, or with Ctrl /
/// Shift toggled in or out) with its track, and the playhead to the click as a lane click
/// puts it ([`locate_click`]). The time selection goes, as with a lane click: S and Delete
/// then act on the clip at the playhead, not on a range left from before. Only a click comes
/// here: a drag moves the clip ([`drag_clips`]), a right-click opens its menu, and the fade
/// handles, the gain line and the edges are widgets on top that take their own presses.
fn clip_clicked(
    app: &mut DawApp,
    ui: &egui::Ui,
    response: &egui::Response,
    track: TrackId,
    clip: ClipId,
    axis: TimeAxis,
) {
    app.select_clip(track, clip, extend_selection(ui));
    app.time_selection = None;
    if let Some(p) = response.interact_pointer_pos() {
        locate_click(app, axis, p.x);
    }
}

/// Drag clips: the one under the pointer, with every other selected clip following (by the
/// same time and the same number of track rows), onto the track under the pointer too. Alt
/// held as it begins drags copies and leaves the clips where they were. The pressed clip's
/// start or end near the playhead goes onto it ([`snap_clip`]) unless Ctrl is held, checked
/// each frame. Ctrl on the press changes no selection here: an unselected clip is picked
/// alone, as without it (Ctrl-click, which never starts a drag, still adds to the
/// selection). Here only the pointer is noted; [`apply_clip_drag`] moves them once every row
/// has been drawn.
#[allow(clippy::too_many_arguments)]
fn drag_clips(
    app: &mut DawApp,
    response: &egui::Response,
    track: TrackId,
    clip: ClipId,
    start: f64,
    axis: TimeAxis,
) {
    if response.drag_started() {
        if !app.is_clip_selected(clip) {
            app.select_clip(track, clip, false);
        }
        let mut ids = app.selected_clip_ids();
        // The pressed clip first, so its copy is the anchor (and the primary selection).
        ids.retain(|&c| c != clip);
        ids.insert(0, clip);
        let copies = response.ctx.input(|i| i.modifiers.alt);
        let mut anchor = clip;
        if copies {
            match app.session.copy_clips_to(&ids, 0, 0) {
                Ok(made) => {
                    anchor = made[0];
                    app.select_clips(&made);
                    ids = made;
                    let key = format!("Copy clips:{anchor}");
                    app.session.label_edit("Copy clips", Some(key));
                }
                Err(e) => app.toast(format!("{e:#}")),
            }
        }
        // The pointer's movement since the press, not since the drag was decided (a few points
        // on): the clip keeps the press point's place under the pointer. A long press decides a
        // drag without a move, and nothing moves until the pointer does (25.1).
        let press = response.ctx.input(|i| i.pointer.press_origin());
        let pointer = response.interact_pointer_pos();
        let (moved, moving) = match (press, pointer) {
            (Some(press), Some(pointer)) => {
                (f64::from(pointer.x - press.x) * axis.spp, pointer != press)
            }
            _ => (0.0, false),
        };
        if let Ok(origin) = app.session.clips_by_id(&ids) {
            app.group_drag = Some(crate::app::GroupDrag {
                grip: clip,
                anchor,
                origin,
                anchor_start: start,
                moved,
                row: app.session.project.track_index(track).unwrap_or(0),
                pointer,
                due: moving,
                // This frame's movement is counted above.
                frame: response.ctx.cumulative_frame_nr(),
                delta: 0,
                rows: 0,
                refused: None,
                copies,
                ctrl: false,
                magnet: false,
                moving,
            });
        }
    }
    if response.dragged() {
        let frame = response.ctx.cumulative_frame_nr();
        if let Some(drag) = app.group_drag.as_mut().filter(|d| d.grip == clip) {
            if drag.frame != frame {
                drag.frame = frame;
                let delta = response.drag_delta();
                drag.moved += f64::from(delta.x) * axis.spp;
                drag.pointer = response.interact_pointer_pos();
                drag.moving |= delta != egui::Vec2::ZERO;
                drag.due = drag.moving;
            }
        }
    }
    if response.drag_stopped() {
        // An Alt-drag let go where it began made copies on top of their originals: none.
        if let Some(drag) = app
            .group_drag
            .take()
            .filter(|d| d.copies && d.delta == 0 && d.rows == 0)
        {
            let ids: Vec<ClipId> = drag.origin.iter().map(|(_, c)| c.id()).collect();
            let _ = app.session.delete_clips(&ids);
            app.select_clip(track, clip, false);
        }
        app.group_drag = None;
        app.session.mark_dirty();
        // The primary clip may be on another track now.
        if let Some((_, primary)) = app.selected_clip {
            if let Some(t) = app.session.project.clip_track(primary) {
                app.selected_clip = Some((t, primary));
                app.selection = Some(Selection::Track(t));
            }
        }
    }
}

/// The clip drag's move for this frame, now that every row is drawn and known: in time (only
/// the anchor snaps, to the playhead by the magnet or else to the grid), and onto the track under the pointer when the tracks can take the
/// clips. Over one that can't (a folder or bus, MIDI onto a track that isn't an instrument),
/// they stay on the last track that could, and that lane says why not.
fn apply_clip_drag(app: &mut DawApp, ui: &egui::Ui) {
    let Some(mut drag) = app.group_drag.take() else {
        return;
    };
    // Ctrl turns the magnet off (or back on) the moment it changes, the pointer still or not.
    let ctrl = ui.input(|i| i.modifiers.ctrl);
    if ctrl != drag.ctrl {
        drag.ctrl = ctrl;
        drag.due |= drag.moving;
    }
    if drag.due {
        drag.due = false;
        // The anchor's edges to the playhead (the magnet), else its start to the grid.
        let span = drag
            .origin
            .iter()
            .find(|(_, c)| c.id() == drag.anchor)
            .map_or(Span::Samples(0.0), |(_, c)| Span::of(c));
        let (to, held) = snap_clip(app, drag.anchor_start + drag.moved, span, ctrl);
        let delta = (to - drag.anchor_start).round() as i64;
        let under = drag.pointer.and_then(|p| {
            app.drop_rows
                .iter()
                .find(|(_, _, r)| r.y_range().contains(p.y))
                .map(|(t, _, _)| *t)
        });
        let wanted = under
            .and_then(|t| app.session.project.track_index(t))
            .map(|r| r as i64 - drag.row as i64);
        drag.refused = None;
        let took = wanted.is_some_and(|rows| {
            app.session
                .move_clips_from(&drag.origin, delta, rows)
                .is_ok()
        });
        if took {
            drag.rows = wanted.expect("took");
        } else {
            drag.refused = under.filter(|_| wanted != Some(drag.rows));
            // Past the first or last row the pointer is over no track: the last one stays.
            if app
                .session
                .move_clips_from(&drag.origin, delta, drag.rows)
                .is_err()
            {
                // Earlier than 0 for a clip behind the anchor: where it was last frame.
                drag.due = false;
                app.group_drag = Some(drag);
                return;
            }
        }
        drag.delta = delta;
        drag.magnet = held;
        let (name, verb) = if drag.copies {
            ("Copy clips", "Copy clips")
        } else if drag.origin.len() > 1 {
            ("Move clips", "Move clips")
        } else {
            ("Move clip", "Move clip")
        };
        app.session
            .label_edit(name, Some(format!("{verb}:{}", drag.anchor)));
    }
    if let Some(refused) = drag.refused {
        let midi = drag
            .origin
            .iter()
            .any(|(_, c)| matches!(c, daw_core::project::AnyClip::Midi(_)));
        let row = app.drop_rows.iter().find(|(t, _, _)| *t == refused);
        if let Some(&(_, kind, rect)) = row {
            let lane = Rect::from_min_max(pos2(rect.left() + HEADER_WIDTH, rect.top()), rect.max);
            let why = if !kind.holds_clips() {
                "Clips can't go on a folder or bus"
            } else if midi {
                "MIDI clips go on instrument tracks"
            } else {
                "The clips can't go here"
            };
            let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
            painter.rect_filled(lane, 0, theme::REFUSED_FILL);
            painter.rect_stroke(
                lane.shrink(1.0),
                0,
                Stroke::new(1.5, theme::REFUSED),
                egui::StrokeKind::Inside,
            );
            let at = drag.pointer.map_or(lane.center().x, |p| {
                p.x.clamp(lane.left() + 120.0, lane.right() - 120.0)
            });
            painter.text(
                pos2(at, lane.center().y),
                Align2::CENTER_CENTER,
                why,
                FontId::proportional(13.0),
                Color32::WHITE,
            );
        }
    }
    app.group_drag = Some(drag);
}

/// A time selection being dragged on the lanes: where it began (a sample, snapped, and the
/// pointer's y), and with Shift the selection it extends.
pub struct RangeDrag {
    anchor: f64,
    anchor_y: f32,
    base: Option<TimeSelection>,
}

/// The clips (audio and MIDI) on `range`'s tracks that overlap it.
pub(crate) fn clips_in(app: &DawApp, range: &TimeSelection) -> Vec<ClipId> {
    let map = &app.tempo_map;
    let (from, to) = (range.start as f64, range.end as f64);
    let mut hit = Vec::new();
    for &track in &range.tracks {
        let Some(t) = app.session.project.track(track) else {
            continue;
        };
        let spans = t
            .clips
            .iter()
            .map(|c| (c.id, c.start as f64, c.end() as f64))
            .chain(t.midi_clips.iter().map(|c| {
                (
                    c.id,
                    map.sample_at_tick(c.start as f64),
                    map.sample_at_tick(c.end() as f64),
                )
            }));
        hit.extend(spans.filter(|s| s.1 < to && s.2 > from).map(|s| s.0));
    }
    hit
}

/// Follow a time selection's drag: the range from where it began to the pointer (snapped when
/// Snap is on), across the tracks the drag crosses, selecting the clips inside as it goes.
/// With Shift it extends the selection it began with.
pub(crate) fn time_selection_drag(app: &mut DawApp, ui: &egui::Ui, axis: TimeAxis) {
    let Some(drag) = &app.range_drag else {
        return;
    };
    let (pointer, down) = ui.input(|i| (i.pointer.latest_pos(), i.pointer.primary_down()));
    if let Some(p) = pointer {
        let here = snap(app, axis.sample(p.x)).max(0.0);
        let (start, end) = (drag.anchor.min(here) as u64, drag.anchor.max(here) as u64);
        let ys = egui::Rangef::new(drag.anchor_y.min(p.y), drag.anchor_y.max(p.y));
        let mut tracks: Vec<TrackId> = app
            .drop_rows
            .iter()
            .filter(|(_, _, row)| row.y_range().intersects(ys))
            .map(|(id, _, _)| *id)
            .collect();
        let mut range = TimeSelection {
            start,
            end,
            tracks: Vec::new(),
        };
        if let Some(base) = &drag.base {
            range.start = range.start.min(base.start);
            range.end = range.end.max(base.end);
            for &t in &base.tracks {
                if !tracks.contains(&t) {
                    tracks.push(t);
                }
            }
        }
        range.tracks = tracks;
        // Nothing until the drag has width: a click clears instead.
        if range.end > range.start && !range.tracks.is_empty() {
            app.set_time_selection(range, false);
        }
    }
    if !down {
        app.range_drag = None;
    }
}

/// Shade the time selection on its tracks' lanes, with its edges.
fn draw_time_selection(app: &DawApp, ui: &egui::Ui, axis: TimeAxis) {
    let Some(range) = &app.time_selection else {
        return;
    };
    let (x0, x1) = (
        axis.x(range.start as f64).max(axis.left),
        axis.x(range.end as f64).max(axis.left),
    );
    if x1 <= x0 {
        return;
    }
    let painter = ui.painter();
    let edge = Stroke::new(1.0, Color32::from_white_alpha(200));
    for (track, _, row) in &app.drop_rows {
        if !range.tracks.contains(track) {
            continue;
        }
        let band = Rect::from_x_y_ranges(x0..=x1, row.y_range());
        painter.rect_filled(band, 0, Color32::from_white_alpha(40));
        for x in [x0, x1] {
            if x > axis.left {
                painter.vline(x, row.y_range(), edge);
            }
        }
    }
}

/// A lane's right-click menu: the time selection's commands while there is one, then
/// "Insert MIDI track" (and on a MIDI track "Insert MIDI clip") over the selection, or two bars
/// at the click without one.
fn lane_menu(app: &mut DawApp, ui: &mut egui::Ui, track: TrackId, kind: TrackKind) {
    if app.time_selection.is_some() {
        selection_menu(app, ui);
        ui.separator();
    }
    let (start, end) = midi_range(app);
    ui.menu_button("Insert MIDI track", |ui| {
        let choices = app.instrument_choices();
        if !choices.is_empty() {
            ui.label(egui::RichText::new("With an instrument").small().weak());
        }
        for path in choices {
            let name = daw_core::plugins::display_name(&path);
            if ui.button(&name).clicked() {
                crate::busy::run(app, format!("Loading {name}"), move |app| {
                    app.insert_midi_track(start, end, Some(track), Some(path));
                });
                ui.close();
            }
        }
        if ui
            .button("Choose instrument…")
            .on_hover_text("Insert the track, then pick its instrument in the browser")
            .clicked()
        {
            if let Some(new) = app.insert_midi_track(start, end, Some(track), None) {
                app.browser
                    .open_at(daw_core::session::InsertTarget::Track(new), Some(0), None);
            }
            ui.close();
        }
        if ui.button("No instrument").clicked() {
            app.insert_midi_track(start, end, Some(track), None);
            ui.close();
        }
    });
    if kind == TrackKind::Instrument && ui.button("Insert MIDI clip").clicked() {
        insert_midi_clip(app, track, start, end);
        ui.close();
    }
}

/// The range a new MIDI clip covers: the time selection (whole bars when Snap is on), or from
/// the right-click (its bar when Snap is on) with no end, for two bars.
fn midi_range(app: &DawApp) -> (u64, Option<u64>) {
    let map = &app.tempo_map;
    let whole_bars = app.settings.snap;
    match &app.time_selection {
        Some(range) if whole_bars => {
            let first = map.bar_at(range.start as f64).0;
            let last = map.bar_at(range.end.saturating_sub(1) as f64).0;
            (
                map.bar_start(first).round() as u64,
                Some(map.bar_start(last + 1).round() as u64),
            )
        }
        Some(range) => (range.start, Some(range.end)),
        None => {
            let at = app.lane_menu_at.map_or(0, |(_, at)| at);
            let start = if whole_bars {
                map.bar_start(map.bar_at(at as f64).0).round() as u64
            } else {
                at
            };
            (start, None)
        }
    }
}

/// An empty MIDI clip on an existing MIDI track over `start..end` (two bars without an end),
/// selected and opened in the piano roll.
fn insert_midi_clip(app: &mut DawApp, track: TrackId, start: u64, end: Option<u64>) {
    let map = &app.tempo_map;
    let from = map.tick_at(start as f64).round() as u64;
    let to = match end {
        Some(end) => map.tick_at(end as f64).round() as u64,
        None => {
            let bar = map.bar_at(start as f64).0;
            from + map
                .tick_of_bar(bar + 2, 0)
                .saturating_sub(map.tick_of_bar(bar, 0))
        }
    };
    app.session.label_edit("Insert MIDI clip", None);
    match app
        .session
        .project
        .add_midi_clip(track, from, to.saturating_sub(from).max(1))
    {
        Ok(clip) => {
            app.session.mark_dirty();
            app.select_clip(track, clip, false);
            if let Err(e) = crate::piano_roll::open(app, clip) {
                app.toast(e);
            }
        }
        Err(e) => app.toast(e),
    }
}

/// The time selection's commands, at the top of the lane menu while there is one.
fn selection_menu(app: &mut DawApp, ui: &mut egui::Ui) {
    let Some(range) = app.time_selection.clone() else {
        return;
    };
    let rate = f64::from(app.session.sample_rate());
    ui.label(
        egui::RichText::new(format!(
            "Selection {:.2}–{:.2} s",
            range.start as f64 / rate,
            range.end as f64 / rate
        ))
        .strong(),
    );
    if ui.button("Loop selection  (L)").clicked() {
        app.loop_selection();
        ui.close();
    }
    if ui.button("Split at edges  (S)").clicked() {
        app.split_selection();
        ui.close();
    }
    if ui.button("Delete in selection  (Del)").clicked() {
        app.delete_selection();
        ui.close();
    }
    if ui.button("Export selection…").clicked() {
        app.export_selection = true;
        app.export_options = true;
        ui.close();
    }
    if ui.button("Clear selection  (Esc)").clicked() {
        app.time_selection = None;
        ui.close();
    }
}

pub fn delete_selected_clip(app: &mut DawApp) {
    let clips = app.selected_clip_ids();
    if clips.is_empty() {
        return;
    }
    if app
        .piano_roll
        .as_ref()
        .is_some_and(|p| clips.contains(&p.clip))
    {
        app.piano_roll = None;
    }
    let deleted = app.session.delete_clips(&clips);
    app.report(deleted);
    if clips.len() > 1 {
        app.session.label_edit("Delete clips", None);
    }
    app.clear_clip_selection();
}

pub fn split_selected_clip(app: &mut DawApp) {
    // Every selected clip (audio or MIDI) under the playhead; the pieces after it stay
    // selected. A MIDI clip keeps its id for the first piece, so a piano roll open on it
    // stays open on that piece.
    let at = app.session.position();
    let mut seconds = Vec::new();
    for clip in app.selected_clip_ids() {
        if let Ok(Some(second)) = app.session.split_clip(clip, at) {
            seconds.push(second);
        }
    }
    if !seconds.is_empty() {
        app.select_clips(&seconds);
        app.session.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 20.4: a range drawn in the ruler sets the loop's (Shift: the punch range's) start and
    /// end only: off it stays off, on it stays on; the buttons' switch is the session's
    /// `set_looping` / `set_punching`.
    #[test]
    fn a_range_drawn_in_the_ruler_leaves_loop_and_punch_as_they_were() {
        let mut s = daw_core::session::Session::new(daw_core::device::AudioSettings {
            output_device: Some("<none>".into()),
            threads: Some(0),
            ..Default::default()
        });
        assert!(!s.project.loop_region.enabled);
        drawn_range(&mut s, 48_000, 96_000, false).unwrap();
        let region = s.project.loop_region;
        assert_eq!((region.start, region.end), (48_000, 96_000));
        assert!(!region.enabled, "drawn with Loop off: still off");
        s.set_looping(true).unwrap();
        drawn_range(&mut s, 0, 24_000, false).unwrap();
        let region = s.project.loop_region;
        assert_eq!((region.start, region.end), (0, 24_000));
        assert!(region.enabled, "drawn with Loop on: still on");

        drawn_range(&mut s, 10_000, 20_000, true).unwrap();
        let punch = s.project.punch;
        assert_eq!((punch.start, punch.end), (10_000, 20_000));
        assert!(!punch.enabled, "a punch range drawn leaves punch off");
        assert_eq!(s.project.loop_region.start, 0, "and the loop alone");
        s.set_punching(true).unwrap();
        drawn_range(&mut s, 12_000, 30_000, true).unwrap();
        assert!(s.project.punch.enabled, "and on, on");
        assert!(
            drawn_range(&mut s, 5, 5, false).is_err(),
            "an empty range is refused"
        );
    }

    #[test]
    fn the_playhead_takes_no_press_that_could_be_a_double_clicks_second() {
        // egui's defaults: two clicks within 0.3 s and 6 points make a double-click.
        let (delay, reach) = (0.3, 6.0);
        let at = pos2(200.0, 80.0);
        let pending =
            |now: f64, pointer| double_click_pending(Some((at, 10.0)), now, pointer, delay, reach);
        // A click at 10 s put the playhead under the pointer: until 0.3 s on, a press there
        // (or a few points off, as a hand jitters) is the clip's, not the playhead's.
        assert!(pending(10.0, Some(at)));
        assert!(pending(10.15, Some(at + vec2(3.0, -2.0))));
        assert!(pending(10.299, Some(at)));
        // Then the playhead's handle is pressable again: later, or once the pointer moved off.
        assert!(!pending(10.3, Some(at)));
        assert!(!pending(11.0, Some(at)));
        assert!(!pending(10.1, Some(at + vec2(6.0, 0.0))));
        assert!(!pending(10.1, Some(at + vec2(0.0, 40.0))));
        // No pointer over the window, or no click yet: nothing pending.
        assert!(!pending(10.1, None));
        assert!(!double_click_pending(None, 10.1, Some(at), delay, reach));
    }

    /// 26.3: one simulated playhead drag, frame by frame as `arrange::show` runs it, with the
    /// engine taking each locate `lag` frames late: the pointer thrown far past the right
    /// edge for 40 frames, then far past the left until the view is back at the start. Each
    /// frame's view start, the drag's target and the engine's position.
    fn simulated_drag(lag: usize, on: bool) -> Vec<(f64, u64, u64)> {
        // 120 at 48 kHz on a 1/8 grid, zoomed in to 8 samples a point: the lines 1500 points
        // apart, wider than the 900-point lanes.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let grid = Grid::Fixed(Division::note(8));
        let (left, right, spp, dt) = (250.0, 1150.0, 8.0, 1.0 / 60.0);
        let mut view = 24_000.0;
        let mut engine = 24_000 + 400 * 8;
        let mut sent: Vec<u64> = Vec::new();
        let mut drag = PlayheadDrag {
            target: engine,
            pointer_x: 650.0,
        };
        let mut frames = Vec::new();
        for frame in 0..400 {
            let pointer_x = if frame < 40 {
                right + 600.0
            } else {
                left - 600.0
            };
            let axis = TimeAxis {
                left,
                start: view,
                spp,
            };
            // The drag's own target: `show` sends a locate only when it changes, and never
            // reads the engine's position back.
            let (start, at) = drag_step(&map, grid, on, axis, right, pointer_x, dt);
            view = start;
            drag.target = at;
            sent.push(drag.target);
            // The audio thread takes the locates `lag` frames late.
            if sent.len() > lag {
                engine = sent[sent.len() - 1 - lag];
            }
            frames.push((view, drag.target, engine));
            if frame >= 40 && view == 0.0 && drag.target == 0 && frames.len() > 60 {
                break;
            }
        }
        frames
    }

    #[test]
    fn a_fast_playhead_drag_keeps_the_playhead_on_screen_however_late_the_engine_follows() {
        let (left, right, spp) = (250.0_f32, 1150.0_f32, 8.0);
        let x = |view: f64, sample: u64| left + ((sample as f64 - view) / spp) as f32;
        for on in [true, false] {
            let prompt = simulated_drag(0, on);
            for lag in [0, 1, 3, 8] {
                let frames = simulated_drag(lag, on);
                // The drag never reads the engine: the same view and playhead every frame
                // however late the engine takes them.
                let drawn: Vec<_> = frames.iter().map(|f| (f.0, f.1)).collect();
                let want: Vec<_> = prompt.iter().map(|f| (f.0, f.1)).collect();
                assert_eq!(drawn, want, "snap {on}, {lag} frames late");
                let mut last = 24_000.0;
                for (n, (view, target, _)) in frames.iter().enumerate() {
                    let at = x(*view, *target);
                    assert!(
                        (left..=right).contains(&at),
                        "snap {on}, lag {lag}, frame {n}: the playhead at x = {at}"
                    );
                    // Past the right edge the view scrolls right every frame; past the left,
                    // back every frame until it is at the start.
                    if n < 40 {
                        assert!(*view > last, "frame {n}: the view at {view}, was {last}");
                    } else {
                        assert!(
                            *view < last || *view == 0.0,
                            "frame {n}: {view}, was {last}"
                        );
                    }
                    last = *view;
                }
                let end = frames.last().unwrap();
                assert_eq!((end.0, end.1), (0.0, 0), "back at the very start");
            }
        }
        // Before 26.3 the handle sat at the engine's position and was dragged only while that
        // was on screen, and the target was the grid line nearest the point inside the edge:
        // with these lines 1500 points apart that line was often past the edge, so once the
        // engine took it the drag stopped. The frames where it would have are there.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let grid = Grid::Fixed(Division::note(8));
        let past = simulated_drag(0, true)
            .iter()
            .take(40)
            .filter(|(view, _, _)| {
                let axis = TimeAxis {
                    left,
                    start: *view,
                    spp,
                };
                let plain = axis.sample_under(right - 1.0);
                let nearest = map.snap_grid(plain, grid, &grid_x(&map, spp), Rounding::Nearest);
                axis.x(nearest as f64) > right
            })
            .count();
        assert!(
            past > 0,
            "the case 26.3 fixed: the nearest line past the edge"
        );
    }

    #[test]
    fn a_dragged_playhead_snaps_only_to_lines_in_the_lanes() {
        // 120 at 48 kHz: a sixteenth is 6000 samples; 1/16 at 300 samples a point is 20
        // points.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let sixteenth = Grid::Fixed(Division::note(16));
        let right = 1000.0;
        let axis = |start: f64, spp: f64| TimeAxis {
            left: 200.0,
            start,
            spp,
        };
        // In the lanes: the nearest line, as before.
        let a = axis(0.0, 300.0);
        assert_eq!(drag_target(&map, sixteenth, true, a, right, 509.0), 90_000);
        // Past the right edge the point is 999 (sample 239,700); its nearest line, 240,000,
        // is at 1000.0, still in.
        assert_eq!(
            drag_target(&map, sixteenth, true, a, right, 1500.0),
            240_000
        );
        // Scrolled on to 4300: the point is 244,000, its nearest line 246,000 past the edge
        // (1005.7); the line inside, 240,000, is 13 points in.
        let b = axis(4_300.0, 300.0);
        assert_eq!(
            drag_target(&map, sixteenth, true, b, right, 1500.0),
            240_000
        );
        // Past the left edge (the point 60,800): the nearest line, 60,000, is past it (198.3);
        // the line inside, 66,000, 17 points in.
        let c = axis(60_500.0, 300.0);
        assert_eq!(drag_target(&map, sixteenth, true, c, right, -400.0), 66_000);
        // Zoomed in past the grid (a 1/16 is 1500 points at 4 samples a point): no line near,
        // so the edge itself.
        let d = axis(30_000.0, 4.0);
        let at = drag_target(&map, sixteenth, true, d, right, 1500.0);
        assert_eq!(at, d.sample_under(right - 1.0), "the edge, unsnapped");
        // Snap off: the sample under the point inside the edge.
        assert_eq!(
            drag_target(&map, sixteenth, false, a, right, 1500.0),
            a.sample_under(999.0)
        );
        // Scrolled to the start and still past it: the very start.
        assert_eq!(drag_target(&map, sixteenth, true, a, right, 100.0), 0);
        assert_eq!(drag_target(&map, sixteenth, false, a, right, 100.0), 0);
    }

    #[test]
    fn the_ruler_and_the_playhead_go_to_the_grid_with_snap_on_and_to_the_sample_off() {
        // 120 at 48 kHz: a sixteenth is 6000 samples; 100 samples a point.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let axis = TimeAxis {
            left: 300.0,
            start: 0.0,
            spp: 100.0,
        };
        // A click (or the playhead's drag) at 76 points: sample 7600.
        let under = axis.sample(376.0);
        assert_eq!(under, 7_600.0);
        for (n, want) in [(4, 0), (8, 12_000), (16, 6_000), (32, 9_000), (64, 7_500)] {
            let grid = Grid::Fixed(Division::note(n));
            assert_eq!(
                snap_to(&map, grid, 100.0, true, under),
                want as f64,
                "1/{n}"
            );
            // Snap off: the sample under the pointer, whatever the grid.
            assert_eq!(snap_to(&map, grid, 100.0, false, 7_637.0), 7_637.0);
        }
        assert_eq!(snap_to(&map, Grid::default(), 100.0, true, -50.0), 0.0);
        // Auto follows the zoom: a sixteenth 20 points wide at 300 samples a point, a
        // sixty-fourth 20 at 75.
        assert_eq!(snap_to(&map, Grid::Auto, 300.0, true, under), 6_000.0);
        assert_eq!(snap_to(&map, Grid::Auto, 75.0, true, under), 7_500.0);
        // Across a meter change (6/8 from bar 2): eighths from its bar line.
        let mut project = daw_core::project::Project::default();
        project.tempo_changes.push(TempoChange {
            bar: 2,
            beat: 0,
            tempo: None,
            time_signature: Some((6, 8)),
        });
        let map = TempoMap::of(&project, 48_000);
        let bar2 = map.bar_start(2);
        let eighth = Grid::Fixed(Division::note(8));
        assert_eq!(
            snap_to(&map, eighth, 100.0, true, bar2 + 13_000.0),
            bar2 + 12_000.0
        );
    }

    #[test]
    fn a_dragged_clips_start_or_end_goes_onto_the_playhead_within_reach() {
        // 120 at 48 kHz: an eighth (the default grid) is 12_000 samples. The playhead sits
        // off the grid; at 100 samples a point the reach (8 points) is 800 samples.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let grid = Grid::default();
        let playhead = 50_037.0;
        let audio = Span::Samples(30_000.0);
        let clip = |spp, on, start, span, ctrl| {
            snap_clip_to(&map, grid, spp, on, playhead, start, span, ctrl)
        };
        for on in [true, false] {
            // The start 7 points after it, or 7.9 before: onto it, Snap on or off.
            assert_eq!(
                clip(100.0, on, playhead + 700.0, audio, false),
                (playhead, true)
            );
            assert_eq!(
                clip(100.0, on, playhead - 790.0, audio, false),
                (playhead, true)
            );
            // The end 6 points past it: the start that puts the end there, to the sample.
            let start = playhead - 30_000.0 + 600.0;
            assert_eq!(
                clip(100.0, on, start, audio, false),
                (playhead - 30_000.0, true)
            );
            // Just past the reach (9 points): the grid with Snap on, the sample with it off.
            let free = playhead + 900.0;
            let want = if on { 48_000.0 } else { free };
            assert_eq!(clip(100.0, on, free, audio, false), (want, false));
            // Ctrl held: no magnet even within reach, the grid or the sample as without it.
            let near = playhead + 300.0;
            let want = if on { 48_000.0 } else { near };
            assert_eq!(clip(100.0, on, near, audio, true), (want, false));
        }
        // The reach is in points: 7_000 samples away holds at 1000 samples a point (7
        // points) and not at 100 (70 points); 9_000 away doesn't at 1000.
        let far = playhead + 7_000.0;
        assert_eq!(clip(1_000.0, false, far, audio, false), (playhead, true));
        assert_eq!(clip(100.0, false, far, audio, false), (far, false));
        let farther = playhead + 9_000.0;
        assert_eq!(
            clip(1_000.0, false, farther, audio, false),
            (farther, false)
        );
        // A short clip with both edges in reach: the nearer one wins.
        let short = Span::Samples(1_000.0);
        let near_start = playhead - 300.0;
        assert_eq!(
            clip(100.0, false, near_start, short, false),
            (playhead, true)
        );
        let near_end = playhead - 900.0;
        assert_eq!(
            clip(100.0, false, near_end, short, false),
            (playhead - 1_000.0, true)
        );
        // An end that could only reach the playhead with the clip starting before 0: no
        // magnet, the clip stays at 0 (it was dragged as far as it goes).
        let early = 5_000.0;
        let long = Span::Samples(5_300.0);
        for on in [true, false] {
            let got = snap_clip_to(&map, grid, 100.0, on, early, -200.0, long, false);
            assert_eq!(got, (0.0, false), "Snap {on}");
        }
        // Its start still goes onto a playhead within reach of it.
        let got = snap_clip_to(&map, grid, 100.0, false, 300.0, 0.0, long, false);
        assert_eq!(got, (300.0, true));
        // A trimmed edge is a clip of no length: onto the playhead within reach.
        let edge = Span::Samples(0.0);
        assert_eq!(
            clip(100.0, true, playhead - 500.0, edge, false),
            (playhead, true)
        );
        assert_eq!(
            clip(100.0, true, playhead - 500.0, edge, true),
            (48_000.0, false)
        );
    }

    #[test]
    fn a_midi_clips_end_goes_onto_the_playhead_through_the_tempo_map() {
        // 120 to bar 2, then 90: a bar-long MIDI clip (3_840 ticks) ending across the change.
        let mut project = daw_core::project::Project::default();
        project.tempo_changes.push(TempoChange {
            bar: 2,
            beat: 0,
            tempo: Some(90.0),
            time_signature: None,
        });
        let map = TempoMap::of(&project, 48_000);
        let bar = Span::Ticks(3_840.0);
        let playhead = map.bar_start(2) + 20_013.0;
        // Dragged so its end (by the map from its start) is 5 points past the playhead.
        let start = map.sample_at_tick(map.tick_at(playhead + 500.0) - 3_840.0);
        let (at, held) = snap_clip_to(
            &map,
            Grid::default(),
            100.0,
            true,
            playhead,
            start,
            bar,
            false,
        );
        assert!(held);
        // Its end, a bar of ticks on from there, is the playhead.
        let end = map.sample_at_tick(map.tick_at(at) + 3_840.0);
        assert!((end - playhead).abs() < 1e-6, "{end} vs {playhead}");
        // Its start onto it as an audio clip's.
        let (at, held) = snap_clip_to(
            &map,
            Grid::default(),
            100.0,
            true,
            playhead,
            playhead + 600.0,
            bar,
            false,
        );
        assert_eq!((at, held), (playhead, true));
    }

    #[test]
    fn the_piano_roll_and_the_waveform_view_show_the_multitracks_division() {
        // One setting: after a change every view reads it.
        let mut project = daw_core::project::Project::default();
        let rate = project.sample_rate;
        let map = TempoMap::of(&project, rate);
        let views = |project: &daw_core::project::Project, ticks_per_px: f64, spp: f64| {
            let grid = project.grid;
            [
                grid_division_at(&map, grid, 0.0, spp),
                crate::piano_roll::shown_division(&map, grid, 0, ticks_per_px),
                crate::waveform::bars_division(project, rate, 0.0, spp),
            ]
        };
        assert_eq!(views(&project, 8.0, 400.0), [Division::note(8); 3]);
        project.grid = Grid::Fixed(Division::note(16));
        assert_eq!(views(&project, 8.0, 400.0), [Division::note(16); 3]);
        // Auto: each view by its own zoom, the same at the same zoom. At 120 and 48 kHz a
        // tick is 25 samples, so 400 samples a point is 16 ticks a point: a sixteenth is 15
        // points in all three.
        project.grid = Grid::Auto;
        assert_eq!(views(&project, 16.0, 400.0), [Division::note(16); 3]);
        assert_eq!(views(&project, 4.0, 100.0), [Division::note(64); 3]);
    }

    #[test]
    fn grid_lines_in_the_multitrack_fit_the_view_and_thin_out() {
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let rect = Rect::from_min_max(pos2(300.0, 0.0), pos2(1300.0, 50.0));
        let grid = Grid::Fixed(Division::note(16));
        // A sixteenth 12 points wide: every one drawn, three between each pair of beats.
        let axis = TimeAxis {
            left: 300.0,
            start: 0.0,
            spp: 500.0,
        };
        let lines = grid_xs(&map, grid, axis, rect);
        assert!(lines.iter().all(|(x, _)| rect.x_range().contains(*x)));
        let count = |kind| lines.iter().filter(|l| l.1 == kind).count();
        assert_eq!(count(LineKind::Bar), 6);
        assert_eq!(
            count(LineKind::Division),
            3 * (count(LineKind::Beat) + count(LineKind::Bar))
        );
        // Zoomed far out: bars only, at least a few points apart.
        let far = TimeAxis {
            spp: 48_000.0,
            ..axis
        };
        let lines = grid_xs(&map, grid, far, rect);
        assert!(lines.iter().all(|l| l.1 == LineKind::Bar));
        assert!(lines.windows(2).all(|w| w[1].0 - w[0].0 >= 6.0));
    }

    #[test]
    fn a_names_telling_ending_is_found() {
        assert_eq!(ending("Acoustic Piano (bass)"), Some("(bass)"));
        assert_eq!(ending("Gtr 12"), Some(" 12"));
        assert_eq!(ending("Acoustic Piano"), None);
        assert_eq!(ending("(bass)"), None, "nothing in front to shorten");
        assert_eq!(ending("Take 2b"), None);
    }

    #[test]
    fn long_names_keep_their_ending() {
        let ctx = egui::Context::default();
        let mut shown = Vec::new();
        let mut output = ctx.run_ui(Default::default(), |ui| {
            shown.clear();
            for name in ["Acoustic Piano (bass)", "Rhythm Guitar 2", "Acoustic Piano"] {
                shown.push(keep_ending(
                    ui,
                    name,
                    90.0,
                    egui::FontId::proportional(14.0),
                ));
            }
            let font = egui::FontId::proportional(14.0);
            shown.push(keep_ending(ui, "Acoustic Piano (bass)", 1000.0, font));
        });
        output.textures_delta.clear();
        assert!(
            shown[0].starts_with("Acou") && shown[0].ends_with("…(bass)"),
            "{}",
            shown[0]
        );
        assert!(shown[1].ends_with("… 2"), "{}", shown[1]);
        assert_eq!(shown[2], "Acoustic Piano", "cut at the end by the label");
        assert_eq!(shown[3], "Acoustic Piano (bass)", "fits");
    }

    /// 25.2: a click on a clip puts the playhead where a click on the empty lane would, through
    /// the one helper both call (`locate_click`): the sample under the pointer with Snap off,
    /// the nearest grid line with it on. The helper doesn't care what was clicked; that a clip's
    /// click reaches it (and a drag, an edge or a fade handle doesn't) is the driven
    /// `--show clip-playhead`'s to prove.
    #[test]
    fn a_click_on_a_clip_places_the_playhead_snapped_or_not() {
        // 120 BPM at 48 kHz: a sixteenth is 6000 samples. 100 samples a point, the lanes from
        // x = 300, the view from sample 96,000 (bar 2).
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let grid = Grid::Fixed(Division::note(16));
        let axis = TimeAxis {
            left: 300.0,
            start: 96_000.0,
            spp: 100.0,
        };
        // A clip from 100,000 to 160,000 is drawn from x = 340 to 940; clicked at x = 512.25,
        // over sample 117,225.
        let x = 512.25;
        assert_eq!(clicked_sample(&map, grid, false, axis, x), 117_225);
        // Snap on: the sixteenth nearest it, 120,000 (117,225 is 2,775 short; 114,000 3,225
        // back).
        assert_eq!(clicked_sample(&map, grid, true, axis, x), 120_000);
        assert_eq!(clicked_sample(&map, grid, true, axis, 340.0), 102_000);
        // As the lane's click always placed it.
        for on in [false, true] {
            assert_eq!(
                clicked_sample(&map, grid, on, axis, x),
                snap_to(&map, grid, axis.spp, on, axis.sample(x)) as u64
            );
        }
        // Zoomed right in, Snap off: the sample under the pointer, not the next one.
        let close = TimeAxis {
            left: 300.0,
            start: 100_000.0,
            spp: 1.0 / 32.0,
        };
        assert_eq!(
            clicked_sample(&map, grid, false, close, 300.0 + 31.0),
            100_000
        );
        assert_eq!(
            clicked_sample(&map, grid, false, close, 300.0 + 33.0),
            100_001
        );
        // Left of the timeline's start: 0, snapped or not.
        let start = TimeAxis { start: 0.0, ..axis };
        assert_eq!(clicked_sample(&map, grid, false, start, 250.0), 0);
        assert_eq!(clicked_sample(&map, grid, true, start, 250.0), 0);
    }

    #[test]
    fn the_sample_under_the_pointer_at_any_zoom() {
        // 32 points per sample: sample 1000 spans x 0..32, 1001 from 32.
        let close = TimeAxis {
            left: 0.0,
            start: 1000.0,
            spp: 1.0 / 32.0,
        };
        assert_eq!(close.sample_under(0.0), 1000);
        assert_eq!(close.sample_under(31.9), 1000);
        assert_eq!(close.sample_under(32.0), 1001);
        // One sample per point, and 480 per point.
        let even = TimeAxis {
            left: 10.0,
            start: 0.0,
            spp: 1.0,
        };
        assert_eq!(even.sample_under(10.0 + 123.5), 123);
        let far = TimeAxis {
            left: 0.0,
            start: 48_000.0,
            spp: 480.0,
        };
        assert_eq!(far.sample_under(2.5), 49_200);
        // Left of the timeline's start: sample 0, never negative.
        assert_eq!(even.sample_under(0.0), 0);
    }

    /// A lane 0..1000 points wide, 100 high, and clips on it by their x ranges, drawn in order.
    fn lane_of(clips: &[(ClipId, f32, f32, bool)]) -> (Rect, Vec<EdgeSpot>) {
        let lane = Rect::from_min_max(pos2(0.0, 0.0), pos2(1000.0, 100.0));
        let rows: Vec<(ClipId, Rect, bool)> = clips
            .iter()
            .map(|&(id, x0, x1, selected)| {
                (
                    id,
                    Rect::from_min_max(pos2(x0, 2.0), pos2(x1, 97.0)),
                    selected,
                )
            })
            .collect();
        (lane, edge_spots(&rows, lane))
    }

    /// The edge `edge_under` picks at `p`: its clip and whether it's the left one.
    fn picked(spots: &[EdgeSpot], x: f32, y: f32) -> Option<(ClipId, bool)> {
        edge_under(spots, pos2(x, y)).map(|i| (spots[i].clip, spots[i].left))
    }

    #[test]
    fn side_by_side_the_selected_clips_edge_wins() {
        // 25.1: A ends where B starts. The user's report: with A selected, grabbing A's right
        // edge took B's left one.
        for (a_selected, b_selected) in [(true, false), (false, true), (false, false)] {
            let (_, spots) =
                lane_of(&[(1, 100.0, 300.0, a_selected), (2, 300.0, 500.0, b_selected)]);
            for x in [297.0, 300.0, 303.0] {
                let want = if a_selected { (1, false) } else { (2, true) };
                // Neither selected: equally near, the topmost (drawn last) wins.
                assert_eq!(
                    picked(&spots, x, 80.0),
                    Some(want),
                    "A {a_selected} B {b_selected} at {x}"
                );
            }
            assert!(
                spots.iter().all(|s| !s.hidden),
                "touching clips hide no edge"
            );
        }
        // A gap of three points, neither selected: the nearer edge.
        let (_, spots) = lane_of(&[(1, 100.0, 300.0, false), (2, 303.0, 500.0, false)]);
        assert_eq!(picked(&spots, 301.0, 80.0), Some((1, false)));
        assert_eq!(picked(&spots, 302.0, 80.0), Some((2, true)));
        // Selected beats nearer: A selected, the pointer nearer B's edge.
        let (_, spots) = lane_of(&[(1, 100.0, 300.0, true), (2, 303.0, 500.0, false)]);
        assert_eq!(picked(&spots, 302.5, 80.0), Some((1, false)));
    }

    #[test]
    fn an_edge_under_another_clip_can_be_grabbed_and_beats_the_body_over_it() {
        // 25.1, the user's screenshot: A (selected) runs on under B, drawn after it.
        let (lane, spots) = lane_of(&[(1, 100.0, 400.0, true), (2, 300.0, 600.0, false)]);
        let a_end = spots.iter().find(|s| s.clip == 1 && !s.left).unwrap();
        assert!(a_end.hidden, "A's end is under B");
        let b_start = spots.iter().find(|s| s.clip == 2 && s.left).unwrap();
        assert!(!b_start.hidden, "B's start is over A, not under it");
        // On A's end, inside B's body: A's end, whichever clip is selected.
        assert_eq!(picked(&spots, 402.0, 80.0), Some((1, false)));
        let (_, other) = lane_of(&[(1, 100.0, 400.0, false), (2, 300.0, 600.0, true)]);
        assert_eq!(picked(&other, 398.0, 80.0), Some((1, false)));
        // Out of reach of every edge: no edge, so the body under the pointer (B) takes it.
        assert_eq!(picked(&spots, 450.0, 80.0), None);
        assert_eq!(picked(&spots, 405.0, 80.0), None, "4 points is the reach");
        assert_eq!(picked(&spots, 404.0, 80.0), Some((1, false)));
        // Up in the title bar, where the fade handles are: no edge.
        assert_eq!(picked(&spots, 400.0, 10.0), None);
        // Nothing near: nothing.
        assert_eq!(picked(&spots, 900.0, 80.0), None);
        // An edge off the lane has nothing to grab: a clip from before the view.
        let (_, spots) = lane_of(&[(3, -50.0, 200.0, true)]);
        assert_eq!(spots.len(), 1);
        assert_eq!(picked(&spots, lane.left() + 1.0, 80.0), None);
        assert_eq!(picked(&spots, 200.0, 80.0), Some((3, false)));
    }

    #[test]
    fn a_press_on_an_edge_changes_nothing_until_the_pointer_moves_and_then_snaps() {
        // 25.1: Snap on, an edge off the grid. The press point 150 samples inside the end.
        let mut s = daw_core::session::Session::new(daw_core::device::AudioSettings {
            output_device: Some("<none>".into()),
            threads: Some(0),
            ..Default::default()
        });
        let track = s.add_track(TrackKind::Audio, None);
        let id = s.project.next_id();
        // Its file isn't loaded, so it can only be shortened: enough here.
        let clip = Clip::new(id, "di".into(), "di.wav".into(), 10_037, 300, 100_011);
        s.project.track_mut(track).unwrap().clips.push(clip.clone());
        let map = s.tempo_map();
        let grid = Grid::default();
        // Far from the edge: no magnet.
        let place =
            |at: f64| snap_clip_to(&map, grid, 100.0, true, 0.0, at, Span::Samples(0.0), false);
        let press = clip.end() as f64 - 150.0;
        let mut drag = EdgeDrag::new(&clip, false, false, press);
        let steps = s.history().0.len();
        for _ in 0..3 {
            // Pressed and held: the press frame and the frames after it.
            s.begin_edit("", None);
            assert!(!drag_edge(&mut s, &mut drag, press, place).unwrap());
            assert!(!s.end_edit(), "a press is no edit");
        }
        assert_eq!(
            s.project.find_clip(id).unwrap().1,
            &clip,
            "the clip as it was"
        );
        assert_eq!(s.history().0.len(), steps, "no undo step");
        // The first move, a sample in: the edge onto the grid line nearest where it went.
        s.begin_edit("", None);
        drag_edge(&mut s, &mut drag, press - 1.0, place).unwrap();
        assert!(s.end_edit());
        let want = snap_to(&map, grid, 100.0, true, clip.end() as f64 - 1.0) as u64;
        assert_ne!(want, clip.end());
        let now = s.project.find_clip(id).unwrap().1.clone();
        assert_eq!((now.start, now.end()), (clip.start, want));
        assert_eq!(s.history().0.first().map(String::as_str), Some("Trim clip"));
        // Back on the press point it still follows, snapped: no jump back to the old end.
        s.begin_edit("", None);
        drag_edge(&mut s, &mut drag, press, place).unwrap();
        s.end_edit();
        assert_eq!(s.project.find_clip(id).unwrap().1.end(), want);
        // Snap off: the edge keeps the press point's distance, to the sample.
        let free =
            |at: f64| snap_clip_to(&map, grid, 100.0, false, 0.0, at, Span::Samples(0.0), false);
        let mut drag = EdgeDrag::new(&now, true, false, now.start as f64 + 120.0);
        s.begin_edit("", None);
        drag_edge(&mut s, &mut drag, now.start as f64 + 120.0 + 2_345.0, free).unwrap();
        s.end_edit();
        assert_eq!(s.project.find_clip(id).unwrap().1.start, now.start + 2_345);
    }

    #[test]
    fn a_fade_handle_follows_the_pointer_from_where_it_was_grabbed() {
        // 25.1: grabbed 3 samples off the handle's place, a press keeps the fade exactly.
        assert_eq!(dragged_fade(4_800, 10_003.0, 10_003.0, true, 48_000), 4_800);
        assert_eq!(dragged_fade(4_800, 10_003.0, 10_103.0, true, 48_000), 4_900);
        // A fade-out grows to the left.
        assert_eq!(
            dragged_fade(4_800, 10_003.0, 10_103.0, false, 48_000),
            4_700
        );
        // Within the clip.
        assert_eq!(dragged_fade(100, 500.0, 0.0, true, 48_000), 0);
        assert_eq!(dragged_fade(47_000, 0.0, 5_000.0, true, 48_000), 48_000);
    }
}
