//! Clip Repair: rebuilds clipped runs, as an insert. Rather than a process in two steps (a
//! scan that lists the clipped runs, then a repair that rebuilds them), it learns in one step
//! and repairs as it plays:
//!
//! - **Scan** ([`capture`], through `Kind::capture` like Noise Reduction's print): per channel
//!   the highest sample M and the lowest m of the audio given, and how many runs the settings
//!   then find. Saved in the effect's state and handed to the DSP without a lock. Not scanned,
//!   it takes M = 1 and m = -1, so plain digital clipping at full scale is caught as it is.
//! - **Detection**, per channel: P = max(M, -m), t = P·Tolerance/100, the
//!   bands [M - t, M] and [m, m + t]; nothing under -24 dBFS ([`FLOOR`]) is ever in a band. A
//!   sample past the learned level counts as in its band (the levels may come from another
//!   stretch of the audio than the one playing). A run is at least Min. Clip Size samples in one
//!   band; a jump from one band straight into the other closes it and starts the next; only the
//!   first 200 samples ([`CAP`]) of a run are rebuilt.
//! - **Repair**, each run from [`CONTEXT`] samples on each side: a cubic (a Hermite
//!   between the samples either side of the run, its end slopes from 3-point parabolas, the
//!   steeper into the clip, x1.22 against a flat clip; for rough surroundings a raised peak or
//!   low-passed slopes), then an AR model fitted to the
//!   context's samples that are in no band (covariance method, order up to 16 as the clean
//!   stretches allow), and the least-squares AR interpolation of the run with the cubic as its
//!   prior (each sample's prior weight the model's error over 10 % of the clip level). A
//!   rebuilt sample is never moved back inside the clip. **FFT Interpolation** switches
//!   on for runs of at least ⌊3N/4⌋ + 1
//!   samples, and for those trusts the model more: order up to N/2 (at most 32), the prior a
//!   hundred times weaker.
//! - **Output:** every sample times the Gain (10^(dB/20)), Auto Gain taking it from the scan
//!   (room for peaks 12 dB over the clip level), then the Limiter at its defaults
//!   (-0.1 dBFS, always on). Audio that never reaches a band comes
//!   back as it went in, times the gain: bit for bit at 0 dB (under -0.1 dBFS).
//! - **Latency** fixed at any setting and block size: [`DELAY`] samples (the longest run and
//!   its right-hand context) plus the limiter's 20 ms. One sample at a time, so the stream, not
//!   the blocks, decides everything; a transport start empties it.

use super::hard_limiter::HardLimiter;
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;
use anyhow::{bail, Result};

// Parameter indices (the editor reads the first three).
pub const GAIN: usize = 0;
pub const AUTO: usize = 1;
pub const TOLERANCE: usize = 2;
const MIN_SIZE: usize = 3;
const FFT: usize = 4;
const FFT_SIZE: usize = 5;

/// -24 dBFS: nothing quieter is ever taken for clipping.
pub const FLOOR: f64 = 0.063_095_734_448_019;
/// The longest stretch of a run that is rebuilt.
pub const CAP: usize = 200;
/// The samples on each side of a run its repair reads (its pre- and post-roll).
pub const CONTEXT: usize = 320;
/// The repair's delay: a run's last rebuilt sample is at most `CAP - 1` after its first, and
/// its right-hand context `CONTEXT` after that; the output reads one sample later still.
pub const DELAY: usize = CAP + CONTEXT;
/// The samples each channel keeps (a power of two, more than the delay and a run's left-hand
/// context).
const RING: usize = 2048;
const MASK: usize = RING - 1;
/// Runs waiting for their right-hand context (a run is at least 3 samples, and one sample
/// apart from the next: about 130 at most within the delay).
const QUEUE: usize = 256;
/// The model's order with the cubic, and at most (FFT Interpolation's long runs).
const ORDER: usize = 16;
const MAX_ORDER: usize = 32;
/// The orders tried, highest first, until the clean stretches give enough equations.
const ORDERS: [usize; 8] = [32, 24, 16, 12, 8, 6, 4, 2];
/// A run with its context on both sides.
const WINDOW: usize = 2 * CONTEXT + CAP;
/// The cubic prior's error, as a fraction of the clip level.
const PRIOR: f64 = 0.1;
/// Samples a gain set by hand glides over.
const GLIDE: usize = 512;
/// Auto Gain's room over the clip level: the most the cubic raises a peak (its
/// 10^(X/(X^1.3/45 + 8)/11) at X = 200 is +12.2 dB).
const ROOM_DB: f64 = 12.0;

// A sample's flags: its band (as it came in), and whether it has been rebuilt.
const NONE: u8 = 0;
const TOP: u8 = 1;
const BOTTOM: u8 = 2;
const BAND: u8 = 3;
const REBUILT: u8 = 4;

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    steps: 0,
    decimals: 0,
    automatable: false,
    link: None,
    log_slider: false,
    format: None,
};

fn format_interpolation(v: f64) -> String {
    if v >= 0.5 { "FFT" } else { "Cubic" }.into()
}

/// The FFT size a setting names (powers of two, 8 to 256).
fn fft_size(bits: f64) -> usize {
    1 << (bits.round() as u32).clamp(3, 8)
}

fn format_fft_size(bits: f64) -> String {
    format!("{}", fft_size(bits))
}

static PARAMS: [ParamSpec; 6] = [
    // Output gain after the repair: room for the rebuilt peaks.
    ParamSpec {
        name: "Gain",
        unit: "dB",
        min: -12.0,
        max: 0.0,
        default: -12.0,
        decimals: 1,
        automatable: true,
        ..PARAM
    },
    // The "Auto" box: the gain from the scan instead.
    ParamSpec {
        name: "Auto Gain",
        steps: 1,
        ..PARAM
    },
    ParamSpec {
        name: "Tolerance",
        unit: "%",
        max: 50.0,
        default: 10.0,
        decimals: 1,
        automatable: true,
        ..PARAM
    },
    ParamSpec {
        name: "Min. Clip Size",
        unit: "samples",
        min: 3.0,
        max: 20.0,
        default: 3.0,
        steps: 17,
        ..PARAM
    },
    ParamSpec {
        name: "FFT Interpolation",
        steps: 1,
        format: Some(format_interpolation),
        ..PARAM
    },
    ParamSpec {
        name: "FFT Size",
        min: 3.0,
        max: 8.0,
        default: 5.0,
        steps: 5,
        format: Some(format_fft_size),
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "clip-repair",
    name: "Clip Repair",
    category: "Restoration",
    params: &PARAMS,
    make: |rate| Box::new(DeClipper::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: Some(capture),
    learns: None,
    reads: None,
    prepares: None,
};

/// What a scan learned on one channel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Learned {
    /// The highest sample (M), at least 0.
    pub top: f64,
    /// The lowest sample (m), at most 0.
    pub bottom: f64,
    /// The runs the scan's settings found in it.
    pub runs: u32,
}

/// Not scanned: clipping at full scale.
pub const FULL_SCALE: Learned = Learned {
    top: 1.0,
    bottom: -1.0,
    runs: 0,
};

/// The levels in a scan ([`capture`]'s numbers), per channel; `None` for anything else.
pub fn learned(print: &[f32]) -> Option<[Learned; CHANNELS]> {
    if print.len() != 3 * CHANNELS || print.iter().any(|v| !v.is_finite()) {
        return None;
    }
    Some(std::array::from_fn(|c| Learned {
        top: f64::from(print[3 * c]),
        bottom: f64::from(print[3 * c + 1]),
        runs: print[3 * c + 2] as u32,
    }))
}

/// A channel's bands: in the top one from `high` up, in the bottom one from `low` down; an
/// infinity where a level never passes -24 dBFS (that band is off).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bands {
    pub high: f64,
    pub low: f64,
}

