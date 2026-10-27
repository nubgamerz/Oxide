//! Timeline zoom maths, kept apart from the drawing so it can be tested.
//!
//! The view is a first visible sample (`start`) and a zoom (`spp`, samples per pixel); `x`
//! values are pixels from the left edge of the lanes.

/// One wheel notch or zoom key zooms by this factor (below 1 zooms in).
pub const STEP: f64 = 0.8;
/// The closest zoom, in both the Multitrack and the Waveform view: 32 points a sample, so
/// single samples show as dots on a line (23.2; it was four samples a point in the Multitrack).
pub const MIN_SPP: f64 = 1.0 / 32.0;
/// The widest zoom, in seconds per pixel.
const MAX_SECONDS_PER_PIXEL: f64 = 2.0;
/// Room left on either side of a fitted range, as a share of its length.
const FIT_MARGIN: f64 = 0.03;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub start: f64,
    pub spp: f64,
}

/// Edge scrolling starts this far inside either edge of the lanes, in points.
pub const EDGE_BAND: f32 = 24.0;
/// Points per second it scrolls for each point the pointer is into the band (or past it).
const EDGE_SPEED: f32 = 12.0;
/// The fastest edge scroll, in points per second.
const EDGE_MAX: f32 = 6000.0;

/// How fast a drag at `x` scrolls lanes spanning `left..right`, in points per second:
/// negative toward the start, 0 away from the edges, faster the further into the band at
/// either edge (or past it) the pointer goes, as following does for a playing playhead.
pub fn edge_scroll(x: f32, left: f32, right: f32) -> f32 {
    let into_left = left + EDGE_BAND - x;
    let into_right = x - (right - EDGE_BAND);
    if into_left > 0.0 {
        -(into_left * EDGE_SPEED).min(EDGE_MAX)
    } else if into_right > 0.0 {
        (into_right * EDGE_SPEED).min(EDGE_MAX)
    } else {
        0.0
    }
}

pub fn max_spp(rate: u32) -> f64 {
    f64::from(rate) * MAX_SECONDS_PER_PIXEL
}

/// The empty room the Multitrack keeps below its last row (double-click there to add a track),
/// in points: the rows scroll until it shows, so it is part of the content's height.
pub const ROOM_BELOW: f32 = 80.0;

/// A track's row as last drawn, for keeping the rows in place while their heights change: its
/// top and bottom on screen (its automation lanes included, which keep their height) and the
/// height of the track's own row before and after the change.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Row {
    pub top: f32,
    pub bottom: f32,
    pub old: f32,
    pub new: f32,
}

/// The Multitrack's vertical scroll offset after the track heights change (Alt+wheel, 26.1),
/// so the content under the pointer stays under it: the row under `pointer_y` (and the point
/// within it, as a share of the track's height) holds still; on the master's row or a lane,
/// which keep their height, the point itself does; below the last row the bottom stays put.
///
/// `content` is where the rows were drawn last frame, from the master's top to the last row's
/// bottom (on screen), `rows` the tracks' rows in order inside it, `view` the scroll area's
/// visible span. The rows above the pointer's count by how much they grew, so a folder's hidden
/// tracks (not drawn) count for nothing. The offset is clamped as the scroll area will clamp it
/// this frame (the content is the rows plus [`ROOM_BELOW`], at least the view), so when the
/// content is shorter than the view it rests at the top, and the frame is drawn where it stays.
pub fn offset_in_place(
    content: egui::Rangef,
    rows: &[Row],
    view: egui::Rangef,
    pointer_y: f32,
) -> f32 {
    let offset = view.min - content.min;
    // Over the ruler or the footer: the edge of the view nearest the pointer holds.
    let y = pointer_y.clamp(view.min, view.max);
    let grown: f32 = rows.iter().map(|r| r.new - r.old).sum();
    // Below the last row, every row above grew: the bottom stays put.
    let mut moved = 0.0;
    for r in rows {
        if y < r.top {
            // Above this row (the master's row): nothing above it changed.
            break;
        }
        if y < r.bottom {
            let into = y - r.top;
            moved += if into < r.old && r.old > 0.0 {
                // In the track's own row: the same share of it.
                into / r.old * (r.new - r.old)
            } else {
                // In its lanes, under the row: the whole change.
                r.new - r.old
            };
            break;
        }
        moved += r.new - r.old;
    }
    let total = content.max - content.min + grown;
    let most = (total + ROOM_BELOW - view.span()).max(0.0);
    (offset + moved).clamp(0.0, most)
}

