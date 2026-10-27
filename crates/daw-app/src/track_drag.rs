//! Tracks reorder by dragging (dev plan 15.5): a track header (its colour stripe, its name or
//! its background) in the arrangement, or a strip's name in the mixer. The track (with its
//! sub-tracks) moves in the project as the pointer crosses the others
//! (`Project::move_track_to`, as the `move_track_to` action), so they make room and the drop
//! looks as it will; the graph is rebuilt on letting go, and Escape puts the old order back.
//!
//! - **Arrangement:** over a track's upper half it goes above it, over the lower half below
//!   it, both beside it (in its folder, or at the top level); over a folder's lower half it
//!   goes into it, first. Above the first track it goes to the top, below the last to the
//!   end of the top level, which is how it leaves a folder at the bottom.
//! - **Mixer:** over a strip's left half before it, the right half after it; past the last
//!   strip, the end.
//!
//! The place is worked out once every row (or strip) of the frame has been drawn
//! ([`apply`]), when all their positions are known.

use crate::app::DawApp;
use crate::theme;
use daw_core::project::{Place, TrackId, TrackKind};
use egui::{Rect, Stroke};

/// Where a track drag began: only that surface moves it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Arrange,
    Mixer,
}

/// A track being dragged.
pub struct TrackDrag {
    pub track: TrackId,
    pub surface: Surface,
    /// Every track's place before the drag: Escape puts them back.
    original: Vec<(TrackId, Option<TrackId>)>,
    pointer: Option<egui::Pos2>,
}

/// A header's (or name's) drag on track `id`: begins, follows or ends a track drag.
pub fn respond(app: &mut DawApp, response: &egui::Response, id: TrackId, surface: Surface) {
    if response.drag_started() && app.track_drag.is_none() {
        app.track_drag = Some(TrackDrag {
            track: id,
            surface,
            original: app.session.project.order(),
            pointer: response.interact_pointer_pos(),
        });
    }
    if response.dragged() {
        if let Some(drag) = app.track_drag.as_mut().filter(|d| d.track == id) {
            drag.pointer = response.interact_pointer_pos();
            response.ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
    }
    if response.drag_stopped() && app.track_drag.as_ref().is_some_and(|d| d.track == id) {
        app.track_drag = None;
        app.session.mark_dirty();
    }
}

/// The drag's move this frame, once `rows` (each track's id, kind and rect as drawn this
/// frame, top to bottom or left to right) are all known; then the dragged track outlined.
pub fn apply(
    app: &mut DawApp,
    ui: &egui::Ui,
    surface: Surface,
    rows: &[(TrackId, TrackKind, Rect)],
) {
    let Some(drag) = app.track_drag.as_ref().filter(|d| d.surface == surface) else {
        return;
    };
    let id = drag.track;
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        if let Some(drag) = app.track_drag.take() {
            app.session.project.restore_order(&drag.original);
        }
        return;
    }
    let project = &app.session.project;
    let mine = |t: TrackId| t == id || project.is_descendant(t, id);
    let place = drag.pointer.and_then(|p| {
        place_at(
            surface,
            p,
            rows,
            &mine,
            project.tracks.first().map(|t| t.id),
        )
    });
    if let Some(place) = place {
        let before = app.session.project.order();
        if app.session.project.move_track_to(id, place).is_ok()
            && app.session.project.order() != before
        {
            app.session
                .label_edit("Move track", Some(format!("Move track:{id}")));
        }
    }
    // The dragged track (and its sub-tracks) outlined where they are drawn.
    let project = &app.session.project;
    for (t, _, rect) in rows {
        if *t == id || project.is_descendant(*t, id) {
            ui.painter().rect_stroke(
                rect.shrink(1.0),
                3,
                Stroke::new(2.0, theme::ACCENT),
                egui::StrokeKind::Inside,
            );
        }
    }
}

