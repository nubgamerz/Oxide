//! Click Remover: clicks found and rebuilt as the audio plays, in fixed frames, with
//! detection and repair kernels of our own:
//!
//! - **Parameters:** Threshold and Complexity, 0..100 %. The engine threshold is
//!   θ = 26 + 0.99·Threshold (55 at the default) and the size S = trunc(0.99·Complexity + 1.5)
//!   (16), which also sets the AR model's order p = S/2 + 20 (28).
//! - **Framing:** each channel on its own; every 2048 samples a frame of the last
//!   3072, with 130 samples of earlier, already repaired, context before it, goes through two
//!   passes of AR fit, prediction error, detection and repair; the latency is 4096 samples at
//!   any rate. The stream decides where the frames fall (a transport start restarts them), so
//!   playback and export cut it alike at any block size.
//! - **Ours, each frame:** its first 2048 samples are final when it is done; the last 1024 are
//!   only the right-hand context of runs near that boundary, and the next frame looks at them
//!   again from the original input. The output is 4096 behind, a hop more than that needs, so
//!   the four passes (two a channel) are spread over the 1024 samples after the hop, 256 apart:
//!   no block carries a whole frame's work.
//!
//! **The detector (ours):** an AR model of order p is fitted to the frame (Hamming-windowed
//! autocorrelation, a -80 dB white-noise floor, Levinson-Durbin). Its forward prediction error
//! starts exactly where a click starts and its backward one (the same coefficients, reversed in
//! time) ends exactly where it ends, while each smears the click over p samples on its own side.
//! Each error is judged against its level in blocks of 256 samples (the lower quartile of its
//! magnitude, as a Gaussian's standard deviation, at least one 16-bit step, 2^-15; the highest
//! of the block and its two neighbours, so silence beside a note is judged by the note). A
//! sample beyond k times that is flagged; flags closer than p group into a cluster, whose click
//! runs from the first forward flag to the last backward one. A cluster longer than the longest
//! click, 2·S + 8 samples (40 at the default), is split where both errors are flagged at once
//! if that is sparse (clicks close together); dense, it is one long event and left alone.
//! **k = θ / 8** (6.9 at the default, 4.75 at θ 38, 14.75 at θ 118): on Gaussian noise the
//! default flags about one sample in 10^11, θ 38 one in 500 000.
//!
//! **The repair (ours):** each run, widened by 2 samples a side, is rebuilt by least-squares AR
//! interpolation: the missing samples are those that minimise the model's prediction error over
//! every equation they appear in, solved by a banded Cholesky. Samples outside a run come back
//! bit for bit, so audio without clicks is exactly the input 4096 samples later. A sound that
//! starts from digital silence at full tilt is a step, and is smoothed like one.

use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

// Parameter indices.
const THRESHOLD: usize = 0;
const COMPLEXITY: usize = 1;

/// The hop, frame and context (samples), and its latency at every rate.
const HOP: usize = 2048;
const FRAME: usize = 3072;
const CONTEXT: usize = 130;
pub const LATENCY: usize = 4096;
/// A frame with its context.
const SPAN: usize = CONTEXT + FRAME;
/// Per channel: the 4096 samples the output is behind and the hop being filled.
const LINE: usize = LATENCY + HOP;
/// Where the frame (with its context) starts in the line when a hop is complete.
const START: usize = LINE - SPAN;
/// The stretches (samples) of the frame whose error levels are judged apart.
const BLOCK: usize = 256;
const BLOCKS: usize = FRAME / BLOCK;

/// The largest size (Complexity 100) and order.
const MAX_SIZE: usize = 100;
const MAX_ORDER: usize = MAX_SIZE / 2 + 20;
/// Samples added each side of a run before it is rebuilt.
const PAD: usize = 2;
/// The longest run ever rebuilt (padding included), and the runs one pass takes in a frame.
const MAX_FILL: usize = longest(MAX_SIZE) + 2 * PAD;
const MAX_RUNS: usize = 256;
/// The error's level is never taken under one 16-bit step (the engine counts in them).
const FLOOR: f64 = 1.0 / 32_768.0;
/// The lower quartile of a Gaussian's magnitude, in standard deviations.
const QUARTER: f64 = 0.318_639;
/// The AR fit's white-noise floor, relative to the frame's power (-80 dB).
const WHITE: f64 = 1e-8;

/// The scale of the percentages.
pub(crate) const SCALE: f64 = 0.99;

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "%",
    min: 0.0,
    max: 100.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

