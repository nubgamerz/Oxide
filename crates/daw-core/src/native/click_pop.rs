//! Click & Pop Repair: finds clicks and pops by a statistic over the audio and rebuilds them,
//! as an insert that plays, with a fixed delay that covers its blocks.
//!
//! - **Parameters:** 0 to 22; the status values (Is Processing, PreviewMode, Correct Count,
//!   Reject Count) are not parameters: the counts are the editor's readouts
//!   ([`NativeEffect::readouts`]).
//! - **Curves** ([`curve`]): the detection and rejection thresholds by level, 1000 entries over
//!   -100..0 dB, a uniform cubic B-spline through 17 control points from the user's three points
//!   (in a fixed order: Avg is the *lowest* level, Min the middle one) and their
//!   extensions to -140/-120 dB (floored at 4) and +20/+40 dB (capped at 60).
//! - **The statistic**, per channel: the pre-emphasis; the smoothed third difference
//!   against its mean over 26 samples and the smoothed low-pass against its mean over 100, the
//!   larger ratio squared; computed backwards, then forwards, and reconciled 0 to 16 samples
//!   ahead, so a click needs evidence from both directions. Link Channels multiplies each
//!   channel's by 1.05 to the other's; Pulse Train Verify zeroes it across regular trains.
//! - **Detection**: the threshold from the detect curve at the level (clamped 6..150),
//!   followed by τ = 0.9·τ + 0.1·threshold and quartered inside a click; Run Size samples under
//!   it end a click, 3 × Run Size cuts it; Detect Big Pops on a jump in the level. Second Level
//!   Verification: a spectral score of the click against the audio either side, against
//!   the reject curve less 0.15 × the click's peak statistic.
//! - **Passes:** each block is scanned again while a pass corrects at least 2 new clicks, up to
//!   Number of Times (once with Multiple Pass off); clicks already handled are not redone.
//! - **The repair:** the Click Remover's least-squares AR interpolation
//!   ([`super::click_remover::Lsar`]) over the span (Pop Size's padding), its model
//!   fitted to the 3072 samples around the click's stretch, of order C/2 (8..32) for the context
//!   size C (FFT Size, or the Auto FFT rule).
//! - **Blocks:** 12288 samples, the first 512 the final samples before them, then 10240 made
//!   final, then 1536 of look-ahead that the next block takes again; a click starting in the
//!   10240 is repaired whole, its tail written into the look-ahead. The latency is [`LATENCY`]
//!   at any rate: the look-ahead and a hop of work, spread over the hop by a budget so no block
//!   carries a whole pass. The stream decides where blocks fall (a transport start restarts
//!   them), so playback and export cut it alike at any block size.
//!
//! Where the study is silent the choices are ours, and listed in the note's "Oxide" section.