impl Bands {
    pub fn new(levels: &Learned, tolerance: f64) -> Self {
        let p = levels.top.max(-levels.bottom).max(0.0);
        let t = p * tolerance.clamp(0.0, 100.0) / 100.0;
        Self {
            high: if levels.top >= FLOOR {
                (levels.top - t).max(FLOOR)
            } else {
                f64::INFINITY
            },
            low: if levels.bottom <= -FLOOR {
                (levels.bottom + t).min(-FLOOR)
            } else {
                f64::NEG_INFINITY
            },
        }
    }

    fn of(&self, x: f64) -> u8 {
        if x >= self.high {
            TOP
        } else if x <= self.low {
            BOTTOM
        } else {
            NONE
        }
    }
}

/// The gain Auto Gain takes for these levels (dB): the louder channel's clip level brought
/// down by [`ROOM_DB`], within the Gain's range.
pub fn auto_gain_db(levels: &[Learned; CHANNELS]) -> f64 {
    let p = levels
        .iter()
        .map(|l| l.top.max(-l.bottom))
        .fold(0.0f64, f64::max);
    if p <= 0.0 {
        return 0.0;
    }
    (-20.0 * p.log10() - ROOM_DB).clamp(-12.0, 0.0)
}

/// The gain the output is multiplied by, for `db`: one f32, the same every time.
pub fn gain_of(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

/// The whole delay at `rate`: the repair's and the limiter's 20 ms.
pub fn latency(rate: f64) -> usize {
    DELAY + HardLimiter::new(rate).latency()
}

/// The run tracking the scan does, a sample at a time, carried across blocks.
#[derive(Debug, Default, Clone, Copy)]
struct Tracker {
    top: usize,
    bottom: usize,
    start: u64,
    /// The run reached [`CAP`] and was handed over; the rest of it is left as it is.
    capped: bool,
}

/// A run to rebuild: its first sample, its length (at most [`CAP`]) and its band.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
struct Run {
    start: u64,
    len: usize,
    band: u8,
}

impl Tracker {
    /// End the run under way: it is a run if it lasted `min` samples (and wasn't handed over
    /// already).
    fn close(&mut self, min: usize) -> Option<Run> {
        let (n, band) = if self.top > 0 {
            (self.top, TOP)
        } else {
            (self.bottom, BOTTOM)
        };
        let run = (n >= min && !self.capped).then_some(Run {
            start: self.start,
            len: n,
            band,
        });
        *self = Self::default();
        run
    }

    /// The sample at `pos` is in `band`: a run that ends here, or reaches [`CAP`].
    fn step(&mut self, band: u8, pos: u64, min: usize) -> Option<Run> {
        if band == NONE {
            return if self.top + self.bottom > 0 {
                self.close(min)
            } else {
                None
            };
        }
        let other = if band == TOP { self.bottom } else { self.top };
        // Straight from one band into the other: that run ends, this one starts.
        let mut ended = if other > 0 { self.close(min) } else { None };
        let count = if band == TOP {
            &mut self.top
        } else {
            &mut self.bottom
        };
        if *count == 0 {
            self.start = pos;
            self.capped = false;
        }
        *count = count.saturating_add(1);
        if !self.capped && *count == CAP {
            self.capped = true;
            ended = Some(Run {
                start: self.start,
                len: CAP,
                band,
            });
        }
        ended
    }
}

/// Learn the clip levels from `audio` (one buffer per channel; mono is used for both), as
/// a scan of a selection does: each channel's highest and lowest sample, and the
/// runs the settings find. Numbers per channel: M, m, runs.
pub fn capture(_rate: f64, values: &[f64], audio: &[Vec<f32>]) -> Result<Vec<f32>> {
    let len = audio.first().map_or(0, Vec::len);
    if len == 0 {
        bail!("select the clipped audio to scan: the selection is empty");
    }
    let min = values[MIN_SIZE].round() as usize;
    let mut out = Vec::with_capacity(3 * CHANNELS);
    let mut loudest = 0.0f64;
    for c in 0..CHANNELS {
        let x = &audio[c.min(audio.len() - 1)];
        let top = x.iter().fold(0.0f64, |m, s| m.max(f64::from(*s)));
        let bottom = x.iter().fold(0.0f64, |m, s| m.min(f64::from(*s)));
        loudest = loudest.max(top).max(-bottom);
        let bands = Bands::new(
            &Learned {
                top,
                bottom,
                runs: 0,
            },
            values[TOLERANCE],
        );
        let mut tracker = Tracker::default();
        let mut runs = 0u32;
        for (i, s) in x.iter().enumerate() {
            let band = bands.of(f64::from(*s));
            runs += u32::from(tracker.step(band, i as u64, min).is_some());
        }
        // A run still open at the end counts: the scan flushes it.
        runs += u32::from(tracker.close(min).is_some());
        out.extend([top as f32, bottom as f32, runs as f32]);
    }
    if loudest < FLOOR {
        bail!(
            "nothing to scan: the selection never reaches -24 dBFS, and quieter audio is never \
             taken for clipping"
        );
    }
    Ok(out)
}

/// One channel: the last [`RING`] samples (rebuilt in place) with their flags, the run under
/// way and the runs waiting for their context.
struct Channel {
    ring: Vec<f32>,
    flags: Vec<u8>,
    tracker: Tracker,
    queue: [Run; QUEUE],
    head: usize,
    waiting: usize,
    levels: Learned,
    bands: Bands,
}

impl Channel {
    fn new() -> Self {
        Self {
            ring: vec![0.0; RING],
            flags: vec![NONE; RING],
            tracker: Tracker::default(),
            queue: [Run::default(); QUEUE],
            head: 0,
            waiting: 0,
            levels: FULL_SCALE,
            bands: Bands::new(&FULL_SCALE, 10.0),
        }
    }

    fn clear(&mut self) {
        self.ring.fill(0.0);
        self.flags.fill(NONE);
        self.tracker = Tracker::default();
        self.head = 0;
        self.waiting = 0;
    }

    fn push(&mut self, run: Run) {
        // Never full in practice (see `QUEUE`); a run that doesn't fit is left as it is.
        if self.waiting < QUEUE {
            self.queue[(self.head + self.waiting) % QUEUE] = run;
            self.waiting += 1;
        }
    }

    /// The oldest waiting run, once its right-hand context is in (at `pos`).
    fn ready(&mut self, pos: u64) -> Option<Run> {
        if self.waiting == 0 {
            return None;
        }
        let run = self.queue[self.head];
        if run.start + (run.len + CONTEXT - 1) as u64 > pos {
            return None;
        }
        self.head = (self.head + 1) % QUEUE;
        self.waiting -= 1;
        Some(run)
    }
}

/// The repair's scratch, made once.
struct Repair {
    /// The run and its context, from the ring, as f64; and their flags.
    x: Vec<f64>,
    flags: Vec<u8>,
    /// The cubic over the run.
    prior: Vec<f64>,
    /// The model's covariance (upper triangle, `MAX_ORDER + 1` wide), its Cholesky factor
    /// (`MAX_ORDER` wide), the coefficients and the error filter.
    phi: Vec<f64>,
    chol: Vec<f64>,
    a: Vec<f64>,
    b: Vec<f64>,
    /// The interpolation's banded normal equations (row i holds i-d for d in 0..=p) and its
    /// right-hand side, then the solution.
    band: Vec<f64>,
    rhs: Vec<f64>,
    /// Runs rebuilt since the start: for tests.
    rebuilt: u64,
}

impl Repair {
    fn new() -> Self {
        let w = MAX_ORDER + 1;
        Self {
            x: vec![0.0; WINDOW],
            flags: vec![NONE; WINDOW],
            prior: vec![0.0; CAP],
            phi: vec![0.0; w * w],
            chol: vec![0.0; MAX_ORDER * MAX_ORDER],
            a: vec![0.0; w],
            b: vec![0.0; w],
            band: vec![0.0; CAP * w],
            rhs: vec![0.0; CAP],
            rebuilt: 0,
        }
    }

    /// Rebuild `run` in `ch`'s ring.
    fn run(&mut self, ch: &mut Channel, run: Run, fft: Option<usize>) {
        let l = run.len;
        let len = 2 * CONTEXT + l;
        let first = run.start.wrapping_sub(CONTEXT as u64);
        for k in 0..len {
            let at = (first.wrapping_add(k as u64) as usize) & MASK;
            self.x[k] = f64::from(ch.ring[at]);
            self.flags[k] = ch.flags[at];
        }
        let (lo, hi) = (CONTEXT, CONTEXT + l - 1);
        let sign = if run.band == TOP { 1.0 } else { -1.0 };
        let level = if run.band == TOP {
            ch.levels.top
        } else {
            ch.levels.bottom
        };
        let clip = ch.levels.top.max(-ch.levels.bottom).max(FLOOR);
        let range = (ch.levels.top - ch.levels.bottom).max(FLOOR);
        self.cubic(lo, hi, sign, level, range);
        // FFT Interpolation's long runs trust the model more.
        let (most, prior) = match fft {
            Some(n) if l > 3 * n / 4 => ((n / 2).clamp(2, MAX_ORDER), 10.0 * PRIOR * clip),
            _ => (ORDER, PRIOR * clip),
        };
        let solved = match self.fit(len, most) {
            Some((p, error)) => {
                let lambda = (error / (prior * prior)).max(1e-12);
                self.interpolate(lo, l, p, lambda)
            }
            None => false,
        };
        for i in 0..l {
            let v = if solved { self.rhs[i] } else { self.prior[i] };
            let v = if v.is_finite() { v } else { self.prior[i] };
            // Never back inside the clip: the true sample was at least as far out.
            let was = self.x[lo + i];
            let v = if sign > 0.0 { v.max(was) } else { v.min(was) };
            let at = (run.start.wrapping_add(i as u64) as usize) & MASK;
            ch.ring[at] = v as f32;
            ch.flags[at] |= REBUILT;
        }
        self.rebuilt += 1;
    }

    /// The cubic over `lo..=hi`, into `prior`: a Hermite from the sample before the
    /// run to the one after it (t = i / (L + 1), so it meets both).
    fn cubic(&mut self, lo: usize, hi: usize, sign: f64, level: f64, range: f64) {
        let x = &self.x;
        let l = hi - lo + 1;
        let span = (l + 1) as f64;
        let points = l + 2;
        let (y0, y1) = (x[lo - 1], x[hi + 1]);
        // Each end's slope (per sample): a parabola through the edge and its two neighbours
        // (one of them clipped), or through the edge and the two good samples behind it; the
        // one pointing more steeply into the clip.
        let left = [
            (x[lo] - x[lo - 2]) / 2.0,
            (3.0 * x[lo - 1] - 4.0 * x[lo - 2] + x[lo - 3]) / 2.0,
        ];
        let right = [
            (x[hi + 2] - x[hi]) / 2.0,
            (-3.0 * x[hi + 1] + 4.0 * x[hi + 2] - x[hi + 3]) / 2.0,
        ];
        let mut s0 = if sign > 0.0 {
            left[0].max(left[1])
        } else {
            left[0].min(left[1])
        };
        let mut s1 = if sign > 0.0 {
            right[0].min(right[1])
        } else {
            right[0].max(right[1])
        };
        // A neighbour flat on the clip level hides how steep the curve was: raised.
        let flat = |v: f64| sign * (v - level) >= -1e-3 * level.abs();
        if flat(x[lo]) {
            s0 *= 1.22;
        }
        if flat(x[hi]) {
            s1 *= 1.22;
        }
        // Roughness: a parabola's one-step prediction error over 4 samples each side, as a
        // part of the clip range.
        let rms = |e: &mut dyn Iterator<Item = f64>| (e.map(|v| v * v).sum::<f64>() / 4.0).sqrt();
        let before =
            rms(&mut (lo - 4..lo).map(|n| x[n] - (3.0 * x[n - 1] - 3.0 * x[n - 2] + x[n - 3])));
        let after =
            rms(&mut (hi + 1..hi + 5).map(|n| x[n] - (3.0 * x[n + 1] - 3.0 * x[n + 2] + x[n + 3])));
        let rough = ((before + after) / range).powf(0.75);
        let xf = points as f64;
        let (m0, m1) = if xf / 200.0 + 2.9 * rough <= 1.0 || points < 4 {
            (s0 * span, s1 * span)
        } else if points < 17 {
            // Short and rough: a raised peak, the higher the longer the run, and the slopes
            // that put the curve's middle on it.
            let edge = if sign > 0.0 { y0.max(y1) } else { y0.min(y1) };
            let peak = edge * 10f64.powf(xf / (xf.powf(1.3) / 45.0 + 8.0) / 11.0);
            (4.0 * (peak - y0), 4.0 * (y1 - peak))
        } else {
            // Long and rough: slopes of the surroundings low-passed (a straight line fitted to
            // as many samples as the run is long, up to 16, on each side).
            let k = (l + 1).min(16);
            (
                line_slope(&x[lo - k..lo]) * span,
                line_slope(&x[hi + 1..hi + 1 + k]) * span,
            )
        };
        let c3 = 2.0 * (y0 - y1) + m0 + m1;
        let c2 = -3.0 * (y0 - y1) - 2.0 * m0 - m1;
        for (i, p) in self.prior[..l].iter_mut().enumerate() {
            let t = (i + 1) as f64 / span;
            *p = y0 + t * (m0 + t * (c2 + t * c3));
        }
    }

    /// An AR model of the context's clean samples (in no band as they came in), by the
    /// covariance method: the highest order up to `most` whose equations (rows whose samples
    /// are all clean) number at least twice the order and 8 more. Its order and the error's
    /// power per row; the coefficients in `a`.
    fn fit(&mut self, len: usize, most: usize) -> Option<(usize, f64)> {
        let mut rows = [0usize; ORDERS.len()];
        let mut clean = 0usize;
        for &f in &self.flags[..len] {
            if f & BAND == NONE {
                clean += 1;
                for (r, &p) in rows.iter_mut().zip(&ORDERS) {
                    *r += usize::from(clean > p);
                }
            } else {
                clean = 0;
            }
        }
        let (p, count) = ORDERS
            .iter()
            .zip(rows)
            .find(|&(&p, r)| p <= most && r >= 2 * p + 8)
            .map(|(&p, r)| (p, r))?;
        let w = MAX_ORDER + 1;
        self.phi.fill(0.0);
        let x = &self.x;
        clean = 0;
        for n in 0..len {
            if self.flags[n] & BAND != NONE {
                clean = 0;
                continue;
            }
            clean += 1;
            if clean <= p {
                continue;
            }
            for i in 0..=p {
                let xi = x[n - i];
                let row = &mut self.phi[i * w..i * w + w];
                for j in i..=p {
                    row[j] += xi * x[n - j];
                }
            }
        }
        // Normal equations R a = r (R from rows/columns 1..=p), a -90 dB diagonal load so a
        // pure tone (rank 2) still solves; Cholesky.
        let load = 1e-9 * (1..=p).map(|i| self.phi[i * w + i]).fold(0.0, f64::max) + 1e-30;
        let c = &mut self.chol;
        for i in 0..p {
            for j in 0..=i {
                let mut s = self.phi[(j + 1) * w + (i + 1)];
                if i == j {
                    s += load;
                }
                for k in 0..j {
                    s -= c[i * MAX_ORDER + k] * c[j * MAX_ORDER + k];
                }
                if i == j {
                    if s.is_nan() || s <= 0.0 {
                        return None;
                    }
                    c[i * MAX_ORDER + i] = s.sqrt();
                } else {
                    c[i * MAX_ORDER + j] = s / c[j * MAX_ORDER + j];
                }
            }
        }
        for i in 0..p {
            let mut s = self.phi[i + 1];
            for k in 0..i {
                s -= c[i * MAX_ORDER + k] * self.a[k + 1];
            }
            self.a[i + 1] = s / c[i * MAX_ORDER + i];
        }
        for i in (0..p).rev() {
            let mut s = self.a[i + 1];
            for k in i + 1..p {
                s -= c[k * MAX_ORDER + i] * self.a[k + 1];
            }
            self.a[i + 1] = s / c[i * MAX_ORDER + i];
        }
        let explained: f64 = (1..=p).map(|i| self.a[i] * self.phi[i]).sum();
        let power = self.phi[0] / count as f64;
        let error = ((self.phi[0] - explained) / count as f64).max(1e-12 * power + 1e-30);
        error.is_finite().then_some((p, error))
    }

    /// The run `lo..lo + l` by least-squares AR interpolation with the cubic as its prior:
    /// the samples that minimise Σ e_n² + λ Σ (u_i - prior_i)², over the model's prediction
    /// errors e_n that the run's samples appear in and whose other samples can be trusted (in
    /// no band, or rebuilt). Banded (width p), solved by Cholesky into `rhs`.
    fn interpolate(&mut self, lo: usize, l: usize, p: usize, lambda: f64) -> bool {
        let w = MAX_ORDER + 1;
        self.b[0] = 1.0;
        for k in 1..=p {
            self.b[k] = -self.a[k];
        }
        for i in 0..l {
            self.band[i * w..i * w + w].fill(0.0);
            self.band[i * w] = lambda;
            self.rhs[i] = lambda * self.prior[i];
        }
        let run = lo..lo + l;
        let b = &self.b[..=p];
        for n in lo..lo + l + p {
            // The known part of the error, if every known sample can be trusted.
            let mut known = 0.0;
            let mut trusted = true;
            for (k, &bk) in b.iter().enumerate() {
                let q = n - k;
                if run.contains(&q) {
                    continue;
                }
                let f = self.flags[q];
                if f & BAND != NONE && f & REBUILT == 0 {
                    trusted = false;
                    break;
                }
                known += bk * self.x[q];
            }
            if !trusted {
                continue;
            }
            for k1 in 0..=p {
                let q1 = n - k1;
                if !run.contains(&q1) {
                    continue;
                }
                let i = q1 - lo;
                self.rhs[i] -= b[k1] * known;
                for (k2, &b2) in b.iter().enumerate().skip(k1) {
                    let q2 = n - k2;
                    if q2 < lo {
                        break;
                    }
                    self.band[i * w + (k2 - k1)] += b[k1] * b2;
                }
            }
        }
        // Banded Cholesky in place: band[i*w + d] holds L[i][i-d].
        let m = &mut self.band;
        for j in 0..l {
            let mut s = m[j * w];
            for d in 1..=p.min(j) {
                s -= m[j * w + d] * m[j * w + d];
            }
            if s.is_nan() || s <= 0.0 {
                return false;
            }
            let diag = s.sqrt();
            m[j * w] = diag;
            for i in j + 1..(j + p + 1).min(l) {
                let mut s = m[i * w + (i - j)];
                // Σ_k L[i][k] L[j][k] over k from i - p to j - 1.
                for k in i.saturating_sub(p)..j {
                    s -= m[i * w + (i - k)] * m[j * w + (j - k)];
                }
                m[i * w + (i - j)] = s / diag;
            }
        }
        for i in 0..l {
            let mut s = self.rhs[i];
            for k in i.saturating_sub(p)..i {
                s -= m[i * w + (i - k)] * self.rhs[k];
            }
            self.rhs[i] = s / m[i * w];
        }
        for i in (0..l).rev() {
            let mut s = self.rhs[i];
            for k in i + 1..(i + p + 1).min(l) {
                s -= m[k * w + (k - i)] * self.rhs[k];
            }
            self.rhs[i] = s / m[i * w];
        }
        self.rhs[..l].iter().all(|v| v.is_finite())
    }
}

/// The least-squares slope (per sample) of a straight line through `x`.
fn line_slope(x: &[f64]) -> f64 {
    let n = x.len() as f64;
    let mid = (n - 1.0) / 2.0;
    let mean = x.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, v) in x.iter().enumerate() {
        let d = i as f64 - mid;
        num += d * (v - mean);
        den += d * d;
    }
    if den > 0.0 {
        num / den
    } else {
        0.0
    }
}

