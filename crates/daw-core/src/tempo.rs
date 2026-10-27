//! The tempo map: where every beat and bar falls in samples, with tempo and time-signature
//! changes along the timeline.
//!
//! Built from the project's opening tempo and meter plus its [`TempoChange`]s, at a sample
//! rate. Everything that turns time into music (the grid, the metronome, the transport plug-ins
//! see, count-in, the bar display) asks the map; nothing else divides by a tempo.
//!
//! Tempo counts quarter notes per minute. A beat is a note of the time signature's lower
//! number: in 6/8 at 60, a beat (an eighth) lasts half a second. Bars and beats count from 0.
//! Changes sit on whole beats, so beat boundaries, bar starts and segment starts all fall on
//! whole beat indices; before 0 the opening segment extends backwards (a count-in).

use crate::graph::Meter;
use crate::grid::{auto_division, Division, Grid, Rounding, MIN_GAP};
use crate::project::{Project, TempoChange};

/// MIDI ticks per quarter note: the resolution of positions in musical time (MIDI clips and
/// their events).
pub const TICKS_PER_QUARTER: u32 = 960;

/// Samples per quarter note of audio played at a constant `tempo` (quarter notes per minute):
/// how a recording's own tempo turns into its frames (clips following the tempo, 7.3).
pub fn quarter_note_samples(sample_rate: u32, tempo: f64) -> f64 {
    f64::from(sample_rate) * 60.0 / tempo
}

/// A stretch of constant tempo and meter.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Segment {
    /// Where it starts, in samples (not rounded).
    pub start: f64,
    /// Beats from the timeline start to here (each a note of its own segment's lower number).
    pub beats: i64,
    /// Quarter notes from the timeline start to here.
    pub quarter_notes: f64,
    /// The bar it starts in, and the beat within that bar.
    pub bar: i64,
    pub beat: u32,
    /// Quarter notes per minute.
    pub tempo: f64,
    pub numerator: u32,
    pub denominator: u32,
}

impl Segment {
    /// Quarter notes in one beat.
    fn beat_quarters(&self) -> f64 {
        4.0 / f64::from(self.denominator.max(1))
    }
}

/// What a grid line marks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Bar,
    Beat,
    /// A line of the grid's division between beats.
    Division,
}

/// One line of the grid ([`TempoMap::grid_lines`]), at a song tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridLine {
    pub tick: u64,
    pub kind: LineKind,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TempoMap {
    pub sample_rate: u32,
    /// In time order, the first at 0. Never empty.
    segments: Vec<Segment>,
}

impl TempoMap {
    /// A single tempo and meter for the whole timeline.
    pub fn constant(sample_rate: u32, tempo: f64, numerator: u32, denominator: u32) -> Self {
        Self::build(sample_rate, tempo, (numerator, denominator), &[])
    }

    pub fn of(project: &Project, sample_rate: u32) -> Self {
        Self::build(
            sample_rate,
            project.tempo,
            project.time_signature,
            &project.tempo_changes,
        )
    }