use super::click_remover::{autocorrelate, Lsar};
use super::{Kind, Learns, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;
use anyhow::{bail, Result};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::PI;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

// Parameter indices.
pub const MAX_THRESHOLD: usize = 0;
pub const AVG_THRESHOLD: usize = 1;
pub const MIN_THRESHOLD: usize = 2;
pub const MAX_LEVEL: usize = 3;
pub const AVG_LEVEL: usize = 4;
pub const MIN_LEVEL: usize = 5;
pub const MAX_REJECT: usize = 6;
pub const AVG_REJECT: usize = 7;
pub const MIN_REJECT: usize = 8;
pub const FFT_SIZE: usize = 9;
pub const POP_SIZE: usize = 10;
pub const RUN_SIZE: usize = 11;
pub const PASSES: usize = 12;
pub const BIG_POP_LEVEL: usize = 13;
pub const SENSITIVITY: usize = 14;
pub const DISCRIMINATION: usize = 15;
pub const SECOND_LEVEL: usize = 16;
pub const MULTIPLE_PASS: usize = 17;
pub const AUTO_FFT: usize = 18;
pub const BIG_POPS: usize = 19;
pub const LIGHT_CRACKLE: usize = 20;
pub const PULSE_TRAIN: usize = 21;
pub const LINK: usize = 22;
/// The parameters (the status values are not among them).
pub const COUNT: usize = 23;

/// The readouts: clicks corrected (big pops included), big pops corrected, clicks rejected by
/// the second level verification, since the transport started.
pub const CORRECTED: usize = 0;
pub const BIG_CORRECTED: usize = 1;
pub const REJECTED: usize = 2;

/// A block, the final samples before it, the samples it makes final (a hop) and its look-ahead.
const BLOCK: usize = 12_288;
const PRE: usize = 512;
const POST: usize = 1536;
const HOP: usize = BLOCK - PRE - POST;
/// The delay at every rate: a block's look-ahead, and a hop to do its work in.
pub const LATENCY: usize = 2 * HOP + POST;
/// Per channel: the samples the output is behind and the hop filling.
const LINE: usize = LATENCY + HOP;
/// Where a block starts in the line when a hop is complete.
const TAKE: usize = LINE - BLOCK;
/// The work's deadline within the hop that follows its block (samples), with room to spare.
const DEADLINE: usize = HOP - 1024;

/// The sweeps over a block go a chunk at a time.
const CHUNK: usize = 1024;
const CHUNKS: usize = BLOCK / CHUNK;
/// The AR models: one per stretch of 1024 samples, fitted to the 3072 around it.
const SEG: usize = 1024;
const SEGS: usize = BLOCK / SEG;
const FIT: usize = 3072;
const MAX_ORDER: usize = 32;
/// Spans this long or longer are left alone: the repair is by least squares, whose cost
/// grows with the span.
pub const MAX_SPAN: usize = 1000;
/// Clicks one pass takes in a block (per channel), and the handled list's length.
const MAX_CLICKS: usize = 256;
const MAX_LIST: usize = 1024;

/// The second level verification: 512-point spectra, the before and after frames centred 135
/// samples outside the span (so it reads 391 past each end), retried 85 samples either side.
const VERIFY: usize = 512;
const VERIFY_GAP: usize = 135;
const VERIFY_REACH: usize = VERIFY_GAP + VERIFY / 2;
const VERIFY_SHIFT: usize = 85;
/// Detect Big Pops looks 35 samples back, and starts a pop there.
const BIG_BACK: usize = 35;

/// The table: 1000 entries over -100..0 dB.
pub const TABLE: usize = 1000;
/// The B-spline's 14 windows, evaluated at 1421 points each.
const WINDOWS: usize = 14;
const EVALUATIONS: usize = 1421;

/// The pre-emphasis (a first-order filter), shared with the scans.
const PRE_POLE: f64 = 0.825_003_885_823_04;
const PRE_B0: f64 = 0.912_850_278_950_88;
const PRE_B1: f64 = 0.912_153_606_872_15;
/// The level's offset and floor.
const LEVEL_OFFSET: f64 = 2.56;
const LEVEL_FLOOR: f64 = 3.05e-15;

const fn param(
    name: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    default: f64,
) -> ParamSpec {
    ParamSpec {
        name,
        unit,
        min,
        max,
        default,
        steps: 0,
        decimals: 1,
        // An offline effect's settings: they apply a block at a time, so no lane plays them.
        automatable: false,
        link: None,
        log_slider: false,
        format: None,
    }
}

const fn whole(
    name: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    default: f64,
) -> ParamSpec {
    ParamSpec {
        steps: (max - min) as u32,
        decimals: 0,
        ..param(name, unit, min, max, default)
    }
}

const fn switch(name: &'static str, on: bool) -> ParamSpec {
    ParamSpec {
        steps: 1,
        decimals: 0,
        ..param(name, "", 0.0, 1.0, if on { 1.0 } else { 0.0 })
    }
}

static PARAMS: [ParamSpec; COUNT] = [
    param("Max Threshold", "", 1.0, 150.0, 30.0),
    param("Avg Threshold", "", 1.0, 150.0, 12.0),
    param("Min Threshold", "", 1.0, 150.0, 15.0),
    param("Max Level", "dB", -120.0, 10.0, -12.0),
    param("Avg Level", "dB", -120.0, 10.0, -50.0),
    param("Min Level", "dB", -120.0, 10.0, -24.0),
    param("Max Rejection Threshold", "", 1.0, 150.0, 40.0),
    param("Avg Rejection Threshold", "", 1.0, 150.0, 18.0),
    param("Min Rejection Threshold", "", 1.0, 150.0, 28.0),
    whole("FFT Size", "samples", 8.0, 4096.0, 32.0),
    whole("Pop Size", "samples", 0.0, 300.0, 12.0),
    whole("Run Size", "samples", 0.0, 1000.0, 10.0),
    whole("Number of Times", "passes", 1.0, 32.0, 8.0),
    param("Big Pop Level", "", 30.0, 200.0, 60.0),
    param("Sensitivity", "", 4.0, 50.0, 14.0),
    param("Discrimination", "", 2.0, 100.0, 20.0),
    switch("Second Level Verification", true),
    switch("Multiple Pass", true),
    switch("Auto FFT", true),
    switch("Detect Big Pops", true),
    switch("Ignore Light Crackle", false),
    switch("Pulse Train Verify", false),
    switch("Link Channels", false),
];

static LEARNS: Learns = Learns {
    scans: &["Scan For Threshold Levels", "Scan For All Levels"],
    run: scan,
    sets,
};

pub static KIND: Kind = Kind {
    id: "click-pop-repair",
    name: "Click & Pop Repair",
    category: "Restoration",
    params: &PARAMS,
    make: |rate| Box::new(ClickPop::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: Some(|rate, values, audio| scan(rate, values, audio, 0)),
    learns: Some(&LEARNS),
    reads: None,
    prepares: None,
};

fn on(values: &[f64], i: usize) -> bool {
    values[i] >= 0.5
}

// ---------------------------------------------------------------------------- the curves

/// The seven points a curve is drawn through, in a fixed order (not sorted): the
/// lower extensions at -140 and -120 dB, the "Avg" point (the lowest level), the "Min" point
/// (the middle), the "Max" point, the upper extensions at +20 and +40 dB. `reject` takes the
/// rejection thresholds (ids 6-8) at the same levels.
pub fn points(values: &[f64], reject: bool) -> [(f64, f64); 7] {
    let (top, low, mid) = if reject {
        (values[MAX_REJECT], values[AVG_REJECT], values[MIN_REJECT])
    } else {
        (
            values[MAX_THRESHOLD],
            values[AVG_THRESHOLD],
            values[MIN_THRESHOLD],
        )
    };
    let low = (values[AVG_LEVEL], low);
    let mid = (values[MIN_LEVEL], mid);
    let top = (values[MAX_LEVEL], top);
    // The lines through the low and middle points and through the middle and top ones; equal
    // levels give a flat line rather than a division by zero.
    let slope = |a: (f64, f64), b: (f64, f64)| {
        if (a.0 - b.0).abs() < 1e-9 {
            0.0
        } else {
            (a.1 - b.1) / (a.0 - b.0)
        }
    };
    let below = slope(low, mid);
    let above = slope(top, mid);
    let under = |l: f64| (mid.1 + below * (l - mid.0)).max(4.0);
    let over = |l: f64| (mid.1 + above * (l - mid.0)).min(60.0);
    [
        (-140.0, under(-140.0)),
        (-120.0, under(-120.0)),
        low,
        mid,
        top,
        (20.0, over(20.0)),
        (40.0, over(40.0)),
    ]
}

/// The 17 control points: the seven points with two more between each pair of neighbours, at
/// 1/2.2 of the span from each end; one only on the first span (at 1/2.2 from its start) and
/// on the last (at 1/2.2 from its end), which is how the study's count of 17 comes out.
fn control_points(p: &[(f64, f64); 7]) -> [(f64, f64); 17] {
    let at = |a: (f64, f64), b: (f64, f64), u: f64| (a.0 + (b.0 - a.0) * u, a.1 + (b.1 - a.1) * u);
    let u = 1.0 / 2.2;
    let mut out = [(0.0, 0.0); 17];
    let mut k = 0;
    let mut push = |v: (f64, f64)| {
        out[k] = v;
        k += 1;
    };
    for s in 0..6 {
        let (a, b) = (p[s], p[s + 1]);
        push(a);
        match s {
            0 => push(at(a, b, u)),
            5 => push(at(a, b, 1.0 - u)),
            _ => {
                push(at(a, b, u));
                push(at(a, b, 1.0 - u));
            }
        }
    }
    push(p[6]);
    out
}

/// Evaluate window `w` (0..14) of the B-spline through `control` into `table`: each entry the
/// curve's level lands on takes its threshold (later evaluations overwrite earlier ones).
fn spline_window(control: &[(f64, f64); 17], w: usize, table: &mut [f64]) {
    let c = &control[w..w + 4];
    for k in 0..EVALUATIONS {
        let t = k as f64 / (EVALUATIONS - 1) as f64;
        let (t2, t3) = (t * t, t * t * t);
        let b = [
            (1.0 - t).powi(3) / 6.0,
            (3.0 * t3 - 6.0 * t2 + 4.0) / 6.0,
            (-3.0 * t3 + 3.0 * t2 + 3.0 * t + 1.0) / 6.0,
            t3 / 6.0,
        ];
        let level = b[0] * c[0].0 + b[1] * c[1].0 + b[2] * c[2].0 + b[3] * c[3].0;
        let threshold = b[0] * c[0].1 + b[1] * c[1].1 + b[2] * c[2].1 + b[3] * c[3].1;
        let i = ((level + 100.0) * 10.0).floor();
        if (0.0..TABLE as f64).contains(&i) {
            table[i as usize] = threshold;
        }
    }
}

/// Entries no evaluation landed on (the study didn't read the table's first contents) take
/// the nearest one that did; with none at all, `fallback`.
fn fill_gaps(table: &mut [f64], fallback: f64) {
    let mut last = None;
    for v in table.iter_mut() {
        if v.is_nan() {
            if let Some(l) = last {
                *v = l;
            }
        } else {
            last = Some(*v);
        }
    }
    let mut next = None;
    for v in table.iter_mut().rev() {
        if v.is_nan() {
            *v = next.unwrap_or(fallback);
        } else {
            next = Some(*v);
        }
    }
}

/// A whole curve: the detection (or with `reject`, the rejection) threshold at each of the
/// 1000 levels from -100 dB in steps of 0.1 dB, before the clamps the effect applies when it
/// reads it (6..150 to detect, 2..150 to reject). The same code the effect builds its tables
/// with, a window at a time.
pub fn curve(values: &[f64], reject: bool) -> Vec<f64> {
    let p = points(values, reject);
    let control = control_points(&p);
    let mut table = vec![f64::NAN; TABLE];
    for w in 0..WINDOWS {
        spline_window(&control, w, &mut table);
    }
    fill_gaps(&mut table, p[3].1);
    table
}

/// The table entry for a level in dB.
fn entry(level: f64) -> usize {
    (((level + 100.0) * 10.0).floor().max(0.0) as usize).min(TABLE - 1)
}

// ---------------------------------------------------------------------------- the statistic

/// A one-pole follower's coefficient for a time in milliseconds.
fn coefficient(ms: f64, rate: f64) -> f64 {
    (-1.0 / (ms * 1e-3 * rate).max(1e-9)).exp()
}

/// The followers' coefficients at a rate: the level's envelope of |y| (attack 0.5 ms, release
/// 80 ms: the one Scan For Threshold Levels measures with, which the study gives for the scan;
/// the processing's own first stage is untraced, so ours is the scan's) and the dB level's
/// smoothing (0.2 ms, 300 ms).
#[derive(Debug, Clone, Copy)]
struct Followers {
    attack: f64,
    release: f64,
    attack_db: f64,
    release_db: f64,
}

impl Followers {
    fn new(rate: f64) -> Self {
        Self {
            attack: coefficient(0.5, rate),
            release: coefficient(80.0, rate),
            attack_db: coefficient(0.2, rate),
            release_db: coefficient(300.0, rate),
        }
    }

    fn envelope(&self, env: f64, e: f64) -> f64 {
        let k = if e > env { self.attack } else { self.release };
        k * env + (1.0 - k) * e
    }

    fn smooth(&self, level: f64, l: f64) -> f64 {
        let k = if l > level {
            self.attack_db
        } else {
            self.release_db
        };
        k * level + (1.0 - k) * l
    }
}

/// The dB level of an envelope.
fn db_of(env: f64) -> f64 {
    20.0 * (env + LEVEL_FLOOR).log10() + LEVEL_OFFSET
}

/// One direction's detector: the smoothed third difference against its mean over the 26
/// samples before, the smoothed low-pass against its mean over the 100 before (each mean
/// floored at 1e-5), the larger ratio squared. The means are of what came before the sample
/// (ours: the study doesn't say whether the sample itself is in them), so a click is judged
/// against the audio leading up to it.
#[derive(Clone)]
struct Detector {
    d: f64,
    s: f64,
    ring_d: [f64; 26],
    ring_s: [f64; 100],
    sum_d: f64,
    sum_s: f64,
    at_d: usize,
    at_s: usize,
    seen: usize,
}

impl Detector {
    fn new() -> Self {
        Self {
            d: 0.0,
            s: 0.0,
            ring_d: [0.0; 26],
            ring_s: [0.0; 100],
            sum_d: 0.0,
            sum_s: 0.0,
            at_d: 0,
            at_s: 0,
            seen: 0,
        }
    }

    /// The statistic for a third difference `d` and low-pass `s`.
    fn step(&mut self, d: f64, s: f64) -> f64 {
        self.d = 0.8 * self.d + 0.2 * d.abs();
        self.s = 0.6 * self.s + 0.4 * s;
        let stat = if self.seen == 0 {
            0.0
        } else {
            let mean_d = (self.sum_d / self.seen.min(26) as f64).max(1e-5);
            let mean_s = (self.sum_s / self.seen.min(100) as f64).max(1e-5);
            (self.d / mean_d).powi(2).max((self.s / mean_s).powi(2))
        };
        self.sum_d += self.d - self.ring_d[self.at_d];
        self.ring_d[self.at_d] = self.d;
        self.at_d = (self.at_d + 1) % 26;
        self.sum_s += self.s - self.ring_s[self.at_s];
        self.ring_s[self.at_s] = self.s;
        self.at_s = (self.at_s + 1) % 100;
        self.seen += 1;
        stat
    }
}

/// The reconciliation: the forward value kept where the backward one `ahead` agrees within
/// x1.75 (or is larger), the backward otherwise (the smaller): ours where the study left the
/// exact pick open.
fn reconcile(forward: f64, ahead: f64) -> f64 {
    if forward <= 1.75 * ahead {
        forward
    } else {
        ahead
    }
}

const OFFSETS: [usize; 5] = [0, 2, 4, 8, 16];

/// A channel's statistic and level over a buffer, swept a chunk at a time (the insert spreads
/// the chunks out; a scan runs them all at once).
struct Sweep {
    /// The pre-emphasised signal, the level (dB), the forward and backward statistics and the
    /// reconciled one.
    y: Vec<f64>,
    level: Vec<f64>,
    forward: Vec<f64>,
    backward: Vec<f64>,
    stat: Vec<f64>,
    // The forward sweep's state.
    v: f64,
    env: f64,
    smooth: f64,
    ahead: Detector,
    behind: Detector,
}

impl Sweep {
    fn new(len: usize) -> Self {
        Self {
            y: vec![0.0; len],
            level: vec![0.0; len],
            forward: vec![0.0; len],
            backward: vec![0.0; len],
            stat: vec![0.0; len],
            v: 0.0,
            env: 0.0,
            smooth: 0.0,
            ahead: Detector::new(),
            behind: Detector::new(),
        }
    }

    fn len(&self) -> usize {
        self.y.len()
    }

    /// Before the first forward chunk.
    fn reset(&mut self) {
        self.v = 0.0;
        self.env = 0.0;
        self.smooth = 0.0;
        self.ahead = Detector::new();
        self.behind = Detector::new();
    }

    /// The pre-emphasis, the level and the forward statistic over `range` of `x` (in order from
    /// 0). The level's envelope starts at the peak of the first 36 samples, as the scan's.
    fn forward_chunk(&mut self, x: &[f64], f: &Followers, range: std::ops::Range<usize>) {
        for n in range.clone() {
            let v = x[n] + PRE_POLE * self.v;
            self.y[n] = PRE_B0 * v - PRE_B1 * self.v;
            self.v = v;
        }
        if range.start == 0 {
            self.env = self.y[..36.min(self.len())]
                .iter()
                .fold(0.0, |m, v| m.max(v.abs()));
        }
        let y = &self.y;
        let at = |n: usize, back: usize| if n >= back { y[n - back] } else { 0.0 };
        for n in range {
            self.env = f.envelope(self.env, y[n].abs());
            let l = db_of(self.env);
            self.smooth = if n == 0 { l } else { f.smooth(self.smooth, l) };
            self.level[n] = self.smooth;
            let d = at(n, 3) - 3.0 * at(n, 2) + 3.0 * at(n, 1) - y[n];
            let s = 2.0 * (y[n] + at(n, 1) + at(n, 2) + at(n, 3)).abs();
            self.forward[n] = self.ahead.step(d, s);
        }
    }

    /// The backward statistic over `range` (from the end of the buffer down).
    fn backward_chunk(&mut self, range: std::ops::Range<usize>) {
        let len = self.len();
        let y = &self.y;
        let at = |n: usize, k: usize| if n + k < len { y[n + k] } else { 0.0 };
        for n in range.rev() {
            let d = at(n, 3) - 3.0 * at(n, 2) + 3.0 * at(n, 1) - y[n];
            let s = 2.0 * (y[n] + at(n, 1) + at(n, 2) + at(n, 3)).abs();
            self.backward[n] = self.behind.step(d, s);
        }
    }

    /// The reconciled statistic over `range`.
    fn reconcile_chunk(&mut self, range: std::ops::Range<usize>) {
        let len = self.len();
        for n in range {
            let f = self.forward[n];
            self.stat[n] = OFFSETS
                .iter()
                .filter(|&&o| n + o < len)
                .map(|&o| reconcile(f, self.backward[n + o]))
                .fold(0.0, f64::max);
        }
    }

    /// All of it over `x` at once.
    fn run(&mut self, x: &[f64], f: &Followers) {
        self.reset();
        self.forward_chunk(x, f, 0..x.len());
        self.backward_chunk(0..x.len());
        self.reconcile_chunk(0..x.len());
    }
}

/// Link Channels over `range`: each statistic times 1.05 to the other's (both from before),
/// the exponent capped at 1000 (ours: 1.05 to a hard click's statistic overflows, and zero
/// times infinity would be no number).
fn link(a: &mut [f64], b: &mut [f64], range: std::ops::Range<usize>) {
    let ln = 1.05f64.ln();
    for n in range {
        let (x, y) = (a[n], b[n]);
        a[n] = x * (ln * y.min(1000.0)).exp();
        b[n] = y * (ln * x.min(1000.0)).exp();
    }
}

/// Pulse Train Verify on one channel's statistic: its peaks above 35 (closer than 5 merged,
/// up to 350) tested in pairs 21 to 2000 samples apart; a lattice of at least four peaks at
/// that spacing (within 1.5 %, at least 1.04 samples, up to 19 multiples, stopping at a hole
/// of more than max(3, period/50) multiples), uniform (max and first at most 1.49 times the
/// mean) and strong ((2·max + mean)/3 at least 19), is zeroed: a regular train is no clicks.
/// Zeroed across each peak while the statistic stays over 6 (the lowest threshold), at least
/// 8 and at most 32 samples each side (ours: the study doesn't give the width).
fn pulse_trains(stat: &mut [f64], peaks: &mut Vec<(usize, f64)>, matched: &mut Vec<bool>) {
    peaks.clear();
    let len = stat.len();
    for n in 1..len.saturating_sub(1) {
        let v = stat[n];
        if v > 35.0 && v >= stat[n - 1] && v > stat[n + 1] {
            let merge = peaks.last().is_some_and(|last| n - last.0 < 5);
            if merge {
                let last = peaks.last_mut().expect("a peak");
                if v > last.1 {
                    *last = (n, v);
                }
            } else if peaks.len() < 350 {
                peaks.push((n, v));
            } else {
                break;
            }
        }
    }
    matched.clear();
    matched.resize(peaks.len(), false);
    let mut members = [0usize; 20];
    for i in 0..peaks.len() {
        if matched[i] {
            continue;
        }
        for j in i + 1..peaks.len() {
            let period = (peaks[j].0 - peaks[i].0) as f64;
            if period < 21.0 {
                continue;
            }
            if period > 2000.0 {
                break;
            }
            let tolerance = (0.015 * period).max(1.04);
            let hole = 3usize.max((period / 50.0) as usize);
            members[0] = i;
            members[1] = j;
            let mut count = 2;
            let mut misses = 0;
            for m in 2..=19 {
                let target = peaks[i].0 as f64 + m as f64 * period;
                if target > len as f64 + tolerance {
                    break;
                }
                // The nearest peak to the target.
                let k = peaks.partition_point(|p| (p.0 as f64) < target);
                let near = [k.checked_sub(1), Some(k)]
                    .into_iter()
                    .flatten()
                    .filter(|&k| k < peaks.len())
                    .min_by(|&a, &b| {
                        let da = (peaks[a].0 as f64 - target).abs();
                        let db = (peaks[b].0 as f64 - target).abs();
                        da.total_cmp(&db)
                    });
                match near {
                    Some(k) if (peaks[k].0 as f64 - target).abs() <= tolerance => {
                        members[count] = k;
                        count += 1;
                        misses = 0;
                    }
                    _ => {
                        misses += 1;
                        if misses > hole {
                            break;
                        }
                    }
                }
            }
            if count < 4 {
                continue;
            }
            let heights = members[..count].iter().map(|&k| peaks[k].1);
            let mean = heights.clone().sum::<f64>() / count as f64;
            let max = heights.fold(0.0, f64::max);
            let first = peaks[i].1;
            if max / mean <= 1.49 && first / mean <= 1.49 && (2.0 * max + mean) / 3.0 >= 19.0 {
                for &k in &members[..count] {
                    matched[k] = true;
                    let at = peaks[k].0;
                    let mut lo = at.saturating_sub(8);
                    while lo > 0 && at - lo < 32 && stat[lo - 1] > 6.0 {
                        lo -= 1;
                    }
                    let mut hi = (at + 8).min(len - 1);
                    while hi + 1 < len && hi - at < 32 && stat[hi + 1] > 6.0 {
                        hi += 1;
                    }
                    stat[lo..=hi].fill(0.0);
                }
                break;
            }
        }
    }
}

// ---------------------------------------------------------------------------- verification

/// The second level verification (a spectral score): 512-point
/// spectra in dB of the audio before the span, after it and at the click, smoothed across bins
/// 32..255; the click scores where its spectrum stands over both neighbours'.
struct Verifier {
    fft: Arc<dyn Fft<f64>>,
    buf: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    window: Vec<f64>,
    /// Smoothed before, after and middle spectra, and the middle's own (bins 32..256).
    before: Vec<f64>,
    after: Vec<f64>,
    middle: Vec<f64>,
    raw: Vec<f64>,
}

const LOW_BIN: usize = 32;
const HIGH_BIN: usize = 256;

impl Verifier {
    fn new() -> Self {
        let fft = FftPlanner::new().plan_fft_forward(VERIFY);
        let scratch = vec![Complex::default(); fft.get_inplace_scratch_len()];
        // Blackman-Harris (4 terms) over the frame: the study's "period 307 x 1.667" is the
        // frame's 512 (ours: the window's exact length is a guess).
        let window = (0..VERIFY)
            .map(|n| {
                let w = 2.0 * PI * n as f64 / VERIFY as f64;
                0.35875 - 0.48829 * w.cos() + 0.14128 * (2.0 * w).cos() - 0.01168 * (3.0 * w).cos()
            })
            .collect();
        Self {
            fft,
            buf: vec![Complex::default(); VERIFY],
            scratch,
            window,
            before: vec![0.0; HIGH_BIN],
            after: vec![0.0; HIGH_BIN],
            middle: vec![0.0; HIGH_BIN],
            raw: vec![0.0; HIGH_BIN],
        }
    }

    /// The dB spectrum (bins 32..256) of the 512 samples of `x` centred on `centre`, in 16-bit
    /// units (20·log10 + 10.25, floored at 0 dB): its raw values in
    /// `raw` and smoothed across bins by a one-pole 0.9/0.1 into `out`.
    fn spectrum(&mut self, x: &[f64], centre: usize, which: u8) {
        let from = centre - VERIFY / 2;
        for (k, b) in self.buf.iter_mut().enumerate() {
            *b = Complex::new(x[from + k] * 32_768.0 * self.window[k], 0.0);
        }
        self.fft
            .process_with_scratch(&mut self.buf, &mut self.scratch);
        let out = match which {
            0 => &mut self.before,
            1 => &mut self.after,
            _ => &mut self.middle,
        };
        let mut s = 0.0;
        let bins = self.buf[LOW_BIN..HIGH_BIN].iter();
        let into = self.raw[LOW_BIN..].iter_mut().zip(&mut out[LOW_BIN..]);
        for (k, (b, (raw, out))) in bins.zip(into).enumerate() {
            let db = (20.0 * b.norm().max(1e-30).log10() + 10.25).max(0.0);
            *raw = db;
            s = if k == 0 { db } else { 0.9 * s + 0.1 * db };
            *out = s;
        }
    }

    /// The score of the click whose span is `lo..=hi`, its middle frame at `centre`: the sum
    /// over the bins of 2^(x/4) - 0.5/2^(x/4), x = min(0.66(M - B), 1.5(M - A)) - 0.2|A - B|;
    /// scaled down by (2400/E)³ when the middle spectrum's peak energy E is over 2400 (ours: E
    /// is the squared excess of its raw bins over its smoothed ones, past 6 dB, so tonal audio
    /// asks more of a click).
    fn score(&mut self, x: &[f64], lo: usize, hi: usize, centre: usize) -> f64 {
        self.spectrum(x, lo - VERIFY_GAP, 0);
        self.spectrum(x, hi + VERIFY_GAP, 1);
        self.spectrum(x, centre, 2);
        let mut score = 0.0;
        let mut energy = 0.0;
        for k in LOW_BIN..HIGH_BIN {
            let (b, a, m) = (self.before[k], self.after[k], self.middle[k]);
            let v = (0.66 * (m - b)).min(1.5 * (m - a)) - 0.2 * (a - b).abs();
            let p = (v / 4.0).exp2();
            score += p - 0.5 / p;
            energy += (self.raw[k] - m - 6.0).max(0.0).powi(2);
        }
        if energy > 2400.0 {
            score *= (2400.0 / energy).powi(3);
        }
        score
    }

    /// Whether a click passes: `threshold` <= 0, or its score at the click (or 85 samples either
    /// side of it) reaches threshold².
    fn passes(&mut self, x: &[f64], lo: usize, hi: usize, threshold: f64) -> bool {
        if threshold <= 0.0 {
            return true;
        }
        let centre = (lo + hi) / 2;
        let need = threshold * threshold;
        [centre, centre - VERIFY_SHIFT, centre + VERIFY_SHIFT]
            .into_iter()
            .any(|c| self.score(x, lo, hi, c) >= need)
    }
}

// ---------------------------------------------------------------------------- one click

/// A click found, in a block's samples: its first and last sample, its peak statistic, whether
/// it is a big pop.
#[derive(Debug, Clone, Copy)]
struct Click {
    start: usize,
    end: usize,
    peak: f64,
    big: bool,
}

/// What a channel's tracking of the threshold carries from one sample to the next.
#[derive(Debug, Clone, Copy, Default)]
struct Tracking {
    tau: f64,
    open: Option<Click>,
    last: usize,
    below: usize,
}

/// The settings a block runs with, from the plain values.
#[derive(Debug, Clone, Copy)]
struct Settings {
    run: usize,
    pop: usize,
    passes: usize,
    big_level: f64,
    verify: bool,
    auto_fft: bool,
    fft: usize,
    big_pops: bool,
    light: bool,
    pulse: bool,
    link: bool,
}

impl Settings {
    fn of(v: &[f64]) -> Self {
        Self {
            run: (v[RUN_SIZE].round() as usize).max(1),
            pop: v[POP_SIZE].round() as usize,
            passes: if on(v, MULTIPLE_PASS) {
                (v[PASSES].round() as usize).clamp(1, 32)
            } else {
                1
            },
            big_level: v[BIG_POP_LEVEL],
            verify: on(v, SECOND_LEVEL),
            auto_fft: on(v, AUTO_FFT),
            fft: (v[FFT_SIZE].round() as usize).clamp(8, 4096),
            big_pops: on(v, BIG_POPS),
            light: on(v, LIGHT_CRACKLE),
            pulse: on(v, PULSE_TRAIN),
            link: on(v, LINK),
        }
    }

    /// The repair span of a click: Pop Size's padding as traced, ⌊Pop/4⌋ + 2 before and
    /// Pop - ⌊Pop/4⌋ after.
    fn span(&self, c: &Click) -> (usize, usize) {
        (
            c.start.saturating_sub(self.pop / 4 + 2),
            c.end + self.pop - self.pop / 4,
        )
    }

    /// The context size for a span of `len` samples: FFT Size, or with Auto FFT
    /// 2^⌊log2(3.5(L + 1)) + 0.75⌋ (big pops 0.5(L + 1)), 16..1024.
    fn context(&self, len: usize, big: bool) -> usize {
        if !self.auto_fft {
            return self.fft;
        }
        let k = if big { 0.5 } else { 3.5 } * (len as f64 + 1.0);
        let e = (k.log2() + 0.75).floor().max(0.0);
        (2f64.powf(e) as usize).clamp(16, 1024)
    }
}

/// Ours: the model's order from the context size C, C/2 within 8..32 (the AR interpolation
/// needs no spectrum, only a model as long as the context it should follow).
pub fn order(context: usize) -> usize {
    (context / 2).clamp(8, MAX_ORDER)
}

/// One step of the threshold tracking at sample `n` of a channel: `stat` the statistic, `level`
/// the level (dB), `detect` the detect table. Returns a click when one closes.
#[allow(clippy::too_many_arguments)]
fn track(
    t: &mut Tracking,
    n: usize,
    stat: f64,
    level: &[f64],
    detect: &[f64],
    s: &Settings,
) -> Option<Click> {
    let threshold = detect[entry(level[n])].clamp(6.0, 150.0);
    t.tau = if n == 0 {
        threshold
    } else {
        0.9 * t.tau + 0.1 * threshold
    };
    if t.open.is_some() {
        t.tau *= 0.25;
    }
    // The levels floored at the table's -100 dB (ours: digital silence is -288 dB, against
    // which any sound at all would be a jump).
    let big = s.big_pops
        && n >= BIG_BACK
        && 3.0 * level[n].max(-100.0) - level[n - BIG_BACK].max(-100.0) + 64.0 > s.big_level;
    let mut closed = None;
    if stat > t.tau || big {
        let click = t.open.get_or_insert(Click {
            start: if big { n - BIG_BACK } else { n },
            end: n,
            peak: 0.0,
            big,
        });
        click.big |= big;
        click.end = n;
        click.peak = click.peak.max(stat);
        t.last = n;
        t.below = 0;
        // Longer than 3 x Run Size: cut there (a big pop runs while its level jump lasts).
        if !click.big && n + 1 - click.start >= 3 * s.run {
            closed = t.open.take();
        }
    } else if t.open.is_some() {
        t.below += 1;
        if t.below >= s.run {
            closed = t.open.take();
        }
    }
    closed
}

/// Where a click stands in the handled list.
#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Corrected,
    Rejected,
    /// Left as it is: a single sample with Ignore Light Crackle, too long, or no room.
    Left,
}

/// A handled click's span (block samples) and state.
#[derive(Debug, Clone, Copy)]
struct Listed {
    lo: usize,
    hi: usize,
    state: State,
}

/// What repairing a click did.
enum Outcome {
    Corrected { big: bool },
    Rejected,
    Left,
}

/// One channel's block: its samples as repaired so far, which were, the sweep, its clicks.
struct Chan {
    work: Vec<f64>,
    touched: Vec<bool>,
    sweep: Sweep,
    clicks: Vec<Click>,
    list: Vec<Listed>,
    /// Spans repaired into the look-ahead, for the next block's list.
    carry: Vec<Listed>,
    tracking: Tracking,
    /// The models' autocorrelations (one per stretch of 1024) and whether this pass has it.
    r: Vec<[f64; MAX_ORDER + 1]>,
    fitted: Vec<bool>,
    /// Still running passes, and what this pass corrected.
    active: bool,
    corrected: usize,
}

impl Chan {
    fn new() -> Self {
        Self {
            work: vec![0.0; BLOCK],
            touched: vec![false; BLOCK],
            sweep: Sweep::new(BLOCK),
            clicks: Vec::with_capacity(MAX_CLICKS),
            list: Vec::with_capacity(MAX_LIST),
            carry: Vec::with_capacity(MAX_LIST),
            tracking: Tracking::default(),
            r: vec![[0.0; MAX_ORDER + 1]; SEGS],
            fitted: vec![false; SEGS],
            active: false,
            corrected: 0,
        }
    }

    fn known(&self, c: &Click) -> bool {
        self.list.iter().any(|l| c.start <= l.hi && c.end >= l.lo)
    }

    fn note(&mut self, lo: usize, hi: usize, state: State) {
        if self.list.len() < MAX_LIST {
            self.list.push(Listed { lo, hi, state });
        }
    }
}

/// The scratch every channel shares.
struct Shared {
    lsar: Lsar,
    verifier: Verifier,
    hamming: Vec<f64>,
    windowed: Vec<f64>,
    peaks: Vec<(usize, f64)>,
    matched: Vec<bool>,
}

impl Shared {
    fn new() -> Self {
        Self {
            lsar: Lsar::new(MAX_SPAN, MAX_ORDER),
            verifier: Verifier::new(),
            hamming: (0..FIT)
                .map(|j| 0.54 - 0.46 * (2.0 * PI * j as f64 / (FIT - 1) as f64).cos())
                .collect(),
            windowed: vec![0.0; FIT],
            peaks: Vec::with_capacity(350),
            matched: Vec::with_capacity(350),
        }
    }

    /// Repair (or reject, or leave) click `c` of `ch` with the detection and rejection tables.
    fn repair(&mut self, ch: &mut Chan, c: Click, s: &Settings, reject: &[f64]) -> Outcome {
        let (lo, hi) = s.span(&c);
        // Never before the block's final samples, which may be playing already.
        let lo = lo.max(PRE);
        let len = hi + 1 - lo;
        if (s.light && c.end == c.start) || len >= MAX_SPAN || hi + VERIFY_REACH >= BLOCK {
            ch.note(lo, hi, State::Left);
            return Outcome::Left;
        }
        if s.verify && !c.big {
            let level = ch.sweep.level[c.start];
            let threshold = reject[entry(level)].clamp(2.0, 150.0) - 0.15 * c.peak;
            if !self.verifier.passes(&ch.work, lo, hi, threshold) {
                ch.note(lo, hi, State::Rejected);
                return Outcome::Rejected;
            }
        }
        let p = order(s.context(len, c.big));
        let seg = ((lo + hi) / 2 / SEG).min(SEGS - 1);
        if !ch.fitted[seg] {
            let from = (seg * SEG + SEG / 2)
                .saturating_sub(FIT / 2)
                .min(BLOCK - FIT);
            for ((w, h), x) in self
                .windowed
                .iter_mut()
                .zip(&self.hamming)
                .zip(&ch.work[from..from + FIT])
            {
                *w = h * x;
            }
            autocorrelate(&self.windowed, MAX_ORDER, &mut ch.r[seg]);
            ch.fitted[seg] = true;
        }
        self.lsar.model(&ch.r[seg][..=p], p);
        if !self.lsar.fill(&mut ch.work, lo, hi) {
            // Never expected (see `Lsar::fill`): the straight line between the neighbours.
            let (before, after) = (ch.work[lo - 1], ch.work[hi + 1]);
            let m = (hi - lo + 2) as f64;
            for (i, x) in ch.work[lo..=hi].iter_mut().enumerate() {
                *x = before + (i + 1) as f64 / m * (after - before);
            }
        }
        ch.touched[lo..=hi].fill(true);
        ch.note(lo, hi, State::Corrected);
        Outcome::Corrected { big: c.big }
    }
}

// ---------------------------------------------------------------------------- the work

/// Where a block's work is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    /// Rebuilding the tables: window k of the detect curve, then of the reject one, then the
    /// gaps.
    Tables(usize),
    /// A sweep of channel `ch`: 0 forward, 1 backward, 2 reconciling; a chunk at a time.
    Sweep {
        ch: usize,
        part: u8,
        chunk: usize,
    },
    Pulse {
        ch: usize,
    },
    Link {
        chunk: usize,
    },
    Detect {
        ch: usize,
        chunk: usize,
    },
    Repair {
        ch: usize,
        k: usize,
    },
    EndPass,
    WriteBack {
        ch: usize,
    },
}

