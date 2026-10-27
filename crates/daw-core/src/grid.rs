//! Grid divisions and rounding. Where the grid lines fall is the tempo map's job
//! ([`crate::tempo::TempoMap::snap`], [`crate::tempo::TempoMap::grid_lines`]).
//!
//! A division `1/N` is one N-th of a whole note; `beat` is a note of the time signature's
//! lower number; `bar` is a whole bar (and `N bars` every N-th bar, counted from the song's
//! first). The song's own grid ([`Grid`]) is one of `1/4` to `1/64` (a triplet too), or
//! Auto, which picks the division from the zoom.

use std::fmt;
use std::str::FromStr;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Division {
    /// Every N-th bar's start (N from 1), counted from the song's first bar: 1 is every bar.
    Bars(u32),
    Beat,
    /// One N-th of a whole note (`1/16` is a sixteenth), optionally as a triplet.
    Note {
        n: u32,
        triplet: bool,
    },
}

impl Division {
    /// One N-th of a whole note, straight.
    pub const fn note(n: u32) -> Self {
        Division::Note { n, triplet: false }
    }

    /// Its length in quarter notes, for the divisions that have a fixed one (notes).
    pub fn quarters(self) -> Option<f64> {
        match self {
            Division::Note { n, triplet } => {
                Some(4.0 / f64::from(n.max(1)) * if triplet { 2.0 / 3.0 } else { 1.0 })
            }
            _ => None,
        }
    }
}

impl fmt::Display for Division {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Division::Bars(1) => write!(f, "bar"),
            Division::Bars(n) => write!(f, "{n} bars"),
            Division::Beat => write!(f, "beat"),
            Division::Note { n, triplet } => {
                write!(f, "1/{n}{}", if triplet { "t" } else { "" })
            }
        }
    }
}

impl FromStr for Division {
    type Err = String;

    /// "bar", "2 bars", "beat", "1/4", "1/8", "1/16", "1/32", and triplets like "1/8t".
    fn from_str(text: &str) -> Result<Self, String> {
        let t = text.trim().to_lowercase();
        match t.as_str() {
            "bar" => return Ok(Division::Bars(1)),
            "beat" => return Ok(Division::Beat),
            _ => {}
        }
        let bad = || {
            format!("'{text}' is not a grid division (bar, beat, 1/4, 1/8, 1/16, 1/32, or 1/8t for triplets)")
        };
        if let Some(n) = t.strip_suffix("bars").map(str::trim) {
            let n: u32 = n.parse().map_err(|_| bad())?;
            return if (1..=1024).contains(&n) {
                Ok(Division::Bars(n))
            } else {
                Err(bad())
            };
        }
        let rest = t.strip_prefix("1/").ok_or_else(bad)?;
        let (digits, triplet) = match rest.strip_suffix('t') {
            Some(d) => (d, true),
            None => (rest, false),
        };
        let n: u32 = digits.parse().map_err(|_| bad())?;
        if !(1..=128).contains(&n) {
            return Err(bad());
        }
        Ok(Division::Note { n, triplet })
    }
}

/// The notes the song's grid offers, as `1/N`: a quarter note down to a sixty-fourth.
pub const GRID_NOTES: [u32; 5] = [4, 8, 16, 32, 64];

/// Lines closer than this many points are thinned out (a level skipped, or every other bar).
pub const MIN_GAP: f64 = 6.0;

/// Auto's spacing: it picks the finest division whose lines are at least this many points
/// apart.
pub const AUTO_GAP: f64 = 14.0;

/// The song's grid, saved with the project: the Multitrack's lines and snapping, the piano
/// roll's and the Waveform view's bars and beats. A fixed division (`1/4` to `1/64`, straight
/// or triplet), or Auto: the division follows each view's zoom ([`auto_division`]).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Grid {
    Auto,
    Fixed(Division),
}

