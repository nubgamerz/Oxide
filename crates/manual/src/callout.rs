//! Where a figure's numbered badges go (27.2): beside their highlight, never over it.
//!
//! A highlight's badge (its label, "1", "2", ...) sits outside the outline so it hides nothing
//! the outline points at: to its left and centred on it when there is room in the figure; else
//! above or below it; else to its right. It mustn't cover another highlight or another badge
//! either. When no side has room inside the picture (a thin strip, a part at the picture's
//! edge), the figure gets a margin round the picture on the side the badge takes
//! ([`Layout::margin`]), and the badge stands in it.
//!
//! Badges are sized in CSS pixels (`manual.css`, `.ox-callout`) while the picture scales with
//! the page, so the layout is worked out for the picture as shown in the full-width column
//! ([`COLUMN`], scaled down when wider). The HTML places each badge against its highlight's
//! edge (`ox-at-left` and the others), so it stays clear of its own outline at any width.

/// The page's column, CSS pixels (`.ox-page`'s `max-width`).
pub const COLUMN: f64 = 760.0;
/// A badge's height, CSS pixels (`.ox-callout`).
pub const BADGE_HEIGHT: f64 = 22.0;
/// Between a badge and its outline's edge, CSS pixels (`.ox-at-*` add the 2 px border).
pub const GAP: f64 = 4.0;
/// Between a badge standing in a margin and the margin's outer edge, CSS pixels.
const EDGE: f64 = 2.0;

/// Which side of its highlight a badge is on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Above,
    Below,
    Right,
}

impl Side {
    /// In order of preference.
    pub const ALL: [Side; 4] = [Side::Left, Side::Above, Side::Below, Side::Right];

    /// Its class in `manual.css`.
    pub fn class(self) -> &'static str {
        match self {
            Side::Left => "ox-at-left",
            Side::Above => "ox-at-above",
            Side::Below => "ox-at-below",
            Side::Right => "ox-at-right",
        }
    }
}

/// A figure's badges: each highlight's side (none without a label), and the margin round the
/// picture they need, CSS pixels: top, right, bottom, left.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub sides: Vec<Option<Side>>,
    pub margin: [u32; 4],
}

/// A badge's width for `label`, CSS pixels: 22 for one character, about 7.5 more for each
/// further one (12.5 px bold, 6 px padding each side).
pub fn badge_width(label: &str) -> f64 {
    (12.0 + 7.5 * label.chars().count() as f64).max(BADGE_HEIGHT)
}

/// The badges of highlights at `rects` (the picture's pixels) with `labels`, on a picture
/// `size` pixels big.
pub fn layout(size: [u32; 2], rects: &[[f64; 4]], labels: &[Option<&str>]) -> Layout {
    let mut margin = [0.0f64; 4];
    let mut sides = Vec::new();
    // A margin narrows the picture in the column, which makes badges relatively bigger: settle.
    for _ in 0..4 {
        let width = f64::from(size[0]).max(1.0);
        let scale = ((COLUMN - margin[1] - margin[3]) / width).min(1.0);
        let (placed, needed) = place(size, rects, labels, scale, margin);
        sides = placed;
        let grown = [0, 1, 2, 3].map(|i| margin[i].max(needed[i]));
        if grown == margin {
            break;
        }
        margin = grown;
    }
    Layout {
        sides,
        margin: margin.map(|m| m.ceil() as u32),
    }
}

/// One pass at `scale` (CSS pixels per picture pixel) with `margin` (CSS pixels) round the
/// picture: each badge's side, and the margin they need.
fn place(
    size: [u32; 2],
    rects: &[[f64; 4]],
    labels: &[Option<&str>],
    scale: f64,
    margin: [f64; 4],
) -> (Vec<Option<Side>>, [f64; 4]) {
    let [w, h] = size.map(f64::from);
    // The figure's box in picture pixels: the picture and its margin.
    let area = [
        -margin[3] / scale,
        -margin[0] / scale,
        w + (margin[1] + margin[3]) / scale,
        h + (margin[0] + margin[2]) / scale,
    ];
    let picture = [0.0, 0.0, w, h];
    let mut badges: Vec<[f64; 4]> = Vec::new();
    let mut sides = Vec::new();
    let mut needed = [0.0f64; 4];
    for (rect, label) in rects.iter().zip(labels) {
        let Some(label) = label else {
            sides.push(None);
            continue;
        };
        let size = [badge_width(label) / scale, BADGE_HEIGHT / scale];
        let gap = GAP / scale;
        let free = |b: &[f64; 4]| {
            !rects.iter().any(|r| meets(b, r)) && !badges.iter().any(|o| meets(b, &grow(o, gap)))
        };
        let spots: Vec<(Side, [f64; 4])> = Side::ALL
            .iter()
            .map(|&side| (side, spot(side, rect, size, gap)))
            .collect();
        // Inside the figure as it is, then anywhere clear (the figure grows a margin for it),
        // then (crowded beyond help) to the left.
        let (side, badge) = spots
            .iter()
            .find(|(_, b)| within(b, &area) && free(b))
            .or_else(|| spots.iter().find(|(_, b)| free(b)))
            .copied()
            .unwrap_or(spots[0]);
        if !within(&badge, &picture) {
            let edge = EDGE;
            needed[0] = needed[0].max(-badge[1] * scale + edge);
            needed[1] = needed[1].max((badge[0] + badge[2] - w) * scale + edge);
            needed[2] = needed[2].max((badge[1] + badge[3] - h) * scale + edge);
            needed[3] = needed[3].max(-badge[0] * scale + edge);
        }
        badges.push(badge);
        sides.push(Some(side));
    }
    (sides, needed.map(|n| n.max(0.0)))
}