// What each step costs, roughly in microseconds in release (they only spread the work: the
// output never depends on them).
const COST_TABLE: f64 = 8.0;
const COST_SWEEP: [f64; 3] = [22.0, 8.0, 6.0];
const COST_PULSE: f64 = 60.0;
const COST_LINK: f64 = 14.0;
const COST_DETECT: f64 = 6.0;
const COST_REPAIR: f64 = 30.0;
const COST_VERIFY: f64 = 25.0;
const COST_WRITE: f64 = 10.0;

/// The clicks a channel's pass is expected to find before any block has said.
const EXPECT: f64 = 32.0;

fn sweep_cost() -> f64 {
    CHUNKS as f64 * (COST_SWEEP[0] + COST_SWEEP[1] + COST_SWEEP[2])
}

struct ClickPop {
    followers: Followers,
    values: [f64; COUNT],
    /// The values the block in work runs with, and those its tables were built from.
    block: [f64; COUNT],
    tabled: Option<[f64; COUNT]>,
    settings: Settings,
    detect: Vec<f64>,
    reject: Vec<f64>,
    lines: [Vec<f32>; CHANNELS],
    chans: [Chan; CHANNELS],
    shared: Shared,
    filled: usize,
    phase: Phase,
    pass: usize,
    /// The work's budget (estimated): what remains of this pass (and the tables and the
    /// write-back), what the passes still planned will cost, and the credit samples earn.
    budget: f64,
    future: f64,
    credit: f64,
    /// The clicks each channel's first pass is expected to find (its last block's), budgeted
    /// before they are found so the repairs don't all fall at the deadline, and the passes a
    /// block is expected to run (the last one's).
    expect: [f64; CHANNELS],
    planned: usize,
    counters: Arc<[AtomicU64]>,
    /// Passes run and blocks finished since it was made (for tests).
    passes_run: u64,
    blocks_run: u64,
}

