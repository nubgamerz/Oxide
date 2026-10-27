//! Parametric EQ: fixed bands of filters and peaks, designed and run like this:
//!
//! - fixed bands, run in this order: high-pass, low-pass (Butterworth cascades of order 1 to 8,
//!   prewarped at their cutoff), low shelf, high shelf (first or second order), five peaking
//!   bands, then the master gain; the signal is rounded to f32 between stages;
//! - peaks and shelves are defined at a reference gain GB, not at −3 dB: the half-gain point
//!   for small gains, 3 dB from the plateau for large ones; peaks are the bilinear
//!   Regalia–Mitra / Orfanidis design, whose bandwidth (Hz, at GB) is exact in the digital
//!   domain; Q is the centre over that bandwidth, held while the frequency moves (constant Q)
//!   or the bandwidth is held instead (constant width);
//! - sections run in direct form I with double state; a change ramps a section's
//!   coefficients linearly over 1323 samples (30 ms at 44.1 kHz, at any rate),
//!   enabling a band fades it in from flat, and a high- or low-pass change starts that filter
//!   afresh; a new instance or a render starts at its settings.
//!
//! Not yet: Ultra-Quiet (the state-space form) and re-mapping
//! frequencies saved at another sample rate (TD102).

use super::biquad::Coefficients;
use super::{flush_all, Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;
use std::f64::consts::{PI, SQRT_2};

/// Samples a coefficient change takes (int(0.03 × 44100), whatever the rate).
const RAMP: usize = 1323;

// Parameter indices.
const MASTER: usize = 0;
const CONSTANT_Q: usize = 1;
const HP_ON: usize = 2;
const HP_FREQ: usize = 3;
const HP_ORDER: usize = 4;
const LP_ON: usize = 5;
const LP_FREQ: usize = 6;
const LP_ORDER: usize = 7;
const LOW_SHELF: usize = 8;
const HIGH_SHELF: usize = 12;
/// A shelf's parameters from its first: enable, frequency, gain, second order.
const SHELF_ON: usize = 0;
const SHELF_FREQ: usize = 1;
const SHELF_GAIN: usize = 2;
const SHELF_SECOND: usize = 3;
const BANDS: usize = 16;
/// A band's parameters from its first: enable, frequency, gain, Q, width.
const BAND_ON: usize = 0;
const BAND_FREQ: usize = 1;
const BAND_GAIN: usize = 2;
const BAND_Q: usize = 3;
const BAND_WIDTH: usize = 4;
const BAND_PARAMS: usize = 5;
const BAND_COUNT: usize = 5;

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} kHz", v / 1000.0)
    } else {
        format!("{v:.1} Hz")
    }
}

fn slope(v: f64) -> String {
    format!("{} dB/oct", 6 * v.round() as i64)
}

const SWITCH: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0,
    max: 1.0,
    default: 1.0,
    steps: 1,
    decimals: 0,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

/// Frequencies are bounded at 22050 Hz (the parameters are defined at 44.1 kHz); the DSP keeps a frequency below Nyquist at lower rates.
const FREQ: ParamSpec = ParamSpec {
    name: "",
    unit: "Hz",
    min: 20.0,
    max: 22_050.0,
    default: 1000.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: true,
    format: Some(hz),
};

const GAIN: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -48.0,
    max: 48.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

const ORDER: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 1.0,
    max: 8.0,
    default: 4.0,
    steps: 7,
    decimals: 0,
    automatable: false,
    link: None,
    log_slider: false,
    format: Some(slope),
};

const Q: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0001,
    max: 10_000.0,
    default: 2.0,
    steps: 0,
    decimals: 2,
    automatable: true,
    link: None,
    log_slider: true,
    format: None,
};

const WIDTH: ParamSpec = ParamSpec {
    name: "",
    unit: "Hz",
    min: 0.0001,
    max: 10_000.0,
    default: 100.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: true,
    format: None,
};

