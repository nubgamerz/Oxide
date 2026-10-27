//! Room Reverb (studied in the research notes on `studio-reverb`, kept outside the
//! repository): a Freeverb-style network per channel, written from the study's description.
//!
//! - **Input:** each channel's audio through a 2nd-order Butterworth low-pass at High Frequency
//!   Cut, then a high-pass at Low Frequency Cut (prewarped, −3 dB at the cutoff, run in direct
//!   form I with double state and f32 in and out, as the Parametric EQ's), then ×0.015.
//! - **8 combs in parallel**, each a delay line with a one-pole low-pass in its feedback
//!   (weight d1 = 0.4·Damping/100 on its previous state) and feedback g from Decay and Room
//!   Size: g = 0.28 + 0.7·log10((Decay·(2 − Room/100) − 100)/200)/log10(50), at most 0.98
//!   ([`feedback`]). Their base times (50 to 72.4465 ms) are divided by 3 − Room/50.
//! - **4 "allpasses" in series** on the combs' sum, each `y = d − g·u`, `buf ← g·d + u` with
//!   g = 0.3 + 0.4·Early Reflections/100: the reference's recurrence exactly, which is not a true
//!   allpass (it colours the sound and changes the level with Early Reflections). Base times
//!   24.7108, 19.5998, 15.1553 and 10 ms divided by 3 − Early Reflections Delay/50.
//! - Odd channels (the right) have every delay 0.53 ms longer. Lengths in samples are
//!   `int(((t/s + spread)·fs)/1000 + 0.5)`, every operand f32, in that grouping ([`delay_length`];
//!   the study saw f32 but not the grouping: the impulse reference case settles a one-sample
//!   flip).
//! - **Output:** each channel's wet w = D·(S + a1 + a2 + a3)·0.25 + (1 − D)·a4 (D = Diffusion/200,
//!   so 0 % is the fully diffused a4 alone, as the reference's code does); y_L = wet1·w_L + wet2·w_R +
//!   dry·x_L and the mirror, with dry = Dry/100, wet = Wet/20, wet1 = wet·(0.5 + Width/200),
//!   wet2 = wet·(0.5 − Width/200). The dry path is the unfiltered input. All reverb state and
//!   arithmetic f32, as the reference's.
//!
//! Oxide's graph is always stereo (a mono clip plays on both channels), so only the
//! reference's stereo matrix exists here; its one-channel path (wet1 alone) is not reproduced (TD127).
//!
//! **Changes:** Room Size or Early Reflections Delay re-initialises the network as the
//! reference reallocates it: every line empty, both filters redesigned with their state zeroed, from the
//! next sample processed (the tail is cut, as in the reference; the lines are cleared within the
//! capacity reserved for the largest settings at [`MAX_RATE`], never reallocated). Those two are
//! not automatable for that reason. A cut's change redesigns its own filter (its state zeroed);
//! anything else just sets the gains. No smoothing anywhere. A transport start
//! re-initialises everything, as the reference's `Resume`, so playback and export start alike.
//! Latency 0.
//!
//! **Denormals:** the reference has no guard. Ours zeroes, below 1e-25 (about −500 dB), each comb's
//! feedback low-pass state, each allpass's stored value and each input filter's output state:
//! without it a dying tail runs through f32 denormals for seconds (measured: up to 2.8 times
//! the cost for the 13 s after an impulse at the defaults) and the double filters settle into a
//! denormal limit cycle that costs twice the reverb forever after the input stops. With it the
//! cost is flat (TD127). Nothing above that level changes.

use super::biquad::{butterworth2, Coefficients};
use super::{flush, Kind, NativeEffect, ParamSpec, TINY};
use crate::graph::CHANNELS;

// Parameter indices: ids 0 to 10 (the reference's 11 and 12, "Is Processing" and the channel
// count, are the host's state, not settings: the transport start and the graph's two channels).
pub const LOW_CUT: usize = 0;
pub const HIGH_CUT: usize = 1;
pub const ROOM: usize = 2;
pub const WIDTH: usize = 3;
pub const DIFFUSION: usize = 4;
pub const DAMPING: usize = 5;
pub const DECAY: usize = 6;
pub const EARLY: usize = 7;
pub const EARLY_DELAY: usize = 8;
pub const DRY: usize = 9;
pub const WET: usize = 10;
pub(crate) const COUNT: usize = 11;