impl Default for Grid {
    /// An eighth note, as the user asked for.
    fn default() -> Self {
        Grid::Fixed(Division::note(8))
    }
}

impl fmt::Display for Grid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Grid::Auto => write!(f, "auto"),
            Grid::Fixed(d) => d.fmt(f),
        }
    }
}

impl FromStr for Grid {
    type Err = String;

    /// "auto", or "1/4", "1/8", "1/16", "1/32", "1/64", each with a "t" for triplets.
    fn from_str(text: &str) -> Result<Self, String> {
        if text.trim().eq_ignore_ascii_case("auto") {
            return Ok(Grid::Auto);
        }
        let bad = || {
            format!(
                "'{text}' is not a song grid: auto, 1/4, 1/8, 1/16, 1/32 or 1/64 (a \"t\" after \
                 it for triplets, as 1/8t)"
            )
        };
        match text.parse::<Division>() {
            Ok(d @ Division::Note { n, .. }) if GRID_NOTES.contains(&n) => Ok(Grid::Fixed(d)),
            _ => Err(bad()),
        }
    }
}

impl serde::Serialize for Grid {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> serde::Deserialize<'de> for Grid {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        text.parse().map_err(serde::de::Error::custom)
    }
}

/// Auto's division where a quarter note is `quarter_px` points wide, in a bar of
/// `bar_quarters` quarter notes whose beats are `beat_quarters` long: the finest of 1/64 to
/// 1/4 whose lines are at least [`AUTO_GAP`] apart, then the beat (when it is longer than a
/// quarter, as in 2/2), then every bar, 2, 4, 8… bars.
pub fn auto_division(quarter_px: f64, beat_quarters: f64, bar_quarters: f64) -> Division {
    let fits = |quarters: f64| quarters * quarter_px >= AUTO_GAP;
    for n in GRID_NOTES.iter().rev() {
        let d = Division::note(*n);
        if d.quarters().is_some_and(fits) {
            return d;
        }
    }
    if beat_quarters > 1.0 && fits(beat_quarters) {
        return Division::Beat;
    }
    let mut bars = 1u32;
    while bars < 1024 && !fits(bar_quarters * f64::from(bars)) {
        bars *= 2;
    }
    Division::Bars(bars)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Nearest,
    Previous,
    Next,
}