macro_rules! band {
    ($n:literal, $f:expr, $w:expr) => {
        [
            ParamSpec {
                name: concat!("Band ", $n, " Enable"),
                ..SWITCH
            },
            ParamSpec {
                name: concat!("Band ", $n, " Frequency"),
                default: $f,
                ..FREQ
            },
            ParamSpec {
                name: concat!("Band ", $n, " Gain"),
                ..GAIN
            },
            ParamSpec {
                name: concat!("Band ", $n, " Q"),
                ..Q
            },
            ParamSpec {
                name: concat!("Band ", $n, " Width"),
                default: $w,
                ..WIDTH
            },
        ]
    };
}

const fn params() -> [ParamSpec; BANDS + BAND_PARAMS * BAND_COUNT] {
    let head = [
        ParamSpec {
            name: "Master Gain",
            min: -96.0,
            max: 48.0,
            ..GAIN
        },
        ParamSpec {
            name: "Constant Q",
            automatable: false,
            ..SWITCH
        },
        ParamSpec {
            name: "High-Pass Enable",
            default: 0.0,
            automatable: false,
            ..SWITCH
        },
        ParamSpec {
            name: "High-Pass Frequency",
            default: 20.0,
            automatable: false,
            ..FREQ
        },
        ParamSpec {
            name: "High-Pass Slope",
            ..ORDER
        },
        ParamSpec {
            name: "Low-Pass Enable",
            default: 0.0,
            automatable: false,
            ..SWITCH
        },
        ParamSpec {
            name: "Low-Pass Frequency",
            default: 20_000.0,
            automatable: false,
            ..FREQ
        },
        ParamSpec {
            name: "Low-Pass Slope",
            ..ORDER
        },
        ParamSpec {
            name: "Low Shelf Enable",
            ..SWITCH
        },
        ParamSpec {
            name: "Low Shelf Frequency",
            default: 40.0,
            ..FREQ
        },
        ParamSpec {
            name: "Low Shelf Gain",
            ..GAIN
        },
        ParamSpec {
            name: "Low Shelf Second Order",
            default: 0.0,
            ..SWITCH
        },
        ParamSpec {
            name: "High Shelf Enable",
            ..SWITCH
        },
        ParamSpec {
            name: "High Shelf Frequency",
            default: 18_000.0,
            ..FREQ
        },
        ParamSpec {
            name: "High Shelf Gain",
            ..GAIN
        },
        ParamSpec {
            name: "High Shelf Second Order",
            default: 0.0,
            ..SWITCH
        },
    ];
    let bands = [
        band!("1", 50.0, 25.0),
        band!("2", 200.0, 100.0),
        band!("3", 800.0, 400.0),
        band!("4", 3200.0, 1600.0),
        band!("5", 12_800.0, 6400.0),
    ];
    let mut all = [SWITCH; BANDS + BAND_PARAMS * BAND_COUNT];
    let mut i = 0;
    while i < BANDS {
        all[i] = head[i];
        i += 1;
    }
    let mut b = 0;
    while b < BAND_COUNT {
        let mut k = 0;
        while k < BAND_PARAMS {
            all[BANDS + b * BAND_PARAMS + k] = bands[b][k];
            k += 1;
        }
        b += 1;
    }
    all
}

static PARAMS: [ParamSpec; BANDS + BAND_PARAMS * BAND_COUNT] = params();

pub static KIND: Kind = Kind {
    id: "parametric-eq",
    name: "Parametric EQ",
    category: "EQ",
    params: &PARAMS,
    make: |rate| Box::new(ParametricEq::new(rate)),
    response: Some(response),
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The editor's curve: the digital response of the sections the DSP would run.
fn response(rate: f64, v: &[f64], hz: f64) -> f64 {
    let d = Design::new(rate, v);
    let mut gain = f64::from(d.master);
    for c in d.hp.iter().chain(&d.lp).flat_map(Cascade::iter) {
        gain *= c.magnitude(hz, rate);
    }
    for c in d.shelves.iter().chain(&d.bands).flatten() {
        gain *= c.magnitude(hz, rate);
    }
    20.0 * gain.max(1e-12).log10()
}

/// A Butterworth cascade: up to four sections (order 8).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Cascade {
    sections: [Coefficients; 4],
    count: usize,
    /// The last section is first order (an odd order).
    odd: bool,
}

