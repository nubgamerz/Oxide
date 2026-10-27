//! Figures: a screenshot of the app with highlights drawn over it.
//!
//! In a page:
//!
//! ```text
//! ::: shot settings-audio
//! crop settings.audio 8
//! highlight settings.audio.processing_block "1" "Gather the device's buffers into a bigger block."
//! highlight settings.audio.buffer_size "2"
//! dim
//! Settings > Audio, where the processing block is set.
//! :::
//! ```
//!
//! The image is `shots/<name>.png`; beside it, `shots/<name>.json` holds the rects of the
//! named parts the app drew (its "marks", 27.2): `{"image": [w, h], "marks": {"name": [x, y,
//! w, h]}}` in image pixels. Highlights are drawn in the HTML over the image (positioned in
//! percentages, so they follow the image as it scales), never into it.
//!
//! A figure's [`Plan`] says which image it shows: the screenshot cut to its `crop` (in whole
//! pixels, grown outwards), scaled down when wider than [`MAX_WIDTH`]; its highlights are the
//! marks moved into that image ([`Plan::place`]). Two figures cropping a shot alike share one
//! file.
//!
//! The figure block is lifted out of the Markdown before it is parsed and replaced by an HTML
//! comment of the same number of lines (so line numbers in messages stay right); the comment
//! is swapped for the figure's HTML after rendering.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Deserialize;

use crate::check::{Known, Problems, RefKind, Severity};
use crate::html::escape;

/// A screenshot's marks: the image's size and each named part's rect, in image pixels.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marks {
    pub image: [f64; 2],
    pub marks: BTreeMap<String, [f64; 4]>,
}

/// What a figure asks to be cropped to.
#[derive(Debug, Clone, PartialEq)]
pub enum Crop {
    /// The rect around one or more marks, grown by `pad` image pixels each side.
    Marks { names: Vec<String>, pad: f64 },
    /// A rect in image pixels.
    Rect([f64; 4]),
}

/// One highlight: an outline round a mark, with an optional numbered callout and caption.
#[derive(Debug, Clone, PartialEq)]
pub struct Highlight {
    pub line: usize,
    pub mark: String,
    /// The callout's label (usually a number); none draws the outline only.
    pub label: Option<String>,
    /// What the callout says, listed under the figure.
    pub text: Option<String>,
}

/// A `::: shot` block.
#[derive(Debug, Clone, PartialEq)]
pub struct Figure {
    pub line: usize,
    pub shot: String,
    pub crop: Option<Crop>,
    /// Dim everything but the highlights.
    pub dim: bool,
    pub highlights: Vec<Highlight>,
    /// The caption, inline Markdown (keys and links work).
    pub caption: String,
}

/// The placeholder a figure leaves in the Markdown: `<!--oxide-figure-N` ... `-->`.
pub const PLACEHOLDER: &str = "<!--oxide-figure-";

/// The widest image a figure keeps, in pixels: twice the page's 760-pixel column, so it stays
/// sharp on a screen at 200 %. Wider crops are scaled down to it.
pub const MAX_WIDTH: u32 = 1520;

/// What a figure shows of its screenshot, and how big.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// The image file in the built manual (`shots/main-window-0-26-1440-46.png`, or
    /// `shots/<shot>.png` uncropped).
    pub file: String,
    /// The part of the screenshot shown, in its pixels: x, y, width, height.
    pub crop: [u32; 4],
    /// The image's size as written: the crop, scaled down past [`MAX_WIDTH`].
    pub size: [u32; 2],
}

impl Plan {
    /// Works out `figure`'s image from its screenshot's marks (whose `image` is its size). A
    /// crop naming a mark the shot lacks shows the whole screenshot (the check reports it).
    pub fn new(figure: &Figure, marks: &Marks) -> Plan {
        let [width, height] = marks.image;
        let crop = crop_rect(figure.crop.as_ref(), marks);
        let size = scaled_size(crop, MAX_WIDTH);
        let whole = crop == [0, 0, width as u32, height as u32];
        let file = if whole {
            format!("shots/{}.png", figure.shot)
        } else {
            format!(
                "shots/{}-{}-{}-{}-{}.png",
                figure.shot, crop[0], crop[1], crop[2], crop[3]
            )
        };
        Plan { file, crop, size }
    }

