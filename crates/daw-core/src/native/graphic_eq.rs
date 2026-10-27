//! Graphic EQ, 10, 20 and 30 bands: a linear-phase FIR, not a bank of filters.
//!
//! - Band centres are 31.25 Hz times powers of 2, √2 or ∛2. Each band's shape is a
//!   raised-cosine crossfade with its neighbours in log frequency (the lowest reaches down to
//!   DC), sampled on a grid, made an impulse by an inverse FFT and windowed with a 4-term
//!   Blackman–Harris window; the kernel is a unit impulse plus each band's shape times its
//!   gain less one, so gains blend linearly in amplitude.
//! - "Accuracy" is the FIR's half-length; it is fixed at 1000 here (TD102), so the
//!   kernel has 1999 taps and the delay is 7193 samples, compensated like any plug-in's.
//! - It runs as FFT overlap-add in double precision, both channels in one complex FFT. A
//!   change, by hand or by automation, takes effect from the next FFT block:
//!   up to 6194 samples later.
//! - Band gains are kept within ±(Range/2 − 0.01) dB.

use super::{Kind, NativeEffect, ParamSpec};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::f64::consts::PI;
use std::sync::Arc;

/// The FIR's half-length (Accuracy, at its default).
const ACCURACY: usize = 1000;
/// The kernel's taps.
const TAPS: usize = 2 * ACCURACY - 1;
/// The convolution's FFT size: 2^⌈log2(4·2L)⌉.
const FFT: usize = 8192;
/// Input samples per FFT block, so a block and the kernel fill the FFT exactly.
const HOP: usize = FFT - TAPS + 1;

const MASTER: usize = 0;
const RANGE: usize = 1;
const FIRST_BAND: usize = 2;

const BAND: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -70.0,
    max: 70.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