impl Cascade {
    fn iter(&self) -> impl Iterator<Item = &Coefficients> {
        self.sections[..self.count].iter()
    }
}

/// What the settings ask for, as coefficients.
struct Design {
    hp: Option<Cascade>,
    lp: Option<Cascade>,
    /// Low then high; `None` when off (or flat).
    shelves: [Option<Coefficients>; 2],
    bands: [Option<Coefficients>; BAND_COUNT],
    master: f32,
}

impl Design {
    fn new(rate: f64, v: &[f64]) -> Self {
        let on = |i: usize| v[i] >= 0.5;
        let shelf = |first: usize, high: bool| {
            (on(first + SHELF_ON))
                .then(|| {
                    shelf(
                        rate,
                        v[first + SHELF_FREQ],
                        v[first + SHELF_GAIN],
                        on(first + SHELF_SECOND),
                        high,
                    )
                })
                .flatten()
        };
        let mut bands = [None; BAND_COUNT];
        for (b, slot) in bands.iter_mut().enumerate() {
            let p = BANDS + b * BAND_PARAMS;
            if on(p + BAND_ON) {
                let f = v[p + BAND_FREQ];
                let q = if on(CONSTANT_Q) {
                    v[p + BAND_Q]
                } else {
                    (f / v[p + BAND_WIDTH]).clamp(1e-4, 1e4)
                };
                *slot = peak(rate, f, v[p + BAND_GAIN], q);
            }
        }
        Self {
            hp: on(HP_ON).then(|| butterworth(rate, v[HP_FREQ], v[HP_ORDER], true)),
            lp: on(LP_ON).then(|| butterworth(rate, v[LP_FREQ], v[LP_ORDER], false)),
            shelves: [shelf(LOW_SHELF, false), shelf(HIGH_SHELF, true)],
            bands,
            master: 10f64.powf(v[MASTER] / 20.0) as f32,
        }
    }
}

/// The reference gain for first-order shelves and peaks: the half-gain point for small
/// gains, 3 dB from the plateau for large ones.
fn reference_db(g_db: f64) -> f64 {
    if g_db > 6.0 {
        g_db - 3.0
    } else if g_db <= -6.0 {
        g_db + 3.0
    } else {
        g_db / 2.0
    }
}

/// A peaking band at `f` Hz, `g_db`, Q (centre over the bandwidth at the reference gain);
/// `None` when it is flat (under 1e-4 dB).
pub(crate) fn peak(rate: f64, f: f64, g_db: f64, q: f64) -> Option<Coefficients> {
    if g_db.abs() < 1e-4 {
        return None;
    }
    let g = 10f64.powf(g_db / 20.0);
    let gb = 10f64.powf(reference_db(g_db) / 20.0);
    // The ratios in f32, then double.
    let w = f64::from(f as f32 / rate as f32).clamp(1e-5, 0.99999);
    let delta = f64::from((f as f32 / q as f32) / rate as f32).clamp(1e-5, 0.49999);
    let beta = ((gb * gb - 1.0) / (g * g - gb * gb)).sqrt() * (PI * delta).tan();
    let n = 1.0 + beta;
    Some(Coefficients {
        b0: (1.0 + g * beta) / n,
        b1: -2.0 * (2.0 * PI * w).cos() / n,
        b2: (1.0 - g * beta) / n,
        a1: -2.0 * (2.0 * PI * w).cos() / n,
        a2: (1.0 - beta) / n,
    })
}