impl ClickPop {
    fn new(rate: f64) -> Self {
        let values: [f64; COUNT] = std::array::from_fn(|i| PARAMS[i].default);
        Self {
            followers: Followers::new(rate),
            values,
            block: values,
            tabled: None,
            settings: Settings::of(&values),
            detect: vec![f64::NAN; TABLE],
            reject: vec![f64::NAN; TABLE],
            lines: std::array::from_fn(|_| vec![0.0; LINE]),
            chans: std::array::from_fn(|_| Chan::new()),
            shared: Shared::new(),
            filled: 0,
            phase: Phase::Idle,
            pass: 0,
            budget: 0.0,
            future: 0.0,
            credit: 0.0,
            expect: [EXPECT; CHANNELS],
            planned: 2,
            counters: (0..3).map(|_| AtomicU64::new(0)).collect(),
            passes_run: 0,
            blocks_run: 0,
        }
    }

    /// A repair's cost (a verified one tries up to three times).
    fn repair_cost(&self) -> f64 {
        if self.settings.verify {
            COST_REPAIR + COST_VERIFY
        } else {
            COST_REPAIR
        }
    }

    /// The clicks channel `c` is expected to find in pass `pass`: its last block's first
    /// pass's, a quarter of that after.
    fn expected(&self, c: usize, pass: usize) -> f64 {
        if pass == 0 {
            self.expect[c]
        } else {
            0.25 * self.expect[c]
        }
    }