    fn build(sample_rate: u32, tempo: f64, meter: (u32, u32), changes: &[TempoChange]) -> Self {
        let mut map = Self {
            sample_rate: sample_rate.max(1),
            segments: vec![Segment {
                start: 0.0,
                beats: 0,
                quarter_notes: 0.0,
                bar: 0,
                beat: 0,
                tempo: tempo.max(1.0),
                numerator: meter.0.max(1),
                denominator: meter.1.max(1),
            }],
        };
        let mut sorted = changes.to_vec();
        sorted.sort_by_key(|c| (c.bar, c.beat));
        for change in sorted {
            let last = *map.segments.last().expect("never empty");
            // A meter can only change at the start of a bar, and a beat must exist in its bar.
            let beat = change.beat.min(last.numerator - 1);
            let meter = change.time_signature.filter(|_| beat == 0);
            let into = (i64::from(change.bar) - last.bar) * i64::from(last.numerator)
                + i64::from(beat)
                - i64::from(last.beat);
            if into < 0 {
                continue;
            }
            let (numerator, denominator) = meter.unwrap_or((last.numerator, last.denominator));
            let next = Segment {
                start: last.start + into as f64 * map.beat_samples(&last),
                beats: last.beats + into,
                quarter_notes: last.quarter_notes + into as f64 * last.beat_quarters(),
                bar: i64::from(change.bar),
                beat,
                tempo: change.tempo.unwrap_or(last.tempo).max(1.0),
                numerator: numerator.max(1),
                denominator: denominator.max(1),
            };
            if into == 0 {
                // At the same place as the last one: it updates that one.
                *map.segments.last_mut().expect("never empty") = Segment {
                    start: last.start,
                    ..next
                };
            } else {
                map.segments.push(next);
            }
        }
        map
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    fn beat_samples(&self, segment: &Segment) -> f64 {
        f64::from(self.sample_rate) * 60.0 / segment.tempo * segment.beat_quarters()
    }

    /// The segment a sample position falls in (the first one before 0).
    pub fn segment_at(&self, sample: f64) -> &Segment {
        let i = self.segments.partition_point(|s| s.start <= sample);
        &self.segments[i.saturating_sub(1)]
    }

    fn segment_of_beat(&self, beat: f64) -> &Segment {
        let i = self.segments.partition_point(|s| s.beats as f64 <= beat);
        &self.segments[i.saturating_sub(1)]
    }

    /// Beats from the timeline start to `sample`, with the fraction.
    pub fn beats_at(&self, sample: f64) -> f64 {
        let s = self.segment_at(sample);
        s.beats as f64 + (sample - s.start) / self.beat_samples(s)
    }

    /// Where beat `beat` (counted from the timeline start, fractions allowed) falls.
    pub fn sample_of_beat(&self, beat: f64) -> f64 {
        let s = self.segment_of_beat(beat);
        s.start + (beat - s.beats as f64) * self.beat_samples(s)
    }

    /// Which beat of its bar beat `beat` is (0 = the downbeat).
    pub fn beat_in_bar(&self, beat: i64) -> u32 {
        let s = self.segment_of_beat(beat as f64);
        (i64::from(s.beat) + beat - s.beats).rem_euclid(i64::from(s.numerator)) as u32
    }

    /// Quarter notes from the timeline start to `sample`.
    pub fn quarter_notes_at(&self, sample: f64) -> f64 {
        let s = self.segment_at(sample);
        s.quarter_notes + (sample - s.start) * s.tempo / 60.0 / f64::from(self.sample_rate)
    }

    /// Where `quarter_notes` falls.
    pub fn sample_at_quarter_notes(&self, quarter_notes: f64) -> f64 {
        let i = self
            .segments
            .partition_point(|s| s.quarter_notes <= quarter_notes);
        let s = &self.segments[i.saturating_sub(1)];
        s.start + (quarter_notes - s.quarter_notes) * 60.0 / s.tempo * f64::from(self.sample_rate)
    }

    /// The bar `sample` is in and how far into it, in beats.
    pub fn bar_at(&self, sample: f64) -> (i64, f64) {
        let s = self.segment_at(sample);
        let beats = f64::from(s.beat) + (sample - s.start) / self.beat_samples(s);
        let bars = (beats / f64::from(s.numerator)).floor();
        (s.bar + bars as i64, beats - bars * f64::from(s.numerator))
    }

    /// The segment a bar starts in.
    fn segment_of_bar(&self, bar: i64) -> &Segment {
        let i = self
            .segments
            .partition_point(|s| s.bar < bar || (s.bar == bar && s.beat == 0));
        &self.segments[i.saturating_sub(1)]
    }

    /// Where bar `bar` starts.
    pub fn bar_start(&self, bar: i64) -> f64 {
        let s = self.segment_of_bar(bar);
        let beats = (bar - s.bar) * i64::from(s.numerator) - i64::from(s.beat);
        s.start + beats as f64 * self.beat_samples(s)
    }

    /// Where `tick` (MIDI ticks from the timeline start, [`TICKS_PER_QUARTER`] to a quarter
    /// note) falls, in samples.
    pub fn sample_at_tick(&self, tick: f64) -> f64 {
        self.sample_at_quarter_notes(tick / f64::from(TICKS_PER_QUARTER))
    }

    /// Ticks from the timeline start to `sample`.
    pub fn tick_at(&self, sample: f64) -> f64 {
        self.quarter_notes_at(sample) * f64::from(TICKS_PER_QUARTER)
    }

    /// The tick where beat `beat` of bar `bar` starts (both from 0). Worked out from the
    /// segment's whole beats rather than through samples, so it is exact: a bar line is
    /// always a whole tick. A beat past the end of its bar runs on into the next.
    pub fn tick_of_bar(&self, bar: i64, beat: u32) -> u64 {
        let s = self.segment_of_bar(bar);
        let beats = (bar - s.bar) * i64::from(s.numerator) - i64::from(s.beat) + i64::from(beat);
        let quarter_notes = s.quarter_notes + beats as f64 * s.beat_quarters();
        (quarter_notes * f64::from(TICKS_PER_QUARTER))
            .round()
            .max(0.0) as u64
    }

    /// The tempo and meter at `sample`, as the engine hands them to plug-ins.
    pub fn meter_at(&self, sample: i64) -> Meter {
        let s = self.segment_at(sample as f64);
        Meter {
            sample_rate: self.sample_rate,
            tempo: s.tempo,
            numerator: s.numerator,
            denominator: s.denominator,
            origin_sample: s.start,
            origin_quarter_notes: s.quarter_notes,
        }
    }

    /// The first sample of the next segment after `sample`, where the engine splits a block so
    /// a change lands on its exact sample.
    pub fn next_change_after(&self, sample: i64) -> Option<i64> {
        let i = self.segments.partition_point(|s| s.start <= sample as f64);
        self.segments.get(i).map(|s| s.start.ceil() as i64)
    }

    /// Snap a position to the grid. Lines run from each bar's start, `division` apart, and
    /// the next bar's start is a line too, so odd meters and triplets restart every bar.
    pub fn snap(&self, position: u64, division: Division, rounding: Rounding) -> u64 {
        let (bar, _) = self.bar_at(position as f64);
        if let Division::Bars(n @ 2..) = division {
            let (a, b) = Self::bar_pair(bar, n);
            let (a, b) = (self.bar_start(a), self.bar_start(b));
            let at = Self::pick(position as f64, a, b, rounding);
            return at.round().max(0.0) as u64;
        }
        let s = *self.segment_of_bar(bar);
        let bar_quarters = f64::from(s.numerator) * s.beat_quarters();
        let step = match division {
            Division::Bars(_) => bar_quarters,
            Division::Beat => s.beat_quarters(),
            Division::Note { .. } => division.quarters().unwrap_or(1.0),
        };
        let origin = self.quarter_notes_at(self.bar_start(bar));
        let into = self.quarter_notes_at(position as f64) - origin;
        let lines = into / step;
        let line = match rounding {
            Rounding::Nearest => lines.round(),
            Rounding::Previous => (lines + 1e-9).floor(),
            Rounding::Next => (lines - 1e-9).ceil(),
        };
        let mut quarters = (line * step).min(bar_quarters);
        // A bar need not hold a whole number of steps: its end may be the nearest line.
        if rounding == Rounding::Nearest && (bar_quarters - into).abs() < (quarters - into).abs() {
            quarters = bar_quarters;
        }
        self.sample_at_quarter_notes(origin + quarters)
            .round()
            .max(0.0) as u64
    }

    /// Where a segment starts, and how long its beats are, in ticks.
    fn segment_ticks(s: &Segment) -> (u64, u64) {
        let q = f64::from(TICKS_PER_QUARTER);
        let start = (s.quarter_notes * q).round().max(0.0) as u64;
        let beat = (s.beat_quarters() * q).round().max(1.0) as u64;
        (start, beat)
    }

    /// The bar (from 0) that `tick` falls in, worked out in ticks rather than through
    /// samples, so a tick on a bar line is always in the bar it starts.
    pub fn bar_of_tick(&self, tick: u64) -> i64 {
        let i = self
            .segments
            .partition_point(|s| Self::segment_ticks(s).0 <= tick);
        let s = &self.segments[i.saturating_sub(1)];
        let (start, beat) = Self::segment_ticks(s);
        let beats = i64::from(s.beat) + ((tick.saturating_sub(start)) / beat) as i64;
        s.bar + beats.div_euclid(i64::from(s.numerator))
    }

    /// Snap `tick` to the grid, in ticks: lines run from each bar's start, `division` apart,
    /// and the next bar's start is a line too, as in [`Self::snap`]. Exact for every meter
    /// with a power-of-two lower number; a division that doesn't split a quarter note into
    /// whole ticks (1/7…) rounds to the nearest tick.
    pub fn snap_tick(&self, tick: u64, division: Division, rounding: Rounding) -> u64 {
        let bar = self.bar_of_tick(tick);
        if let Division::Bars(n @ 2..) = division {
            let (a, b) = Self::bar_pair(bar, n);
            let (a, b) = (self.tick_of_bar(a, 0), self.tick_of_bar(b, 0));
            return Self::pick(tick as f64, a as f64, b as f64, rounding) as u64;
        }
        let origin = self.tick_of_bar(bar, 0);
        let bar_len = self.tick_of_bar(bar + 1, 0).saturating_sub(origin).max(1) as f64;
        let s = self.segment_of_bar(bar);
        let step = match division {
            Division::Bars(_) => bar_len,
            Division::Beat => Self::segment_ticks(s).1 as f64,
            Division::Note { .. } => {
                f64::from(TICKS_PER_QUARTER) * division.quarters().unwrap_or(1.0)
            }
        };
        let into = tick.saturating_sub(origin) as f64;
        let lines = into / step;
        let line = match rounding {
            Rounding::Nearest => lines.round(),
            Rounding::Previous => (lines + 1e-9).floor(),
            Rounding::Next => (lines - 1e-9).ceil(),
        };
        let mut at = (line * step).min(bar_len);
        // A bar need not hold a whole number of steps: its end may be the nearest line.
        if rounding == Rounding::Nearest && (bar_len - into).abs() < (at - into).abs() {
            at = bar_len;
        }
        origin + at.round() as u64
    }

    /// Every `n`-th bar (counted from bar 0) at or before `bar`, and the next one.
    fn bar_pair(bar: i64, n: u32) -> (i64, i64) {
        let a = bar - bar.rem_euclid(i64::from(n));
        (a, a + i64::from(n))
    }

    /// `a` or `b` (`a <= at < b`), as `rounding` picks between them.
    fn pick(at: f64, a: f64, b: f64, rounding: Rounding) -> f64 {
        match rounding {
            Rounding::Previous => a,
            Rounding::Next if at > a => b,
            Rounding::Next => a,
            Rounding::Nearest if at - a <= b - at => a,
            Rounding::Nearest => b,
        }
    }

    /// Where bar `bar` starts, how long it is and how long its beats are, in ticks.
    fn bar_ticks(&self, bar: i64) -> (u64, u64, u64) {
        let origin = self.tick_of_bar(bar, 0);
        let len = self.tick_of_bar(bar + 1, 0).saturating_sub(origin).max(1);
        (origin, len, Self::segment_ticks(self.segment_of_bar(bar)).1)
    }

    /// The division `grid` puts in bar `bar`: a fixed one as it is; Auto's from the zoom
    /// there, `x` placing a song tick on screen (in points, from any origin), so a bar
    /// played slower (wider on a timeline in samples) may get a finer division than a faster
    /// one.
    pub fn division_in(&self, grid: Grid, bar: i64, x: &dyn Fn(f64) -> f64) -> Division {
        match grid {
            Grid::Fixed(division) => division,
            Grid::Auto => {
                let (origin, len, beat) = self.bar_ticks(bar);
                let q = f64::from(TICKS_PER_QUARTER);
                let width = x((origin + len) as f64) - x(origin as f64);
                let bar_quarters = len as f64 / q;
                auto_division(width / bar_quarters, beat as f64 / q, bar_quarters)
            }
        }
    }

    /// Snap a sample position to the song's grid `grid`: to the division in effect in the bar
    /// it falls in ([`Self::division_in`], `x` giving the zoom for Auto).
    pub fn snap_grid(
        &self,
        position: u64,
        grid: Grid,
        x: &dyn Fn(f64) -> f64,
        rounding: Rounding,
    ) -> u64 {
        let bar = self.bar_at(position as f64).0;
        self.snap(position, self.division_in(grid, bar, x), rounding)
    }

    /// [`Self::snap_grid`] in ticks.
    pub fn snap_grid_tick(
        &self,
        tick: u64,
        grid: Grid,
        x: &dyn Fn(f64) -> f64,
        rounding: Rounding,
    ) -> u64 {
        let bar = self.bar_of_tick(tick);
        self.snap_tick(tick, self.division_in(grid, bar, x), rounding)
    }

    /// The grid's lines from song tick `first` to `last`, in order, for drawing: each bar's
    /// start, its beats, and the lines of the division in effect in it
    /// ([`Self::division_in`]) from the bar's start, so they restart every bar as snapping
    /// does (a bar that doesn't hold a whole number of them has a shorter last one). A line
    /// on a beat is a beat, on a bar's start a bar. `x` places a tick on screen (points);
    /// a level whose lines would be under [`MIN_GAP`] apart there is thinned: the division
    /// doubled until they aren't (straight divisions reach the beats, and add nothing past
    /// them), the beats left out (and the division with them), the bars kept every 2nd,
    /// 4th… (Auto's "N bars": every N-th, at least).
    pub fn grid_lines(
        &self,
        first: u64,
        last: u64,
        grid: Grid,
        x: &dyn Fn(f64) -> f64,
    ) -> Vec<GridLine> {
        let mut out = Vec::new();
        let apart = |a: f64, b: f64| x(b) - x(a) >= MIN_GAP;
        let mut bar = self.bar_of_tick(first);
        // Bounded, should a view ask for more than could ever be told apart.
        while out.len() < 8192 {
            let (origin, len, beat) = self.bar_ticks(bar);
            if origin > last {
                break;
            }
            let division = self.division_in(grid, bar, x);
            let mut every = match division {
                Division::Bars(n) => i64::from(n.max(1)),
                _ => 1,
            };
            while every < 1 << 20 && !apart(origin as f64, self.tick_of_bar(bar + every, 0) as f64)
            {
                every *= 2;
            }
            if bar.rem_euclid(every) != 0 {
                // Not one of the bars shown: on to the next that is.
                bar += every - bar.rem_euclid(every);
                continue;
            }
            if origin >= first {
                out.push(GridLine {
                    tick: origin,
                    kind: LineKind::Bar,
                });
            }
            if every > 1 {
                bar += every;
                continue;
            }
            let o = origin as f64;
            let beats = apart(o, o + beat as f64);
            let mut step = division
                .quarters()
                .map_or(f64::INFINITY, |q| q * f64::from(TICKS_PER_QUARTER));
            while step < len as f64 && !apart(o, o + step) {
                step *= 2.0;
            }
            let mut inside: Vec<GridLine> = Vec::new();
            if beats {
                inside.extend(
                    (1..)
                        .map(|k| k * beat)
                        .take_while(|t| *t < len)
                        .map(|t| GridLine {
                            tick: origin + t,
                            kind: LineKind::Beat,
                        }),
                );
                // Lines on beats are beats; a division coarser than the beat adds lines
                // only where it falls between them (a triplet's).
                if step < len as f64 {
                    inside.extend(
                        (1..)
                            .map(|k| (f64::from(k) * step).round() as u64)
                            .take_while(|t| *t < len)
                            .filter(|t| t % beat != 0)
                            .map(|t| GridLine {
                                tick: origin + t,
                                kind: LineKind::Division,
                            }),
                    );
                }
            }
            inside.sort_unstable_by_key(|l| l.tick);
            out.extend(
                inside
                    .into_iter()
                    .filter(|l| (first..=last).contains(&l.tick)),
            );
            bar += 1;
        }
        out
    }

    /// The time signature of bar `bar` (from 0). A meter only changes at a bar's start, so
    /// it holds for the whole bar.
    pub fn meter_of_bar(&self, bar: i64) -> (u32, u32) {
        let s = self.segment_of_bar(bar);
        (s.numerator, s.denominator)
    }

    /// How long a beat of bar `bar` lasts, in ticks (480 in 6/8, 960 in 4/4).
    pub fn beat_ticks(&self, bar: i64) -> u64 {
        self.bar_ticks(bar).2
    }

    /// Where `tick` falls as bar, beat and ticks into the beat (all from 0), worked out in
    /// ticks, so it round-trips with [`Self::tick_of_bar`] exactly: a tempo change moves no
    /// tick, only a meter change moves the bar lines.
    pub fn bar_beat_tick(&self, tick: u64) -> (i64, u32, u64) {
        let bar = self.bar_of_tick(tick);
        let (origin, _, beat) = self.bar_ticks(bar);
        let numerator = u64::from(self.meter_of_bar(bar).0);
        let into = tick.saturating_sub(origin);
        // A bar is a whole number of beats; the clamp only guards odd lower numbers.
        let beats = (into / beat).min(numerator.saturating_sub(1));
        (bar, beats as u32, into - beats * beat)
    }

    /// Bar starts (0-based bar, sample) from the bar containing `from` up to `to`.
    pub fn bars_between(&self, from: f64, to: f64) -> impl Iterator<Item = (i64, f64)> + '_ {
        let (first, _) = self.bar_at(from);
        (first..)
            .map(|bar| (bar, self.bar_start(bar)))
            .take_while(move |(_, at)| *at <= to)
    }