/// A low or high shelf at `f` Hz, first or second order; `None` when flat.
pub(crate) fn shelf(
    rate: f64,
    f: f64,
    g_db: f64,
    second: bool,
    high: bool,
) -> Option<Coefficients> {
    if g_db.abs() < 1e-4 {
        return None;
    }
    let g = 10f64.powf(g_db / 20.0);
    let ratio = f64::from(f as f32 / rate as f32);
    if !second {
        let gb = 10f64.powf(reference_db(g_db) / 20.0);
        let t = (PI * ratio.min(0.49999)).tan();
        let k = ((gb * gb - 1.0) / (g * g - gb * gb)).sqrt();
        let beta = if high { k / t } else { k * t };
        let n = 1.0 + beta;
        let sign = if high { 1.0 } else { -1.0 };
        return Some(Coefficients {
            b0: (1.0 + g * beta) / n,
            b1: sign * (1.0 - g * beta) / n,
            b2: 0.0,
            a1: sign * (1.0 - beta) / n,
            a2: 0.0,
        });
    }
    // Second order: Butterworth-damped poles of radius k, zeros of radius A, designed with the
    // shelf at a quarter of the rate and moved to `f` by an allpass substitution.
    let gb = if g_db.abs() >= 6.0 {
        if g > 1.0 {
            g / SQRT_2
        } else {
            g * SQRT_2
        }
    } else {
        g.sqrt()
    };
    let k = if (g * g - gb * gb).abs() <= 1.65436e-24 {
        1.0
    } else {
        ((gb * gb - 1.0) / (g * g - gb * gb)).powf(0.25)
    };
    let a = g.sqrt() * k;
    let poly = |r: f64| {
        (
            r * r + SQRT_2 * r + 1.0,
            -2.0 * (1.0 - r * r),
            r * r - SQRT_2 * r + 1.0,
        )
    };
    let (n0, n1, n2) = poly(a);
    let (d0, d1, d2) = poly(k);
    let alpha = if high {
        (PI * (0.25 - ratio)).tan()
    } else {
        (PI * (ratio - 0.25)).tan()
    };
    let warp = |p0: f64, p1: f64, p2: f64| {
        (
            p0 + alpha * p1 + alpha * alpha * p2,
            p1 * (1.0 + alpha * alpha) + 2.0 * alpha * (p0 + p2),
            alpha * alpha * p0 + alpha * p1 + p2,
        )
    };
    let (bb0, bb1, bb2) = warp(n0, n1, n2);
    let (aa0, aa1, aa2) = warp(d0, d1, d2);
    let mirror = if high { -1.0 } else { 1.0 };
    let inv = 1.0 / aa0;
    Some(Coefficients {
        b0: bb0 * inv,
        b1: mirror * bb1 * inv,
        b2: bb2 * inv,
        a1: mirror * aa1 * inv,
        a2: aa2 * inv,
    })
}

/// A Butterworth high- or low-pass of `order` (rounded, 1 to 8) at `f` Hz, prewarped there.
fn butterworth(rate: f64, f: f64, order: f64, high: bool) -> Cascade {
    let n = (order.round() as usize).clamp(1, 8);
    let wc = (PI * (f / rate).min(0.49)).tan();
    let mut sections = [Coefficients::IDENTITY; 4];
    let pairs = n / 2;
    for (i, slot) in sections.iter_mut().enumerate().take(pairs) {
        let zeta = 2.0 * (PI * (2 * i + 1) as f64 / (2 * n) as f64).sin();
        let (a0, a1, a2) = (
            1.0 + zeta * wc + wc * wc,
            2.0 * (wc * wc - 1.0),
            1.0 - zeta * wc + wc * wc,
        );
        *slot = if high {
            Coefficients::new(1.0, -2.0, 1.0, a0, a1, a2)
        } else {
            Coefficients::new(wc * wc, 2.0 * wc * wc, wc * wc, a0, a1, a2)
        };
    }
    let odd = n % 2 == 1;
    if odd {
        sections[pairs] = if high {
            Coefficients::new(1.0, -1.0, 0.0, 1.0 + wc, wc - 1.0, 0.0)
        } else {
            Coefficients::new(wc, wc, 0.0, 1.0 + wc, wc - 1.0, 0.0)
        };
    }
    Cascade {
        sections,
        count: pairs + usize::from(odd),
        odd,
    }
}

/// Direct form I memory for one channel: x1, x2, y1, y2.
type Memory = [f64; 4];