    /// `rect` (the screenshot's pixels) in the written image's pixels: moved by the crop, cut
    /// to it and scaled with it. `None` when none of it is in the crop.
    pub fn place(&self, rect: [f64; 4]) -> Option<[f64; 4]> {
        let [cx, cy, cw, ch] = self.crop.map(f64::from);
        let x0 = (rect[0] - cx).max(0.0);
        let y0 = (rect[1] - cy).max(0.0);
        let x1 = (rect[0] + rect[2] - cx).min(cw);
        let y1 = (rect[1] + rect[3] - cy).min(ch);
        if x1 <= x0 || y1 <= y0 {
            return None;
        }
        let sx = f64::from(self.size[0]) / cw;
        let sy = f64::from(self.size[1]) / ch;
        Some([x0 * sx, y0 * sy, (x1 - x0) * sx, (y1 - y0) * sy])
    }

    /// Whether the crop cuts into `rect` (some of it shows, not all).
    pub fn cuts(&self, rect: [f64; 4]) -> bool {
        let [cx, cy, cw, ch] = self.crop.map(f64::from);
        rect[0] < cx || rect[1] < cy || rect[0] + rect[2] > cx + cw || rect[1] + rect[3] > cy + ch
    }
}

/// The part of a screenshot `crop` asks for, in whole pixels (grown outwards) inside the
/// image `marks.image` gives: x, y, width, height. No crop, or one naming only marks the shot
/// lacks, is the whole screenshot (the figure check, and the README's list, report those).
pub fn crop_rect(crop: Option<&Crop>, marks: &Marks) -> [u32; 4] {
    let [width, height] = marks.image;
    let wanted = match crop {
        None => [0.0, 0.0, width, height],
        Some(Crop::Rect(rect)) => *rect,
        Some(Crop::Marks { names, pad }) => {
            let rects: Vec<&[f64; 4]> = names.iter().filter_map(|n| marks.marks.get(n)).collect();
            if rects.is_empty() {
                [0.0, 0.0, width, height]
            } else {
                let x0 = rects.iter().map(|r| r[0]).fold(f64::INFINITY, f64::min);
                let y0 = rects.iter().map(|r| r[1]).fold(f64::INFINITY, f64::min);
                let x1 = rects
                    .iter()
                    .map(|r| r[0] + r[2])
                    .fold(f64::NEG_INFINITY, f64::max);
                let y1 = rects
                    .iter()
                    .map(|r| r[1] + r[3])
                    .fold(f64::NEG_INFINITY, f64::max);
                [x0 - pad, y0 - pad, x1 - x0 + 2.0 * pad, y1 - y0 + 2.0 * pad]
            }
        }
    };
    let x0 = wanted[0].floor().clamp(0.0, (width - 1.0).max(0.0));
    let y0 = wanted[1].floor().clamp(0.0, (height - 1.0).max(0.0));
    let x1 = (wanted[0] + wanted[2])
        .ceil()
        .clamp(x0 + 1.0, width.max(x0 + 1.0));
    let y1 = (wanted[1] + wanted[3])
        .ceil()
        .clamp(y0 + 1.0, height.max(y0 + 1.0));
    [x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32]
}

/// The size an image cut to `crop` is written at: the crop's own, scaled down (keeping its
/// shape) when it is wider than `max_width`.
pub fn scaled_size(crop: [u32; 4], max_width: u32) -> [u32; 2] {
    if crop[2] > max_width {
        let scale = f64::from(max_width) / f64::from(crop[2]);
        [
            max_width,
            ((f64::from(crop[3]) * scale).round() as u32).max(1),
        ]
    } else {
        [crop[2], crop[3]]
    }
}