    /// Beats (index, sample) from the beat at or before `from` up to `to`.
    pub fn beats_between(&self, from: f64, to: f64) -> impl Iterator<Item = (i64, f64)> + '_ {
        let first = self.beats_at(from).floor() as i64;
        (first..)
            .map(|beat| (beat, self.sample_of_beat(beat as f64)))
            .take_while(move |(_, at)| *at <= to)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::GRID_NOTES;

    /// 4/4 at 120 for two bars, then 3/4 at 90 from bar 2, then 180 from bar 4 beat 1.
    fn changing() -> TempoMap {
        let changes = [
            TempoChange {
                bar: 2,
                beat: 0,
                tempo: Some(90.0),
                time_signature: Some((3, 4)),
            },
            TempoChange {
                bar: 4,
                beat: 1,
                tempo: Some(180.0),
                time_signature: None,
            },
        ];
        TempoMap::build(48_000, 120.0, (4, 4), &changes)
    }

    #[test]
    fn bars_and_beats_are_exact_ticks_in_any_meter() {
        let q = u64::from(TICKS_PER_QUARTER);
        // 4/4 bars of 4 quarter notes, then 3/4 from bar 2 (the tempo change mid-bar 4
        // doesn't move bar lines in ticks).
        let map = changing();
        assert_eq!(map.tick_of_bar(1, 0), 4 * q);
        assert_eq!(map.tick_of_bar(2, 0), 8 * q);
        assert_eq!(map.tick_of_bar(3, 2), 11 * q + 2 * q);
        assert_eq!(map.tick_of_bar(5, 0), 8 * q + 3 * 3 * q);
        // 6/8: a beat is an eighth, half a quarter note; a bar is 3 quarter notes.
        let six_eight = TempoMap::constant(48_000, 120.0, 6, 8);
        assert_eq!(six_eight.tick_of_bar(1, 0), 3 * q);
        assert_eq!(six_eight.tick_of_bar(1, 5), 3 * q + 5 * q / 2);
        // And the exact tick lands on the bar line in samples.
        for bar in 0..8 {
            let sample = map.sample_at_tick(map.tick_of_bar(bar, 0) as f64);
            assert!((sample - map.bar_start(bar)).abs() < 1e-6, "bar {bar}");
        }
    }