/// One section run in direct form I, double state: the second-order sum
/// `((b1·x1 + b2·x2) − a1·y1 − a2·y2) + b0·x`, the first-order `b0·x + (b1·x1 − a1·y1)`.
/// A section that has died away (its whole memory under [`super::TINY64`]) is zeroed: left
/// alone, its recursion would run on through f64 denormals and settle in a limit cycle there,
/// for ever after the input stops, as the Room Reverb's same sections did (dev plan 18.1).
/// The memory goes as a whole, never one value of a live recursion, which would kick it into a
/// limit cycle of its own just over the floor.
fn run(c: &Coefficients, first_order: bool, m: &mut Memory, buf: &mut [f32]) {
    for s in buf {
        let x = f64::from(*s);
        let y = if first_order {
            c.b0 * x + (c.b1 * m[0] - c.a1 * m[2])
        } else {
            ((c.b1 * m[0] + c.b2 * m[1]) - c.a1 * m[2] - c.a2 * m[3]) + c.b0 * x
        };
        *m = flush_all([x, m[0], y, m[2]]);
        *s = m[2] as f32;
    }
}

/// A shelf or band: its coefficients now, where they are ramping to, and its memory.
#[derive(Debug, Clone)]
struct Section {
    now: Coefficients,
    target: Coefficients,
    step: Coefficients,
    left: usize,
    /// Running (its target is not flat or off), or still ramping out of a change.
    on: bool,
    first_order: bool,
    memory: [Memory; CHANNELS],
}

impl Default for Section {
    fn default() -> Self {
        Self {
            now: Coefficients::IDENTITY,
            target: Coefficients::IDENTITY,
            step: Coefficients::IDENTITY,
            left: 0,
            on: false,
            first_order: false,
            memory: [[0.0; 4]; CHANNELS],
        }
    }
}

impl Section {
    /// Head for `target` (`None`: off) over `samples`. Turning on fades in from flat;
    /// turning off is at once; changing order starts afresh.
    fn retarget(&mut self, target: Option<Coefficients>, first_order: bool, samples: usize) {
        let Some(target) = target else {
            self.on = false;
            self.left = 0;
            self.now = Coefficients::IDENTITY;
            self.target = Coefficients::IDENTITY;
            return;
        };
        if first_order != self.first_order {
            self.first_order = first_order;
            self.memory = [[0.0; 4]; CHANNELS];
            self.now = Coefficients::IDENTITY;
        }
        if !self.on {
            self.on = true;
            self.now = Coefficients::IDENTITY;
        }
        self.target = target;
        if samples == 0 || self.now == target {
            self.now = target;
            self.left = 0;
            return;
        }
        let n = samples as f64;
        self.step = Coefficients {
            b0: (target.b0 - self.now.b0) / n,
            b1: (target.b1 - self.now.b1) / n,
            b2: (target.b2 - self.now.b2) / n,
            a1: (target.a1 - self.now.a1) / n,
            a2: (target.a2 - self.now.a2) / n,
        };
        self.left = samples;
    }

    fn snap(&mut self) {
        self.now = self.target;
        self.left = 0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        if !self.on {
            return;
        }
        // Steady: each channel in one run. Ramping: sample by sample, both channels together,
        // the coefficients stepping after each sample and landing on the target.
        if self.left == 0 {
            for (buf, m) in io.iter_mut().zip(self.memory.iter_mut()) {
                run(&self.now, self.first_order, m, &mut buf[from..to]);
            }
            return;
        }
        for k in from..to {
            for (buf, m) in io.iter_mut().zip(self.memory.iter_mut()) {
                run(
                    &self.now,
                    self.first_order,
                    m,
                    std::slice::from_mut(&mut buf[k]),
                );
            }
            if self.left > 0 {
                self.left -= 1;
                if self.left == 0 {
                    self.now = self.target;
                } else {
                    let (c, s) = (&mut self.now, &self.step);
                    c.b0 += s.b0;
                    c.b1 += s.b1;
                    c.b2 += s.b2;
                    c.a1 += s.a1;
                    c.a2 += s.a2;
                }
            }
        }
    }
}

/// A high- or low-pass cascade and its memory. A change of frequency or slope starts it
/// afresh (its state zeroed), with no ramp.
#[derive(Debug, Clone, Default)]
struct Pass {
    cascade: Option<Cascade>,
    memory: [[Memory; 4]; CHANNELS],
}