/// What the effect's repair does to one channel's `range` (the Diagnostics'
/// Clip Repair): the runs the scan finds in it at these `levels`, `tolerance` (%) and
/// `min_size` (samples), each rebuilt as the insert rebuilds it. Returns each run's first
/// sample and its samples as rebuilt, in order.
///
/// The insert's own tracker and repair, a run at a time in the order the insert rebuilds them
/// (earlier runs rebuilt, later ones as they came in), each from its context in `x` (silence
/// outside it, as the insert's ring before its first sample). A run still open at the range's
/// end closes there, as the scan flushes it. No gain and no limiter: those are the
/// insert's output stage, not the repair, so for the whole of `x` the result is the insert's
/// output at Gain 0 dB wherever the limiter leaves it alone (peaks under -0.1 dBFS).
pub fn repair_offline(
    x: &[f32],
    range: std::ops::Range<usize>,
    levels: &Learned,
    tolerance: f64,
    min_size: usize,
    fft: Option<usize>,
) -> Vec<(usize, Vec<f32>)> {
    let (a, b) = (range.start, range.end.min(x.len()));
    if b <= a {
        return Vec::new();
    }
    let bands = Bands::new(levels, tolerance);
    let mut tracker = Tracker::default();
    let mut runs = Vec::new();
    for (p, s) in x.iter().enumerate().take(b).skip(a) {
        if let Some(run) = tracker.step(bands.of(f64::from(*s)), p as u64, min_size) {
            runs.push(run);
        }
    }
    runs.extend(tracker.close(min_size));
    if runs.is_empty() {
        return Vec::new();
    }
    // The channel as the repair goes along: rebuilt samples and their flags.
    let mut y = x.to_vec();
    let mut flags: Vec<u8> = x.iter().map(|s| bands.of(f64::from(*s))).collect();
    let mut ch = Channel::new();
    ch.levels = *levels;
    ch.bands = bands;
    let mut repair = Repair::new();
    let mut out = Vec::with_capacity(runs.len());
    for run in runs {
        // The run and its context into the ring, where the insert's would hold them.
        let first = run.start.wrapping_sub(CONTEXT as u64);
        for k in 0..2 * CONTEXT + run.len {
            let p = first.wrapping_add(k as u64);
            let (v, f) = usize::try_from(p)
                .ok()
                .filter(|&p| p < y.len())
                .map_or((0.0, NONE), |p| (y[p], flags[p]));
            let at = p as usize & MASK;
            ch.ring[at] = v;
            ch.flags[at] = f;
        }
        repair.run(&mut ch, run, fft);
        let start = run.start as usize;
        for i in 0..run.len {
            let at = (start + i) & MASK;
            y[start + i] = ch.ring[at];
            flags[start + i] = ch.flags[at];
        }
        out.push((start, y[start..start + run.len].to_vec()));
    }
    out
}