impl View {
    /// Zoom by `factor`, keeping the sample under `x` where it is on screen.
    pub fn zoom_around(self, factor: f64, x: f64, rate: u32) -> Self {
        let anchor = self.start + x * self.spp;
        let spp = (self.spp * factor).clamp(MIN_SPP, max_spp(rate));
        Self {
            start: (anchor - x * spp).max(0.0),
            spp,
        }
    }

    /// Zoom around `sample` when it is on screen, else around the middle of the lanes.
    pub fn zoom_toward(self, factor: f64, sample: f64, width: f64, rate: u32) -> Self {
        let x = (sample - self.start) / self.spp;
        let x = if (0.0..=width).contains(&x) {
            x
        } else {
            width / 2.0
        };
        self.zoom_around(factor, x, rate)
    }

    /// Show `start..end` across `width` pixels, with a small margin on either side.
    pub fn fit(start: u64, end: u64, width: f64, rate: u32) -> Self {
        let length = end.saturating_sub(start).max(1) as f64;
        let margin = length * FIT_MARGIN;
        let spp = ((length + 2.0 * margin) / width.max(1.0)).clamp(MIN_SPP, max_spp(rate));
        // Centre the range when the closest zoom still leaves room.
        let shown = spp * width;
        let start = (start as f64 + length / 2.0 - shown / 2.0).max(0.0);
        Self { start, spp }
    }