impl Pass {
    fn set(&mut self, cascade: Cascade) {
        if self.cascade != Some(cascade) {
            self.cascade = Some(cascade);
            self.memory = [[[0.0; 4]; 4]; CHANNELS];
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let Some(c) = &self.cascade else {
            return;
        };
        for (buf, memory) in io.iter_mut().zip(self.memory.iter_mut()) {
            for (k, (section, m)) in c.sections[..c.count].iter().zip(memory).enumerate() {
                let first = c.odd && k == c.count - 1;
                run(section, first, m, &mut buf[from..to]);
            }
        }
    }
}

struct ParametricEq {
    rate: f64,
    values: [f64; BANDS + BAND_PARAMS * BAND_COUNT],
    hp: Pass,
    lp: Pass,
    hp_on: bool,
    lp_on: bool,
    shelves: [Section; 2],
    bands: [Section; BAND_COUNT],
    master: f32,
    master_target: f32,
    master_step: f32,
    master_left: usize,
}

impl ParametricEq {
    fn new(rate: f64) -> Self {
        let mut values = [0.0; BANDS + BAND_PARAMS * BAND_COUNT];
        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        let mut eq = Self {
            rate,
            values,
            hp: Pass::default(),
            lp: Pass::default(),
            hp_on: false,
            lp_on: false,
            shelves: Default::default(),
            bands: Default::default(),
            master: 1.0,
            master_target: 1.0,
            master_step: 0.0,
            master_left: 0,
        };
        eq.redesign(None, 0);
        eq
    }

    /// Recompute the stages parameter `changed` touches (all of them for `None`), ramping
    /// shelves, bands and the master over `samples`.
    fn redesign(&mut self, changed: Option<usize>, samples: usize) {
        let d = Design::new(self.rate, &self.values);
        let all = changed.is_none();
        let i = changed.unwrap_or(0);
        let on = |v: &[f64], k: usize| v[k] >= 0.5;
        if all || (HP_ON..=HP_ORDER).contains(&i) {
            self.hp_on = on(&self.values, HP_ON);
            self.hp.set(butterworth(
                self.rate,
                self.values[HP_FREQ],
                self.values[HP_ORDER],
                true,
            ));
        }
        if all || (LP_ON..=LP_ORDER).contains(&i) {
            self.lp_on = on(&self.values, LP_ON);
            self.lp.set(butterworth(
                self.rate,
                self.values[LP_FREQ],
                self.values[LP_ORDER],
                false,
            ));
        }
        for (s, first) in [LOW_SHELF, HIGH_SHELF].into_iter().enumerate() {
            if all || (first..first + 4).contains(&i) {
                let first_order = !on(&self.values, first + SHELF_SECOND);
                self.shelves[s].retarget(d.shelves[s], first_order, samples);
            }
        }
        for b in 0..BAND_COUNT {
            let first = BANDS + b * BAND_PARAMS;
            if all || i == CONSTANT_Q || (first..first + BAND_PARAMS).contains(&i) {
                self.bands[b].retarget(d.bands[b], false, samples);
            }
        }
        if all || i == MASTER {
            self.master_target = d.master;
            if samples == 0 {
                self.master = d.master;
                self.master_left = 0;
            } else {
                self.master_step = (d.master - self.master) / samples as f32;
                self.master_left = samples;
            }
        }
    }
}

impl NativeEffect for ParametricEq {
    fn set(&mut self, index: usize, value: f64) {
        self.ramp_to(index, value, RAMP);
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        self.ramp_to(index, value, samples);
    }