    /// The cost of pass `pass`'s sweeps, detection and expected repairs, for the channels
    /// running.
    fn pass_cost(&self, pass: usize) -> f64 {
        let s = &self.settings;
        let expected: f64 = (0..CHANNELS)
            .filter(|&c| self.chans[c].active)
            .map(|c| self.expected(c, pass))
            .sum();
        let active = self.chans.iter().filter(|c| c.active).count() as f64;
        let swept = if s.link && active > 0.0 {
            CHANNELS as f64
        } else {
            active
        };
        swept * (sweep_cost() + if s.pulse { COST_PULSE } else { 0.0 })
            + if s.link {
                CHUNKS as f64 * COST_LINK
            } else {
                0.0
            }
            + active * CHUNKS as f64 * COST_DETECT
            + expected * self.repair_cost()
    }

    /// A hop is complete: finish the last block's work if it isn't (never expected), take each
    /// channel's block and move the lines on.
    fn hop(&mut self) {
        while self.phase != Phase::Idle {
            self.step();
        }
        for (line, ch) in self.lines.iter_mut().zip(self.chans.iter_mut()) {
            for (w, s) in ch.work.iter_mut().zip(&line[TAKE..]) {
                *w = f64::from(*s);
            }
            ch.touched.fill(false);
            std::mem::swap(&mut ch.list, &mut ch.carry);
            ch.carry.clear();
            ch.active = true;
            line.copy_within(HOP.., 0);
        }
        self.filled = 0;
        self.block = self.values;
        self.settings = Settings::of(&self.block);
        self.pass = 0;
        let curves = |v: &[f64; COUNT]| v[..9].to_vec();
        let stale = self
            .tabled
            .is_none_or(|t| curves(&t) != curves(&self.block));
        self.phase = if stale {
            self.detect.fill(f64::NAN);
            self.reject.fill(f64::NAN);
            Phase::Tables(0)
        } else {
            self.first_pass_phase()
        };
        self.budget = if stale {
            (2 * WINDOWS + 1) as f64 * COST_TABLE
        } else {
            0.0
        } + self.pass_cost(0)
            + CHANNELS as f64 * COST_WRITE;
        self.plan();
        self.credit = 0.0;
    }