    #[test]
    fn snapping_in_ticks_is_exact_in_any_meter() {
        let q = u64::from(TICKS_PER_QUARTER);
        let map = changing(); // 4/4 for bars 0-1, then 3/4
        let sixteenth = "1/16".parse().unwrap();
        let near = |t, d| map.snap_tick(t, d, Rounding::Nearest);
        for bar in 0..6 {
            let line = map.tick_of_bar(bar, 0);
            assert_eq!(near(line, sixteenth), line, "a bar line stays on it");
            // One tick short of a bar line snaps forward onto it.
            assert_eq!(near(line + 4 * q, Division::Bars(1)).min(line), line);
            if bar > 0 {
                assert_eq!(near(line - 1, sixteenth), line, "bar {bar}");
                assert_eq!(map.bar_of_tick(line), bar);
                assert_eq!(map.bar_of_tick(line - 1), bar - 1);
            }
        }
        // Across the change to 3/4: bar 2 starts at 8 quarter notes, bar 3 at 11.
        assert_eq!(near(10 * q + 100, Division::Bars(1)), 11 * q);
        // Sixteenths are 240 ticks: 130 past a line is nearer the next one.
        assert_eq!(near(9 * q + 130, sixteenth), 9 * q + 240);
        assert_eq!(
            map.snap_tick(9 * q + 130, sixteenth, Rounding::Previous),
            9 * q
        );
        // Eighth-note triplets in 6/8 (320 ticks), restarting each 3-quarter bar.
        let six_eight = TempoMap::constant(48_000, 120.0, 6, 8);
        let triplet = "1/8t".parse().unwrap();
        assert_eq!(
            six_eight.snap_tick(3 * q + 330, triplet, Rounding::Nearest),
            3 * q + 320
        );
        assert_eq!(
            six_eight.snap_tick(6 * q - 10, triplet, Rounding::Nearest),
            6 * q
        );
        assert_eq!(
            six_eight.snap_tick(500, Division::Beat, Rounding::Nearest),
            480
        );
    }