/// The combs' base times (ms), in the reference's order.
pub const COMB_MS: [f32; 8] = [
    50.0, 53.226, 57.2135, 60.753, 63.71, 66.801, 69.758, 72.4465,
];
/// The allpasses' base times (ms), in processing order.
pub const ALLPASS_MS: [f32; 4] = [24.7108, 19.5998, 15.1553, 10.0];
/// Odd channels' extra delay (ms): Freeverb's stereo spread, as a time.
pub const SPREAD_MS: f32 = 0.53;
/// The fixed gain into the combs.
const INPUT_GAIN: f32 = 0.015;
/// The feedback's cap.
const MAX_FEEDBACK: f64 = 0.98;
/// The highest rate the lines are sized for up front (the device's top rate); an instance made
/// at a higher one sizes for its own.
pub const MAX_RATE: f64 = 192_000.0;

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} kHz", v / 1000.0)
    } else {
        format!("{v:.0} Hz")
    }
}

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0,
    max: 100.0,
    default: 0.0,
    steps: 0,
    decimals: 0,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Low Frequency Cut",
        unit: "Hz",
        min: 20.0,
        max: 4000.0,
        default: 880.0,
        log_slider: true,
        format: Some(hz),
        ..PARAM
    },
    ParamSpec {
        name: "High Frequency Cut",
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        default: 13_800.0,
        log_slider: true,
        format: Some(hz),
        ..PARAM
    },
    // Each change empties the network (it is reallocated): no lane.
    ParamSpec {
        name: "Room Size",
        min: 1.0,
        default: 70.0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Width",
        default: 25.0,
        ..PARAM
    },
    ParamSpec {
        name: "Diffusion",
        unit: "%",
        default: 50.0,
        ..PARAM
    },
    ParamSpec {
        name: "Damping",
        unit: "%",
        default: 50.0,
        ..PARAM
    },
    ParamSpec {
        name: "Decay",
        unit: "ms",
        min: 200.0,
        max: 10_000.0,
        default: 2500.0,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Early Reflections",
        unit: "%",
        default: 52.0,
        ..PARAM
    },
    // Not in the reference's dialog (it holds 50); a change empties the network.
    ParamSpec {
        name: "Early Reflections Delay",
        default: 50.0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Dry",
        unit: "%",
        default: 75.0,
        ..PARAM
    },
    ParamSpec {
        name: "Wet",
        unit: "%",
        default: 25.0,
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "room-reverb",
    name: "Room Reverb",
    category: "Reverb",
    params: &PARAMS,
    make: |rate| Box::new(StudioReverb::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The combs' feedback for Decay (ms) and Room Size: computed in double, kept in f32, as
/// the reference's. Not an RT60 ([`rt60`] is).
pub fn feedback(decay_ms: f64, room: f64) -> f32 {
    let arg = ((decay_ms * (2.0 - room / 100.0) - 100.0) / 200.0).max(1e-9);
    let g = 0.28 + 0.7 * arg.log10() / 50f64.log10();
    g.min(MAX_FEEDBACK) as f32
}

/// A delay's length in samples: base time `ms` divided by the size factor `size`, plus the odd
/// channels' spread, at `rate`, rounded half up; every operand f32.
pub fn delay_length(rate: f64, ms: f32, size: f32, odd: bool) -> usize {
    let spread = if odd { SPREAD_MS } else { 0.0 };
    ((ms / size + spread) * rate as f32 / 1000.0 + 0.5) as usize
}

/// The size factor 3 − v/50 of Room Size (combs) or Early Reflections Delay (allpasses).
fn size_factor(v: f64) -> f32 {
    3.0 - v as f32 / 50.0
}

/// The combs' lengths (samples) at `rate` for Room Size `room`, on an even or odd channel.
pub fn comb_lengths(rate: f64, room: f64, odd: bool) -> [usize; 8] {
    COMB_MS.map(|ms| delay_length(rate, ms, size_factor(room), odd))
}

/// The allpasses' lengths (samples) at `rate` for Early Reflections Delay `er_delay`.
pub fn allpass_lengths(rate: f64, er_delay: f64, odd: bool) -> [usize; 4] {
    ALLPASS_MS.map(|ms| delay_length(rate, ms, size_factor(er_delay), odd))
}

/// The comb low-pass's gain (linear) at `hz` for damping weight `d1`: d2/|1 − d1·e^(−jω)|.
fn damping_gain(rate: f64, d1: f64, hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / rate;
    (1.0 - d1) / (1.0 - 2.0 * d1 * w.cos() + d1 * d1).sqrt()
}

/// The time (s) each comb takes to fall 60 dB at `hz` (its loop gain there: the feedback times
/// the damping's low-pass), for plain `values` at `rate`, on the left channel: the shortest and
/// the longest comb's. The reverb's decay lies between them (the allpasses only smear it).
pub fn rt60(rate: f64, values: &[f64], hz: f64) -> (f64, f64) {
    let g = f64::from(feedback(values[DECAY], values[ROOM]));
    let d1 = 0.4 * values[DAMPING] / 100.0;
    let loop_gain = g * damping_gain(rate, d1, hz);
    let lengths = comb_lengths(rate, values[ROOM], false);
    let t = |l: usize| -3.0 * l as f64 / (rate * loop_gain.log10());
    (t(lengths[0]), t(lengths[7]))
}

/// The combs' energy envelope (dB, 0 at the start) `seconds` after an impulse at `hz`, the
/// eight combs' exponential decays added: the editor's decay sketch.
pub fn decay_db(rate: f64, values: &[f64], hz: f64, seconds: f64) -> f64 {
    let g = f64::from(feedback(values[DECAY], values[ROOM]));
    let d1 = 0.4 * values[DAMPING] / 100.0;
    let loop_gain = g * damping_gain(rate, d1, hz);
    let energy: f64 = comb_lengths(rate, values[ROOM], false)
        .iter()
        .map(|&l| loop_gain.powf(2.0 * seconds * rate / l as f64))
        .sum::<f64>()
        / 8.0;
    10.0 * energy.max(1e-30).log10()
}

/// A delay line with room for the largest settings; `len` of it in use.
struct Line {
    buf: Vec<f32>,
    len: usize,
    pos: usize,
}

impl Line {
    fn new(capacity: usize) -> Self {
        Self {
            buf: vec![0.0; capacity.max(1)],
            len: 1,
            pos: 0,
        }
    }

    /// Use `len` samples of it, empty. No allocation.
    fn reset(&mut self, len: usize) {
        self.len = len.clamp(1, self.buf.len());
        self.buf[..self.len].fill(0.0);
        self.pos = 0;
    }

    fn advance(&mut self) {
        self.pos += 1;
        if self.pos == self.len {
            self.pos = 0;
        }
    }
}

/// A lowpass-feedback comb: Freeverb's, its output the line's oldest sample.
struct Comb {
    line: Line,
    /// The feedback low-pass's state.
    z: f32,
}

impl Comb {
    #[inline]
    fn tick(&mut self, v: f32, g: &Gains) -> f32 {
        let line = &mut self.line;
        let o = line.buf[line.pos];
        self.z = flush(o * g.d2 + self.z * g.d1);
        line.buf[line.pos] = v + self.z * g.feedback;
        line.advance();
        o
    }
}

/// One of the series "allpasses", exactly as the reference's recurrence (not a true allpass).
#[inline]
fn allpass(line: &mut Line, u: f32, g: f32) -> f32 {
    let d = line.buf[line.pos];
    let y = d - g * u;
    line.buf[line.pos] = flush(g * d + u);
    line.advance();
    y
}

/// One second-order section in direct form I, double state, f32 in and out (the Parametric
/// EQ's realisation): `((b1·x1 + b2·x2) − a1·y1 − a2·y2) + b0·x`.
#[derive(Clone, Copy)]
struct Df1 {
    c: Coefficients,
    /// x1, x2, y1, y2.
    m: [f64; 4],
}

impl Df1 {
    fn new(c: Coefficients) -> Self {
        Self { c, m: [0.0; 4] }
    }

    #[inline]
    fn tick(&mut self, x: f32) -> f32 {
        let (c, m) = (&self.c, &mut self.m);
        let x = f64::from(x);
        let y = ((c.b1 * m[0] + c.b2 * m[1]) - c.a1 * m[2] - c.a2 * m[3]) + c.b0 * x;
        let y = if y.abs() < f64::from(TINY) { 0.0 } else { y };
        m[1] = m[0];
        m[0] = x;
        m[3] = m[2];
        m[2] = y;
        y as f32
    }
}

/// The gains the settings give, all f32 as the reference keeps them.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Gains {
    feedback: f32,
    d1: f32,
    d2: f32,
    allpass: f32,
    /// D = Diffusion/200, and 1 − D.
    diffuse: f32,
    direct: f32,
    dry: f32,
    wet1: f32,
    wet2: f32,
}

impl Gains {
    fn new(v: &[f64; COUNT]) -> Self {
        let d1 = 0.4f32 * v[DAMPING] as f32 / 100.0;
        let diffuse = v[DIFFUSION] as f32 / 200.0;
        let wet = v[WET] as f32 / 20.0;
        let width = v[WIDTH] as f32 / 200.0;
        Self {
            feedback: feedback(v[DECAY], v[ROOM]),
            d1,
            d2: 1.0 - d1,
            allpass: 0.3f32 + 0.4f32 * v[EARLY] as f32 / 100.0,
            diffuse,
            direct: 1.0 - diffuse,
            dry: v[DRY] as f32 / 100.0,
            wet1: wet * (0.5 + width),
            wet2: wet * (0.5 - width),
        }
    }
}

/// One channel's network: its filters, combs and allpasses.
struct Channel {
    low_pass: Df1,
    high_pass: Df1,
    combs: [Comb; 8],
    allpasses: [Line; 4],
}

impl Channel {
    /// The channel's wet signal for input `x`.
    #[inline]
    fn tick(&mut self, x: f32, g: &Gains) -> f32 {
        let v = self.high_pass.tick(self.low_pass.tick(x)) * INPUT_GAIN;
        let mut s = 0.0f32;
        for comb in &mut self.combs {
            s += comb.tick(v, g);
        }
        let [p1, p2, p3, p4] = &mut self.allpasses;
        let a1 = allpass(p1, s, g.allpass);
        let a2 = allpass(p2, a1, g.allpass);
        let a3 = allpass(p3, a2, g.allpass);
        let a4 = allpass(p4, a3, g.allpass);
        g.diffuse * (s + a1 + a2 + a3) * 0.25 + g.direct * a4
    }
}

struct StudioReverb {
    rate: f64,
    values: [f64; COUNT],
    gains: Gains,
    channels: [Channel; CHANNELS],
    /// Room Size or Early Reflections Delay changed: re-initialise before the next sample.
    reinit: bool,
}

impl StudioReverb {
    fn new(rate: f64) -> Self {
        // Every line sized for its longest setting (Room Size and Early Reflections Delay at
        // 100, the odd channel's spread) at the highest rate, plus one.
        let top = rate.max(MAX_RATE);
        let combs = comb_lengths(top, 100.0, true);
        let allpasses = allpass_lengths(top, 100.0, true);
        let values = PARAMS.map(|p| p.default);
        let channel = || Channel {
            low_pass: Df1::new(Coefficients::IDENTITY),
            high_pass: Df1::new(Coefficients::IDENTITY),
            combs: combs.map(|n| Comb {
                line: Line::new(n + 1),
                z: 0.0,
            }),
            allpasses: allpasses.map(|n| Line::new(n + 1)),
        };
        let mut reverb = Self {
            rate,
            gains: Gains::new(&values),
            values,
            channels: std::array::from_fn(|_| channel()),
            reinit: true,
        };
        reverb.initialise();
        reverb
    }

    /// The (re-)initialisation: the lines at their lengths and empty, both filters
    /// redesigned with no state, the gains recomputed. No allocation.
    fn initialise(&mut self) {
        let v = self.values;
        for (c, channel) in self.channels.iter_mut().enumerate() {
            let odd = c % 2 == 1;
            for (comb, len) in channel
                .combs
                .iter_mut()
                .zip(comb_lengths(self.rate, v[ROOM], odd))
            {
                comb.line.reset(len);
                comb.z = 0.0;
            }
            for (line, len) in
                channel
                    .allpasses
                    .iter_mut()
                    .zip(allpass_lengths(self.rate, v[EARLY_DELAY], odd))
            {
                line.reset(len);
            }
        }
        self.design(true, true);
        self.gains = Gains::new(&v);
        self.reinit = false;
    }

    /// Redesign the low-pass (High Frequency Cut) and/or the high-pass (Low Frequency Cut),
    /// zeroing their state, as the reference's redesign does.
    fn design(&mut self, low_pass: bool, high_pass: bool) {
        let lp = butterworth2(self.rate, self.values[HIGH_CUT], false);
        let hp = butterworth2(self.rate, self.values[LOW_CUT], true);
        for channel in &mut self.channels {
            if low_pass {
                channel.low_pass = Df1::new(lp);
            }
            if high_pass {
                channel.high_pass = Df1::new(hp);
            }
        }
    }
}

impl NativeEffect for StudioReverb {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        match index {
            ROOM | EARLY_DELAY => self.reinit = true,
            LOW_CUT => self.design(false, true),
            HIGH_CUT => self.design(true, false),
            _ => {}
        }
        self.gains = Gains::new(&self.values);
    }

    fn start(&mut self) {
        self.initialise();
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        if self.reinit {
            self.initialise();
        }
        let g = self.gains;
        let [left, right] = &mut self.channels;
        let [l, r, ..] = io else {
            return;
        };
        for (xl, xr) in l[from..to].iter_mut().zip(&mut r[from..to]) {
            let wl = left.tick(*xl, &g);
            let wr = right.tick(*xr, &g);
            *xl = g.wet1 * wl + g.wet2 * wr + g.dry * *xl;
            *xr = g.wet1 * wr + g.wet2 * wl + g.dry * *xr;
        }
    }
}

#[cfg(test)]
#[path = "studio_reverb_tests.rs"]
mod tests;