/// Where the pointer at `p` puts the track: see the module docs. None over the track itself
/// (or its sub-tracks): it stays where it is.
fn place_at(
    surface: Surface,
    p: egui::Pos2,
    rows: &[(TrackId, TrackKind, Rect)],
    mine: &dyn Fn(TrackId) -> bool,
    first: Option<TrackId>,
) -> Option<Place> {
    let (first_row, last_row) = (rows.first()?, rows.last()?);
    let (along, start, end) = match surface {
        Surface::Arrange => (p.y, first_row.2.top(), last_row.2.bottom()),
        Surface::Mixer => (p.x, first_row.2.left(), last_row.2.right()),
    };
    if along < start {
        return first.filter(|&f| !mine(f)).map(Place::Before);
    }
    if along >= end {
        return Some(Place::End);
    }
    let (t, kind, rect) = rows.iter().find(|(_, _, r)| match surface {
        Surface::Arrange => r.y_range().contains(along),
        Surface::Mixer => r.x_range().contains(along),
    })?;
    if mine(*t) {
        return None;
    }
    let early = match surface {
        Surface::Arrange => along < rect.center().y,
        Surface::Mixer => along < rect.center().x,
    };
    Some(match (surface, early, kind) {
        (_, true, _) => Place::Before(*t),
        (Surface::Arrange, false, TrackKind::Folder) => Place::Into {
            folder: *t,
            index: 0,
        },
        (_, false, _) => Place::After(*t),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{pos2, vec2};

    fn rows() -> Vec<(TrackId, TrackKind, Rect)> {
        let row = |id, kind, top: f32| {
            (
                id,
                kind,
                Rect::from_min_size(pos2(0.0, top), vec2(100.0, 40.0)),
            )
        };
        vec![
            row(1, TrackKind::Audio, 0.0),
            row(2, TrackKind::Folder, 40.0),
            row(3, TrackKind::Audio, 80.0),
        ]
    }

    #[test]
    fn the_pointer_picks_the_place_in_the_arrangement() {
        let at = |y: f32, mine: &dyn Fn(TrackId) -> bool| {
            place_at(Surface::Arrange, pos2(50.0, y), &rows(), mine, Some(1))
        };
        let not = |_| false;
        assert_eq!(at(-5.0, &not), Some(Place::Before(1)), "above: the top");
        assert_eq!(at(130.0, &not), Some(Place::End), "below: the end");
        assert_eq!(at(10.0, &not), Some(Place::Before(1)));
        assert_eq!(at(30.0, &not), Some(Place::After(1)));
        assert_eq!(at(50.0, &not), Some(Place::Before(2)));
        assert_eq!(
            at(70.0, &not),
            Some(Place::Into {
                folder: 2,
                index: 0
            }),
            "a folder's lower half: into it"
        );
        assert_eq!(at(90.0, &|t| t == 3), None, "over itself: no move");
        assert_eq!(at(-5.0, &|t| t == 1), None, "already the first");
    }

    #[test]
    fn the_mixer_goes_by_left_and_right_halves() {
        let strips: Vec<_> = (0..3)
            .map(|i| {
                let kind = if i == 1 {
                    TrackKind::Folder
                } else {
                    TrackKind::Audio
                };
                (
                    i + 1,
                    kind,
                    Rect::from_min_size(pos2(i as f32 * 80.0, 0.0), vec2(80.0, 300.0)),
                )
            })
            .collect();
        let at = |x: f32| {
            place_at(
                Surface::Mixer,
                pos2(x, 100.0),
                &strips,
                &|t| t == 3,
                Some(1),
            )
        };
        assert_eq!(at(10.0), Some(Place::Before(1)));
        assert_eq!(at(70.0), Some(Place::After(1)));
        assert_eq!(
            at(120.0),
            Some(Place::After(2)),
            "no into-folder in the mixer"
        );
        assert_eq!(at(200.0), None);
        assert_eq!(at(300.0), Some(Place::End));
    }
}