    #[test]
    fn ticks_follow_tempo_changes_in_samples() {
        let q = f64::from(TICKS_PER_QUARTER);
        let map = changing();
        // A note 9 quarter notes in sits after the change to 90 at bar 2 (quarter note 8):
        // 8 quarter notes at 120 (24000 each), then one at 90 (32000).
        assert_eq!(map.sample_at_tick(9.0 * q), 192_000.0 + 32_000.0);
        assert!((map.tick_at(224_000.0) - 9.0 * q).abs() < 1e-9);
        // Halve the opening tempo: the same tick moves later; the tick itself doesn't change.
        let slower = TempoMap::build(48_000, 60.0, (4, 4), &[]);
        assert_eq!(slower.sample_at_tick(4.0 * q), 192_000.0);
    }

    #[test]
    fn segments_start_where_the_bars_before_them_end() {
        let map = changing();
        let s = map.segments();
        assert_eq!(s.len(), 3);
        // Two 4/4 bars at 120: 8 beats of 24000.
        assert_eq!(
            (s[1].start, s[1].beats, s[1].quarter_notes),
            (192_000.0, 8, 8.0)
        );
        // Two 3/4 bars at 90 (32000 a beat), then one beat more: 7 beats.
        assert_eq!((s[2].start, s[2].beats), (192_000.0 + 7.0 * 32_000.0, 15));
        assert_eq!((s[2].bar, s[2].beat, s[2].numerator), (4, 1, 3));
    }