/// Lifts every `::: shot` block out of `text`. Returns the Markdown with placeholders and the
/// figures in order. Fenced code is left alone, so a manual can show the syntax.
pub fn lift(text: &str, file: &str, problems: &mut Problems) -> (String, Vec<Figure>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = String::with_capacity(text.len());
    let mut figures = Vec::new();
    let mut fence: Option<(char, usize)> = None;
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let trimmed = line.trim_start();
        if let Some((ch, len)) = fence {
            let run = trimmed.chars().take_while(|&c| c == ch).count();
            if run >= len && trimmed[run * ch.len_utf8()..].trim().is_empty() {
                fence = None;
            }
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }
        for ch in ['`', '~'] {
            let run = trimmed.chars().take_while(|&c| c == ch).count();
            if run >= 3 {
                fence = Some((ch, run));
            }
        }
        if fence.is_some() || !trimmed.starts_with(":::") {
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        }
        // A block: find its end.
        let start = i;
        let Some(end) = (i + 1..lines.len()).find(|&j| lines[j].trim() == ":::") else {
            problems.error(
                RefKind::Syntax,
                file,
                start + 1,
                "a ::: block that never ends (close it with a line holding only :::)",
            );
            out.push_str(line);
            out.push('\n');
            i += 1;
            continue;
        };
        let head = trimmed[3..].trim();
        // A block that can't be read leaves a plain comment, so its lines don't show.
        let mut placed = false;
        match head.split_once(char::is_whitespace).unwrap_or((head, "")) {
            ("shot", name) if !name.trim().is_empty() && !name.trim().contains(' ') => {
                let figure = parse(
                    name.trim(),
                    &lines[start + 1..end],
                    start + 1,
                    file,
                    problems,
                );
                let _ = write!(out, "{PLACEHOLDER}{}", figures.len());
                figures.push(figure);
                placed = true;
            }
            // An effect's parameter table (27.5, [`crate::effects`]): made when the page
            // renders, so its words reach the search; nothing goes inside.
            ("params", id) if !id.trim().is_empty() && !id.trim().contains(' ') => {
                if lines[start + 1..end].iter().any(|l| !l.trim().is_empty()) {
                    problems.error(
                        RefKind::Syntax,
                        file,
                        start + 2,
                        "::: params takes nothing inside: the table is made from the effect",
                    );
                }
                let _ = write!(out, "{}{}", crate::effects::PLACEHOLDER, id.trim());
                placed = true;
            }
            ("params", _) => problems.error(
                RefKind::Syntax,
                file,
                start + 1,
                "::: params needs one effect id (::: params parametric-eq)",
            ),
            ("shot", _) => problems.error(
                RefKind::Syntax,
                file,
                start + 1,
                "::: shot needs one screenshot name (::: shot settings-audio)",
            ),
            (other, _) => problems.error(
                RefKind::Syntax,
                file,
                start + 1,
                format!("unknown block \"::: {other}\" (only ::: shot and ::: params)"),
            ),
        }
        if !placed {
            out.push_str("<!--");
        }
        // Keep the line count: blank lines, then the comment's end on the block's last line.
        for _ in start..end {
            out.push('\n');
        }
        out.push_str("-->\n");
        i = end + 1;
    }
    (out, figures)
}

