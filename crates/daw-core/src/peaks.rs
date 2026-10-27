//! A file's waveform at every zoom (23.2): its min/max peaks at several resolutions, and what
//! a view draws of a span of it at a zoom.
//!
//! A view draws one min/max column per point while a point covers a sample or more, and the
//! samples themselves (a line, with dots once they are a few points apart) when it covers
//! less. The column's min and max come from the coarsest data that still resolves the point:
//! the samples below [`FINE_BLOCK`] samples a point, the 16-sample bins below
//! [`PEAK_BLOCK`], and the 256-sample bins above it, which the coarser levels (1024 to 65,536
//! samples a bin, each folded from four of the one below) only speed up: a run of whole
//! coarse bins stands in for the 256 bins it holds, so the column is exactly the 256 bins'
//! fold. That keeps a clip's look at 256 samples a point and wider the one a
//! take had while it recorded (21.1: a take has only 256-sample bins, [`Wave::of_peaks`]),
//! so nothing jumps when the clip takes the take's place.
//!
//! The Multitrack (clips and recording takes) and the Waveform view both lay out through
//! [`layout`], so they draw alike at every zoom; painting (gains, colour, meshes) is the app's.
//! Nothing here reads a file: the levels are built with the decoded audio
//! ([`crate::media::AudioData::new`], on the loading worker), and a frame only folds bins.

use crate::media::PEAK_BLOCK;

/// Samples per bin of the finest level. A point covering fewer samples than this is drawn
/// from the samples themselves (at most this many per point, so a frame stays cheap).
pub const FINE_BLOCK: usize = 16;
/// How many levels there are.
pub const LEVELS: usize = 6;
/// Each level's samples per bin, finest first; [`Wave::levels`] follows this order. Those
/// above [`PEAK_BLOCK`] only speed up wide points; four bins of one make a bin of the next, so
/// a column folds at most a few bins at each.
pub const BLOCKS: [usize; LEVELS] = [FINE_BLOCK, PEAK_BLOCK, 1024, 4096, 16_384, 65_536];
/// Below this many samples a point the samples are drawn as a line through them.
pub const LINE_BELOW: f64 = 1.0;
/// Below this many samples a point (each sample 4 points or more from the next) the line
/// gets a dot on each sample.
pub const DOTS_BELOW: f64 = 0.25;

/// How close (in samples) a sample may sit to a point's edge and still count as on it: the
/// edges are computed in floating point, and a sample exactly on one must fall on the same
/// side in every frame and every view.
const EDGE: f64 = 1e-6;

/// The first whole sample at or after `s` (with [`EDGE`]'s slack).
fn at_or_after(s: f64) -> f64 {
    (s - EDGE).ceil()
}

/// The last whole sample at or before `s` (with [`EDGE`]'s slack).
fn at_or_before(s: f64) -> f64 {
    (s + EDGE).floor()
}

/// One bin's `(min, max)`.
pub type Peak = (f32, f32);

/// Nothing folded yet: `min > max`.
const EMPTY: Peak = (f32::MAX, f32::MIN);

fn union(a: Peak, b: Peak) -> Peak {
    (a.0.min(b.0), a.1.max(b.1))
}

fn fold_bins(acc: Peak, bins: &[Peak]) -> Peak {
    bins.iter().fold(acc, |acc, &b| union(acc, b))
}

fn fold_samples(acc: Peak, samples: &[f32]) -> Peak {
    samples
        .iter()
        .fold(acc, |(lo, hi), &s| (lo.min(s), hi.max(s)))
}

/// Every level of [`BLOCKS`] for one channel, in one pass over the samples: the 16-sample
/// bins from the samples, each coarser level folded from the one below. A bin's min and max
/// are exactly its samples' (the last bin of a level may be short).
pub fn build(samples: &[f32]) -> [Vec<Peak>; LEVELS] {
    let mut levels: [Vec<Peak>; LEVELS] = Default::default();
    levels[0] = samples
        .chunks(FINE_BLOCK)
        .map(|c| fold_samples(EMPTY, c))
        .collect();
    for l in 1..LEVELS {
        levels[l] = levels[l - 1]
            .chunks(BLOCKS[l] / BLOCKS[l - 1])
            .map(|c| fold_bins(EMPTY, c))
            .collect();
    }
    levels
}