    #[test]
    fn samples_quarter_notes_and_bars_round_trip_across_changes() {
        let map = changing();
        for sample in [
            0.0, 1.0, 95_999.0, 192_000.0, 200_000.0, 416_000.0, 500_000.0, -40_000.0,
        ] {
            let q = map.quarter_notes_at(sample);
            assert!(
                (map.sample_at_quarter_notes(q) - sample).abs() < 1e-6,
                "{sample}"
            );
            let b = map.beats_at(sample);
            assert!((map.sample_of_beat(b) - sample).abs() < 1e-6, "{sample}");
        }
        assert_eq!(map.bar_start(1), 96_000.0);
        assert_eq!(map.bar_start(2), 192_000.0);
        assert_eq!(
            map.bar_start(3),
            192_000.0 + 96_000.0,
            "a 3/4 bar at 90 is 96000"
        );
        // Bar 4's first beat is still at 90; 180 starts on its second beat.
        assert_eq!(
            map.bar_start(5),
            192_000.0 + 2.0 * 96_000.0 + 32_000.0 + 2.0 * 16_000.0
        );
        assert_eq!(map.bar_at(200_000.0), (2, 0.25));
        // Before 0 the opening extends: a count-in bar.
        assert_eq!(map.bar_start(-1), -96_000.0);
        assert_eq!(map.bar_at(-24_000.0), (-1, 3.0));
    }

    #[test]
    fn downbeats_follow_the_meter() {
        let map = changing();
        let downbeats: Vec<i64> = (0..18).filter(|&b| map.beat_in_bar(b) == 0).collect();
        // 4/4 bars at 0 and 4; 3/4 bars at 8, 11, 14; 17 after bar 4's tempo change.
        assert_eq!(downbeats, vec![0, 4, 8, 11, 14, 17]);
        assert_eq!(map.beat_in_bar(-1), 3, "the beat before 0 ends a 4/4 bar");
    }

    #[test]
    fn plug_ins_get_the_segment_at_the_block() {
        let map = changing();
        let m = map.meter_at(200_000);
        assert_eq!((m.tempo, m.numerator, m.denominator), (90.0, 3, 4));
        assert!((m.quarter_notes(200_000) - map.quarter_notes_at(200_000.0)).abs() < 1e-9);
        assert_eq!(map.next_change_after(0), Some(192_000));
        assert_eq!(map.next_change_after(192_000), Some(416_000));
        assert_eq!(map.next_change_after(416_000), None);
    }

    #[test]
    fn snapping_restarts_every_bar() {
        let map = changing();
        let near = |p, d: &str| map.snap(p, d.parse().unwrap(), Rounding::Nearest);
        // 287000 is 2.97 quarter notes into a 3-quarter bar: the next bar's start (288000)
        // beats the last half-note line inside it (256000).
        assert_eq!(near(30_000, "beat"), 24_000);
        assert_eq!(near(190_000, "bar"), 192_000);
        // In the 3/4 bar at 90, beats are 32000 apart from 192000.
        assert_eq!(near(240_000, "beat"), 256_000);
        assert_eq!(near(230_000, "1/8"), 224_000);
        // Past the last line of a bar, the next bar's start is the line.
        assert_eq!(near(287_000, "1/2"), 288_000);
        assert_eq!(
            map.snap(193_000, "beat".parse().unwrap(), Rounding::Previous),
            192_000
        );
    }