struct DeClipper {
    values: [f64; 6],
    channels: [Channel; CHANNELS],
    /// The next input sample's position since the start.
    pos: u64,
    /// The scan in use (swapped in by [`NativeEffect::take_print`]); empty: none.
    print: Vec<f32>,
    /// The gain: where it is and is heading (dB), the step per sample and the samples left,
    /// and its factor now.
    gain_db: f64,
    target_db: f64,
    step_db: f64,
    left: usize,
    gain: f32,
    limiter: HardLimiter,
    repair: Repair,
}

impl DeClipper {
    fn new(rate: f64) -> Self {
        let mut values = [0.0; 6];
        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        let mut fx = Self {
            values,
            channels: std::array::from_fn(|_| Channel::new()),
            pos: 0,
            print: Vec::new(),
            gain_db: 0.0,
            target_db: 0.0,
            step_db: 0.0,
            left: 0,
            gain: 1.0,
            limiter: HardLimiter::new(rate),
            repair: Repair::new(),
        };
        fx.relevel();
        fx.retarget(0);
        fx
    }

    /// The levels in use (the scan's, or full scale) and the bands from them.
    fn levels(&self) -> [Learned; CHANNELS] {
        learned(&self.print).unwrap_or([FULL_SCALE; CHANNELS])
    }