/// What a view draws from at a zoom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// The samples (min/max per point, or the samples themselves below one sample a point).
    Samples,
    /// The bins of `BLOCKS[i]`.
    Level(usize),
}

/// One channel's waveform as far as it is known: its samples and its levels, borrowed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Wave<'a> {
    /// The samples, or empty when they aren't in memory (a take still recording).
    pub samples: &'a [f32],
    /// The bins of each of [`BLOCKS`]; an empty slice is a level it doesn't have. A coarser
    /// level, when there, must be folded from the one below it (as [`build`] makes them).
    pub levels: [&'a [Peak]; LEVELS],
}

impl<'a> Wave<'a> {
    /// A take still recording (21.1): only its 256-sample bins, as the audio thread sends them.
    pub fn of_peaks(peaks: &'a [Peak]) -> Self {
        Self {
            samples: &[],
            levels: [&[], peaks, &[], &[], &[], &[]],
        }
    }

    /// Samples it covers: the samples', or its finest level's bins'.
    pub fn len(&self) -> usize {
        if !self.samples.is_empty() {
            return self.samples.len();
        }
        (0..BLOCKS.len())
            .find(|&l| !self.levels[l].is_empty())
            .map_or(0, |l| self.levels[l].len() * BLOCKS[l])
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// What a view at `spp` samples a point draws from: the samples below [`FINE_BLOCK`] (and
    /// below one sample a point, where they are drawn one by one), the 16-sample bins below
    /// [`PEAK_BLOCK`], the 256-sample bins from there on. When that isn't there, the next
    /// coarser level that is (a take's 256 bins, drawn as blocks zoomed in); `None` when it
    /// has nothing at all.
    pub fn source(&self, spp: f64) -> Option<Source> {
        let wanted = if spp < FINE_BLOCK as f64 {
            if !self.samples.is_empty() {
                return Some(Source::Samples);
            }
            0
        } else if spp < PEAK_BLOCK as f64 {
            0
        } else {
            1
        };
        // The coarse levels only speed up the 256 one; never what a point resolves.
        (wanted..2)
            .find(|&l| !self.levels[l].is_empty())
            .map(Source::Level)
            .or_else(|| (!self.samples.is_empty()).then_some(Source::Samples))
    }

    /// Min and max over `source`'s data covering samples `first..end`: the samples in it, or
    /// every bin that overlaps it, folded. Runs of whole bins of a coarser level (when there)
    /// are folded as those bins, which gives the same answer for far less work. `None` when
    /// the range holds nothing.
    pub fn min_max(&self, source: Source, first: usize, end: usize) -> Option<Peak> {
        let (lo, hi) = match source {
            Source::Samples => {
                let end = end.min(self.samples.len());
                if first >= end {
                    return None;
                }
                fold_samples(EMPTY, &self.samples[first..end])
            }
            Source::Level(start) => {
                let block = BLOCKS[start];
                let mut level = start;
                let mut i = first / block;
                let mut j = end.div_ceil(block).min(self.levels[level].len());
                if i >= j {
                    return None;
                }
                let mut acc = EMPTY;
                loop {
                    let bins = self.levels[level];
                    let up = level + 1;
                    let ratio = BLOCKS.get(up).map_or(0, |b| b / BLOCKS[level]);
                    if up < BLOCKS.len() && !self.levels[up].is_empty() && j - i >= 2 * ratio {
                        // The ragged ends at this level, the whole groups between from the
                        // next (j - i >= 2 groups, so at least one whole group lies between).
                        let inner = i.div_ceil(ratio) * ratio;
                        let outer = j / ratio * ratio;
                        acc = fold_bins(acc, &bins[i..inner]);
                        acc = fold_bins(acc, &bins[outer..j]);
                        (i, j, level) = (inner / ratio, outer / ratio, up);
                    } else {
                        acc = fold_bins(acc, &bins[i..j]);
                        break;
                    }
                }
                acc
            }
        };
        (lo <= hi).then_some((lo, hi))
    }
}

/// Where a span of a file sits in a view, all in the file's own samples and the view's points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Place {
    /// Where the file's sample 0 is, in points (far off screen when zoomed in on a long file:
    /// kept in `f64`, so positions don't wobble).
    pub origin: f64,
    /// Samples a point (the view's zoom).
    pub spp: f64,
    /// The file's samples drawn, `from..to` (a clip's offset and its end in the file).
    pub from: u64,
    pub to: u64,
    /// The x range drawn (the visible part of the clip or lane), in points.
    pub left: f32,
    pub right: f32,
}