    /// The passes still planned after this one: one more than the last block ran (more if this
    /// one goes on), at most Number of Times.
    fn plan(&mut self) {
        let planned = (self.planned + 1).min(self.settings.passes);
        self.future = planned.saturating_sub(self.pass + 1) as f64 * self.pass_cost(1);
    }

    fn first_pass_phase(&mut self) -> Phase {
        for ch in &mut self.chans {
            ch.fitted.fill(false);
            ch.corrected = 0;
        }
        match self.swept_after(None) {
            Some(ch) => Phase::Sweep {
                ch,
                part: 0,
                chunk: 0,
            },
            None => Phase::WriteBack { ch: 0 },
        }
    }

    /// Whether channel `c` needs its statistic this pass: running, or linked to one that is.
    fn swept(&self, c: usize) -> bool {
        self.chans[c].active || (self.settings.link && self.chans.iter().any(|c| c.active))
    }

    /// The next channel after `after` whose statistic this pass needs.
    fn swept_after(&self, after: Option<usize>) -> Option<usize> {
        let from = after.map_or(0, |c| c + 1);
        (from..CHANNELS).find(|&c| self.swept(c))
    }

    fn active_after(&self, after: Option<usize>) -> Option<usize> {
        let from = after.map_or(0, |c| c + 1);
        (from..CHANNELS).find(|&c| self.chans[c].active)
    }

    /// The phase after the statistics are done: pulse trains, the link, then detection.
    fn after_sweeps(&self) -> Phase {
        if self.settings.pulse {
            if let Some(ch) = self.swept_after(None) {
                return Phase::Pulse { ch };
            }
        }
        self.after_pulses()
    }

    fn after_pulses(&self) -> Phase {
        if self.settings.link {
            Phase::Link { chunk: 0 }
        } else {
            self.detect_from(None)
        }
    }

    fn detect_from(&self, after: Option<usize>) -> Phase {
        match self.active_after(after) {
            Some(ch) => Phase::Detect { ch, chunk: 0 },
            None => Phase::EndPass,
        }
    }