const fn params<const P: usize>(names: [&'static str; P]) -> [ParamSpec; P] {
    let mut all = [BAND; P];
    all[MASTER] = ParamSpec {
        name: "Master Gain",
        min: -64.0,
        max: 64.0,
        ..BAND
    };
    all[RANGE] = ParamSpec {
        name: "Range",
        min: 1.5,
        max: 120.0,
        default: 48.0,
        automatable: false,
        ..BAND
    };
    let mut i = FIRST_BAND;
    while i < P {
        all[i] = ParamSpec {
            name: names[i],
            ..BAND
        };
        i += 1;
    }
    all
}

static PARAMS_10: [ParamSpec; 12] = params([
    "", "", "31 Hz", "63 Hz", "125 Hz", "250 Hz", "500 Hz", "1 kHz", "2 kHz", "4 kHz", "8 kHz",
    "16 kHz",
]);
static PARAMS_20: [ParamSpec; 22] = params([
    "", "", "31 Hz", "44 Hz", "63 Hz", "88 Hz", "125 Hz", "177 Hz", "250 Hz", "354 Hz", "500 Hz",
    "707 Hz", "1 kHz", "1.4 kHz", "2 kHz", "2.8 kHz", "4 kHz", "5.7 kHz", "8 kHz", "11.3 kHz",
    "16 kHz", "22.6 kHz",
]);
static PARAMS_30: [ParamSpec; 32] = params([
    "", "", "31 Hz", "39 Hz", "50 Hz", "63 Hz", "79 Hz", "99 Hz", "125 Hz", "157 Hz", "198 Hz",
    "250 Hz", "315 Hz", "397 Hz", "500 Hz", "630 Hz", "794 Hz", "1 kHz", "1.26 kHz", "1.59 kHz",
    "2 kHz", "2.52 kHz", "3.17 kHz", "4 kHz", "5.04 kHz", "6.35 kHz", "8 kHz", "10.1 kHz",
    "12.7 kHz", "16 kHz", "20.2 kHz", "25.4 kHz",
]);

pub static KIND_10: Kind = Kind {
    id: "graphic-eq-10",
    name: "Graphic EQ (10 bands)",
    category: "EQ",
    params: &PARAMS_10,
    make: |rate| Box::new(GraphicEq::new(rate, 10)),
    response: Some(|rate, v, hz| response(rate, 10, v, hz)),
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

pub static KIND_20: Kind = Kind {
    id: "graphic-eq-20",
    name: "Graphic EQ (20 bands)",
    category: "EQ",
    params: &PARAMS_20,
    make: |rate| Box::new(GraphicEq::new(rate, 20)),
    response: Some(|rate, v, hz| response(rate, 20, v, hz)),
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

pub static KIND_30: Kind = Kind {
    id: "graphic-eq-30",
    name: "Graphic EQ (30 bands)",
    category: "EQ",
    params: &PARAMS_30,
    make: |rate| Box::new(GraphicEq::new(rate, 30)),
    response: Some(|rate, v, hz| response(rate, 30, v, hz)),
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The ratio between neighbouring centres.
fn ratio(bands: usize) -> f64 {
    match bands {
        10 => 2.0,
        20 => std::f64::consts::SQRT_2,
        _ => 2f64.powf(1.0 / 3.0),
    }
}

/// Where `hz` falls among the centres: the band whose centre is at or below it (−1 below the
/// first) and how far towards the next it is (0..1), in log frequency.
fn place(hz: f64, r: f64) -> (i64, f64) {
    let u = (hz / 31.25).ln() / r.ln();
    if u < 0.0 {
        return (-1, 0.0);
    }
    let j = (u + 1e-7).floor();
    (j as i64, u - j)
}

/// Band `i`'s weight at `hz` in the design (before windowing): falling from its centre to the
/// next, rising from the previous centre to its own; the first band covers everything below.
fn weight(i: usize, hz: f64, r: f64) -> f64 {
    let (j, t) = place(hz, r);
    let falling = (1.0 + (PI * t).cos()) / 2.0;
    if j < 0 {
        f64::from(u8::from(i == 0))
    } else if j as usize == i {
        falling
    } else if j as usize + 1 == i {
        1.0 - falling
    } else {
        0.0
    }
}

/// Each band's gain, kept within the range.
fn gains(v: &[f64], bands: usize) -> impl Iterator<Item = f64> + '_ {
    let half = v[RANGE] / 2.0;
    (0..bands).map(move |b| {
        let db = v[FIRST_BAND + b];
        let db = if db >= half {
            half - 0.01
        } else if db <= -half {
            -half + 0.01
        } else {
            db
        };
        10f64.powf(db / 20.0)
    })
}

/// The editor's curve: the designed response, before the window smooths it (by about
/// ±4·fs/(2·1000), so the curve is a little sharper than the sound at the lowest bands).
fn response(_rate: f64, bands: usize, v: &[f64], hz: f64) -> f64 {
    let r = ratio(bands);
    let a = 1.0
        + gains(v, bands)
            .enumerate()
            .map(|(i, g)| (g - 1.0) * weight(i, hz, r))
            .sum::<f64>();
    20.0 * (a.abs() * 10f64.powf(v[MASTER] / 20.0)).max(1e-12).log10()
}

/// The 4-term Blackman–Harris window at `k` of `0..=d`.
fn blackman_harris(k: f64, d: f64) -> f64 {
    let x = 2.0 * PI * k / d;
    0.35875 - 0.48829 * x.cos() + 0.14128 * (2.0 * x).cos() - 0.01168 * (3.0 * x).cos()
}

/// Band `i`'s windowed impulse at gain 1: `TAPS` taps, its centre at `ACCURACY − 1`.
fn shape(i: usize, bands: usize, rate: f64) -> Vec<f64> {
    let r = ratio(bands);
    // The design grid: 2^(⌊log2(1.5·(2L − 2))⌋ + 1) bins (4096 for L = 1000).
    let p = 1usize << (((1.5 * (2 * ACCURACY - 2) as f64).log2().floor() as u32) + 1);
    let mut grid = vec![Complex::new(0.0, 0.0); p];
    for n in 1..=p / 2 {
        let a = weight(i, n as f64 * rate / p as f64, r);
        grid[n] = Complex::new(a, 0.0);
        grid[(p - n) % p] = Complex::new(a, 0.0);
    }
    // DC belongs to the lowest band; Nyquist to the band whose upper edge reaches it while
    // its centre is more than a bin below it.
    let centre = 31.25 * r.powi(i as i32);
    let upper = if i + 1 == bands { 50_000.0 } else { centre * r };
    grid[0] = Complex::new(f64::from(u8::from(i == 0)), 0.0);
    let nyquist = rate / 2.0;
    if upper >= nyquist && centre < nyquist - rate / p as f64 {
        grid[p / 2] = Complex::new(1.0, 0.0);
    }
    FftPlanner::new().plan_fft_inverse(p).process(&mut grid);
    let d = (2 * ACCURACY - 2) as f64;
    (0..TAPS)
        .map(|t| {
            let m = t as i64 - (ACCURACY as i64 - 1);
            let at = m.rem_euclid(p as i64) as usize;
            grid[at].re / p as f64 * blackman_harris(t as f64, d)
        })
        .collect()
}

struct GraphicEq {
    bands: usize,
    values: Vec<f64>,
    /// Each band's shape at gain 1 (made once, off the audio thread).
    shapes: Vec<Vec<f64>>,
    /// The kernel and its spectrum, rebuilt at the next block after a change.
    kernel: Vec<Complex<f64>>,
    spectrum: Vec<Complex<f64>>,
    stale: bool,
    forward: Arc<dyn Fft<f64>>,
    inverse: Arc<dyn Fft<f64>>,
    scratch: Vec<Complex<f64>>,
    /// The block being filled (left + i·right), what it is producing, and the overlap.
    input: Vec<Complex<f64>>,
    output: Vec<Complex<f64>>,
    overlap: Vec<Complex<f64>>,
    work: Vec<Complex<f64>>,
    filled: usize,
}

impl GraphicEq {
    fn new(rate: f64, bands: usize) -> Self {
        let mut planner = FftPlanner::new();
        let forward = planner.plan_fft_forward(FFT);
        let inverse = planner.plan_fft_inverse(FFT);
        let scratch_len = forward
            .get_inplace_scratch_len()
            .max(inverse.get_inplace_scratch_len());
        let params = match bands {
            10 => &PARAMS_10[..],
            20 => &PARAMS_20[..],
            _ => &PARAMS_30[..],
        };
        let zero = Complex::new(0.0, 0.0);
        let mut eq = Self {
            bands,
            values: params.iter().map(|p| p.default).collect(),
            shapes: (0..bands).map(|i| shape(i, bands, rate)).collect(),
            kernel: vec![zero; FFT],
            spectrum: vec![zero; FFT],
            stale: true,
            forward,
            inverse,
            scratch: vec![zero; scratch_len],
            input: vec![zero; HOP],
            output: vec![zero; HOP],
            overlap: vec![zero; FFT],
            work: vec![zero; FFT],
            filled: 0,
        };
        eq.rebuild();
        eq
    }

    /// The kernel from the gains, and its spectrum, with the master gain and the inverse
    /// FFT's scale in it.
    fn rebuild(&mut self) {
        let zero = Complex::new(0.0, 0.0);
        self.kernel.fill(zero);
        self.kernel[ACCURACY - 1] = Complex::new(1.0, 0.0);
        // On the audio thread: no allocation, the gains straight from the values.
        for (shape, g) in self.shapes.iter().zip(gains(&self.values, self.bands)) {
            if g == 1.0 {
                continue;
            }
            for (k, s) in self.kernel.iter_mut().zip(shape) {
                k.re += (g - 1.0) * s;
            }
        }
        let master = 10f64.powf(self.values[MASTER] / 20.0) / FFT as f64;
        self.spectrum.copy_from_slice(&self.kernel);
        self.forward
            .process_with_scratch(&mut self.spectrum, &mut self.scratch);
        for s in &mut self.spectrum {
            *s *= master;
        }
        self.stale = false;
    }

    /// Convolve the filled block: its output, and the tail it leaves for the next ones.
    fn block(&mut self) {
        if self.stale {
            self.rebuild();
        }
        let zero = Complex::new(0.0, 0.0);
        self.work[..HOP].copy_from_slice(&self.input);
        self.work[HOP..].fill(zero);
        self.forward
            .process_with_scratch(&mut self.work, &mut self.scratch);
        for (w, s) in self.work.iter_mut().zip(&self.spectrum) {
            *w *= s;
        }
        self.inverse
            .process_with_scratch(&mut self.work, &mut self.scratch);
        for (o, w) in self.overlap.iter_mut().zip(&self.work) {
            *o += w;
        }
        self.output.copy_from_slice(&self.overlap[..HOP]);
        self.overlap.copy_within(HOP.., 0);
        self.overlap[FFT - HOP..].fill(zero);
    }
}

impl NativeEffect for GraphicEq {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            if *v != value {
                *v = value;
                self.stale = true;
            }
        }
    }

    /// Automation too only marks the kernel stale: it is rebuilt once, at the next FFT block,
    /// not at every point of a moving lane.
    fn ramp(&mut self, index: usize, value: f64, _samples: usize) {
        self.set(index, value);
    }

    fn snap(&mut self) {
        if self.stale {
            self.rebuild();
        }
    }

    fn start(&mut self) {
        // From the first sample played, in fresh blocks: what played before (while stopped,
        // or the last pass) doesn't decide where this pass's blocks fall.
        let zero = Complex::new(0.0, 0.0);
        self.input.fill(zero);
        self.output.fill(zero);
        self.overlap.fill(zero);
        self.filled = 0;
    }

    fn latency(&self) -> usize {
        // A block's worth of buffering, then the kernel's centre.
        HOP + ACCURACY - 1
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let [left, right] = io else {
            return;
        };
        for k in from..to {
            self.input[self.filled] = Complex::new(f64::from(left[k]), f64::from(right[k]));
            let out = self.output[self.filled];
            left[k] = out.re as f32;
            right[k] = out.im as f32;
            self.filled += 1;
            if self.filled == HOP {
                self.block();
                self.filled = 0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn the_bands_crossfade_to_a_flat_sum() {
        for bands in [10, 20, 30] {
            let r = ratio(bands);
            for hz in [10.0, 40.0, 333.0, 1000.0, 5000.0] {
                let sum: f64 = (0..bands).map(|i| weight(i, hz, r)).sum();
                assert!((sum - 1.0).abs() < 1e-9, "{bands} bands at {hz} Hz: {sum}");
            }
        }
    }

    #[test]
    fn flat_settings_are_a_pure_delay() {
        let n = 20_000;
        let x: Vec<f32> = (0..n).map(|i| ((i as f32) * 0.37).sin() * 0.5).collect();
        let y = render(
            &KIND_10,
            48_000.0,
            &BTreeMap::new(),
            &[x.clone(), x.clone()],
            512,
        )
        .unwrap();
        let worst = y[0]
            .iter()
            .zip(&x)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-6, "{worst}");
    }

    #[test]
    fn a_band_boosts_its_centre_and_the_render_follows_the_curve() {
        let settings: BTreeMap<String, f64> = [("1 kHz".to_string(), 12.0)].into();
        let n = 48_000;
        let tone = |hz: f64| -> Vec<f32> {
            (0..n)
                .map(|i| (0.1 * (2.0 * PI * hz * i as f64 / 48_000.0).sin()) as f32)
                .collect()
        };
        let a = render(
            &KIND_10,
            48_000.0,
            &settings,
            &[tone(1000.0), tone(250.0)],
            512,
        )
        .unwrap();
        let b = render(
            &KIND_10,
            48_000.0,
            &settings,
            &[tone(1000.0), tone(250.0)],
            77,
        )
        .unwrap();
        assert_eq!(a, b);
        let rms = |x: &[f32]| {
            (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        let level = |x: &[f32]| 20.0 * (rms(&x[10_000..40_000]) / (0.1 / 2f64.sqrt())).log10();
        assert!((level(&a[0]) - 12.0).abs() < 0.1, "1 kHz: {}", level(&a[0]));
        // Two octaves away the band's weight is gone.
        assert!(level(&a[1]).abs() < 0.1, "250 Hz: {}", level(&a[1]));
        let mut v: Vec<f64> = PARAMS_10.iter().map(|p| p.default).collect();
        v[FIRST_BAND + 5] = 12.0;
        assert!((response(48_000.0, 10, &v, 1000.0) - 12.0).abs() < 1e-9);
    }

    #[test]
    fn gains_stay_inside_the_range() {
        let mut v: Vec<f64> = PARAMS_10.iter().map(|p| p.default).collect();
        v[RANGE] = 36.0;
        v[FIRST_BAND] = 30.0;
        let g = gains(&v, 10).next().unwrap();
        assert!((20.0 * g.log10() - 17.99).abs() < 1e-9);
    }
}