    #[test]
    fn a_constant_map_matches_the_old_grid() {
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        assert_eq!(map.bar_start(3), 288_000.0);
        assert_eq!(
            map.snap(6_100, "1/16".parse().unwrap(), Rounding::Nearest),
            6_000
        );
        // 6/8 at 60 quarter notes: an eighth-note beat is half a second.
        let map = TempoMap::constant(48_000, 60.0, 6, 8);
        assert_eq!(map.sample_of_beat(1.0), 24_000.0);
        assert_eq!(map.bar_start(1), 144_000.0);
    }

    /// The grid's own points in the first `bars` bars for `step` ticks (from each bar's
    /// start, every step short of its end), and the beats' ticks.
    fn points(map: &TempoMap, bars: i64, step: f64) -> (Vec<u64>, Vec<u64>) {
        let (mut lines, mut beats) = (Vec::new(), Vec::new());
        for bar in 0..bars {
            let (origin, len, beat) = map.bar_ticks(bar);
            lines.extend(
                (0..)
                    .map(|k| (f64::from(k) * step).round() as u64)
                    .take_while(|t| *t < len)
                    .map(|t| origin + t),
            );
            beats.extend(
                (0..)
                    .map(|k| k * beat)
                    .take_while(|t| *t < len)
                    .map(|t| origin + t),
            );
        }
        (lines, beats)
    }

    #[test]
    fn every_division_draws_and_snaps_to_the_same_lines_across_tempo_and_meter_changes() {
        let maps = [
            ("4/4 to 3/4, 120 to 90 to 180", changing()),
            ("7/8 at 100", TempoMap::constant(48_000, 100.0, 7, 8)),
            ("6/8 at 60", TempoMap::constant(44_100, 60.0, 6, 8)),
        ];
        // A point a tick: nothing is thinned.
        let one_to_one = |t: f64| t;
        for (name, map) in &maps {
            let bars = 8;
            let end = map.tick_of_bar(bars, 0);
            for n in GRID_NOTES {
                for triplet in [false, true] {
                    let division = Division::Note { n, triplet };
                    let grid = Grid::Fixed(division);
                    let step = division.quarters().unwrap() * f64::from(TICKS_PER_QUARTER);
                    let (want, beats) = points(map, bars, step);
                    let drawn = map.grid_lines(0, end - 1, grid, &one_to_one);
                    let ticks: Vec<u64> = drawn.iter().map(|l| l.tick).collect();
                    let mut all: Vec<u64> = want.iter().chain(&beats).copied().collect();
                    all.sort_unstable();
                    all.dedup();
                    assert_eq!(ticks, all, "{name}, {division}: the lines drawn");
                    for line in &drawn {
                        let (origin, _, beat) = map.bar_ticks(map.bar_of_tick(line.tick));
                        let kind = if line.tick == origin {
                            LineKind::Bar
                        } else if (line.tick - origin) % beat == 0 {
                            LineKind::Beat
                        } else {
                            LineKind::Division
                        };
                        assert_eq!(line.kind, kind, "{name}, {division}: tick {}", line.tick);
                    }
                    // Snapping lands on these points (the next bar's start is one too), in
                    // ticks and in samples through the tempo map.
                    let mut targets = want.clone();
                    targets.push(end);
                    let sample = |t: u64| map.sample_at_tick(t as f64).round() as u64;
                    for pair in targets.windows(2) {
                        let (a, b) = (pair[0], pair[1]);
                        let near = |t| map.snap_grid_tick(t, grid, &one_to_one, Rounding::Nearest);
                        assert_eq!(near(a + (b - a) / 3), a, "{name}, {division}: {a}..{b}");
                        assert_eq!(near(b - (b - a) / 3), b, "{name}, {division}: {a}..{b}");
                        let between = sample(a) + (sample(b) - sample(a)) / 3;
                        let snapped = map.snap_grid(between, grid, &one_to_one, Rounding::Nearest);
                        assert!(
                            snapped.abs_diff(sample(a)) <= 1,
                            "{name}, {division}: sample {between} snapped to {snapped}, not {}",
                            sample(a)
                        );
                    }
                }
            }
        }
        // By hand: 7/8 at 1/4 has lines at 0, 1, 2 and 3 quarter notes, then the next bar's
        // start at 3.5; 6/8 at 1/8 is its beats, and at 1/16 has a line between each.
        let q = u64::from(TICKS_PER_QUARTER);
        let seven = TempoMap::constant(48_000, 100.0, 7, 8);
        let quarter = Grid::Fixed(Division::note(4));
        let near = |t| seven.snap_grid_tick(t, quarter, &one_to_one, Rounding::Nearest);
        assert_eq!(near(3 * q + q / 3), 7 * q / 2);
        assert_eq!(near(3 * q + q / 5), 3 * q);
        let six = TempoMap::constant(48_000, 60.0, 6, 8);
        let kinds = |grid| {
            six.grid_lines(0, 3 * q - 1, grid, &one_to_one)
                .iter()
                .map(|l| l.kind)
                .collect::<Vec<_>>()
        };
        use LineKind::{Bar, Beat};
        assert_eq!(
            kinds(Grid::Fixed(Division::note(8))),
            [Bar, Beat, Beat, Beat, Beat, Beat]
        );
        let sixteenths = kinds(Grid::Fixed(Division::note(16)));
        assert_eq!(sixteenths.len(), 12);
        assert_eq!(sixteenths[1], LineKind::Division);
    }