/// Snap a position to evenly spaced lines (multiples of `step` from 0). The project grid,
/// which follows tempo changes, is [`crate::tempo::TempoMap::snap`].
pub fn quantize(position: u64, step: f64, rounding: Rounding) -> u64 {
    let lines = position as f64 / step;
    let line = match rounding {
        Rounding::Nearest => lines.round(),
        Rounding::Previous => (lines + 1e-9).floor(),
        Rounding::Next => (lines - 1e-9).ceil(),
    };
    (line.max(0.0) * step).round() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn divisions_parse() {
        assert_eq!("bar".parse::<Division>(), Ok(Division::Bars(1)));
        assert_eq!("4 bars".parse::<Division>(), Ok(Division::Bars(4)));
        for text in ["bar", "4 bars", "beat", "1/16", "1/8t"] {
            assert_eq!(text.parse::<Division>().unwrap().to_string(), text);
        }
        assert_eq!(
            "1/8T".parse::<Division>(),
            Ok(Division::Note {
                n: 8,
                triplet: true
            })
        );
        assert!("1/0".parse::<Division>().is_err());
        assert!("quarter".parse::<Division>().unwrap_err().contains("1/8t"));
    }

    #[test]
    fn grid_lines_follow_the_tempo_and_meter() {
        use crate::tempo::TempoMap;
        let snap = |map: &TempoMap, p, d: &str| map.snap(p, d.parse().unwrap(), Rounding::Nearest);
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        assert_eq!(snap(&map, 23_000, "beat"), 24_000);
        assert_eq!(snap(&map, 100_000, "bar"), 96_000);
        assert_eq!(snap(&map, 6_100, "1/16"), 6_000);
        assert_eq!(snap(&map, 8_100, "1/8t"), 8_000);
        // 7/8 at 60 quarter notes: an eighth-note beat is half a second, a sixteenth 12000.
        let map = TempoMap::constant(48_000, 60.0, 7, 8);
        assert_eq!(snap(&map, 13_000, "1/16"), 12_000);
        assert_eq!(snap(&map, 167_000, "bar"), 168_000);
    }

    #[test]
    fn the_songs_grid_is_auto_or_a_quarter_to_a_sixty_fourth() {
        for text in [
            "auto", "1/4", "1/8", "1/16", "1/32", "1/64", "1/8t", "1/64t",
        ] {
            let grid: Grid = text.parse().unwrap();
            assert_eq!(grid.to_string(), text);
            let json = serde_json::to_string(&grid).unwrap();
            assert_eq!(json, format!("\"{text}\""));
            assert_eq!(serde_json::from_str::<Grid>(&json).unwrap(), grid);
        }
        assert_eq!("AUTO".parse::<Grid>(), Ok(Grid::Auto));
        for bad in ["1/2", "1/128", "beat", "bar", "2 bars", "1/12", "eighth"] {
            let e = bad.parse::<Grid>().unwrap_err();
            assert!(e.contains(bad) && e.contains("1/64"), "{e}");
        }
        assert!(serde_json::from_str::<Grid>("\"1/2\"").is_err());
        assert_eq!(Grid::default(), Grid::Fixed(Division::note(8)));
        // A project from before the grid opens on 1/8; one saved with it keeps it.
        let mut project = crate::project::Project::default();
        let mut json: serde_json::Value = serde_json::to_value(&project).unwrap();
        assert_eq!(json["grid"], "1/8");
        json.as_object_mut().unwrap().remove("grid");
        let old: crate::project::Project = serde_json::from_value(json).unwrap();
        assert_eq!(old.grid, Grid::Fixed(Division::note(8)));
        project.grid = Grid::Auto;
        let text = serde_json::to_string(&project).unwrap();
        let back: crate::project::Project = serde_json::from_str(&text).unwrap();
        assert_eq!(back.grid, Grid::Auto);
        // Its undo step is named.
        assert_eq!(crate::history::describe(&old, &project).0, "Grid");
    }

    #[test]
    fn auto_goes_from_a_sixty_fourth_to_bars_as_a_quarter_note_narrows() {
        let at = |quarter_px| auto_division(quarter_px, 1.0, 4.0);
        assert_eq!(at(1000.0), Division::note(64));
        // A sixteenth is a quarter of a quarter note: 14 points at 56.
        assert_eq!(at(56.0), Division::note(16));
        assert_eq!(at(55.9), Division::note(8));
        assert_eq!(at(14.0), Division::note(4));
        assert_eq!(at(13.9), Division::Bars(1));
        assert_eq!(at(1.0), Division::Bars(4));
        assert_eq!(at(0.1), Division::Bars(64));
        // 2/2: the half-note beat before the bar; 6/8: bars of three quarter notes.
        assert_eq!(auto_division(10.0, 2.0, 4.0), Division::Beat);
        assert_eq!(auto_division(2.0, 0.5, 3.0), Division::Bars(4));
    }

    #[test]
    fn rounding() {
        assert_eq!(quantize(6_100, 6_000.0, Rounding::Nearest), 6_000);
        assert_eq!(quantize(9_100, 6_000.0, Rounding::Nearest), 12_000);
        assert_eq!(quantize(11_999, 6_000.0, Rounding::Previous), 6_000);
        assert_eq!(quantize(12_000, 6_000.0, Rounding::Previous), 12_000);
        assert_eq!(quantize(12_000, 6_000.0, Rounding::Next), 12_000);
        assert_eq!(quantize(12_001, 6_000.0, Rounding::Next), 18_000);
    }
}