/// Parses a figure's body; `first` is the 1-based line of `::: shot`.
fn parse(shot: &str, body: &[&str], first: usize, file: &str, problems: &mut Problems) -> Figure {
    let mut figure = Figure {
        line: first,
        shot: shot.to_string(),
        crop: None,
        dim: false,
        highlights: Vec::new(),
        caption: String::new(),
    };
    for (n, raw) in body.iter().enumerate() {
        let line = first + 1 + n;
        let text = raw.trim();
        let (word, rest) = text.split_once(char::is_whitespace).unwrap_or((text, ""));
        let rest = rest.trim();
        match word {
            "" => {}
            "dim" if rest.is_empty() => figure.dim = true,
            "highlight" => {
                let (mark, rest) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                if mark.is_empty() || mark.starts_with('"') {
                    problems.error(
                        RefKind::Syntax,
                        file,
                        line,
                        "highlight needs a mark name: highlight transport.record \"1\" \"Record\"",
                    );
                    continue;
                }
                match quoted(rest) {
                    Some(strings) if strings.len() <= 2 => {
                        let mut strings = strings.into_iter();
                        figure.highlights.push(Highlight {
                            line,
                            mark: mark.to_string(),
                            label: strings.next().filter(|s| !s.is_empty()),
                            text: strings.next().filter(|s| !s.is_empty()),
                        });
                    }
                    _ => problems.error(
                        RefKind::Syntax,
                        file,
                        line,
                        "after the mark, a highlight takes up to two quoted strings: the \
                         callout's label and its text",
                    ),
                }
            }
            "crop" => {
                let parts: Vec<&str> = rest.split_whitespace().collect();
                let numbers: Option<Vec<f64>> = parts.iter().map(|p| p.parse().ok()).collect();
                // Marks, then an optional padding; or four numbers, a rect.
                let (names, pad) = match parts.split_last() {
                    Some((last, names)) if last.parse::<f64>().is_ok() => {
                        (names, last.parse::<f64>().ok())
                    }
                    _ => (parts.as_slice(), None),
                };
                let marks_only = !names.is_empty()
                    && names
                        .iter()
                        .all(|n| n.parse::<f64>().is_err() && !n.starts_with('"'));
                figure.crop = match numbers {
                    Some(n) if n.len() == 4 => Some(Crop::Rect([n[0], n[1], n[2], n[3]])),
                    _ if marks_only => Some(Crop::Marks {
                        names: names.iter().map(|n| n.to_string()).collect(),
                        pad: pad.unwrap_or(0.0),
                    }),
                    _ => {
                        problems.error(
                            RefKind::Syntax,
                            file,
                            line,
                            "crop takes one or more marks and an optional padding (crop \
                             status.device status.pdc 6) or a rect in image pixels (crop x y w h)",
                        );
                        None
                    }
                };
            }
            _ => {
                if !figure.caption.is_empty() {
                    figure.caption.push(' ');
                }
                figure.caption.push_str(text);
            }
        }
    }
    figure
}

/// `"a" "b c"` → `["a", "b c"]`; `\"` inside a string is a quote. `None` on anything else.
fn quoted(mut s: &str) -> Option<Vec<String>> {
    let mut out = Vec::new();
    loop {
        s = s.trim_start();
        if s.is_empty() {
            return Some(out);
        }
        let mut chars = s.strip_prefix('"')?.char_indices();
        let mut text = String::new();
        let end = loop {
            match chars.next()? {
                (_, '\\') => text.push(chars.next()?.1),
                (i, '"') => break i,
                (_, c) => text.push(c),
            }
        };
        out.push(text);
        s = &s[end + 2..];
    }
}