impl Place {
    /// The x of file sample `at`.
    pub fn x(&self, at: f64) -> f64 {
        self.origin + at / self.spp
    }

    /// The file sample at `x` (fractional).
    pub fn sample(&self, x: f64) -> f64 {
        (x - self.origin) * self.spp
    }
}

/// One point-wide column: x of its left edge (whole points), the file sample it starts at
/// (for gains that vary along a clip), and the min and max of the audio under it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Column {
    pub x: f32,
    pub at: u64,
    pub lo: f32,
    pub hi: f32,
}

/// One sample drawn as itself: where it is, which sample, its value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub at: u64,
    pub value: f32,
}

/// What to draw of one channel: columns, or points (a line, `dots` on them); the buffers are
/// kept between calls, so a frame allocates nothing once they have grown.
#[derive(Debug, Default)]
pub struct Layout {
    pub columns: Vec<Column>,
    pub points: Vec<Point>,
    pub dots: bool,
}

/// Lay out `wave`'s span at `place` into `out` (cleared first): below [`LINE_BELOW`] samples a
/// point, each sample from just before the left edge to just after the right one as a
/// [`Point`] (so the line runs in from off screen), with dots below [`DOTS_BELOW`]; otherwise
/// a [`Column`] per whole point the span covers, each over the samples whose positions fall in
/// it (`ceil(sample(x))..ceil(sample(x + 1))`: every sample in exactly one column), from
/// [`Wave::source`]'s data. A take zoomed in past its bins gets a column per point from the
/// bin under it: blocks, until its file is in.
pub fn layout(wave: &Wave, place: &Place, out: &mut Layout) {
    out.columns.clear();
    out.points.clear();
    out.dots = false;
    let spp = place.spp;
    let to = place.to.min(wave.len() as u64);
    // (A NaN zoom or edge draws nothing too.)
    let drawable = spp > 0.0 && place.right > place.left;
    if place.from >= to || !drawable {
        return;
    }
    let Some(source) = wave.source(spp) else {
        return;
    };
    let (from_f, to_f) = (place.from as f64, to as f64);
    if spp < LINE_BELOW && source == Source::Samples {
        let first = at_or_before(place.sample(f64::from(place.left))).max(from_f);
        let last = at_or_after(place.sample(f64::from(place.right))).min(to_f - 1.0);
        if first > last {
            return;
        }
        for at in first as u64..=last as u64 {
            out.points.push(Point {
                x: place.x(at as f64) as f32,
                at,
                value: wave.samples[at as usize],
            });
        }
        out.dots = spp < DOTS_BELOW;
        return;
    }
    // Only the whole points the span reaches, within the drawn range.
    let x0 = f64::from(place.left).floor().max(place.x(from_f).floor());
    let x1 = f64::from(place.right).ceil().min(place.x(to_f).ceil());
    let mut x = x0;
    while x < x1 {
        let s0 = place.sample(x);
        let s1 = place.sample(x + 1.0);
        let (mut first, mut end) = (at_or_after(s0).max(from_f), at_or_after(s1).min(to_f));
        if first >= end {
            // Narrower than a sample, drawn from bins (a take zoomed in): the sample under it.
            let under = at_or_before(s0);
            if under < from_f || under >= to_f {
                x += 1.0;
                continue;
            }
            (first, end) = (under, under + 1.0);
        }
        if let Some((lo, hi)) = wave.min_max(source, first as usize, end as usize) {
            out.columns.push(Column {
                x: x as f32,
                at: first as u64,
                lo,
                hi,
            });
        }
        x += 1.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A noisy signal with some structure, and a ragged length (no level ends on a whole bin).
    fn signal(len: usize) -> Vec<f32> {
        let mut state = 0x2545_f491_u32;
        (0..len)
            .map(|i| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                let noise = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
                0.6 * (i as f32 * 0.003).sin() + 0.3 * noise
            })
            .collect()
    }

    fn brute(samples: &[f32], first: usize, end: usize) -> Option<Peak> {
        let end = end.min(samples.len());
        (first < end).then(|| fold_samples(EMPTY, &samples[first..end]))
    }

    fn wave<'a>(samples: &'a [f32], levels: &'a [Vec<Peak>; LEVELS]) -> Wave<'a> {
        Wave {
            samples,
            levels: std::array::from_fn(|l| levels[l].as_slice()),
        }
    }

    #[test]
    fn every_level_is_the_brute_force_min_and_max_of_its_samples() {
        let samples = signal(3 * BLOCKS[LEVELS - 1] + 1234);
        let levels = build(&samples);
        for (l, &block) in BLOCKS.iter().enumerate() {
            assert_eq!(
                levels[l].len(),
                samples.len().div_ceil(block),
                "level {block}"
            );
            for (i, &bin) in levels[l].iter().enumerate() {
                let want = brute(&samples, i * block, (i + 1) * block).unwrap();
                assert_eq!(bin, want, "level {block} bin {i}");
            }
        }
        // The 256 level is the one the file's `peaks` always were (takes compare with it).
        let old: Vec<Peak> = samples
            .chunks(PEAK_BLOCK)
            .map(|c| fold_samples(EMPTY, c))
            .collect();
        assert_eq!(levels[1], old);
    }

    #[test]
    fn a_range_folds_exactly_the_bins_it_covers_whatever_levels_speed_it_up() {
        let samples = signal(9 * 4096 + 777);
        let levels = build(&samples);
        let full = wave(&samples, &levels);
        // The same 256 bins without the coarse levels: the slow, plain fold to compare with.
        let plain = Wave {
            samples: &samples,
            levels: [&levels[0], &levels[1], &[], &[], &[], &[]],
        };
        let mut state = 7u64;
        for _ in 0..2000 {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let first = (state >> 33) as usize % samples.len();
            let span = 1 + (state >> 13) as usize % (6 * 4096);
            let end = first + span;
            for (l, &block) in BLOCKS.iter().enumerate().take(2) {
                // Every bin overlapping first..end, folded from the samples themselves.
                let want = brute(&samples, first / block * block, end.div_ceil(block) * block);
                assert_eq!(full.min_max(Source::Level(l), first, end), want);
                assert_eq!(plain.min_max(Source::Level(l), first, end), want);
            }
            assert_eq!(
                full.min_max(Source::Samples, first, end),
                brute(&samples, first, end)
            );
        }
        // Past the end, or empty: nothing.
        assert_eq!(
            full.min_max(Source::Level(1), samples.len() + 300, samples.len() + 900),
            None
        );
        assert_eq!(full.min_max(Source::Samples, 10, 10), None);
    }

    #[test]
    fn the_level_follows_the_zoom() {
        let samples = signal(100_000);
        let levels = build(&samples);
        let full = wave(&samples, &levels);
        let cases = [
            (1.0 / 32.0, Source::Samples),
            (0.9, Source::Samples),
            (1.0, Source::Samples),
            (15.9, Source::Samples),
            (16.0, Source::Level(0)),
            (255.0, Source::Level(0)),
            (256.0, Source::Level(1)),
            (5000.0, Source::Level(1)),
            (96_000.0, Source::Level(1)),
        ];
        for (spp, want) in cases {
            assert_eq!(full.source(spp), Some(want), "{spp} samples a point");
        }
        // A take recording has only its 256 bins: those at every zoom (blocks when zoomed in).
        let take = Wave::of_peaks(&levels[1]);
        for spp in [1.0 / 32.0, 4.0, 100.0, 300.0, 50_000.0] {
            assert_eq!(take.source(spp), Some(Source::Level(1)), "{spp}");
        }
        assert_eq!(Wave::default().source(10.0), None);
    }

    fn place(spp: f64, from: u64, to: u64, width: f32) -> Place {
        Place {
            origin: 100.0 - from as f64 / spp,
            spp,
            from,
            to,
            left: 100.0,
            right: 100.0 + width,
        }
    }

    #[test]
    fn at_the_closest_zoom_the_points_are_the_samples() {
        let samples = signal(50_000);
        let levels = build(&samples);
        let full = wave(&samples, &levels);
        let mut out = Layout::default();
        for spp in [1.0 / 32.0, 1.0 / 8.0, 0.5] {
            // From sample 20,000 on, 900 points wide.
            let at = place(spp, 20_000, 50_000, 900.0);
            layout(&full, &at, &mut out);
            assert!(out.columns.is_empty());
            assert_eq!(out.dots, spp < DOTS_BELOW, "{spp}");
            let first = out.points[0].at;
            assert_eq!(first, 20_000, "the first sample at the left edge");
            for (k, p) in out.points.iter().enumerate() {
                assert_eq!(p.at, first + k as u64, "one point per sample, in order");
                assert_eq!(p.value, samples[p.at as usize], "the sample itself");
                assert!((f64::from(p.x) - at.x(p.at as f64)).abs() < 1e-3);
            }
            let last = out.points.last().unwrap();
            assert!(
                f64::from(last.x) >= 1000.0 - 1.0 / spp,
                "reaches the right edge"
            );
            assert_eq!(out.points.len() as f64, (900.0 * spp).ceil() + 1.0, "{spp}");
        }
        // Starting off screen to the left: the line runs in from the sample before the edge.
        let mut at = place(1.0 / 8.0, 0, 50_000, 400.0);
        at.origin = 100.0 - 30_000.5 * 8.0;
        layout(&full, &at, &mut out);
        assert_eq!(out.points[0].at, 30_000);
        assert!(out.points[0].x < 100.0);
    }

    #[test]
    fn columns_cover_every_sample_once_and_stop_at_the_span() {
        let samples = signal(200_000);
        let levels = build(&samples);
        let full = wave(&samples, &levels);
        let mut out = Layout::default();
        for spp in [1.0, 3.7, 15.0, 16.0, 100.0, 256.0, 900.0, 20_000.0] {
            let at = place(spp, 10_000, 150_000, 600.0);
            layout(&full, &at, &mut out);
            assert!(out.points.is_empty());
            let source = full.source(spp).unwrap();
            let mut next = 10_000;
            for c in &out.columns {
                assert_eq!(c.at, next, "{spp}: no sample skipped or drawn twice");
                let end = at_or_after(at.sample(f64::from(c.x) + 1.0)).min(150_000.0) as usize;
                assert_eq!(full.min_max(source, c.at as usize, end), Some((c.lo, c.hi)));
                if source == Source::Samples {
                    assert_eq!(brute(&samples, c.at as usize, end), Some((c.lo, c.hi)));
                }
                next = end as u64;
            }
            // Wide enough to reach the span's end within 600 points: it stops there.
            if 600.0 * spp > 140_000.0 {
                assert_eq!(next, 150_000, "{spp}");
            }
        }
    }

    #[test]
    fn a_take_zoomed_in_draws_the_bin_under_each_point() {
        let samples = signal(10_000);
        let levels = build(&samples);
        let take = Wave::of_peaks(&levels[1]);
        let mut out = Layout::default();
        let at = place(0.5, 1000, 9000, 300.0);
        layout(&take, &at, &mut out);
        assert!(out.points.is_empty());
        assert_eq!(out.columns.len(), 300);
        for c in &out.columns {
            assert_eq!((c.lo, c.hi), levels[1][c.at as usize / PEAK_BLOCK]);
        }
    }

    /// The cost of a frame of the Multitrack at its worst: 40 lanes (20 stereo tracks) of a
    /// 10-minute file, 1900 points wide, at every kind of zoom. Each column folds a bounded
    /// number of bins or samples, so this stays a fraction of a frame. (Opt-level 1 in tests;
    /// `wave_draw_cost` measures the release build.)
    #[test]
    fn laying_out_forty_lanes_of_a_ten_minute_file_is_quick() {
        let samples = signal(48_000 * 600);
        let started = std::time::Instant::now();
        let levels = build(&samples);
        let built = started.elapsed();
        let full = wave(&samples, &levels);
        let mut out = Layout::default();
        let mut worst = std::time::Duration::ZERO;
        for spp in [15_000.0, 2_000.0, 300.0, 100.0, 15.0, 2.0, 0.5, 1.0 / 32.0] {
            let started = std::time::Instant::now();
            let mut drawn = 0;
            for lane in 0..40u64 {
                let at = place(spp, lane * 1000, samples.len() as u64, 1900.0);
                layout(&full, &at, &mut out);
                drawn += out.columns.len() + out.points.len();
            }
            let took = started.elapsed();
            assert!(drawn > 0);
            println!("{spp:>9} samples a point: {took:?} for 40 lanes");
            worst = worst.max(took);
        }
        println!("built in {built:?}; worst frame {worst:?}");
        assert!(worst.as_millis() < 30, "worst frame {worst:?}");
    }
}