    /// Do the next piece of the block's work; returns its cost.
    fn step(&mut self) -> f64 {
        let s = self.settings;
        let (next, cost) = match self.phase {
            Phase::Idle => return 0.0,
            Phase::Tables(k) => {
                if k < 2 * WINDOWS {
                    let reject = k >= WINDOWS;
                    let control = control_points(&points(&self.block, reject));
                    let table = if reject {
                        &mut self.reject
                    } else {
                        &mut self.detect
                    };
                    spline_window(&control, k % WINDOWS, table);
                    (Phase::Tables(k + 1), COST_TABLE)
                } else {
                    fill_gaps(&mut self.detect, points(&self.block, false)[3].1);
                    fill_gaps(&mut self.reject, points(&self.block, true)[3].1);
                    self.tabled = Some(self.block);
                    (self.first_pass_phase(), COST_TABLE)
                }
            }
            Phase::Sweep { ch, part, chunk } => {
                let f = self.followers;
                let c = &mut self.chans[ch];
                let range = |k: usize| k * CHUNK..(k + 1) * CHUNK;
                match part {
                    0 => {
                        if chunk == 0 {
                            c.sweep.reset();
                        }
                        c.sweep.forward_chunk(&c.work, &f, range(chunk));
                    }
                    // Backward: the last chunk first.
                    1 => c.sweep.backward_chunk(range(CHUNKS - 1 - chunk)),
                    _ => c.sweep.reconcile_chunk(range(chunk)),
                }
                let next = if chunk + 1 < CHUNKS {
                    Phase::Sweep {
                        ch,
                        part,
                        chunk: chunk + 1,
                    }
                } else if part < 2 {
                    Phase::Sweep {
                        ch,
                        part: part + 1,
                        chunk: 0,
                    }
                } else {
                    match self.swept_after(Some(ch)) {
                        Some(ch) => Phase::Sweep {
                            ch,
                            part: 0,
                            chunk: 0,
                        },
                        None => self.after_sweeps(),
                    }
                };
                (next, COST_SWEEP[part as usize])
            }
            Phase::Pulse { ch } => {
                let sh = &mut self.shared;
                pulse_trains(
                    &mut self.chans[ch].sweep.stat,
                    &mut sh.peaks,
                    &mut sh.matched,
                );
                let next = match self.swept_after(Some(ch)) {
                    Some(ch) => Phase::Pulse { ch },
                    None => self.after_pulses(),
                };
                (next, COST_PULSE)
            }
            Phase::Link { chunk } => {
                let [a, b] = &mut self.chans;
                link(
                    &mut a.sweep.stat,
                    &mut b.sweep.stat,
                    chunk * CHUNK..(chunk + 1) * CHUNK,
                );
                let next = if chunk + 1 < CHUNKS {
                    Phase::Link { chunk: chunk + 1 }
                } else {
                    self.detect_from(None)
                };
                (next, COST_LINK)
            }
            Phase::Detect { ch, chunk } => {
                let c = &mut self.chans[ch];
                if chunk == 0 {
                    c.clicks.clear();
                    c.tracking = Tracking::default();
                }
                for n in chunk * CHUNK..(chunk + 1) * CHUNK {
                    let stat = c.sweep.stat[n];
                    let closed = track(&mut c.tracking, n, stat, &c.sweep.level, &self.detect, &s);
                    let closed = if n + 1 == BLOCK {
                        closed.or(c.tracking.open.take())
                    } else {
                        closed
                    };
                    // Only clicks that start in the samples this block makes final, and are
                    // not handled already.
                    if let Some(click) = closed {
                        if (PRE..PRE + HOP).contains(&click.start)
                            && !c.known(&click)
                            && c.clicks.len() < MAX_CLICKS
                        {
                            c.clicks.push(click);
                        }
                    }
                }
                let next = if chunk + 1 < CHUNKS {
                    Phase::Detect {
                        ch,
                        chunk: chunk + 1,
                    }
                } else {
                    let found = c.clicks.len() as f64;
                    let empty = c.clicks.is_empty();
                    self.budget += (found - self.expected(ch, self.pass)) * self.repair_cost();
                    if self.pass == 0 {
                        self.expect[ch] = found.max(1.0);
                    }
                    if empty {
                        self.detect_from(Some(ch))
                    } else {
                        Phase::Repair { ch, k: 0 }
                    }
                };
                (next, COST_DETECT)
            }
            Phase::Repair { ch, k } => {
                let c = &mut self.chans[ch];
                let click = c.clicks[k];
                match self.shared.repair(c, click, &s, &self.reject) {
                    Outcome::Corrected { big } => {
                        c.corrected += 1;
                        self.counters[CORRECTED].fetch_add(1, Ordering::Relaxed);
                        if big {
                            self.counters[BIG_CORRECTED].fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Outcome::Rejected => {
                        self.counters[REJECTED].fetch_add(1, Ordering::Relaxed);
                    }
                    Outcome::Left => {}
                }
                let c = &self.chans[ch];
                let next = if k + 1 < c.clicks.len() {
                    Phase::Repair { ch, k: k + 1 }
                } else {
                    self.detect_from(Some(ch))
                };
                (next, self.repair_cost())
            }
            Phase::EndPass => {
                self.passes_run += 1;
                self.pass += 1;
                for c in &mut self.chans {
                    // Again while a pass corrects at least 2 new clicks, up to Number of Times.
                    if c.active && (self.pass >= s.passes || c.corrected < 2) {
                        c.active = false;
                    }
                    c.corrected = 0;
                    c.fitted.fill(false);
                }
                // The next pass's cost is now this pass's budget.
                if self.chans.iter().any(|c| c.active) {
                    self.budget += self.pass_cost(self.pass);
                }
                self.plan();
                let next = match self.swept_after(None) {
                    Some(ch) => Phase::Sweep {
                        ch,
                        part: 0,
                        chunk: 0,
                    },
                    None => Phase::WriteBack { ch: 0 },
                };
                (next, 0.0)
            }
            Phase::WriteBack { ch } => {
                let c = &mut self.chans[ch];
                let line = &mut self.lines[ch];
                // The line has moved on a hop since the block was taken.
                for i in PRE..BLOCK {
                    if c.touched[i] {
                        line[TAKE - HOP + i] = c.work[i] as f32;
                    }
                }
                // Spans repaired into the look-ahead are the next block's handled ones.
                for l in &c.list {
                    if l.hi >= PRE + HOP && c.carry.len() < MAX_LIST {
                        c.carry.push(Listed {
                            lo: l.lo.saturating_sub(HOP),
                            hi: l.hi - HOP,
                            state: l.state,
                        });
                    }
                }
                let next = if ch + 1 < CHANNELS {
                    Phase::WriteBack { ch: ch + 1 }
                } else {
                    self.blocks_run += 1;
                    self.planned = self.pass;
                    Phase::Idle
                };
                (next, COST_WRITE)
            }
        };
        self.phase = next;
        self.budget -= cost;
        cost
    }
}

impl NativeEffect for ClickPop {
    fn set(&mut self, index: usize, value: f64) {
        // Taken up by the next block (the tables are rebuilt as the first of its work).
        if let Some(v) = self.values.get_mut(index) {
            *v = value;
        }
    }

    fn latency(&self) -> usize {
        LATENCY
    }

    fn start(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        for ch in &mut self.chans {
            ch.list.clear();
            ch.carry.clear();
        }
        self.filled = 0;
        self.phase = Phase::Idle;
        self.credit = 0.0;
        self.budget = 0.0;
        self.future = 0.0;
        for c in self.counters.iter() {
            c.store(0, Ordering::Relaxed);
        }
    }

    fn readouts(&self) -> Option<Arc<[AtomicU64]>> {
        Some(self.counters.clone())
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        for k in from..to {
            // The block's work, at the rate that finishes what is left by the deadline.
            if self.phase != Phase::Idle {
                if self.filled >= DEADLINE {
                    while self.phase != Phase::Idle {
                        self.step();
                    }
                } else {
                    let left = (self.budget + self.future).max(COST_REPAIR);
                    self.credit += left / (DEADLINE - self.filled) as f64;
                    while self.credit > 0.0 && self.phase != Phase::Idle {
                        self.credit -= self.step();
                    }
                    if self.phase == Phase::Idle {
                        self.credit = 0.0;
                    }
                }
            }
            for (buf, line) in io.iter_mut().zip(self.lines.iter_mut()) {
                let s = buf[k];
                buf[k] = line[self.filled];
                line[LATENCY + self.filled] = s;
            }
            self.filled += 1;
            if self.filled == HOP {
                self.hop();
            }
        }
    }
}

// ---------------------------------------------------------------------------- the scans

/// What a scan learned (its print): which scan, the levels (max, min, mean in dB) and, for
/// Scan For All Levels, the thresholds and how many trial clicks they came from.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scanned {
    pub all: bool,
    pub max: f64,
    pub min: f64,
    pub mean: f64,
    /// Max, Avg and Min Threshold; Max, Avg and Min Rejection Threshold (ids 0-2, 6-8).
    pub thresholds: Option<[f64; 6]>,
    pub clicks: u32,
}

/// A print's numbers as [`Scanned`], if it is one.
pub fn scanned(print: &[f32]) -> Option<Scanned> {
    let v = |i: usize| f64::from(print[i]);
    match print.len() {
        4 if print[0] == 0.0 => Some(Scanned {
            all: false,
            max: v(1),
            min: v(2),
            mean: v(3),
            thresholds: None,
            clicks: 0,
        }),
        11 if print[0] == 1.0 => Some(Scanned {
            all: true,
            max: v(1),
            min: v(2),
            mean: v(3),
            thresholds: Some([v(4), v(5), v(6), v(7), v(8), v(9)]),
            clicks: print[10] as u32,
        }),
        _ => None,
    }
}

/// What a print sets: the levels as Scan For Threshold Levels writes them (the maximum to
/// Max Level, the minimum to *Avg* Level and the mean to *Min* Level: the labels are swapped,
/// so the "Avg" point is the lowest), and the thresholds for Scan For All Levels.
fn sets(print: &[f32]) -> Vec<(usize, f64)> {
    let Some(s) = scanned(print) else {
        return Vec::new();
    };
    let mut out = vec![(MAX_LEVEL, s.max), (AVG_LEVEL, s.min), (MIN_LEVEL, s.mean)];
    if let Some(t) = s.thresholds {
        out.extend([
            (MAX_THRESHOLD, t[0]),
            (AVG_THRESHOLD, t[1]),
            (MIN_THRESHOLD, t[2]),
            (MAX_REJECT, t[3]),
            (AVG_REJECT, t[4]),
            (MIN_REJECT, t[5]),
        ]);
    }
    out
}

/// Rounded down to 0.1, in a range.
fn tenth(v: f64, lo: f64, hi: f64) -> f64 {
    ((v * 10.0).floor() / 10.0).clamp(lo, hi)
}

/// Each channel's level per sample as Scan For Threshold Levels measures it: the pre-emphasis,
/// |y| followed with attack 0.5 ms and release 80 ms (from the peak of the first 36 samples),
/// 20·log10(env + 3.05e-15) + 2.56 dB.
pub fn levels(rate: f64, x: &[f32]) -> Vec<f64> {
    let f = Followers::new(rate);
    let mut v = 0.0;
    let y: Vec<f64> = x
        .iter()
        .map(|s| {
            let n = f64::from(*s) + PRE_POLE * v;
            let y = PRE_B0 * n - PRE_B1 * v;
            v = n;
            y
        })
        .collect();
    let mut env = y[..36.min(y.len())]
        .iter()
        .fold(0.0f64, |m, v| m.max(v.abs()));
    y.iter()
        .map(|y| {
            env = f.envelope(env, y.abs());
            db_of(env)
        })
        .collect()
}

/// The highest, lowest and mean level over every channel of `audio`, each rounded down to
/// 0.1 dB and within the levels' range (-120..10 dB).
pub fn measure(rate: f64, audio: &[Vec<f32>]) -> Result<(f64, f64, f64)> {
    let len = audio.first().map_or(0, Vec::len);
    if len == 0 {
        bail!("select the audio to scan: the selection is empty");
    }
    let (mut max, mut min, mut sum, mut n) = (f64::MIN, f64::MAX, 0.0, 0usize);
    for x in audio {
        for l in levels(rate, x) {
            max = max.max(l);
            min = min.min(l);
            sum += l;
            n += 1;
        }
    }
    if max < -120.0 {
        bail!("nothing to scan: the selection is silent");
    }
    Ok((
        tenth(max, -120.0, 10.0),
        tenth(min, -120.0, 10.0),
        tenth(sum / n as f64, -120.0, 10.0),
    ))
}

/// The value at quantile `q` of `values` weighted by `weights` (the smallest whose cumulative
/// weight reaches q of the total), or `None` without weight.
fn quantile(values: &[(f64, [f64; 3])], which: usize, q: f64) -> Option<f64> {
    let total: f64 = values.iter().map(|v| v.1[which]).sum();
    if total <= 1e-9 {
        return None;
    }
    let mut acc = 0.0;
    for (v, w) in values {
        acc += w[which];
        if acc >= q * total {
            return Some(*v);
        }
    }
    values.last().map(|v| v.0)
}

/// How much a level counts towards each of the three points (low, middle, top): raised cosines
/// between neighbouring points, all to the end point outside them.
fn weights(level: f64, low: f64, mid: f64, top: f64) -> [f64; 3] {
    let rise = |u: f64| 0.5 - 0.5 * (PI * u.clamp(0.0, 1.0)).cos();
    if level <= low {
        [1.0, 0.0, 0.0]
    } else if level >= top {
        [0.0, 0.0, 1.0]
    } else if level < mid {
        let w = if mid > low {
            rise((level - low) / (mid - low))
        } else {
            1.0
        };
        [1.0 - w, w, 0.0]
    } else {
        let w = if top > mid {
            rise((level - mid) / (top - mid))
        } else {
            1.0
        };
        [0.0, 1.0 - w, w]
    }
}

/// A scan of `audio` (one buffer per channel at `rate`) for plain `values`: 0 is Scan For
/// Threshold Levels (the levels only), 1 Scan For All Levels (the levels, and the thresholds
/// from a trial detection). Its print, for [`scanned`].
///
/// Scan For All Levels (ours on the study's outline, whose statistics weren't traced): every
/// channel's statistic over the whole selection; trial clicks wherever it is over Sensitivity,
/// ended by ⌊Discrimination/4⌋ + 6 samples under it; for each its peak statistic and the
/// square root of its verification score, weighted towards the three points by its level
/// (raised cosines between the scanned levels). The detect thresholds are the quantiles
/// q1^18 (Avg, the low point), q1^10 (Min, the middle) and q1^5 (Max) of the peaks, q1 =
/// 1 - 1/(Discrimination + 8); the reject thresholds q2^3.8, q2^1.5 and q2^0.9 of the scores,
/// q2 = 1 - 1/(Discrimination + 1). A point no trial click reaches takes 9 (detect) or 10
/// (reject). Each is rounded down to 0.1 and kept in 1..150.
pub fn scan(rate: f64, values: &[f64], audio: &[Vec<f32>], scan: usize) -> Result<Vec<f32>> {
    let (max, min, mean) = measure(rate, audio)?;
    if scan == 0 {
        return Ok(vec![0.0, max as f32, min as f32, mean as f32]);
    }
    let s = Settings::of(values);
    let f = Followers::new(rate);
    let sensitivity = values[SENSITIVITY];
    let d = values[DISCRIMINATION];
    let gap = (0.25 * d + 6.0) as usize;
    let mut verifier = Verifier::new();
    let mut found: Vec<(f64, f64, f64)> = Vec::new();
    for x in audio {
        let x: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
        let mut sweep = Sweep::new(x.len());
        sweep.run(&x, &f);
        let mut open: Option<(usize, usize, f64)> = None;
        let mut below = 0;
        for n in 0..x.len() {
            let stat = sweep.stat[n];
            if stat > sensitivity {
                let c = open.get_or_insert((n, n, 0.0));
                c.1 = n;
                c.2 = c.2.max(stat);
                below = 0;
            } else if open.is_some() {
                below += 1;
            }
            if open.is_some() && (below >= gap || n + 1 == x.len()) {
                let (start, end, peak) = open.take().expect("open");
                let click = Click {
                    start,
                    end,
                    peak,
                    big: false,
                };
                let (lo, hi) = s.span(&click);
                if lo >= VERIFY_REACH + VERIFY_SHIFT
                    && hi + VERIFY_REACH + VERIFY_SHIFT < x.len()
                    && found.len() < 100_000
                {
                    let centre = (lo + hi) / 2;
                    let score = verifier.score(&x, lo, hi, centre).max(0.0).sqrt();
                    found.push((peak, score, sweep.level[start]));
                }
            }
        }
    }
    let weighted = |pick: fn(&(f64, f64, f64)) -> f64| -> Vec<(f64, [f64; 3])> {
        let mut v: Vec<(f64, [f64; 3])> = found
            .iter()
            .map(|c| (pick(c), weights(c.2, min, mean, max)))
            .collect();
        v.sort_by(|a, b| a.0.total_cmp(&b.0));
        v
    };
    let peaks = weighted(|c| c.0);
    let scores = weighted(|c| c.1);
    let q1 = 1.0 - 1.0 / (d + 8.0);
    let q2 = 1.0 - 1.0 / (d + 1.0);
    let at = |v: &[(f64, [f64; 3])], which: usize, q: f64, fallback: f64| {
        tenth(quantile(v, which, q).unwrap_or(fallback), 1.0, 150.0)
    };
    let thresholds = [
        at(&peaks, 2, q1.powf(5.0), 9.0),
        at(&peaks, 0, q1.powf(18.0), 9.0),
        at(&peaks, 1, q1.powf(10.0), 9.0),
        at(&scores, 2, q2.powf(0.9), 10.0),
        at(&scores, 0, q2.powf(3.8), 10.0),
        at(&scores, 1, q2.powf(1.5), 10.0),
    ];
    let mut print = vec![1.0, max as f32, min as f32, mean as f32];
    print.extend(thresholds.iter().map(|v| *v as f32));
    print.push(found.len() as f32);
    Ok(print)
}

/// The statistic and the level of a channel, as the effect computes them over a block, over
/// all of `x` (for tests and the editor's numbers).
pub fn statistic(rate: f64, x: &[f32]) -> (Vec<f64>, Vec<f64>) {
    let x: Vec<f64> = x.iter().map(|v| f64::from(*v)).collect();
    let mut sweep = Sweep::new(x.len());
    sweep.run(&x, &Followers::new(rate));
    (sweep.stat, sweep.level)
}

/// The detection threshold the effect starts from at a level (dB): the detect curve's entry,
/// clamped to 6..150.
pub fn threshold_at(values: &[f64], level: f64) -> f64 {
    curve(values, false)[entry(level)].clamp(6.0, 150.0)
}

#[cfg(test)]
#[path = "click_pop_tests.rs"]
mod tests;