/// Checks a figure's references: its shot (listed in `shots.toml`, made), and the marks its
/// highlights and crop name (drawn in that shot, and inside the crop).
pub fn check(figure: &Figure, known: &Known, problems: &mut Problems, file: &str) {
    if known
        .listed_shots
        .as_ref()
        .is_some_and(|listed| !listed.contains(&figure.shot))
    {
        problems.error(
            RefKind::Shot,
            file,
            figure.line,
            format!(
                "screenshot \"{}\" isn't in {}: list it with the --show scenario that makes it",
                figure.shot,
                crate::shots::LIST
            ),
        );
    }
    let severity = if known.strict_shots {
        Severity::Error
    } else {
        Severity::Warning
    };
    let shot = known.shots.get(&figure.shot);
    if !shot.is_some_and(|s| s.image) {
        problems.push(
            severity,
            RefKind::Shot,
            file,
            figure.line,
            format!(
                "screenshot \"{0}\" isn't there yet (shots/{0}.png); a placeholder is shown",
                figure.shot
            ),
        );
        return;
    }
    let Some(marks) = shot.and_then(|s| s.marks.as_ref()) else {
        if !figure.highlights.is_empty() || figure.crop.is_some() {
            problems.push(
                severity,
                RefKind::Shot,
                file,
                figure.line,
                format!(
                    "screenshot \"{0}\" has no marks (shots/{0}.json), so its highlights can't \
                     be drawn",
                    figure.shot
                ),
            );
        }
        return;
    };
    let plan = Plan::new(figure, marks);
    for highlight in &figure.highlights {
        match marks.marks.get(&highlight.mark) {
            None => problems.error(
                RefKind::Mark,
                file,
                highlight.line,
                format!(
                    "screenshot \"{}\" has no mark \"{}\" (it has: {})",
                    figure.shot,
                    highlight.mark,
                    names(marks)
                ),
            ),
            Some(rect) if plan.place(*rect).is_none() => problems.error(
                RefKind::Mark,
                file,
                highlight.line,
                format!(
                    "mark \"{}\" is outside the figure's crop: crop to a part that holds it, or \
                     give the highlight a figure of its own",
                    highlight.mark
                ),
            ),
            Some(rect) if plan.cuts(*rect) => problems.warn(
                RefKind::Mark,
                file,
                highlight.line,
                format!(
                    "the crop cuts into mark \"{}\": its outline shows only the part inside",
                    highlight.mark
                ),
            ),
            Some(_) => {}
        }
    }
    match &figure.crop {
        Some(Crop::Marks { names: wanted, .. }) => {
            for name in wanted.iter().filter(|n| !marks.marks.contains_key(*n)) {
                problems.error(
                    RefKind::Mark,
                    file,
                    figure.line,
                    format!(
                        "the crop names mark \"{name}\", which screenshot \"{}\" doesn't have \
                         (it has: {})",
                        figure.shot,
                        names(marks)
                    ),
                );
            }
        }
        Some(Crop::Rect([x, y, w, h]))
            if *x < 0.0
                || *y < 0.0
                || *w <= 0.0
                || *h <= 0.0
                || x + w > marks.image[0]
                || y + h > marks.image[1] =>
        {
            problems.error(
                RefKind::Syntax,
                file,
                figure.line,
                format!(
                    "the crop {x} {y} {w} {h} isn't inside the {}x{} image",
                    marks.image[0], marks.image[1]
                ),
            )
        }
        _ => {}
    }
}

fn names(marks: &Marks) -> String {
    if marks.marks.is_empty() {
        return "none".to_string();
    }
    marks.marks.keys().cloned().collect::<Vec<_>>().join(", ")
}