    fn relevel(&mut self) {
        let levels = self.levels();
        for (ch, l) in self.channels.iter_mut().zip(levels) {
            ch.levels = l;
            ch.bands = Bands::new(&l, self.values[TOLERANCE]);
        }
    }

    /// Head for the gain the settings ask for, over `samples` (0: at once).
    fn retarget(&mut self, samples: usize) {
        self.target_db = if self.values[AUTO] >= 0.5 {
            auto_gain_db(&self.levels())
        } else {
            self.values[GAIN]
        };
        if samples == 0 {
            self.gain_db = self.target_db;
            self.left = 0;
        } else {
            self.step_db = (self.target_db - self.gain_db) / samples as f64;
            self.left = samples;
        }
        self.gain = gain_of(self.gain_db);
    }

    /// FFT Interpolation's size, when on.
    fn fft(&self) -> Option<usize> {
        (self.values[FFT] >= 0.5).then(|| fft_size(self.values[FFT_SIZE]))
    }
}

impl NativeEffect for DeClipper {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        match index {
            TOLERANCE => self.relevel(),
            GAIN | AUTO => self.retarget(GLIDE),
            _ => {}
        }
    }

    /// The Gain's lanes draw their line in dB; the other settings take effect at once.
    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        if index == GAIN {
            self.values[GAIN] = value;
            self.retarget(samples);
        } else {
            self.set(index, value);
            self.snap();
        }
    }

    fn snap(&mut self) {
        self.retarget(0);
    }

    fn latency(&self) -> usize {
        DELAY + self.limiter.latency()
    }

    fn start(&mut self) {
        for ch in &mut self.channels {
            ch.clear();
        }
        self.pos = 0;
        self.limiter.start();
    }

    fn take_print(&mut self, print: &mut Vec<f32>) {
        std::mem::swap(&mut self.print, print);
        self.relevel();
        if self.values[AUTO] >= 0.5 {
            self.retarget(GLIDE);
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let min = self.values[MIN_SIZE].round() as usize;
        let fft = self.fft();
        for k in from..to {
            let pos = self.pos;
            let at = pos as usize & MASK;
            let out = pos.wrapping_sub(DELAY as u64) as usize & MASK;
            for (ch, buf) in self.channels.iter_mut().zip(io.iter_mut()) {
                let x = buf[k];
                let band = ch.bands.of(f64::from(x));
                ch.ring[at] = x;
                ch.flags[at] = band;
                if let Some(run) = ch.tracker.step(band, pos, min) {
                    ch.push(run);
                }
                while let Some(run) = ch.ready(pos) {
                    self.repair.run(ch, run, fft);
                }
                buf[k] = ch.ring[out];
            }
            self.pos += 1;
            if self.left > 0 {
                self.left -= 1;
                self.gain_db = if self.left == 0 {
                    self.target_db
                } else {
                    self.gain_db + self.step_db
                };
                self.gain = gain_of(self.gain_db);
            }
            if self.gain != 1.0 {
                for buf in io.iter_mut().take(CHANNELS) {
                    buf[k] *= self.gain;
                }
            }
        }
        self.limiter.process(io, from, to);
    }
}

#[cfg(test)]
mod tests {
    use super::super::{render, render_with_print, Native};
    use super::*;
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    const RATE: f64 = 48_000.0;
    /// The whole delay at 48 kHz: 520 and the limiter's 960.
    const LATENCY: usize = DELAY + 960;