    #[test]
    fn lines_too_close_to_tell_apart_are_thinned_a_level_at_a_time() {
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        let grid = Grid::Fixed(Division::note(64));
        let spacing = |ticks_per_point: f64, kind| {
            let x = move |t: f64| t / ticks_per_point;
            let lines = map.grid_lines(0, 16 * 4 * 960, grid, &x);
            let of: Vec<u64> = lines
                .iter()
                .filter(|l| l.kind == kind)
                .map(|l| l.tick)
                .collect();
            (of.len(), of.windows(2).map(|w| w[1] - w[0]).min())
        };
        // A sixty-fourth (60 ticks) at 3 points: drawn as thirty-seconds, 6 points apart.
        assert_eq!(spacing(20.0, LineKind::Division).1, Some(120));
        // At 80 ticks a point, eighths (1/64 doubled three times: 480 ticks, 6 points), so
        // the division's lines are the off-beat eighths between the beats.
        assert_eq!(spacing(80.0, LineKind::Division), (64, Some(960)));
        assert_eq!(spacing(80.0, LineKind::Beat), (48, Some(960)));
        // Beats under 6 points apart: neither they nor any division, but every bar.
        assert_eq!(spacing(200.0, LineKind::Division).0, 0);
        assert_eq!(spacing(200.0, LineKind::Beat).0, 0);
        assert_eq!(spacing(200.0, LineKind::Bar).1, Some(4 * 960));
        // Bars 3.84 points apart: every other one.
        assert_eq!(spacing(1000.0, LineKind::Bar).1, Some(8 * 960));
    }

    #[test]
    fn auto_picks_the_division_from_the_zoom_and_snaps_to_it() {
        // A timeline in samples (`spp` a point): a slower bar is wider.
        let map = &changing();
        let at = |spp: f64| move |t: f64| map.sample_at_tick(t) / spp;
        let auto = |spp: f64, bar| map.division_in(Grid::Auto, bar, &at(spp));
        // At 120 a sixteenth is 6000 samples: 15 points at 400 samples a point (a
        // thirty-second, 7.5, is under 14).
        assert_eq!(auto(400.0, 0), Division::note(16));
        // In the 3/4 bars at 90 a sixteenth is 8000 samples, 20 points; a thirty-second 10.
        assert_eq!(auto(400.0, 2), Division::note(16));
        // At 180 (bar 5) a sixteenth is 4000 samples, 10 points: eighths.
        assert_eq!(auto(400.0, 5), Division::note(8));
        // Zoomed in, finer; zoomed out, coarser, past the beat to bars.
        assert_eq!(auto(100.0, 0), Division::note(64));
        assert_eq!(auto(200.0, 0), Division::note(32));
        assert_eq!(auto(1_600.0, 0), Division::note(4));
        assert_eq!(auto(3_200.0, 0), Division::Bars(1));
        assert_eq!(auto(24_000.0, 0), Division::Bars(4));
        // A beat longer than a quarter note (2/2) comes between 1/4 and the bar.
        let cut = TempoMap::constant(48_000, 120.0, 2, 2);
        let x = |t: f64| cut.sample_at_tick(t) / 2_400.0;
        assert_eq!(cut.division_in(Grid::Auto, 0, &x), Division::Beat);

        // Snapping follows the division shown: 6800 is nearest 6000 on sixteenths, 7500 on
        // sixty-fourths (1500 apart).
        let snap = |p, spp: f64| map.snap_grid(p, Grid::Auto, &at(spp), Rounding::Nearest);
        assert_eq!(snap(6_800, 400.0), 6_000);
        assert_eq!(snap(6_800, 100.0), 7_500);
        // At 180, on eighths (8000 samples): 4100 past bar 5's start goes to the eighth.
        let bar5 = map.bar_start(5) as u64;
        assert_eq!(bar5, 448_000);
        assert_eq!(snap(bar5 + 4_100, 400.0), bar5 + 8_000);
        // Every fourth bar: just past bar 3's start (288000) goes to bar 4's (384000).
        assert_eq!(snap(290_000, 24_000.0), 384_000);
        assert_eq!(snap(150_000, 24_000.0), 0);
        // The lines drawn are Auto's, nothing between: every fourth of the 4 point bars at
        // the start, then every eighth once the 3/4 bars from bar 4 are under 4 points.
        let lines = map.grid_lines(0, map.tick_of_bar(17, 0), Grid::Auto, &at(24_000.0));
        let bars: Vec<i64> = lines.iter().map(|l| map.bar_of_tick(l.tick)).collect();
        assert_eq!(bars, [0, 8, 16]);
        assert!(lines.iter().all(|l| l.kind == LineKind::Bar));
    }
}