/// The figure's HTML. `number` makes its SVG mask's id unique on the page; `caption` is the
/// caption already rendered from Markdown (inline HTML), `caption_text` its plain text (the
/// image's alt text). The image is the figure's [`Plan`]'s; `data-size` on the shot and
/// `data-rect` on each highlight give them in the image's pixels (the browser check reads
/// them).
pub fn html(
    figure: &Figure,
    number: usize,
    known: &Known,
    caption: &str,
    caption_text: &str,
    callouts: &[(Option<String>, String)],
) -> String {
    let shot = known.shots.get(&figure.shot);
    let image = shot.is_some_and(|s| s.image);
    let marks = shot.and_then(|s| s.marks.as_ref());
    let plan = marks.map(|m| Plan::new(figure, m));
    let (width, height) = plan.as_ref().map_or((16.0, 9.0), |p| {
        (f64::from(p.size[0]), f64::from(p.size[1]))
    });
    let name = escape(&figure.shot);
    // The highlights in the picture's pixels, and where their badges go (`callout`).
    let rects: Vec<(&Highlight, [f64; 4])> = match (marks.filter(|_| image), &plan) {
        (Some(marks), Some(plan)) => figure
            .highlights
            .iter()
            .filter_map(|h| {
                let rect = marks.marks.get(&h.mark)?;
                plan.place(*rect).map(|r| (h, r))
            })
            .collect(),
        _ => Vec::new(),
    };
    let badges = plan.as_ref().map(|p| {
        let boxes: Vec<[f64; 4]> = rects.iter().map(|(_, r)| *r).collect();
        let labels: Vec<Option<&str>> = rects.iter().map(|(h, _)| h.label.as_deref()).collect();
        crate::callout::layout(p.size, &boxes, &labels)
    });
    let mut out = String::new();
    out.push_str("<figure class=\"ox-figure\">\n");
    // Room round the picture for badges that have none in it.
    let margin = badges.as_ref().map_or([0; 4], |b| b.margin);
    if margin != [0; 4] {
        let _ = write!(
            out,
            "<div class=\"ox-frame\" style=\"padding: {}px {}px {}px {}px\">",
            margin[0], margin[1], margin[2], margin[3]
        );
    }
    let _ = write!(
        out,
        "<div class=\"ox-shot{}\" style=\"aspect-ratio: {} / {}{}\"{}>",
        if image { "" } else { " ox-missing" },
        number_text(width),
        number_text(height),
        // Never wider than the image itself: a narrow crop is shown pixel for pixel, not
        // stretched across the column.
        match &plan {
            Some(p) => format!("; max-width: {}px", p.size[0]),
            None => String::new(),
        },
        match &plan {
            Some(p) => format!(" data-size=\"{} {}\"", p.size[0], p.size[1]),
            None => String::new(),
        }
    );
    if image {
        let src = plan
            .as_ref()
            .map_or_else(|| format!("shots/{}.png", figure.shot), |p| p.file.clone());
        let _ = write!(
            out,
            "<img src=\"{}\" alt=\"{}\" loading=\"lazy\">",
            escape(&src),
            escape(caption_text)
        );
    } else {
        let _ = write!(
            out,
            "<span class=\"ox-missing-text\">Screenshot \u{201c}{name}\u{201d} isn't made yet.</span>"
        );
    }
    if !rects.is_empty() {
        if figure.dim {
            let _ = write!(
                out,
                "<svg class=\"ox-dim\" viewBox=\"0 0 {w} {h}\" preserveAspectRatio=\"none\" \
                 aria-hidden=\"true\"><defs><mask id=\"ox-mask-{number}\"><rect width=\"{w}\" \
                 height=\"{h}\" fill=\"white\"/>",
                w = number_text(width),
                h = number_text(height)
            );
            for (_, [x, y, w, h]) in &rects {
                let _ = write!(
                    out,
                    "<rect x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\" rx=\"4\" fill=\"black\"/>",
                    number_text(*x),
                    number_text(*y),
                    number_text(*w),
                    number_text(*h)
                );
            }
            let _ = write!(
                out,
                "</mask></defs><rect width=\"{}\" height=\"{}\" mask=\"url(#ox-mask-{number})\"/></svg>",
                number_text(width),
                number_text(height)
            );
        }
        let sides = badges.as_ref().map(|b| b.sides.clone()).unwrap_or_default();
        for (n, (highlight, [x, y, w, h])) in rects.into_iter().enumerate() {
            let _ = write!(
                out,
                "<span class=\"ox-hl\" data-mark=\"{}\" data-rect=\"{} {} {} {}\" style=\"left: {}%; top: {}%; width: {}%; height: {}%\">",
                escape(&highlight.mark),
                number_text(x),
                number_text(y),
                number_text(w),
                number_text(h),
                percent(x, width),
                percent(y, height),
                percent(w, width),
                percent(h, height)
            );
            if let Some(label) = &highlight.label {
                let side = sides
                    .get(n)
                    .copied()
                    .flatten()
                    .unwrap_or(crate::callout::Side::Left);
                let _ = write!(
                    out,
                    "<span class=\"ox-callout {}\">{}</span>",
                    side.class(),
                    escape(label)
                );
            }
            out.push_str("</span>");
        }
    }
    out.push_str("</div>\n");
    if margin != [0; 4] {
        out.push_str("</div>\n");
    }
    if !caption.is_empty() || !callouts.is_empty() {
        out.push_str("<figcaption>");
        if !caption.is_empty() {
            let _ = write!(out, "<p>{caption}</p>");
        }
        if !callouts.is_empty() {
            out.push_str("<ol class=\"ox-callouts\">");
            for (label, text) in callouts {
                match label {
                    Some(label) => {
                        let _ = write!(
                            out,
                            "<li><span class=\"ox-callout\">{}</span> {text}</li>",
                            escape(label)
                        );
                    }
                    None => {
                        let _ = write!(out, "<li>{text}</li>");
                    }
                }
            }
            out.push_str("</ol>");
        }
        out.push_str("</figcaption>\n");
    }
    out.push_str("</figure>");
    out
}

fn percent(value: f64, whole: f64) -> String {
    number_text(if whole > 0.0 {
        value / whole * 100.0
    } else {
        0.0
    })
}