/// Where a badge `size` big goes on `side` of `rect`, `gap` away.
fn spot(side: Side, rect: &[f64; 4], size: [f64; 2], gap: f64) -> [f64; 4] {
    let [x, y, w, h] = *rect;
    let [bw, bh] = size;
    let middle = y + h / 2.0 - bh / 2.0;
    match side {
        Side::Left => [x - gap - bw, middle, bw, bh],
        Side::Above => [x, y - gap - bh, bw, bh],
        Side::Below => [x, y + h + gap, bw, bh],
        Side::Right => [x + w + gap, middle, bw, bh],
    }
}

/// Whether two rects overlap (touching edges don't).
fn meets(a: &[f64; 4], b: &[f64; 4]) -> bool {
    a[0] < b[0] + b[2] && b[0] < a[0] + a[2] && a[1] < b[1] + b[3] && b[1] < a[1] + a[3]
}

fn within(a: &[f64; 4], area: &[f64; 4]) -> bool {
    const SLACK: f64 = 1e-6;
    a[0] >= area[0] - SLACK
        && a[1] >= area[1] - SLACK
        && a[0] + a[2] <= area[0] + area[2] + SLACK
        && a[1] + a[3] <= area[1] + area[3] + SLACK
}

fn grow(r: &[f64; 4], by: f64) -> [f64; 4] {
    [r[0] - by, r[1] - by, r[2] + 2.0 * by, r[3] + 2.0 * by]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_badge_goes_left_of_its_highlight_when_there_is_room() {
        let laid = layout([600, 400], &[[100.0, 100.0, 200.0, 40.0]], &[Some("1")]);
        assert_eq!(laid.sides, vec![Some(Side::Left)]);
        assert_eq!(laid.margin, [0, 0, 0, 0]);
    }

    #[test]
    fn at_the_edge_it_goes_above_then_below_then_right() {
        // Flush left: above.
        let above = layout([600, 400], &[[2.0, 100.0, 200.0, 40.0]], &[Some("1")]);
        assert_eq!(above.sides, vec![Some(Side::Above)]);
        // Flush left and at the top: below.
        let below = layout([600, 400], &[[2.0, 2.0, 200.0, 40.0]], &[Some("1")]);
        assert_eq!(below.sides, vec![Some(Side::Below)]);
        // Flush left, as tall as the picture: right.
        let right = layout([600, 40], &[[2.0, 2.0, 200.0, 36.0]], &[Some("1")]);
        assert_eq!(right.sides, vec![Some(Side::Right)]);
        assert_eq!(right.margin, [0, 0, 0, 0]);
    }

    #[test]
    fn badges_keep_off_other_highlights_and_badges() {
        // Two rows one under the other, flush left: the first's badge can't go below (the
        // second row), the second's not above (the first's badge is there).
        let rects = [[2.0, 50.0, 300.0, 30.0], [2.0, 84.0, 300.0, 30.0]];
        let laid = layout([600, 400], &rects, &[Some("1"), Some("2")]);
        assert_eq!(laid.sides, vec![Some(Side::Above), Some(Side::Below)]);
        // Side by side on one line: the second's left would cover the first.
        let rects = [[30.0, 100.0, 100.0, 30.0], [134.0, 100.0, 100.0, 30.0]];
        let laid = layout([600, 400], &rects, &[Some("1"), Some("2")]);
        assert_eq!(laid.sides, vec![Some(Side::Left), Some(Side::Above)]);
    }

    #[test]
    fn a_thin_strip_gets_a_margin_for_its_badges() {
        // The status bar's crop (614 x 27): no side has room inside the picture.
        let rects = [
            [6.0, 6.0, 318.0, 15.0],
            [337.0, 6.0, 165.0, 15.0],
            [515.0, 6.0, 93.0, 15.0],
        ];
        let laid = layout([614, 27], &rects, &[Some("1"), Some("2"), Some("3")]);
        assert_eq!(
            laid.sides,
            vec![Some(Side::Left), Some(Side::Above), Some(Side::Above)]
        );
        // Left: 22 + 4 - 6 = 20, and 2 to spare; above: 22 + 4 - 6 = 20, and 2.
        assert_eq!(laid.margin, [22, 0, 0, 22]);
        // No label, no badge, no margin.
        let bare = layout([614, 27], &rects[..1], &[None]);
        assert_eq!((bare.sides, bare.margin), (vec![None], [0, 0, 0, 0]));
    }

    #[test]
    fn a_wide_picture_shown_smaller_needs_bigger_badges_in_its_pixels() {
        // 1440 wide in a 760 column: a badge is 22 / 0.528 = 41.7 picture pixels; 30 of room
        // to the left isn't enough, 50 is.
        let tight = layout([1440, 900], &[[30.0, 400.0, 200.0, 40.0]], &[Some("1")]);
        assert_eq!(tight.sides, vec![Some(Side::Above)]);
        let room = layout([1440, 900], &[[50.0, 400.0, 200.0, 40.0]], &[Some("1")]);
        assert_eq!(room.sides, vec![Some(Side::Left)]);
        assert_eq!(badge_width("12"), 27.0);
    }
}