    fn settings(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn sine(n: usize, hz: f64, amp: f64, phase: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * hz * i as f64 / RATE + phase).sin()) as f32)
            .collect()
    }

    fn clip(x: &[f32], top: f32, bottom: f32) -> Vec<f32> {
        x.iter().map(|s| s.clamp(bottom, top)).collect()
    }

    fn db(x: f64) -> f64 {
        20.0 * x.log10()
    }

    /// Gaussian-ish noise (sum of uniforms) from a fixed seed.
    fn noise(n: usize, seed: u64, sigma: f64) -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        let mut uniform = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (x >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        (0..n)
            .map(|_| (sigma * (0..12).map(|_| uniform()).sum::<f64>()) as f32)
            .collect()
    }

    /// The effect with these levels scanned (M, m per channel), at `pairs`.
    fn scanned(levels: [(f32, f32); 2], pairs: &[(usize, f64)]) -> DeClipper {
        let mut fx = DeClipper::new(RATE);
        for &(i, v) in pairs {
            fx.set(i, v);
        }
        let mut print = vec![levels[0].0, levels[0].1, 0.0, levels[1].0, levels[1].1, 0.0];
        fx.take_print(&mut print);
        fx.snap();
        fx
    }

    /// Run `x` (one per channel) through `fx` in blocks of `block`, and line the output up
    /// with the input (the latency dropped, zeros fed after the end).
    fn run(fx: &mut DeClipper, x: &[Vec<f32>], block: usize) -> Vec<Vec<f32>> {
        let n = x[0].len();
        let latency = fx.latency();
        let mut io: Vec<Vec<f32>> = (0..CHANNELS)
            .map(|c| {
                let mut v = x[c.min(x.len() - 1)].clone();
                v.resize(n + latency, 0.0);
                v
            })
            .collect();
        let mut at = 0;
        while at < n + latency {
            let to = (at + block).min(n + latency);
            fx.process(&mut io, at, to);
            at = to;
        }
        io.into_iter().map(|v| v[latency..].to_vec()).collect()
    }

    /// Per whole cycle of `period` samples (from `from`): the highest and lowest sample, in
    /// dB against `amp`, the worst of both.
    fn peak_errors(x: &[f32], period: usize, from: usize, to: usize, amp: f64) -> Vec<f64> {
        (from / period..to / period)
            .map(|k| {
                let cycle = &x[k * period..(k + 1) * period];
                let hi = cycle.iter().fold(f32::MIN, |m, s| m.max(*s));
                let lo = cycle.iter().fold(f32::MAX, |m, s| m.min(*s));
                let e = |v: f32| db(f64::from(v.abs()) / amp);
                if e(hi).abs() > e(lo).abs() {
                    e(hi)
                } else {
                    e(lo)
                }
            })
            .collect()
    }

    #[test]
    fn a_sine_clipped_at_minus_6_dbfs_is_scanned_and_its_peaks_rebuilt_within_1_db() {
        // 480 Hz: a whole cycle every 100 samples. A sine at -3 dBFS (out at Gain 0) and one at
        // 0 dBFS (out at Gain -6 dB, under the limiter), both hard-clipped at ±0.5 (-6 dBFS).
        let n = RATE as usize;
        for (amp, gain) in [(10f64.powf(-3.0 / 20.0), 0.0), (1.0, -6.0)] {
            let clean = sine(n, 480.0, amp, 0.3);
            let clipped = clip(&clean, 0.5, -0.5);
            let audio = vec![clipped.clone(), clipped.clone()];
            let s = settings(&[("Gain", gain)]);
            let out = render_with_print(&KIND, RATE, &s, Some(&audio), &audio, 512).unwrap();
            // The scan learned ±0.5 and one run a peak.
            let mut native = Native::new(&KIND, RATE, None).unwrap();
            native.set(0, PARAMS[GAIN].normalized(gain)).unwrap();
            native.capture(RATE, &audio).unwrap();
            let l = learned(native.print().unwrap()).unwrap();
            assert_eq!((l[0].top, l[0].bottom), (0.5, -0.5));
            assert_eq!(l[0].runs, 2 * 480, "{l:?}");
            let g = 10f64.powf(gain / 20.0);
            let clipped_err = peak_errors(&clipped, 100, 2000, n - 2000, amp)
                .into_iter()
                .fold(0.0f64, |m, e| m.max(e.abs()));
            let errors = peak_errors(&out[0], 100, 2000, n - 2000, amp * g);
            let worst = errors.iter().fold(0.0f64, |m, e| m.max(e.abs()));
            let mean = errors.iter().map(|e| e.abs()).sum::<f64>() / errors.len() as f64;
            println!(
                "sine at {:.1} dBFS clipped at -6 dBFS (peaks {clipped_err:.2} dB down), Gain \
                 {gain} dB: rebuilt peaks within {worst:.3} dB (mean {mean:.3} dB)",
                db(amp)
            );
            assert!(worst < 1.0, "{worst}");
            assert_eq!(out[0], out[1]);
        }
    }

    #[test]
    fn the_limiter_holds_rebuilt_peaks_at_minus_0_1_dbfs() {
        // A 0 dBFS sine clipped at -6 dBFS, rebuilt at Gain 0 dB: its peaks (back at 0 dBFS)
        // are held at the limiter's -0.1 dBFS, never above.
        let n = RATE as usize;
        let clipped = clip(&sine(n, 480.0, 1.0, 0.3), 0.5, -0.5);
        let audio = vec![clipped];
        let s = settings(&[("Gain", 0.0)]);
        let out = render_with_print(&KIND, RATE, &s, Some(&audio), &audio, 512).unwrap();
        let ceiling = gain_of(-0.1);
        let steady = &out[0][4000..n - 4000];
        let peak = steady.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        println!(
            "rebuilt to 0 dBFS at Gain 0: peaks {:.3} dBFS",
            db(f64::from(peak))
        );
        assert!(out[0].iter().all(|v| v.abs() <= ceiling));
        assert!(db(f64::from(peak / ceiling)).abs() < 0.02, "{peak}");
    }

    #[test]
    fn a_guitar_like_tone_clipped_has_its_peaks_rebuilt() {
        // Plucks of 196 Hz with decaying harmonics every half second, peaking near 0.9, a
        // little noise, hard-clipped at ±0.45; each cycle's peak against the clean one's.
        let n = 2 * RATE as usize;
        let hiss = noise(n, 3, 1e-4);
        let clean: Vec<f32> = (0..n)
            .map(|i| {
                let t = (i % 24_000) as f64 / RATE;
                let w = 2.0 * PI * 196.0 * i as f64 / RATE;
                let env = (-t * 3.0).exp() * (1.0 - (-t * 400.0).exp());
                let tone = 0.55 * w.sin()
                    + 0.3 * (2.0 * w + 0.4).sin() * (-t * 2.0).exp()
                    + 0.15 * (3.0 * w + 1.1).sin() * (-t * 4.0).exp()
                    + 0.08 * (4.0 * w + 2.0).sin() * (-t * 6.0).exp();
                (env * tone) as f32 + hiss[i]
            })
            .collect();
        let clipped = clip(&clean, 0.45, -0.45);
        let audio = vec![clipped.clone()];
        let out = render_with_print(
            &KIND,
            RATE,
            &settings(&[("Gain", 0.0)]),
            Some(&audio),
            &audio,
            512,
        )
        .unwrap();
        // Over the cycles whose clean peak was clipped by more than 1 dB.
        let period = (RATE / 196.0).round() as usize;
        let (mut before, mut after, mut count) = (0.0, 0.0, 0usize);
        let mut worst = 0.0f64;
        for k in 0..n / period - 1 {
            let r = k * period..(k + 1) * period;
            let peak = |x: &[f32]| x[r.clone()].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            let truth = f64::from(peak(&clean));
            if truth < 0.45 * 10f64.powf(1.0 / 20.0) {
                continue;
            }
            let b = db(f64::from(peak(&clipped)) / truth).abs();
            let a = db(f64::from(peak(&out[0])) / truth).abs();
            before += b;
            after += a;
            worst = worst.max(a);
            count += 1;
        }
        let (before, after) = (before / count as f64, after / count as f64);
        println!(
            "guitar-like tone clipped at -6.9 dBFS: {count} cycles clipped, their peaks {before:.2} \
             dB down on average, rebuilt to within {after:.2} dB (worst {worst:.2} dB)"
        );
        assert!(count > 50);
        assert!(after < before / 3.0, "{before} {after}");
        assert!(after < 1.0, "{after}");
    }

    #[test]
    fn audio_that_never_reaches_the_bands_comes_back_unchanged_times_the_gain() {
        // Not scanned (bands from 0.9 up and -0.9 down), and scanned at ±0.6 (bands from
        // 0.54): a tone with noise peaking under them, at Gain 0, -12 and -5.5 dB, and Auto.
        let n = 30_000;
        let tone: Vec<f32> = sine(n, 440.0, 0.4, 0.0)
            .iter()
            .zip(noise(n, 9, 0.02))
            .map(|(a, b)| a + b)
            .collect();
        let x = [tone.clone(), sine(n, 97.0, 0.5, 1.0)];
        assert!(x.iter().flatten().all(|s| s.abs() < 0.54));
        for (scan, pairs, db) in [
            (None, vec![(GAIN, 0.0)], 0.0),
            (None, vec![], -12.0),
            (Some(0.6f32), vec![(GAIN, -5.5)], -5.5),
            // Auto: the clip level 0.6 brought down by 12 dB: -7.56 dB.
            (
                Some(0.6),
                vec![(AUTO, 1.0)],
                -20.0 * f64::from(0.6f32).log10() - 12.0,
            ),
        ] {
            let mut fx = match scan {
                Some(level) => scanned([(level, -level); 2], &pairs),
                None => {
                    let mut fx = DeClipper::new(RATE);
                    for &(i, v) in &pairs {
                        fx.set(i, v);
                    }
                    fx.snap();
                    fx
                }
            };
            assert_eq!(fx.latency(), LATENCY);
            let g = gain_of(db);
            let mut io: Vec<Vec<f32>> = x
                .iter()
                .map(|c| {
                    let mut v = c.clone();
                    v.resize(n + LATENCY, 0.0);
                    v
                })
                .collect();
            fx.process(&mut io, 0, n + LATENCY);
            for c in 0..CHANNELS {
                assert!(io[c][..LATENCY].iter().all(|s| *s == 0.0));
                let changed = (0..n)
                    .filter(|&i| io[c][i + LATENCY].to_bits() != (x[c][i] * g).to_bits())
                    .count();
                assert_eq!(changed, 0, "{scan:?} {pairs:?}, channel {c}");
            }
            if db == 0.0 {
                assert_eq!(io[0][LATENCY..], tone[..]);
            }
            assert_eq!(fx.repair.rebuilt, 0);
        }
    }

    /// `base` with flat stretches at `level` of each length in `lengths`, 2000 samples apart.
    fn plateaus(base: &[f32], level: f32, lengths: &[usize]) -> (Vec<f32>, Vec<usize>) {
        let mut x = base.to_vec();
        let mut at = Vec::new();
        for (k, &len) in lengths.iter().enumerate() {
            let start = 3000 + 2000 * k;
            x[start..start + len].fill(level);
            at.push(start);
        }
        (x, at)
    }

    /// How many runs `fx` rebuilds in `x`, and whether anything outside `keep` moved.
    fn rebuilds(
        fx: &mut DeClipper,
        x: &[f32],
        keep: Option<std::ops::Range<usize>>,
    ) -> (u64, bool) {
        let out = run(fx, &[x.to_vec()], 512);
        let moved = (0..x.len())
            .any(|i| out[0][i] != x[i] && !keep.as_ref().is_some_and(|r| r.contains(&i)));
        (fx.repair.rebuilt / 2, moved)
    }

    #[test]
    fn min_clip_size_tolerance_and_the_cap_behave_as_the_note_says() {
        // A quiet 100 Hz tone (0.2) with a flat stretch at the clip level 0.5 (scanned ±0.5):
        // a run when it is at least Min. Clip Size long; nothing else moves.
        let n = 20_000;
        let base = sine(n, 100.0, 0.2, 0.0);
        for len in [2, 3, 5, 19, 20, 21] {
            let (x, at) = plateaus(&base, 0.5, &[len]);
            for min in [3.0, 20.0] {
                let mut fx = scanned([(0.5, -0.5); 2], &[(GAIN, 0.0), (MIN_SIZE, min)]);
                let (runs, moved) = rebuilds(&mut fx, &x, Some(at[0]..at[0] + len));
                assert_eq!(
                    runs,
                    u64::from(len as f64 >= min),
                    "{len} samples, Min {min}"
                );
                assert!(!moved);
            }
        }

        // Tolerance: a tone at 0.47 has its peaks in the band at 10 % (from 0.45; each one a
        // run, rebuilt as the tone it is), not at 2 % (0.49); at 0 % only what sits on the
        // level, the flat stretch.
        let (y, at) = plateaus(&sine(n, 100.0, 0.47, 0.0), 0.5, &[10]);
        let peaks = 2 * n / 480;
        for (tolerance, expect) in [(10.0, peaks), (2.0, 1), (0.0, 1)] {
            let mut fx = scanned([(0.5, -0.5); 2], &[(GAIN, 0.0), (TOLERANCE, tolerance)]);
            let (runs, _) = rebuilds(&mut fx, &y, None);
            assert!(
                runs as usize >= expect && runs as usize <= expect + 1,
                "Tolerance {tolerance}: {runs} runs"
            );
        }
        let _ = at;
        // The bands' width follows the louder level: scanned +0.5 / -0.25, at 10 % both are
        // 0.05 wide.
        let b = Bands::new(
            &Learned {
                top: 0.5,
                bottom: -0.25,
                runs: 0,
            },
            10.0,
        );
        assert!(
            (b.high - 0.45).abs() < 1e-12 && (b.low + 0.2).abs() < 1e-12,
            "{b:?}"
        );

        // A run longer than 200 samples (a 20 Hz sine at 0.8 clipped at 0.5: about 680): its
        // first 200 samples rebuilt above the clip, the rest left on it.
        let x = clip(&sine(n, 20.0, 0.8, 0.0), 0.5, -0.5);
        let mut fx = scanned([(0.5, -0.5); 2], &[(GAIN, 0.0)]);
        let out = run(&mut fx, std::slice::from_ref(&x), 512);
        // The first run starts where the sine first reaches 0.45.
        let s = x.iter().position(|v| *v >= 0.45).unwrap();
        let end = s + x[s..].iter().position(|v| *v < 0.45).unwrap();
        assert!(end - s > 600, "{}", end - s);
        let flat = (s..s + CAP).filter(|&i| x[i] == 0.5).count();
        let above = (s..s + CAP)
            .filter(|&i| x[i] == 0.5 && out[0][i] > 0.5)
            .count();
        println!(
            "a run of {}: {above} of the {flat} clipped samples in its first 200 raised",
            end - s
        );
        // (The rebuilt stretch ends on the clipped sample after it, so its last ones come
        // back down to the clip.)
        assert!(4 * above >= 3 * flat, "{above} of {flat}");
        assert!((s + CAP..end).all(|i| out[0][i] == x[i]));
    }

    #[test]
    fn nothing_quieter_than_minus_24_dbfs_is_taken_for_clipping() {
        let n = 20_000;
        let base = sine(n, 100.0, 0.02, 0.0);
        // Scanned at 0.07 (just over -24 dBFS) with Tolerance 50 %: the band would reach down
        // to 0.035, but stops at the floor (0.0631): a stretch at 0.05 is left, one at 0.07
        // is a run.
        let pairs = [(GAIN, 0.0), (TOLERANCE, 50.0)];
        let mut fx = scanned([(0.07, -0.07); 2], &pairs);
        assert_eq!(fx.channels[0].bands.high, FLOOR);
        let (x, _) = plateaus(&base, 0.05, &[10]);
        assert_eq!(rebuilds(&mut fx, &x, None), (0, false));
        let (y, at) = plateaus(&base, 0.07, &[10]);
        let mut fx = scanned([(0.07, -0.07); 2], &pairs);
        assert_eq!(rebuilds(&mut fx, &y, Some(at[0]..at[0] + 10)).0, 1);
        // A scan of audio that never reaches -24 dBFS is refused; a silent channel's bands
        // are off.
        let values: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let err = capture(RATE, &values, &[clip(&base, 0.05, -0.05)]).unwrap_err();
        assert!(err.to_string().contains("-24 dBFS"), "{err}");
        let loud = clip(&sine(n, 100.0, 0.9, 0.0), 0.5, -0.5);
        let print = capture(RATE, &values, &[loud, vec![0.0; n]]).unwrap();
        let l = learned(&print).unwrap();
        let off = Bands::new(&l[1], 10.0);
        assert_eq!((off.high, off.low), (f64::INFINITY, f64::NEG_INFINITY));
        assert_eq!(l[1].runs, 0);
        // 100 Hz for 20 000 samples: 41 2/3 cycles, 84 peaks (the last one cut short by the end,
        // counted as the scan flushes it).
        assert_eq!(l[0].runs, 84);
    }

    #[test]
    fn not_scanned_it_takes_full_scale_so_digital_clipping_is_caught() {
        // A 480 Hz sine at +3.5 dBFS clipped at ±1 (as a float file written to 16 bits would
        // be): its peaks rebuilt, at Gain -12 dB (under the limiter), within 1 dB.
        let n = 24_000;
        let amp = 1.5;
        let x = clip(&sine(n, 480.0, amp, 0.2), 1.0, -1.0);
        let out = render(&KIND, RATE, &settings(&[]), std::slice::from_ref(&x), 512).unwrap();
        let g = f64::from(gain_of(-12.0));
        let errors = peak_errors(&out[0], 100, 2000, n - 2000, amp * g);
        let worst = errors.iter().fold(0.0f64, |m, e| m.max(e.abs()));
        println!("no scan, a sine clipped at 0 dBFS (3.5 dB): peaks within {worst:.3} dB");
        assert!(worst < 1.0, "{worst}");
    }

    #[test]
    fn the_same_at_any_block_size_and_the_channels_apart() {
        let n = 40_000;
        let left = clip(&sine(n, 480.0, 0.8, 0.0), 0.5, -0.5);
        let hiss = noise(n, 4, 0.01);
        let right: Vec<f32> = clip(&sine(n, 211.0, 0.7, 0.5), 0.45, -0.3)
            .iter()
            .zip(&hiss)
            .map(|(a, b)| a + b)
            .collect();
        let audio = vec![left.clone(), right.clone()];
        let s = settings(&[
            ("Gain", -3.0),
            ("FFT Interpolation", 1.0),
            ("FFT Size", 5.0),
        ]);
        let a = render_with_print(&KIND, RATE, &s, Some(&audio), &audio, 512).unwrap();
        for block in [77, 1, 4096] {
            let b = render_with_print(&KIND, RATE, &s, Some(&audio), &audio, block).unwrap();
            assert_eq!(a, b, "{block}");
        }
        // Each channel on its own: the left the same whatever the right holds (quiet noise,
        // which the linked limiter leaves alone).
        let other = vec![left.clone(), noise(n, 5, 0.05)];
        let print = vec![left.clone(), right.clone()];
        let c = render_with_print(&KIND, RATE, &s, Some(&print), &other, 512).unwrap();
        assert_eq!(a[0], c[0]);
        assert_ne!(a[1], c[1]);
    }

    #[test]
    fn the_scan_learns_each_channels_levels_and_is_saved_with_the_state() {
        let n = 24_000;
        let left = clip(&sine(n, 480.0, 0.8, 0.0), 0.5, -0.5);
        let right = clip(&sine(n, 240.0, 0.8, 0.0), 0.5, -0.3);
        let audio = vec![left, right];
        let mut a = Native::new(&KIND, RATE, None).unwrap();
        assert!(a.print().is_none());
        a.capture(RATE, &audio).unwrap();
        let l = learned(a.print().unwrap()).unwrap();
        assert_eq!((l[0].top, l[0].bottom, l[0].runs), (0.5, -0.5, 480));
        assert_eq!(
            (l[1].top as f32, l[1].bottom as f32, l[1].runs),
            (0.5, -0.3, 240)
        );
        let b = Native::new(&KIND, RATE, Some(&a.save_state())).unwrap();
        assert_eq!(a.print(), b.print());
        // A mono scan is both channels'.
        a.capture(RATE, &audio[..1]).unwrap();
        let mono = learned(a.print().unwrap()).unwrap();
        assert_eq!(mono[0], mono[1]);
        assert!(a.capture(RATE, &[Vec::new()]).is_err());
    }

    #[test]
    fn a_transport_start_empties_it_and_new_is_started() {
        let n = 10_000;
        let x = vec![clip(&sine(n, 480.0, 0.8, 0.0), 0.5, -0.5)];
        let mut fresh = scanned([(0.5, -0.5); 2], &[]);
        let first = run(&mut fresh, &x, 333);
        let mut used = scanned([(0.5, -0.5); 2], &[]);
        run(&mut used, &[noise(5000, 2, 0.3)], 512);
        used.start();
        assert_eq!(run(&mut used, &x, 333), first);
    }

    #[test]
    fn the_settings_show_as_expected() {
        let defaults: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let shown: Vec<String> = PARAMS
            .iter()
            .zip(&defaults)
            .map(|(p, v)| p.display(*v))
            .collect();
        assert_eq!(
            shown,
            ["-12.0 dB", "Off", "10.0 %", "3 samples", "Cubic", "32"]
        );
        for p in PARAMS.iter() {
            for v in [p.min, p.default, p.max] {
                assert!((p.plain(p.normalized(v)) - v).abs() < 1e-9, "{}", p.name);
            }
        }
        assert_eq!(fft_size(3.0), 8);
        assert_eq!(fft_size(8.0), 256);
        // Auto Gain: full scale -12 dB, a clip at -6 dBFS about -6 dB, quieter 0 dB.
        let at = |p: f64| {
            auto_gain_db(
                &[Learned {
                    top: p,
                    bottom: -p,
                    runs: 0,
                }; 2],
            )
        };
        assert!((at(1.0) + 12.0).abs() < 1e-9);
        assert!((at(0.5) + 5.979).abs() < 1e-3);
        assert_eq!(at(0.2), 0.0);
        assert_eq!(latency(RATE), LATENCY);
        assert_eq!(latency(44_100.0), DELAY + 882);
    }

    /// The cost: run by hand in release (`cargo test --release -p daw-core declipper_cost --
    /// --ignored --nocapture`); also what a scan of the DI finds, if present.
    #[test]
    #[ignore]
    fn declipper_cost() {
        let n = 48_000 * 10;
        for (name, x, fft) in [
            (
                "480 Hz clipped 3 dB",
                clip(&sine(n, 480.0, 0.707, 0.0), 0.5, -0.5),
                0.0,
            ),
            (
                "2 kHz clipped 6 dB",
                clip(&sine(n, 2000.0, 1.0, 0.0), 0.5, -0.5),
                0.0,
            ),
            (
                "110 Hz clipped 6 dB, FFT 256",
                clip(&sine(n, 110.0, 1.0, 0.0), 0.5, -0.5),
                8.0,
            ),
        ] {
            let pairs: Vec<(usize, f64)> = if fft > 0.0 {
                vec![(FFT, 1.0), (FFT_SIZE, fft)]
            } else {
                vec![]
            };
            let mut fx = scanned([(0.5, -0.5); 2], &pairs);
            let mut io = vec![x.clone(); CHANNELS];
            let (mut total, mut worst) = (0.0f64, 0.0f64);
            for at in (0..n).step_by(64) {
                let t = std::time::Instant::now();
                fx.process(&mut io, at, (at + 64).min(n));
                let took = t.elapsed().as_secs_f64();
                total += took;
                worst = worst.max(took);
            }
            println!(
                "{name}: {} runs, {:.2} % of real time (stereo, 48 kHz); the worst block of 64: \
                 {:.3} ms",
                fx.repair.rebuilt / 2,
                100.0 * total / (n as f64 / RATE),
                worst * 1e3
            );
        }
        let di = crate::test_audio();
        if let Ok(audio) = crate::wav::read(&di) {
            let defaults: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
            let print = capture(f64::from(audio.sample_rate), &defaults, &audio.channels).unwrap();
            println!(
                "the DI scanned at the defaults: {:?}",
                learned(&print).unwrap()[0]
            );
            // What its own scan then changes at Gain 0 dB (its loudest peaks are "clipped").
            let x = &audio.channels[0];
            let rate = f64::from(audio.sample_rate);
            let mut fx = DeClipper::new(rate);
            fx.set(GAIN, 0.0);
            fx.take_print(&mut print.clone());
            fx.snap();
            let latency = fx.latency();
            let mut io = vec![x.clone(); CHANNELS];
            for v in &mut io {
                v.resize(x.len() + latency, 0.0);
            }
            fx.process(&mut io, 0, x.len() + latency);
            let out = &io[0][latency..];
            let changed = (0..x.len()).filter(|&i| out[i] != x[i]).count();
            let lift = (0..x.len())
                .map(|i| f64::from(out[i].abs()) - f64::from(x[i].abs()))
                .fold(0.0f64, f64::max);
            println!(
                "the DI through its own scan: {} runs, {changed} samples changed, peaks raised \
                 by at most {lift:.4}",
                fx.repair.rebuilt / 2
            );
        }
    }
}