/// A number with at most three decimals and no trailing zeros.
fn number_text(value: f64) -> String {
    let text = format!("{value:.3}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text.is_empty() || text == "-" {
        "0".to_string()
    } else {
        text.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_figure_block_is_lifted_and_keeps_the_line_count() {
        let text = "# T\n\nBefore.\n\n::: shot status-bar\ncrop status 4\nhighlight status.dsp \"1\" \"The DSP readout\"\nhighlight status.gaps\ndim\nThe status bar, [[F1]].\n:::\nAfter.\n";
        let mut problems = Problems::default();
        let (md, figures) = lift(text, "a.md", &mut problems);
        assert!(problems.list.is_empty(), "{:?}", problems.list);
        assert_eq!(md.lines().count(), text.lines().count());
        assert_eq!(md.lines().nth(11), Some("After."));
        assert!(md.contains("<!--oxide-figure-0"));
        let figure = &figures[0];
        assert_eq!(figure.shot, "status-bar");
        assert_eq!(figure.line, 5);
        assert!(figure.dim);
        assert_eq!(
            figure.crop,
            Some(Crop::Marks {
                names: vec!["status".into()],
                pad: 4.0
            })
        );
        assert_eq!(figure.highlights.len(), 2);
        assert_eq!(figure.highlights[0].label.as_deref(), Some("1"));
        assert_eq!(
            figure.highlights[0].text.as_deref(),
            Some("The DSP readout")
        );
        assert_eq!(figure.highlights[0].line, 7);
        assert_eq!(figure.highlights[1].label, None);
        assert_eq!(figure.caption, "The status bar, [[F1]].");
    }

    #[test]
    fn fenced_code_shows_the_syntax_without_making_a_figure() {
        let text = "```text\n::: shot x\n:::\n```\n";
        let mut problems = Problems::default();
        let (md, figures) = lift(text, "a.md", &mut problems);
        assert!(figures.is_empty());
        assert_eq!(md, text);
    }

    #[test]
    fn bad_blocks_are_reported() {
        for (text, what) in [
            ("::: shot x\n", "never ends"),
            ("::: note\n:::\n", "unknown block"),
            ("::: shot\n:::\n", "one screenshot name"),
            ("::: shot x\nhighlight \"1\"\n:::\n", "needs a mark"),
            ("::: shot x\nhighlight m 1\n:::\n", "quoted strings"),
            ("::: shot x\ncrop 1 2\n:::\n", "crop takes"),
        ] {
            let mut problems = Problems::default();
            lift(text, "a.md", &mut problems);
            assert_eq!(problems.errors(), 1, "{text:?}: {:?}", problems.list);
            assert!(
                problems.list[0].message.contains(what),
                "{text:?}: {}",
                problems.list[0].message
            );
        }
    }

    #[test]
    fn quoted_strings_take_escaped_quotes() {
        assert_eq!(
            quoted(r#""1"  "say \"hi\"""#),
            Some(vec!["1".to_string(), "say \"hi\"".to_string()])
        );
        assert_eq!(quoted(""), Some(vec![]));
        assert_eq!(quoted("x"), None);
        assert_eq!(quoted("\"open"), None);
    }

    fn figure_with(crop: Option<Crop>) -> Figure {
        Figure {
            line: 1,
            shot: "window".into(),
            crop,
            dim: false,
            highlights: Vec::new(),
            caption: String::new(),
        }
    }

    fn marks(image: [f64; 2], list: &[(&str, [f64; 4])]) -> Marks {
        Marks {
            image,
            marks: list.iter().map(|(n, r)| (n.to_string(), *r)).collect(),
        }
    }

    #[test]
    fn a_crop_is_cut_in_whole_pixels_and_moves_the_marks_into_it() {
        let shot = marks(
            [1440.0, 900.0],
            &[
                ("a", [100.5, 50.25, 40.0, 20.0]),
                ("b", [300.0, 60.0, 10.0, 10.0]),
            ],
        );
        // Two marks and a padding: their union grown by 4, out to whole pixels.
        let plan = Plan::new(
            &figure_with(Some(Crop::Marks {
                names: vec!["a".into(), "b".into()],
                pad: 4.0,
            })),
            &shot,
        );
        assert_eq!(plan.crop, [96, 46, 218, 29]);
        assert_eq!(plan.size, [218, 29]);
        assert_eq!(plan.file, "shots/window-96-46-218-29.png");
        assert_eq!(
            plan.place([100.5, 50.25, 40.0, 20.0]),
            Some([4.5, 4.25, 40.0, 20.0])
        );
        assert_eq!(
            plan.place([300.0, 60.0, 10.0, 10.0]),
            Some([204.0, 14.0, 10.0, 10.0])
        );
        // Outside: none; across the edge: cut to it.
        assert_eq!(plan.place([0.0, 0.0, 50.0, 40.0]), None);
        assert_eq!(
            plan.place([90.0, 70.0, 20.0, 20.0]),
            Some([0.0, 24.0, 14.0, 5.0])
        );
        assert!(plan.cuts([90.0, 70.0, 20.0, 20.0]));
        assert!(!plan.cuts([100.5, 50.25, 40.0, 20.0]));
        // A crop past the picture's edge stops at it; no crop is the whole picture.
        let edge = Plan::new(
            &figure_with(Some(Crop::Rect([1400.0, 880.0, 100.0, 100.0]))),
            &shot,
        );
        assert_eq!(edge.crop, [1400, 880, 40, 20]);
        let whole = Plan::new(&figure_with(None), &shot);
        assert_eq!((whole.crop, whole.size), ([0, 0, 1440, 900], [1440, 900]));
        assert_eq!(whole.file, "shots/window.png");
    }

    #[test]
    fn a_crop_wider_than_the_page_allows_is_scaled_with_its_marks() {
        // A 2x screen's window: 2880 pixels wide, halved to MAX_WIDTH.
        let shot = marks([2880.0, 1800.0], &[("dsp", [2380.0, 74.0, 492.0, 44.0])]);
        let plan = Plan::new(&figure_with(None), &shot);
        assert_eq!(plan.crop, [0, 0, 2880, 1800]);
        assert_eq!(plan.size, [MAX_WIDTH, 950]);
        let scale = f64::from(MAX_WIDTH) / 2880.0;
        let placed = plan.place([2380.0, 74.0, 492.0, 44.0]).expect("inside");
        let want = [
            2380.0 * scale,
            74.0 * 950.0 / 1800.0,
            492.0 * scale,
            44.0 * 950.0 / 1800.0,
        ];
        for (got, want) in placed.iter().zip(want) {
            assert!((got - want).abs() < 1e-9, "{placed:?} {want:?}");
        }
        // A crop within the width keeps its pixels.
        let narrow = Plan::new(
            &figure_with(Some(Crop::Marks {
                names: vec!["dsp".into()],
                pad: 0.0,
            })),
            &shot,
        );
        assert_eq!(narrow.size, [492, 44]);
    }

    #[test]
    fn crops_name_marks_with_an_optional_padding() {
        let crop = |line: &str| {
            let mut problems = Problems::default();
            let (_, figures) = lift(
                &format!(
                    "::: shot x
{line}
:::
"
                ),
                "a.md",
                &mut problems,
            );
            (figures[0].crop.clone(), problems.errors())
        };
        assert_eq!(
            crop("crop status.device status.pdc 6"),
            (
                Some(Crop::Marks {
                    names: vec!["status.device".into(), "status.pdc".into()],
                    pad: 6.0
                }),
                0
            )
        );
        assert_eq!(
            crop("crop transport"),
            (
                Some(Crop::Marks {
                    names: vec!["transport".into()],
                    pad: 0.0
                }),
                0
            )
        );
        assert_eq!(
            crop("crop 1 2 3 4"),
            (Some(Crop::Rect([1.0, 2.0, 3.0, 4.0])), 0)
        );
        assert_eq!(crop("crop 8").1, 1);
        assert_eq!(crop("crop").1, 1);
    }

    #[test]
    fn numbers_are_short() {
        assert_eq!(number_text(12.5), "12.5");
        assert_eq!(number_text(100.0), "100");
        assert_eq!(number_text(1.0 / 3.0), "0.333");
        assert_eq!(number_text(0.0), "0");
    }
}