static PARAMS: [ParamSpec; 2] = [
    // Higher: fewer clicks found.
    ParamSpec {
        name: "Threshold",
        default: 29.0 / SCALE,
        ..PARAM
    },
    // Higher: longer clicks repaired, with a longer model.
    ParamSpec {
        name: "Complexity",
        default: 15.0 / SCALE,
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "click-remover",
    name: "Click Remover",
    category: "Restoration",
    params: &PARAMS,
    make: |_| Box::new(ClickRemover::new()),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The longest click (samples) a size S repairs.
const fn longest(size: usize) -> usize {
    2 * size + 8
}

/// What the engine runs with, from the plain parameter values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Engine {
    /// The engine threshold θ.
    pub threshold: f64,
    /// The size S.
    pub size: usize,
    /// The AR model's order.
    pub order: usize,
    /// Ours: the multiple of the prediction error's robust level that flags a sample.
    pub multiple: f64,
    /// Ours: the longest click (samples) repaired.
    pub longest: usize,
}

pub fn engine(values: &[f64]) -> Engine {
    let threshold = 26.0 + SCALE * values[THRESHOLD];
    let size = ((SCALE * values[COMPLEXITY] + 1.5) as usize).clamp(1, MAX_SIZE);
    Engine {
        threshold,
        size,
        order: size / 2 + 20,
        multiple: threshold / 8.0,
        longest: longest(size),
    }
}

/// One frame's work: the scratch every channel shares, made once.
struct Repair {
    engine: Engine,
    window: Vec<f64>,
    /// The frame with its context, as it is being repaired.
    work: Vec<f64>,
    windowed: Vec<f64>,
    forward: Vec<f64>,
    backward: Vec<f64>,
    sorted: Vec<f64>,
    /// Each block's flagging limit for the forward and backward errors.
    limit_f: [f64; BLOCKS],
    limit_b: [f64; BLOCKS],
    touched: Vec<bool>,
    /// The frame's autocorrelation.
    r: Vec<f64>,
    /// The model and the interpolation.
    lsar: Lsar,
    runs: Vec<(usize, usize)>,
    /// Runs rebuilt since the start (both passes, both channels): for tests.
    repaired: u64,
}

impl Repair {
    fn new() -> Self {
        let defaults: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let window = (0..SPAN)
            .map(|j| {
                0.54 - 0.46 * (2.0 * std::f64::consts::PI * j as f64 / (SPAN - 1) as f64).cos()
            })
            .collect();
        Self {
            engine: engine(&defaults),
            window,
            work: vec![0.0; SPAN],
            windowed: vec![0.0; SPAN],
            forward: vec![0.0; SPAN],
            backward: vec![0.0; SPAN],
            sorted: vec![0.0; SPAN],
            limit_f: [0.0; BLOCKS],
            limit_b: [0.0; BLOCKS],
            touched: vec![false; SPAN],
            r: vec![0.0; MAX_ORDER + 1],
            lsar: Lsar::new(MAX_FILL, MAX_ORDER),
            runs: Vec::with_capacity(MAX_RUNS),
            repaired: 0,
        }
    }

    /// One pass over `frame`: fit, prediction errors, detection, repair.
    fn pass(&mut self, frame: &mut Frame) {
        std::mem::swap(&mut self.work, &mut frame.work);
        std::mem::swap(&mut self.touched, &mut frame.touched);
        self.fit();
        self.residuals();
        self.detect();
        for k in 0..self.runs.len() {
            let (lo, hi) = self.runs[k];
            self.fill(lo, hi);
        }
        std::mem::swap(&mut self.work, &mut frame.work);
        std::mem::swap(&mut self.touched, &mut frame.touched);
    }

    /// The AR model of the frame as it is now: windowed autocorrelation, Levinson-Durbin.
    fn fit(&mut self) {
        let p = self.engine.order;
        for ((x, w), s) in self.windowed.iter_mut().zip(&self.window).zip(&self.work) {
            *x = w * s;
        }
        autocorrelate(&self.windowed, p, &mut self.r);
        self.lsar.model(&self.r, p);
    }

    /// Forward and backward prediction errors over the frame (the backward one only where the
    /// samples after it are all in the frame).
    fn residuals(&mut self) {
        let p = self.engine.order;
        let b = &self.lsar.b[..=p];
        let x = &self.work;
        // From the context on too (as far as the model reaches), for the level before the
        // frame. A tap at a time over the whole frame, which vectorises.
        self.forward.fill(0.0);
        self.backward.fill(0.0);
        for (k, &b) in b.iter().enumerate() {
            for (e, x) in self.forward[p..].iter_mut().zip(&x[p - k..SPAN - k]) {
                *e += b * x;
            }
            for (e, x) in self.backward[..SPAN - p]
                .iter_mut()
                .zip(&x[k..SPAN - p + k])
            {
                *e += b * x;
            }
        }
    }

    /// Each block's flagging limit for error `e`, defined over `valid`: `multiple` times the
    /// highest robust level of the block and its two neighbours (before the first block, the
    /// context). The level is the lower quartile of the error's magnitude over [`QUARTER`] (a
    /// Gaussian's standard deviation), at least [`FLOOR`]: a click of up to half a block barely
    /// moves it, and a quiet stretch beside a loud one (silence before a note, after the end)
    /// is judged by the loud one's level.
    fn limits(
        sorted: &mut [f64],
        e: &[f64],
        valid: std::ops::Range<usize>,
        multiple: f64,
        out: &mut [f64],
    ) {
        // levels[0] is the context's; levels[k + 1] block k's.
        let mut levels = [0.0; BLOCKS + 1];
        for (k, level) in levels.iter_mut().enumerate() {
            let from = (CONTEXT + k * BLOCK).saturating_sub(BLOCK).max(valid.start);
            let to = (CONTEXT + k * BLOCK).min(valid.end);
            if to < from + BLOCK / 4 {
                // Too little to judge by: its neighbours' level.
                continue;
            }
            let n = to - from;
            for (s, v) in sorted.iter_mut().zip(&e[from..to]) {
                *s = v.abs();
            }
            let (_, quarter, _) = sorted[..n].select_nth_unstable_by(n / 4, f64::total_cmp);
            *level = *quarter / QUARTER;
        }
        for (k, limit) in out.iter_mut().enumerate() {
            let near = &levels[k..(k + 3).min(BLOCKS + 1)];
            *limit = multiple * near.iter().fold(FLOOR, |m, l| m.max(*l));
        }
    }

    /// The runs to rebuild in this pass, in order, padded and not overlapping.
    fn detect(&mut self) {
        self.runs.clear();
        let p = self.engine.order;
        let k = self.engine.multiple;
        Self::limits(
            &mut self.sorted,
            &self.forward,
            p..SPAN,
            k,
            &mut self.limit_f,
        );
        Self::limits(
            &mut self.sorted,
            &self.backward,
            0..SPAN - p,
            k,
            &mut self.limit_b,
        );
        let block = |n: usize| (n - CONTEXT) / BLOCK;
        let f = |e: &Self, n: usize| e.forward[n].abs() > e.limit_f[block(n)];
        let g = |e: &Self, n: usize| n + p < SPAN && e.backward[n].abs() > e.limit_b[block(n)];
        let mut n = CONTEXT;
        while n < SPAN {
            if !(f(self, n) || g(self, n)) {
                n += 1;
                continue;
            }
            let (start, mut last) = (n, n);
            let mut m = n + 1;
            while m < SPAN && m - last <= p {
                if f(self, m) || g(self, m) {
                    last = m;
                }
                m += 1;
            }
            let first_f = (start..=last).find(|&k| f(self, k));
            let last_g = (start..=last).rev().find(|&k| g(self, k));
            let (lo, hi) = match (first_f, last_g) {
                (Some(a), Some(b)) => (a.min(b), a.max(b)),
                (Some(a), None) => (a, a),
                (None, Some(b)) => (b, b),
                (None, None) => (start, start),
            };
            let core = (lo..=hi).filter(|&k| f(self, k) && g(self, k)).count();
            if hi - lo < self.engine.longest {
                self.push(lo, hi);
            } else if 2 * core < hi - lo + 1 {
                // Clicks close together (both errors are flagged only at the clicks, sparsely):
                // where both are flagged, in pieces short enough to be clicks. Dense, it is one
                // long event, and left alone.
                let mut k = lo;
                while k <= hi {
                    if !(f(self, k) && g(self, k)) {
                        k += 1;
                        continue;
                    }
                    let (from, mut to) = (k, k);
                    let mut j = k + 1;
                    while j <= hi && j - to <= 2 * PAD + 1 {
                        if f(self, j) && g(self, j) {
                            to = j;
                        }
                        j += 1;
                    }
                    if to - from < self.engine.longest {
                        self.push(from, to);
                    }
                    k = to + 1;
                }
            }
            n = last + 1;
        }
    }

    /// Add run `lo..=hi`, padded, inside the frame (never the context), after the last one.
    fn push(&mut self, lo: usize, hi: usize) {
        let mut lo = lo.saturating_sub(PAD).max(CONTEXT);
        let hi = (hi + PAD).min(SPAN - 1);
        if let Some(&(_, end)) = self.runs.last() {
            lo = lo.max(end + 1);
        }
        if lo <= hi && self.runs.len() < MAX_RUNS {
            self.runs.push((lo, hi));
        }
    }

    /// Rebuild samples `lo..=hi` of the frame by least-squares AR interpolation.
    fn fill(&mut self, lo: usize, hi: usize) {
        if !self.lsar.fill(&mut self.work, lo, hi) {
            return self.restore(lo, hi);
        }
        self.touched[lo..=hi].fill(true);
        self.repaired += 1;
    }

    /// A run the interpolation can't solve (never expected: the matrix is the error filter's
    /// Gram matrix, positive definite) becomes the straight line between its neighbours.
    fn restore(&mut self, lo: usize, hi: usize) {
        let before = self.work[lo - 1];
        let after = self.work.get(hi + 1).copied().unwrap_or(before);
        let m = (hi - lo + 2) as f64;
        for (i, x) in self.work[lo..=hi].iter_mut().enumerate() {
            *x = before + (i + 1) as f64 / m * (after - before);
        }
        self.touched[lo..=hi].fill(true);
    }
}

/// The autocorrelation of `windowed` at lags 0..=`order` into `r`, lag 0 raised by the
/// -80 dB white-noise floor ([`WHITE`]), for [`Lsar::model`].
pub(super) fn autocorrelate(windowed: &[f64], order: usize, r: &mut [f64]) {
    let n = windowed.len();
    for l in 0..=order {
        r[l] = dot(&windowed[..n - l], &windowed[l..]);
    }
    r[0] *= 1.0 + WHITE;
}

/// Our click repair, shared by the Click Remover and Click & Pop Repair: an
/// AR model from an autocorrelation (Levinson-Durbin), and the least-squares AR interpolation
/// of a run of samples under it (the values that minimise the model's prediction error over
/// every equation they appear in, by a banded Cholesky). Made once with room for the longest
/// run and the highest order; no allocation after.
pub(super) struct Lsar {
    /// The order of the model in use.
    order: usize,
    max_order: usize,
    /// The model (a), Levinson's scratch, the error filter (b = 1, -a) and its own
    /// autocorrelation (the interpolation's matrix away from the buffer's end).
    a: Vec<f64>,
    previous: Vec<f64>,
    pub(super) b: Vec<f64>,
    c: Vec<f64>,
    /// The interpolation's normal equations (the lower band, `max_order + 1` wide), its
    /// right-hand side and the prediction error with the run zeroed.
    band: Vec<f64>,
    rhs: Vec<f64>,
    error: Vec<f64>,
}

impl Lsar {
    pub(super) fn new(max_fill: usize, max_order: usize) -> Self {
        Self {
            order: 0,
            max_order,
            a: vec![0.0; max_order + 1],
            previous: vec![0.0; max_order + 1],
            b: vec![0.0; max_order + 1],
            c: vec![0.0; max_order + 1],
            band: vec![0.0; max_fill * (max_order + 1)],
            rhs: vec![0.0; max_fill],
            error: vec![0.0; max_fill + max_order],
        }
    }

    /// The model of order `p` (at most the largest) for autocorrelation `r` (lags 0..=p):
    /// Levinson-Durbin, stopping at a reflection of 1 or more.
    pub(super) fn model(&mut self, r: &[f64], p: usize) {
        self.order = p;
        self.a.fill(0.0);
        let mut err = r[0];
        if err > 1e-30 {
            for i in 1..=p {
                let acc = r[i] - (1..i).map(|j| self.a[j] * r[i - j]).sum::<f64>();
                let k = acc / err;
                if k.is_nan() || k.abs() >= 1.0 {
                    break;
                }
                self.previous[..i].copy_from_slice(&self.a[..i]);
                self.a[i] = k;
                for j in 1..i {
                    self.a[j] = self.previous[j] - k * self.previous[i - j];
                }
                err *= 1.0 - k * k;
            }
        }
        self.b[0] = 1.0;
        for k in 1..=p {
            self.b[k] = -self.a[k];
        }
        for d in 0..=p {
            self.c[d] = (0..=p - d).map(|k| self.b[k] * self.b[k + d]).sum();
        }
    }

    /// Rebuild `work[lo..=hi]` under the model (`lo` at least its order in): the values that
    /// minimise the prediction error over every equation they appear in, up to the end of
    /// `work`. False (the run zeroed) if the equations can't be solved, which is never
    /// expected: the matrix is the error filter's Gram matrix, positive definite.
    pub(super) fn fill(&mut self, work: &mut [f64], lo: usize, hi: usize) -> bool {
        let p = self.order;
        let m = hi - lo + 1;
        let end = (hi + p).min(work.len() - 1);
        let last = end - lo;
        let b = &self.b[..=p];
        for x in &mut work[lo..=hi] {
            *x = 0.0;
        }
        // The prediction errors with the run at zero: what the known samples contribute.
        for q in 0..=last {
            let n = lo + q;
            self.error[q] = b.iter().enumerate().map(|(k, b)| b * work[n - k]).sum();
        }
        // Normal equations: M[i][j] = Σ_q b[q-i]·b[q-j], rhs[i] = -Σ_q b[q-i]·error[q], over
        // the equations q the unknown i appears in; banded (|i - j| <= p), and Toeplitz (the
        // filter's own autocorrelation) away from the buffer's end. Entry (i, j), j <= i, is
        // `band[i * w + i - j]`.
        let w = self.max_order + 1;
        for i in 0..m {
            let top = (i + p).min(last);
            self.rhs[i] = -(i..=top).map(|q| b[q - i] * self.error[q]).sum::<f64>();
            for j in i.saturating_sub(p)..=i {
                let d = i - j;
                self.band[i * w + d] = if j + p <= last {
                    self.c[d]
                } else {
                    (i..=(j + p).min(last)).map(|q| b[q - i] * b[q - j]).sum()
                };
            }
        }
        // Banded Cholesky in place (lower triangle), then the two triangular solves.
        let at = |i: usize, j: usize| i * w + i - j;
        let mat = &mut self.band;
        for j in 0..m {
            let k0 = j.saturating_sub(p);
            let mut s = mat[at(j, j)];
            for k in k0..j {
                s -= mat[at(j, k)] * mat[at(j, k)];
            }
            if s.is_nan() || s <= 0.0 {
                return false;
            }
            let d = s.sqrt();
            mat[at(j, j)] = d;
            for i in j + 1..(j + p + 1).min(m) {
                let mut s = mat[at(i, j)];
                for k in i.saturating_sub(p)..j {
                    s -= mat[at(i, k)] * mat[at(j, k)];
                }
                mat[at(i, j)] = s / d;
            }
        }
        for i in 0..m {
            let mut s = self.rhs[i];
            for k in i.saturating_sub(p)..i {
                s -= mat[at(i, k)] * self.rhs[k];
            }
            self.rhs[i] = s / mat[at(i, i)];
        }
        for i in (0..m).rev() {
            let mut s = self.rhs[i];
            for k in i + 1..(i + p + 1).min(m) {
                s -= mat[at(k, i)] * self.rhs[k];
            }
            self.rhs[i] = s / mat[at(i, i)];
        }
        for (x, v) in work[lo..=hi].iter_mut().zip(&self.rhs) {
            *x = *v;
        }
        true
    }
}

/// The dot product of `a` and `b` (the shorter's length), in four running sums so it
/// vectorises; the same order every time, so the same result.
fn dot(a: &[f64], b: &[f64]) -> f64 {
    let n = a.len().min(b.len());
    let (ca, cb) = (a[..n].chunks_exact(4), b[..n].chunks_exact(4));
    let tail: f64 = ca
        .remainder()
        .iter()
        .zip(cb.remainder())
        .map(|(x, y)| x * y)
        .sum();
    let mut acc = [0.0f64; 4];
    for (x, y) in ca.zip(cb) {
        for i in 0..4 {
            acc[i] += x[i] * y[i];
        }
    }
    (acc[0] + acc[1]) + (acc[2] + acc[3]) + tail
}

/// A channel's frame between its passes: the samples as repaired so far, and which were.
struct Frame {
    work: Vec<f64>,
    touched: Vec<bool>,
}

/// The passes (two per channel) a frame's work is split into, and the samples between them:
/// spread over the first 1024 samples after the hop, before the output reaches the first
/// sample they make final, so no one block carries a whole frame's work.
const STAGES: usize = 2 * CHANNELS;
const STAGE_EVERY: usize = (FRAME - HOP) / STAGES;

struct ClickRemover {
    values: [f64; 2],
    /// Per channel: the last 4096 samples (the first 3072 final, or about to be) and the hop
    /// filling.
    lines: [Vec<f32>; CHANNELS],
    frames: [Frame; CHANNELS],
    filled: usize,
    /// The next pass to run (`STAGES`: none waiting).
    stage: usize,
    repair: Repair,
}

impl ClickRemover {
    fn new() -> Self {
        let mut values = [0.0; 2];
        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        Self {
            values,
            lines: std::array::from_fn(|_| vec![0.0; LINE]),
            frames: std::array::from_fn(|_| Frame {
                work: vec![0.0; SPAN],
                touched: vec![false; SPAN],
            }),
            filled: 0,
            stage: STAGES,
            repair: Repair::new(),
        }
    }

    /// A hop is complete: take each channel's frame (the last 3072 samples and the context
    /// before them) and move the lines on by a hop.
    fn hop(&mut self) {
        for (line, frame) in self.lines.iter_mut().zip(self.frames.iter_mut()) {
            for (w, s) in frame.work.iter_mut().zip(&line[START..]) {
                *w = f64::from(*s);
            }
            frame.touched.fill(false);
            line.copy_within(HOP.., 0);
        }
        self.filled = 0;
        self.stage = 0;
    }

    /// Run the waiting pass; after a channel's second, write what it rebuilt of the frame's
    /// first 2048 samples into the line (moved on by a hop since the frame was taken).
    fn run_stage(&mut self) {
        let c = self.stage / 2;
        self.repair.pass(&mut self.frames[c]);
        if self.stage % 2 == 1 {
            let frame = &self.frames[c];
            let done = START - HOP + CONTEXT..START - HOP + CONTEXT + HOP;
            for ((s, w), t) in self.lines[c][done]
                .iter_mut()
                .zip(&frame.work[CONTEXT..CONTEXT + HOP])
                .zip(&frame.touched[CONTEXT..CONTEXT + HOP])
            {
                if *t {
                    *s = *w as f32;
                }
            }
        }
        self.stage += 1;
    }
}

impl NativeEffect for ClickRemover {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            *v = value;
            // From the next pass; the buffers are sized for the largest order.
            self.repair.engine = engine(&self.values);
        }
    }

    fn latency(&self) -> usize {
        LATENCY
    }

    fn start(&mut self) {
        for line in &mut self.lines {
            line.fill(0.0);
        }
        self.filled = 0;
        self.stage = STAGES;
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        for k in from..to {
            if self.stage < STAGES && self.filled == self.stage * STAGE_EVERY {
                self.run_stage();
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

/// What the effect does to one channel's `range` (the Diagnostics' Click Repair): the
/// channel as the insert would play it from a start at `range.start`, `values` its plain
/// parameter values. Returns the range's samples as repaired and which of them were rebuilt.
///
/// The same frames, passes and kernels as the insert: frame k is taken when `range.start` +
/// 2048·(k + 1) samples have come in, its context from the samples already final, and makes
/// its first 2048 samples final. Where the insert has silence (before the start of the
/// audio, after its end) this has `x`'s samples when there are any, so a range's edges are
/// judged with the audio around them; at the ends of `x` it is the insert's silence, so for
/// the whole of `x` the result is the insert's output, 4096 samples earlier, bit for bit.
pub fn repair_offline(
    x: &[f32],
    range: std::ops::Range<usize>,
    values: &[f64],
) -> (Vec<f32>, Vec<bool>) {
    let (a, b) = (range.start, range.end.min(x.len()));
    if b <= a {
        return (Vec::new(), Vec::new());
    }
    let mut repair = Repair::new();
    repair.engine = engine(values);
    let mut frame = Frame {
        work: vec![0.0; SPAN],
        touched: vec![false; SPAN],
    };
    // The frames whose final samples reach `b`: frame k makes `a + 2048k - 1024` on final.
    let frames = (b - a + HOP / 2).div_ceil(HOP);
    // y[i] is position `a - SPAN + i`; the last frame ends at `a + 2048·frames`.
    let len = SPAN + HOP * frames;
    let mut y: Vec<f32> = (0..len)
        .map(|i| {
            (a + i)
                .checked_sub(SPAN)
                .and_then(|p| x.get(p))
                .copied()
                .unwrap_or(0.0)
        })
        .collect();
    let mut rebuilt = vec![false; len];
    for k in 0..frames {
        let at = HOP * (k + 1);
        for (w, s) in frame.work.iter_mut().zip(&y[at..at + SPAN]) {
            *w = f64::from(*s);
        }
        frame.touched.fill(false);
        repair.pass(&mut frame);
        repair.pass(&mut frame);
        let done = at + CONTEXT..at + CONTEXT + HOP;
        for (((s, r), w), t) in y[done.clone()]
            .iter_mut()
            .zip(&mut rebuilt[done])
            .zip(&frame.work[CONTEXT..CONTEXT + HOP])
            .zip(&frame.touched[CONTEXT..CONTEXT + HOP])
        {
            if *t {
                *s = *w as f32;
                *r = true;
            }
        }
    }
    (
        y[SPAN..SPAN + b - a].to_vec(),
        rebuilt[SPAN..SPAN + b - a].to_vec(),
    )
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    /// The tests' own settings (Threshold %, Complexity %), each set so the engine gets a whole
    /// θ and S: the defaults (θ 55, S 16); deep (θ 40, S 91: long clicks, found low); medium
    /// (θ 76, S 51); light (θ 116, S 21: only the plainest clicks).
    const VARIANTS: [(&str, [f64; 2]); 4] = [
        ("the defaults", [29.0 / SCALE, 15.0 / SCALE]),
        ("deep", [14.0 / SCALE, 90.0 / SCALE]),
        ("medium", [50.0 / SCALE, 50.0 / SCALE]),
        ("light", [90.0 / SCALE, 20.0 / SCALE]),
    ];

    fn variant(name: &str) -> [f64; 2] {
        VARIANTS
            .iter()
            .find(|(n, _)| *n == name)
            .map(|(_, v)| *v)
            .expect("a test setting")
    }

    const RATE: f64 = 48_000.0;

    fn settings(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    /// Gaussian noise of standard deviation `sigma` from a fixed seed (xorshift, Box-Muller).
    fn gauss(n: usize, seed: u64, sigma: f64) -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut uniform = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64
        };
        (0..n)
            .map(|_| {
                let (u, v) = (uniform(), uniform());
                (sigma * (-2.0 * u.ln()).sqrt() * (2.0 * PI * v).cos()) as f32
            })
            .collect()
    }

    fn tone(n: usize, hz: f64, amp: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * hz * i as f64 / RATE).sin()) as f32)
            .collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    #[derive(Clone, Copy, Debug)]
    enum Click {
        /// One sample.
        Impulse,
        /// Random samples, `len` long.
        Burst(usize),
        /// A constant offset, `len` long.
        Step(usize),
    }

    impl Click {
        fn len(self) -> usize {
            match self {
                Click::Impulse => 1,
                Click::Burst(n) | Click::Step(n) => n,
            }
        }
    }

    /// The clicks the tests put in: every kind, 1 to 20 samples, every 3001 samples from 6000
    /// (so they fall everywhere in the frames), one straddling a frame's last final sample.
    fn clicks() -> Vec<(usize, Click)> {
        let kinds = [
            Click::Impulse,
            Click::Burst(2),
            Click::Step(3),
            Click::Burst(5),
            Click::Step(8),
            Click::Burst(12),
            Click::Step(16),
            Click::Burst(20),
            Click::Step(20),
            Click::Burst(8),
            Click::Impulse,
        ];
        let mut out: Vec<(usize, Click)> = kinds
            .iter()
            .enumerate()
            .map(|(i, &c)| (6000 + 3001 * i, c))
            .collect();
        // The frame at 2048·20 makes [2048·20 - 3072, 2048·20 - 1024) final.
        out.push((2048 * 20 - 1024 - 6, Click::Burst(12)));
        out
    }

    /// `clean` with the clicks added at `amp`: impulses and steps of alternating sign, bursts
    /// of random signs and sizes from half to all of `amp`.
    fn with_clicks(clean: &[f32], clicks: &[(usize, Click)], amp: f32, seed: u64) -> Vec<f32> {
        let mut x = clean.to_vec();
        let noise = gauss(256, seed, 1.0);
        for (k, &(at, click)) in clicks.iter().enumerate() {
            let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
            for i in 0..click.len() {
                let v = noise[(i + 7 * k) % noise.len()];
                x[at + i] += match click {
                    Click::Impulse | Click::Step(_) => sign * amp,
                    Click::Burst(_) => amp * v.signum() * (0.5 + 0.5 * v.abs().min(1.0)),
                };
            }
        }
        x
    }

    /// Per click: its own error against `clean` and the output's, in dB of the click's, over
    /// the click and 8 samples each side.
    fn depths(
        clean: &[f32],
        clicky: &[f32],
        out: &[f32],
        clicks: &[(usize, Click)],
    ) -> Vec<(Click, f64)> {
        clicks
            .iter()
            .map(|&(at, click)| {
                let range = at - 8..at + click.len() + 8;
                let err = |x: &[f32]| -> f64 {
                    range
                        .clone()
                        .map(|i| (f64::from(x[i]) - f64::from(clean[i])).powi(2))
                        .sum()
                };
                (click, 10.0 * (err(out) / err(clicky)).log10())
            })
            .collect()
    }

    #[test]
    fn clicks_of_1_to_20_samples_on_a_tone_and_on_noise_are_repaired() {
        let n = 48_000;
        let list = clicks();
        // A 440 Hz tone at -12 dBFS with a little noise (-90 dBFS), and Gaussian noise at
        // -40 dBFS RMS; clicks at 0.5 on each.
        let cases = [
            (
                "tone",
                mix(&tone(n, 440.0, 0.25), &gauss(n, 3, 3e-5)),
                0.5f32,
            ),
            ("noise", gauss(n, 4, 0.01), 0.5),
        ];
        for setting in ["the defaults", "deep", "medium"] {
            let values = variant(setting);
            let s = settings(&[("Threshold", values[0]), ("Complexity", values[1])]);
            for (name, clean, amp) in &cases {
                let clicky = with_clicks(clean, &list, *amp, 9);
                let out = render(&KIND, RATE, &s, std::slice::from_ref(&clicky), 512).unwrap();
                for (click, depth) in depths(clean, &clicky, &out[0], &list) {
                    println!("{setting}, {name}: {click:?} {depth:.1} dB");
                    assert!(depth < -20.0, "{setting}, {name}: {click:?} {depth:.1} dB");
                }
            }
        }
    }

    #[test]
    fn audio_without_clicks_comes_back_4096_samples_later_bit_for_bit() {
        let n = 3 * 48_000;
        // A tone over Gaussian noise, the noise alone, a quiet tone over very quiet noise and a
        // tone alone, each faded in and out over 10 ms: a sound that starts from digital
        // silence at full tilt is a step, which the detector rightly takes for a click.
        let fade = |x: Vec<f32>| -> Vec<f32> {
            let ramp = 480;
            let len = x.len();
            x.into_iter()
                .enumerate()
                .map(|(i, s)| {
                    let edge = i.min(len - 1 - i);
                    if edge < ramp {
                        let g = 0.5 - 0.5 * (PI * edge as f64 / ramp as f64).cos();
                        (f64::from(s) * g) as f32
                    } else {
                        s
                    }
                })
                .collect()
        };
        let signals = [
            fade(mix(&tone(n, 440.0, 0.25), &gauss(n, 5, 0.003))),
            fade(gauss(n, 6, 0.05)),
            fade(mix(&tone(n, 1000.0, 0.01), &gauss(n, 7, 1e-4))),
            fade(tone(n, 440.0, 0.5)),
        ];
        for (k, x) in signals.iter().enumerate() {
            for (name, v) in VARIANTS {
                let mut fx = ClickRemover::new();
                fx.set(THRESHOLD, v[0]);
                fx.set(COMPLEXITY, v[1]);
                let mut io = vec![x.clone(); CHANNELS];
                io[1].reverse();
                let input = io.clone();
                fx.process(&mut io, 0, n);
                for c in 0..CHANNELS {
                    assert!(io[c][..LATENCY].iter().all(|s| *s == 0.0));
                    let changed = (LATENCY..n)
                        .filter(|&i| io[c][i].to_bits() != input[c][i - LATENCY].to_bits())
                        .count();
                    let (diff, level) = (LATENCY..n).fold((0.0, 0.0), |(d, l), i| {
                        let (y, x) = (f64::from(io[c][i]), f64::from(input[c][i - LATENCY]));
                        (d + (y - x).powi(2), l + x * x)
                    });
                    let null = 10.0 * (diff / level).max(1e-30).log10();
                    println!("signal {k}, {name}, channel {c}: {changed} changed, {null:.1} dB");
                    if name == "deep" {
                        // 5 times the error's level: Gaussian noise has a few samples out
                        // there, which it rebuilds (each a few samples of noise, -40 dB on the
                        // whole null).
                        assert!(changed <= 20, "signal {k}, {name}, channel {c}: {changed}");
                    } else {
                        assert_eq!(changed, 0, "signal {k}, {name}, channel {c}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_same_at_any_block_size_and_the_channels_apart() {
        let n = 40_000;
        let list = clicks()
            .into_iter()
            .filter(|c| c.0 + 30 < n)
            .collect::<Vec<_>>();
        let left = with_clicks(
            &mix(&tone(n, 440.0, 0.25), &gauss(n, 1, 1e-3)),
            &list,
            0.5,
            2,
        );
        let right_clean = gauss(n, 8, 0.02);
        let audio = [left.clone(), right_clean.clone()];
        let s = settings(&[]);
        let a = render(&KIND, RATE, &s, &audio, 512).unwrap();
        for block in [77, 1, 4096] {
            assert_eq!(
                a,
                render(&KIND, RATE, &s, &audio, block).unwrap(),
                "{block}"
            );
        }
        // The clicks are only on the left: the right comes back untouched, and the left is the
        // same whatever the right holds.
        assert_eq!(a[1], right_clean);
        assert_ne!(a[0], left);
        let other = render(&KIND, RATE, &s, &[left, gauss(n, 9, 0.3)], 512).unwrap();
        assert_eq!(a[0], other[0]);
    }

    #[test]
    fn a_transport_start_starts_the_frames_afresh() {
        let n = 20_000;
        let x = with_clicks(&tone(n, 300.0, 0.3), &[(9000, Click::Burst(6))], 0.4, 3);
        let run = |fx: &mut ClickRemover| {
            let mut io = vec![x.clone(); CHANNELS];
            fx.process(&mut io, 0, n);
            io
        };
        let mut fresh = ClickRemover::new();
        let first = run(&mut fresh);
        let mut used = ClickRemover::new();
        let mut io = vec![gauss(5000, 1, 0.2); CHANNELS];
        used.process(&mut io, 0, 5000);
        used.start();
        assert_eq!(run(&mut used), first);
    }

    #[test]
    fn the_settings_map_as_expected() {
        let defaults: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let e = engine(&defaults);
        assert!((e.threshold - 55.0).abs() < 1e-9);
        assert_eq!((e.size, e.order, e.longest), (16, 28, 40));
        // θ and S of each test setting.
        for (name, theta, size) in [
            ("the defaults", 55.0, 16),
            ("deep", 40.0, 91),
            ("medium", 76.0, 51),
            ("light", 116.0, 21),
        ] {
            let v = &variant(name);
            for (p, v) in PARAMS.iter().zip(v) {
                assert!((p.min..=p.max).contains(v), "{name}: {} {v}", p.name);
                assert!((p.plain(p.normalized(*v)) - v).abs() < 1e-9, "{name}");
            }
            let e = engine(v);
            assert!((e.threshold - theta).abs() < 1e-9, "{name}: {e:?}");
            assert_eq!(e.size, size, "{name}");
            assert_eq!(e.order, size / 2 + 20);
        }
        // The defaults are the first setting's values, within the parameters' precision.
        for (d, v) in defaults.iter().zip(variant("the defaults")) {
            assert!((d - v).abs() < 0.1, "{d} {v}");
        }
        // The ends of the ranges: order 20 to 70, the buffers sized for the largest.
        assert_eq!(engine(&[0.0, 0.0]).order, 20);
        let top = engine(&[100.0, 100.0]);
        assert_eq!((top.size, top.order), (100, MAX_ORDER));
        assert!(top.longest + 2 * PAD <= MAX_FILL);
        assert_eq!(PARAMS[THRESHOLD].display(defaults[0]), "29.3 %");
        assert_eq!(PARAMS[COMPLEXITY].display(defaults[1]), "15.2 %");
    }

    #[test]
    fn longer_events_than_the_longest_click_are_left_alone() {
        // A 60-sample burst at the default (the longest click 40 samples) stays as it is;
        // deep (S 91: up to 190 samples) repairs it.
        let n = 30_000;
        let clean = mix(&tone(n, 440.0, 0.25), &gauss(n, 11, 3e-5));
        let list = [(15_000, Click::Burst(60))];
        let clicky = with_clicks(&clean, &list, 0.5, 4);
        let at_default = render(
            &KIND,
            RATE,
            &settings(&[]),
            std::slice::from_ref(&clicky),
            512,
        )
        .unwrap();
        let d = depths(&clean, &clicky, &at_default[0], &list)[0].1;
        println!("60-sample burst at the default: {d:.1} dB");
        assert!(d > -1.0, "{d}");
        let v = variant("deep");
        let s = settings(&[("Threshold", v[0]), ("Complexity", v[1])]);
        let deep = render(&KIND, RATE, &s, std::slice::from_ref(&clicky), 512).unwrap();
        let d = depths(&clean, &clicky, &deep[0], &list)[0].1;
        println!("60-sample burst, deep: {d:.1} dB");
        assert!(d < -20.0, "{d}");
    }

    /// The cost: run by hand in release (`cargo test --release -p daw-core click_remover_cost
    /// -- --ignored --nocapture`); also reports what the defaults change on the DI, if present.
    #[test]
    #[ignore]
    fn click_remover_cost() {
        let n = 48_000 * 20;
        let x = mix(&tone(n, 440.0, 0.25), &gauss(n, 5, 0.01));
        for name in ["the defaults", "medium", "deep"] {
            let v = variant(name);
            let mut fx = ClickRemover::new();
            fx.set(THRESHOLD, v[0]);
            fx.set(COMPLEXITY, v[1]);
            let mut io = vec![x.clone(); CHANNELS];
            // In blocks of 64, as a small buffer runs it: the whole, and the worst block.
            let (mut total, mut worst) = (0.0f64, 0.0f64);
            for at in (0..n).step_by(64) {
                let t = std::time::Instant::now();
                fx.process(&mut io, at, (at + 64).min(n));
                let took = t.elapsed().as_secs_f64();
                total += took;
                worst = worst.max(took);
            }
            let per_hop = total / (n / HOP) as f64;
            println!(
                "{name}: {:.3} ms per 2048 samples (stereo), {:.2} % of real time at 48 kHz; \
                 the worst block of 64: {:.3} ms",
                per_hop * 1e3,
                100.0 * per_hop / (HOP as f64 / RATE),
                worst * 1e3
            );
        }
        let di = crate::test_audio();
        if let Ok(audio) = crate::wav::read(&di) {
            let x = &audio.channels[0];
            for (name, v) in &VARIANTS {
                let mut fx = ClickRemover::new();
                fx.set(THRESHOLD, v[0]);
                fx.set(COMPLEXITY, v[1]);
                let mut io = vec![x.clone(); CHANNELS];
                let t = std::time::Instant::now();
                fx.process(&mut io, 0, x.len());
                let elapsed = t.elapsed().as_secs_f64();
                let changed = (LATENCY..x.len())
                    .filter(|&i| io[0][i].to_bits() != x[i - LATENCY].to_bits())
                    .count();
                println!(
                    "DI ({:.1} s), {name}: {} runs, {changed} samples changed ({:.4} %), {:.3} \
                     ms per 2048",
                    x.len() as f64 / f64::from(audio.sample_rate),
                    fx.repair.repaired / 2,
                    100.0 * changed as f64 / x.len() as f64,
                    elapsed * 1e3 / (x.len() / HOP) as f64
                );
            }
        }
    }
}
