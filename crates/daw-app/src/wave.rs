//! Painting a waveform at any zoom (23.2), for the Multitrack's clips and recording takes and
//! the Waveform view alike: [`daw_core::peaks::layout`] says what to draw (min/max columns, or
//! the samples themselves zoomed in past one a point), this paints it.
//!
//! Columns go into one mesh per channel (a quad each), not a shape per point, so many tracks
//! zoomed out cost little to paint; each column reaches the one before it, so steep audio
//! drawn a sample or two a point still reads as one connected shape where the line takes
//! over. Samples are a line through them, with dots once they are 4 points apart. The layout's
//! buffers are kept between frames ([`LAYOUT`]), so a frame allocates only the shapes it
//! hands egui.

use daw_core::peaks::{layout, Layout, Place, Wave};
use egui::epaint::Mesh;
use egui::{pos2, Color32, Pos2, Rect, Shape, Stroke};
use std::cell::RefCell;

/// A dot's radius on each sample once they are far apart, in points.
const DOT_RADIUS: f32 = 2.0;

thread_local! {
    /// The layout buffers every waveform on the UI thread reuses.
    static LAYOUT: RefCell<Layout> = RefCell::new(Layout::default());
}

/// How a channel stands in its lane: the zero line's y, the points from it to full scale, and
/// its colour.
#[derive(Debug, Clone, Copy)]
pub struct Look {
    pub mid: f32,
    pub half: f32,
    pub color: Color32,
}

impl Look {
    /// The y of `value` (already scaled by its gain); beyond full scale it is cut off.
    fn y(&self, value: f32) -> f32 {
        self.mid - value.clamp(-1.0, 1.0) * self.half
    }
}

/// Paint one channel: `wave`'s span at `place`, `gain(sample)` scaling each column (at its
/// first sample) and each sample (the display gain, a clip's gain and fades).
pub fn paint(
    painter: &egui::Painter,
    wave: &Wave,
    place: &Place,
    look: Look,
    gain: impl Fn(u64) -> f32,
) {
    LAYOUT.with(|cell| {
        let mut out = cell.borrow_mut();
        layout(wave, place, &mut out);
        shapes(&out, look, gain, |shape| {
            painter.add(shape);
        });
    });
}

/// The shapes for a layout: a mesh of columns, or a line through the samples and its dots.
fn shapes(out: &Layout, look: Look, gain: impl Fn(u64) -> f32, mut add: impl FnMut(Shape)) {
    if !out.columns.is_empty() {
        let mut mesh = Mesh::default();
        mesh.reserve_vertices(out.columns.len() * 4);
        mesh.reserve_triangles(out.columns.len() * 2);
        // The column before (its x and its own top and bottom), to join up with.
        let mut before: Option<(f32, f32, f32)> = None;
        for c in &out.columns {
            let g = gain(c.at);
            let top = look.y(c.hi * g);
            let bottom = look.y(c.lo * g).max(top + 1.0);
            let (mut from, mut to) = (top, bottom);
            if let Some((x, top_before, bottom_before)) = before {
                if x + 1.0 == c.x {
                    from = from.min(bottom_before);
                    to = to.max(top_before);
                }
            }
            before = Some((c.x, top, bottom));
            mesh.add_colored_rect(
                Rect::from_x_y_ranges(c.x..=c.x + 1.0, from..=to),
                look.color,
            );
        }
        add(Shape::mesh(mesh));
    }
    if !out.points.is_empty() {
        let points: Vec<Pos2> = out
            .points
            .iter()
            .map(|p| pos2(p.x, look.y(p.value * gain(p.at))))
            .collect();
        if out.dots || points.len() == 1 {
            for &p in &points {
                add(Shape::circle_filled(p, DOT_RADIUS, look.color));
            }
        }
        if points.len() > 1 {
            add(Shape::line(points, Stroke::new(1.0, look.color)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::media::AudioData;

    fn look() -> Look {
        Look {
            mid: 50.0,
            half: 40.0,
            color: Color32::WHITE,
        }
    }

    fn file() -> AudioData {
        let samples = (0..20_000).map(|i| (i as f32 * 0.05).sin() * 0.8).collect();
        AudioData::new(48_000, vec![samples])
    }

    fn drawn(audio: &AudioData, spp: f64, from: u64) -> Vec<Shape> {
        let place = Place {
            origin: 10.0 - from as f64 / spp,
            spp,
            from: 0,
            to: audio.len() as u64,
            left: 10.0,
            right: 410.0,
        };
        let mut out = Layout::default();
        layout(&audio.wave(0), &place, &mut out);
        let mut shapes_out = Vec::new();
        shapes(&out, look(), |_| 1.0, |s| shapes_out.push(s));
        shapes_out
    }

    /// The closest zoom draws the samples themselves: the line's points are the samples (both
    /// views paint through here), and each has its dot.
    #[test]
    fn the_closest_zoom_draws_each_sample_with_its_dot() {
        let audio = file();
        let shapes = drawn(&audio, 1.0 / 32.0, 5000);
        let line = shapes
            .iter()
            .find_map(|s| match s {
                Shape::Path(path) => Some(path.points.clone()),
                _ => None,
            })
            .expect("a line");
        let dots = shapes
            .iter()
            .filter(|s| matches!(s, Shape::Circle(_)))
            .count();
        assert_eq!(dots, line.len());
        for (k, p) in line.iter().enumerate() {
            let at = 5000 + k;
            assert!((p.x - (10.0 + 32.0 * k as f32)).abs() < 1e-3);
            assert!((p.y - look().y(audio.channels[0][at])).abs() < 1e-4, "{k}");
        }
    }

    /// Zoomed out, one mesh of joined columns: each reaches the one before it.
    #[test]
    fn columns_are_one_mesh_and_join_up() {
        let audio = file();
        let shapes = drawn(&audio, 3.0, 0);
        assert_eq!(shapes.len(), 1);
        let Shape::Mesh(mesh) = &shapes[0] else {
            panic!("a mesh");
        };
        assert_eq!(mesh.vertices.len(), 400 * 4);
        let spans: Vec<(f32, f32)> = mesh
            .vertices
            .chunks(4)
            .map(|q| {
                let ys = q.iter().map(|v| v.pos.y);
                (
                    ys.clone().fold(f32::MAX, f32::min),
                    ys.fold(f32::MIN, f32::max),
                )
            })
            .collect();
        for pair in spans.windows(2) {
            let ((a0, a1), (b0, b1)) = (pair[0], pair[1]);
            assert!(b0 <= a1 && b1 >= a0, "{pair:?}: a gap between columns");
        }
    }
}