    fn snap(&mut self) {
        self.redesign(None, 0);
        for s in self.shelves.iter_mut().chain(self.bands.iter_mut()) {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        if self.hp_on {
            self.hp.process(io, from, to);
        }
        if self.lp_on {
            self.lp.process(io, from, to);
        }
        for s in self.shelves.iter_mut().chain(self.bands.iter_mut()) {
            s.process(io, from, to);
        }
        for k in from..to {
            for buf in io.iter_mut().take(CHANNELS) {
                buf[k] *= self.master;
            }
            if self.master_left > 0 {
                self.master += self.master_step;
                self.master_left -= 1;
                if self.master_left == 0 {
                    self.master = self.master_target;
                }
            }
        }
    }
}

impl ParametricEq {
    fn ramp_to(&mut self, index: usize, value: f64, samples: usize) {
        if index >= self.values.len() || self.values[index] == value {
            return;
        }
        self.values[index] = value;
        self.redesign(Some(index), samples);
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;

    fn plain(pairs: &[(&str, f64)]) -> Vec<f64> {
        let mut v: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        for (name, value) in pairs {
            let i = PARAMS.iter().position(|p| p.name == *name).unwrap();
            v[i] = *value;
        }
        v
    }

    fn db(c: &Coefficients, hz: f64) -> f64 {
        20.0 * c.magnitude(hz, 48_000.0).log10()
    }

    #[test]
    fn a_peak_has_its_gain_and_its_width_at_the_reference_level() {
        // +12 dB at 1 kHz, Q 2: 500 Hz wide where the gain is 9 dB (12 − 3).
        let c = peak(48_000.0, 1000.0, 12.0, 2.0).unwrap();
        assert!((db(&c, 1000.0) - 12.0).abs() < 1e-6);
        assert!(db(&c, 0.0).abs() < 1e-9 && db(&c, 23_999.0).abs() < 1e-3);
        // The two 9 dB points, found by bisection either side of the centre.
        let edge = |mut lo: f64, mut hi: f64, rising: bool| {
            for _ in 0..60 {
                let mid = (lo + hi) / 2.0;
                if (db(&c, mid) < 9.0) == rising {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
            lo
        };
        let (below, above) = (edge(1.0, 1000.0, true), edge(1000.0, 20_000.0, false));
        assert!(
            (above - below - 500.0).abs() < 0.5,
            "{below:.1}..{above:.1}"
        );
        // A small gain is defined at half its level: +4 dB is 500 Hz wide at +2 dB.
        let c = peak(48_000.0, 1000.0, 4.0, 2.0).unwrap();
        assert!((db(&c, 1000.0) - 4.0).abs() < 1e-6);
        assert!(peak(48_000.0, 1000.0, 0.0, 2.0).is_none());
    }

    #[test]
    fn shelves_reach_their_gain_and_the_reference_at_their_frequency() {
        for second in [false, true] {
            let low = shelf(48_000.0, 1000.0, 12.0, second, false).unwrap();
            let high = shelf(48_000.0, 1000.0, -12.0, second, true).unwrap();
            assert!((db(&low, 1.0) - 12.0).abs() < 0.01, "{second}");
            assert!(db(&low, 20_000.0).abs() < 0.1, "{second}");
            assert!((db(&high, 23_000.0) + 12.0).abs() < 0.1, "{second}");
            assert!(db(&high, 1.0).abs() < 0.01, "{second}");
            // At 1 kHz: 9 dB (first order, 12 − 3) or 12 − 3.01 dB (second order, g/√2).
            let at = db(&low, 1000.0);
            assert!((at - 9.0).abs() < 0.02, "{second}: {at}");
        }
    }

    #[test]
    fn butterworth_is_3_db_down_at_its_cutoff_and_falls_6_db_per_order() {
        for n in 1..=8 {
            let lp = butterworth(48_000.0, 1000.0, f64::from(n), false);
            let hp = butterworth(48_000.0, 1000.0, f64::from(n), true);
            let total = |c: &Cascade, hz: f64| -> f64 { c.iter().map(|s| db(s, hz)).sum() };
            assert!((total(&lp, 1000.0) + 3.0103).abs() < 1e-3, "lp {n}");
            assert!((total(&hp, 1000.0) + 3.0103).abs() < 1e-3, "hp {n}");
            assert!(total(&lp, 10.0).abs() < 1e-3 && total(&hp, 20_000.0).abs() < 0.1);
            // Well past the cutoff, 6 dB per octave per order.
            let lp = butterworth(48_000.0, 100.0, f64::from(n), false);
            let slope = total(&lp, 800.0) - total(&lp, 1600.0);
            assert!((slope - 6.0 * f64::from(n)).abs() < 0.5, "lp {n}: {slope}");
        }
    }

    #[test]
    fn constant_width_holds_the_bandwidth_in_hz() {
        let q_mode = plain(&[("Band 3 Gain", 12.0), ("Band 3 Q", 4.0)]);
        let w_mode = plain(&[
            ("Band 3 Gain", 12.0),
            ("Constant Q", 0.0),
            ("Band 3 Width", 200.0),
        ]);
        // Band 3 at 800 Hz: Q 4 is 200 Hz wide, so both are the same filter there.
        let a = response(48_000.0, &q_mode, 900.0);
        let b = response(48_000.0, &w_mode, 900.0);
        assert!((a - b).abs() < 1e-9, "{a} vs {b}");
    }

    #[test]
    fn a_render_matches_the_curve_and_is_the_same_at_any_block_size() {
        let settings: BTreeMap<String, f64> = [
            ("Band 2 Gain".to_string(), -9.0),
            ("Low Shelf Gain".to_string(), 6.0),
            ("Low Shelf Second Order".to_string(), 1.0),
            ("High-Pass Enable".to_string(), 1.0),
            ("High-Pass Frequency".to_string(), 60.0),
            ("Master Gain".to_string(), -2.0),
        ]
        .into();
        let n = 48_000;
        let tone = |hz: f64| -> Vec<f32> {
            (0..n)
                .map(|i| (0.25 * (2.0 * PI * hz * i as f64 / 48_000.0).sin()) as f32)
                .collect()
        };
        let a = render(
            &KIND,
            48_000.0,
            &settings,
            &[tone(200.0), tone(3000.0)],
            512,
        )
        .unwrap();
        let b = render(&KIND, 48_000.0, &settings, &[tone(200.0), tone(3000.0)], 77).unwrap();
        assert_eq!(a, b);
        let mut v = plain(&[]);
        for (k, value) in &settings {
            v[PARAMS.iter().position(|p| p.name == k).unwrap()] = *value;
        }
        let rms = |x: &[f32]| {
            (x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
        };
        for (c, hz) in [(0, 200.0), (1, 3000.0)] {
            let measured = 20.0 * (rms(&a[c][24_000..]) / (0.25 / SQRT_2)).log10();
            let curve = response(48_000.0, &v, hz);
            assert!(
                (measured - curve).abs() < 0.01,
                "{hz} Hz: {measured:.3} vs {curve:.3}"
            );
        }
    }

    #[test]
    fn a_ringing_band_ends_at_zero_not_in_the_denormals() {
        // A narrow boost and steep passes ring on after the input stops; their direct form I
        // memory reaches an exact zero instead of running on (or sticking) in f64's denormals,
        // and the output never holds a denormal.
        let mut eq = ParametricEq::new(48_000.0);
        for (name, value) in [
            ("Band 1 Gain", 18.0),
            ("Band 1 Q", 40.0),
            ("High-Pass Enable", 1.0),
            ("High-Pass Frequency", 30.0),
            ("High-Pass Slope", 8.0),
            ("Low-Pass Enable", 1.0),
            ("Low-Pass Frequency", 6000.0),
        ] {
            let i = PARAMS.iter().position(|p| p.name == name).unwrap();
            eq.set(i, value);
        }
        eq.snap();
        let mut io = vec![vec![0.0f32; 4800]; CHANNELS];
        io[0][0] = 1.0;
        io[1][0] = -1.0;
        eq.process(&mut io, 0, 4800);
        let mut silence = vec![vec![0.0f32; 48_000]; CHANNELS];
        let mut denormal = false;
        for _ in 0..30 {
            silence.iter_mut().for_each(|c| c.fill(0.0));
            eq.process(&mut silence, 0, 48_000);
            denormal |= silence.iter().flatten().any(|s| s.is_subnormal());
        }
        assert!(!denormal);
        let memories = eq
            .shelves
            .iter()
            .chain(eq.bands.iter())
            .flat_map(|s| s.memory.iter())
            .chain(eq.hp.memory.iter().flatten())
            .chain(eq.lp.memory.iter().flatten());
        for m in memories {
            assert_eq!(*m, [0.0; 4]);
        }
    }
}