    /// Scroll by `pixels` (positive moves the content right, showing earlier time).
    pub fn scroll(self, pixels: f64) -> Self {
        Self {
            start: (self.start - pixels * self.spp).max(0.0),
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edge_scrolling_speeds_up_with_the_distance_past_the_edge() {
        let at = |x| edge_scroll(x, 100.0, 900.0);
        assert_eq!(at(500.0), 0.0, "the middle: still");
        assert_eq!(at(100.0 + EDGE_BAND), 0.0, "the band's inner edge: still");
        assert_eq!(at(900.0 - EDGE_BAND), 0.0);
        // Into the band and past the edge: faster the further, toward that edge.
        let speeds: Vec<f32> = [110.0, 100.0, 60.0, -200.0].into_iter().map(at).collect();
        assert!(speeds.iter().all(|&s| s < 0.0), "{speeds:?}");
        assert!(speeds.windows(2).all(|w| w[1] < w[0]), "{speeds:?}");
        assert_eq!(at(100.0), -EDGE_BAND * EDGE_SPEED);
        assert_eq!(at(1000.0), (100.0 + EDGE_BAND) * EDGE_SPEED);
        assert_eq!(at(-5000.0), -EDGE_MAX, "capped");
    }

    const RATE: u32 = 48_000;

    #[test]
    fn zooming_keeps_the_sample_under_the_pointer_still() {
        let view = View {
            start: 10_000.0,
            spp: 100.0,
        };
        let x = 300.0;
        let under = view.start + x * view.spp;
        let zoomed = view.zoom_around(STEP, x, RATE);
        assert!(zoomed.spp < view.spp);
        assert!((zoomed.start + x * zoomed.spp - under).abs() < 1e-6);
        // And back out lands where it started.
        let back = zoomed.zoom_around(1.0 / STEP, x, RATE);
        assert!((back.spp - view.spp).abs() < 1e-9);
        assert!((back.start - view.start).abs() < 1e-6);
    }

    #[test]
    fn zoom_is_clamped_and_never_scrolls_before_zero() {
        let view = View {
            start: 0.0,
            spp: 5.0,
        };
        let closest = view.zoom_around(1e-6, 500.0, RATE);
        assert_eq!(closest.spp, MIN_SPP);
        let widest = view.zoom_around(1e9, 500.0, RATE);
        assert_eq!(widest.spp, max_spp(RATE));
        assert_eq!(widest.start, 0.0);
    }

    /// 23.2: zooming in step by step (the wheel, the keys, `set_view`'s `in`) goes on until
    /// single samples are dots on a line, the Waveform view's closest zoom too.
    #[test]
    fn zooming_in_reaches_single_samples_drawn_as_dots() {
        let mut view = View {
            start: 0.0,
            spp: max_spp(RATE),
        };
        let mut steps = 0;
        while view.spp > MIN_SPP {
            view = view.zoom_around(STEP, 400.0, RATE);
            steps += 1;
            assert!(steps < 200, "never reached the closest zoom");
        }
        assert_eq!(view.spp, MIN_SPP);
        const {
            assert!(
                MIN_SPP < daw_core::peaks::DOTS_BELOW,
                "the closest zoom shows dots"
            )
        };
        // At the closest zoom the waveform is the samples themselves.
        let audio =
            daw_core::media::AudioData::new(RATE, vec![(0..4000).map(|i| i as f32).collect()]);
        let place = daw_core::peaks::Place {
            origin: -1000.0 / MIN_SPP,
            spp: MIN_SPP,
            from: 0,
            to: 4000,
            left: 0.0,
            right: 640.0,
        };
        let mut out = daw_core::peaks::Layout::default();
        daw_core::peaks::layout(&audio.wave(0), &place, &mut out);
        assert!(out.dots && out.columns.is_empty());
        let values: Vec<f32> = out.points.iter().map(|p| p.value).collect();
        assert_eq!(values, (1000..=1020).map(|i| i as f32).collect::<Vec<_>>());
    }

    #[test]
    fn zoom_keys_follow_the_playhead_only_while_it_is_on_screen() {
        let view = View {
            start: 0.0,
            spp: 100.0,
        };
        // Playhead on screen at x = 200: it stays at x = 200.
        let zoomed = view.zoom_toward(STEP, 20_000.0, 1000.0, RATE);
        assert!(((20_000.0 - zoomed.start) / zoomed.spp - 200.0).abs() < 1e-6);
        // Off screen: the middle stays put instead.
        let zoomed = view.zoom_toward(STEP, 1e9, 1000.0, RATE);
        let middle = view.start + 500.0 * view.spp;
        assert!((zoomed.start + 500.0 * zoomed.spp - middle).abs() < 1e-6);
    }

    #[test]
    fn fit_shows_the_whole_range_with_a_margin() {
        let view = View::fit(48_000, 480_000, 1000.0, RATE);
        let end = view.start + 1000.0 * view.spp;
        assert!(view.start < 48_000.0 && view.start > 30_000.0);
        assert!(end > 480_000.0 && end < 500_000.0);
        // A clip shorter than the closest zoom allows is centred.
        let view = View::fit(100_000, 100_010, 1000.0, RATE);
        assert_eq!(view.spp, MIN_SPP);
        assert!((view.start + 500.0 * view.spp - 100_005.0).abs() < 1e-6);
    }

    #[test]
    fn scrolling_stops_at_zero() {
        let view = View {
            start: 1000.0,
            spp: 10.0,
        };
        assert_eq!(view.scroll(-50.0).start, 1500.0);
        assert_eq!(view.scroll(500.0).start, 0.0);
    }

    /// The master's row (and its lanes), at the content's top.
    const MASTER: f32 = 90.0;
    /// The scroll area's visible span on screen.
    const VIEW: egui::Rangef = egui::Rangef {
        min: 100.0,
        max: 700.0,
    };

    /// Where the rows land on screen at `offset`: the content's span, and each track's row
    /// (its own height `h`, `lanes` below it) as `offset_in_place` gets them.
    fn drawn(tracks: &[(f32, f32)], offset: f32) -> (egui::Rangef, Vec<(f32, f32)>) {
        let top = VIEW.min - offset;
        let mut y = top + MASTER;
        let rows = tracks
            .iter()
            .map(|&(h, lanes)| {
                let row = (y, y + h + lanes);
                y = row.1;
                row
            })
            .collect();
        (egui::Rangef::new(top, y), rows)
    }

    /// Heights `old` changed to `new` with the pointer at `y`: the new offset, and the rows as
    /// drawn before and after.
    struct Zoomed {
        offset: f32,
        before: Vec<(f32, f32)>,
        after: Vec<(f32, f32)>,
        content_before: egui::Rangef,
        content_after: egui::Rangef,
    }

    fn zoom(old: &[(f32, f32)], new: &[f32], offset: f32, y: f32) -> Zoomed {
        let (content_before, before) = drawn(old, offset);
        let rows: Vec<Row> = before
            .iter()
            .zip(old)
            .zip(new)
            .map(|((&(top, bottom), &(h, _)), &n)| Row {
                top,
                bottom,
                old: h,
                new: n,
            })
            .collect();
        let offset = offset_in_place(content_before, &rows, VIEW, y);
        let grown: Vec<(f32, f32)> = old.iter().zip(new).map(|(&(_, l), &n)| (n, l)).collect();
        let (content_after, after) = drawn(&grown, offset);
        Zoomed {
            offset,
            before,
            after,
            content_before,
            content_after,
        }
    }

    /// Every track's height times `factor`, clamped as `scale_track_heights` clamps them.
    fn scaled(old: &[(f32, f32)], factor: f32) -> Vec<f32> {
        old.iter()
            .map(|&(h, _)| (h * factor).clamp(32.0, 400.0))
            .collect()
    }

    /// The pointer at `y`, a share `f` down track `i`'s own row before, is the same share down
    /// it after.
    fn assert_held(z: &Zoomed, old: &[(f32, f32)], new: &[f32], i: usize, y: f32) {
        let f = (y - z.before[i].0) / old[i].0;
        assert!((0.0..1.0).contains(&f), "the pointer is on track {i}'s row");
        let now = z.after[i].0 + f * new[i];
        assert!(
            (now - y).abs() < 1e-3,
            "track {i}'s point at {now}, the pointer at {y} (offset {})",
            z.offset
        );
    }

    /// Ten tracks of 120 points, the second with two lanes under it.
    fn tracks() -> Vec<(f32, f32)> {
        (0..10)
            .map(|i| (120.0, if i == 1 { 144.0 } else { 0.0 }))
            .collect()
    }

    #[test]
    fn a_row_near_the_top_stays_under_the_pointer() {
        let old = tracks();
        let offset = 40.0;
        // Track 0 is drawn from 150 to 270: the pointer 30 points into it.
        let y = 180.0;
        for factor in [1.5, 1.1, 0.7] {
            let new = scaled(&old, factor);
            let z = zoom(&old, &new, offset, y);
            assert_held(&z, &old, &new, 0, y);
        }
    }

    #[test]
    fn a_row_in_the_middle_stays_under_the_pointer_growing_and_shrinking() {
        let old = tracks();
        let offset = 300.0;
        let (_, rows) = drawn(&old, offset);
        // Track 4, two thirds down its row.
        let y = rows[4].0 + 80.0;
        assert!(VIEW.contains(y));
        for factor in [1.3, 2.0, 0.8, 0.5] {
            let new = scaled(&old, factor);
            let z = zoom(&old, &new, offset, y);
            assert_held(&z, &old, &new, 4, y);
            // Growing scrolls down (more above the pointer), shrinking up.
            assert_eq!(z.offset > offset, factor > 1.0, "factor {factor}");
        }
        // Grown and shrunk back: where it started.
        let up = scaled(&old, 1.25);
        let z = zoom(&old, &up, offset, y);
        let grown: Vec<(f32, f32)> = up.iter().zip(&old).map(|(&h, &(_, l))| (h, l)).collect();
        let back = zoom(&grown, &scaled(&grown, 0.8), z.offset, y);
        assert!((back.offset - offset).abs() < 1e-3, "{}", back.offset);
    }

    #[test]
    fn below_the_last_row_the_bottom_stays_put() {
        // Four tracks of 120: content 90 + 480 points, scrolled to its end (50 down), so the
        // last row ends at 620 on screen with the room below it showing.
        let old: Vec<(f32, f32)> = vec![(120.0, 0.0); 4];
        let offset = 50.0;
        let y = 650.0;
        assert!(
            y > drawn(&old, offset).0.max,
            "the pointer is below the rows"
        );
        for factor in [1.4, 0.9] {
            let new = scaled(&old, factor);
            let z = zoom(&old, &new, offset, y);
            assert!(
                (z.content_after.max - z.content_before.max).abs() < 1e-3,
                "factor {factor}: the bottom moved from {} to {}",
                z.content_before.max,
                z.content_after.max
            );
        }
    }

    #[test]
    fn heights_clamped_at_their_limits_are_counted_as_they_are() {
        // One track already at the tallest, one at the shortest, the rest between.
        let mut old = tracks();
        old[0].0 = 400.0;
        old[2].0 = 32.0;
        let offset = 500.0;
        let (_, rows) = drawn(&old, offset);
        let y = rows[5].0 + 60.0;
        assert!(VIEW.contains(y));
        let grown = scaled(&old, 1.5);
        assert_eq!(grown[0], 400.0, "the tallest stays");
        let z = zoom(&old, &grown, offset, y);
        assert_held(&z, &old, &grown, 5, y);
        let shrunk = scaled(&old, 0.5);
        assert_eq!(shrunk[2], 32.0, "the shortest stays");
        let z = zoom(&old, &shrunk, offset, y);
        assert_held(&z, &old, &shrunk, 5, y);
    }

    #[test]
    fn content_shorter_than_the_view_rests_at_the_top() {
        // Ten rows scrolled down, shrunk to 32 points: 90 + 10 × 32 + 144 = 554 points, which
        // with the room below fits the 600-point view.
        let old = tracks();
        let offset = 300.0;
        let (_, rows) = drawn(&old, offset);
        let y = rows[6].0 + 10.0;
        let new = scaled(&old, 0.1);
        let z = zoom(&old, &new, offset, y);
        assert_eq!(z.offset, 0.0);
    }

    #[test]
    fn shrinking_near_the_end_stops_at_the_end() {
        // Scrolled to the very end (the room below the last row showing), the pointer on a row
        // in view: shrunk, the content's end can't come up past the view's bottom.
        let old = tracks();
        let total = MASTER + 10.0 * 120.0 + 144.0;
        let most = total + ROOM_BELOW - VIEW.span();
        let (_, rows) = drawn(&old, most);
        let y = rows[6].0 + 10.0;
        assert!(VIEW.contains(y));
        let new = scaled(&old, 0.8);
        let z = zoom(&old, &new, most, y);
        let new_most = total - 10.0 * 24.0 + ROOM_BELOW - VIEW.span();
        assert!(
            (z.offset - new_most).abs() < 1e-3,
            "{} vs {new_most}",
            z.offset
        );
    }

    #[test]
    fn the_master_row_a_lane_and_the_ruler_hold_their_point() {
        let old = tracks();
        let new = scaled(&old, 1.5);
        // At the top: the pointer on the master's row, nothing above it grows.
        let z = zoom(&old, &new, 0.0, VIEW.min + 40.0);
        assert_eq!(z.offset, 0.0);
        // On track 1's lanes (under its 120-point row): the lane's point stays where it is.
        let offset = 100.0;
        let (_, rows) = drawn(&old, offset);
        let y = rows[1].0 + 120.0 + 50.0;
        let z = zoom(&old, &new, offset, y);
        let lane_top = z.after[1].0 + new[1];
        assert!((lane_top + 50.0 - y).abs() < 1e-3, "{lane_top}");
        // Over the ruler (above the view): the view's top holds.
        let (_, rows) = drawn(&old, 200.0);
        let z = zoom(&old, &new, 200.0, VIEW.min - 30.0);
        let f = (VIEW.min - rows[0].0) / 120.0;
        assert!((z.after[0].0 + f * new[0] - VIEW.min).abs() < 1e-3);
    }
}
