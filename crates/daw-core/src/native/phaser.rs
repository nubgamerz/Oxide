//! Phaser: on the parameters of a reference phaser (studied in the research notes on `phaser`,
//! kept outside the repository). The study decoded its parameters, ranges and defaults but not
//! its per-sample kernel, so **the DSP is our own design** on what they mean
//! (every guess in TD137, with the study's renders that settle each):
//!
//! - **Stages** (1 to 6) are second-order allpass sections in series, each a conjugate pair of
//!   first-order allpass poles at radius R and the sweep's angle, so each stage makes **one
//!   notch** where the chain's phase passes an odd multiple of π: Mix 50 % nulls there
//!   exactly. (A single real first-order allpass reaches -π only at Nyquist, so one stage
//!   would make no notch.)
//! - **Intensity sets the pole radius**, the engine's "R" (0.001 to 0.999): R = 0.001 +
//!   0.998·Intensity at 44.1 kHz, the same notch width in Hz at other rates (R^(44100/fs)).
//!   High R: narrow notches gathered round the sweep's frequency; low: wide ones spread over the
//!   spectrum, hardly moving with the sweep.
//! - **The sweep:** the LFO (Mod Rate, from its top at a start; Shape triangle or sine) moves
//!   the sections' frequency between Upper Freq·(1 − Depth) (at least 20 Hz) and Upper Freq,
//!   on a logarithmic or linear scale (Step). The right channel's LFO leads by Phase Diff
//!   unless Sync Channels puts both on the left's.
//! - **Feedback** (-90 to 90 %) sends the chain's last output back to its input a sample
//!   later; **Mix** blends the dry input with the wet, the wet being the chain's output plus
//!   what was fed back, so at 50 % the notches stay exact at any feedback and feedback raises
//!   resonant peaks between them (they look deeper); at 0 % the input, untouched. Then Output
//!   Gain.
//!
//! Every sample: the LFO, the frequency and the coefficients, so it is the same at any block
//! size. Hand changes glide over 10 ms; lanes draw lines; Stages, Shape, Step and Sync Channels
//! apply at once (a stage switched in starts empty). A transport start empties the chain and
//! starts the LFO at its top. Latency 0. Host Tempo Sync (the element has it only when built
//! with its flag; the study couldn't tell whether the reference's is) isn't offered.

use super::delay_line::Ramp;
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;
use std::f64::consts::PI;

pub const STAGES: usize = 0;
pub const INTENSITY: usize = 1;
pub const DEPTH: usize = 2;
pub const RATE: usize = 3;
pub const PHASE: usize = 4;
pub const UPPER: usize = 5;
pub const FEEDBACK: usize = 6;
pub const MIX: usize = 7;
pub const GAIN: usize = 8;
pub const SHAPE: usize = 9;
pub const STEP: usize = 10;
pub const SYNC: usize = 11;
pub const COUNT: usize = 12;

pub const MAX_STAGES: usize = 6;
/// The rate the Intensity → R law is written for: R keeps the notches' width in Hz at others.
pub const R_RATE: f64 = 44_100.0;
/// The sweep's lowest frequency (Hz).
pub const LOWEST_HZ: f64 = 20.0;
/// A change by hand glides over this long (ms).
const GLIDE_MS: f64 = 10.0;

pub const SHAPES: [&str; 2] = ["Triangle", "Sine"];
pub const STEPS: [&str; 2] = ["Logarithmic", "Linear"];

fn shape_name(v: f64) -> String {
    SHAPES[usize::from(v >= 0.5)].into()
}

fn step_name(v: f64) -> String {
    STEPS[usize::from(v >= 0.5)].into()
}

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

const SWITCH: ParamSpec = ParamSpec {
    unit: "",
    max: 1.0,
    steps: 1,
    decimals: 0,
    ..PARAM
};

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Stages",
        unit: "",
        min: 1.0,
        max: MAX_STAGES as f64,
        default: 3.0,
        steps: (MAX_STAGES - 1) as u32,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Intensity",
        default: 95.0,
        ..PARAM
    },
    ParamSpec {
        name: "Depth",
        default: 70.0,
        ..PARAM
    },
    ParamSpec {
        name: "Mod Rate",
        unit: "Hz",
        max: 5.0,
        default: 0.25,
        decimals: 2,
        ..PARAM
    },
    ParamSpec {
        name: "Phase Diff",
        unit: "°",
        min: -180.0,
        max: 180.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Upper Freq",
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        default: 14_000.0,
        decimals: 0,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Feedback",
        min: -90.0,
        max: 90.0,
        ..PARAM
    },
    ParamSpec {
        name: "Mix",
        default: 50.0,
        ..PARAM
    },
    ParamSpec {
        name: "Output Gain",
        unit: "dB",
        min: -18.0,
        max: 6.0,
        ..PARAM
    },
    ParamSpec {
        name: "Shape",
        format: Some(shape_name),
        ..SWITCH
    },
    ParamSpec {
        name: "Step",
        default: 1.0,
        format: Some(step_name),
        ..SWITCH
    },
    ParamSpec {
        name: "Sync Channels",
        ..SWITCH
    },
];

pub static KIND: Kind = Kind {
    id: "phaser",
    name: "Phaser",
    category: "Modulation",
    params: &PARAMS,
    make: |rate| Box::new(Phaser::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The stage count for plain `values`.
pub fn stages(values: &[f64]) -> usize {
    (values[STAGES] + 0.5).clamp(1.0, MAX_STAGES as f64) as usize
}

/// The pole radius for an Intensity of `percent` at `rate`.
pub fn radius(rate: f64, percent: f64) -> f64 {
    let r = 0.001 + 0.998 * (percent / 100.0).clamp(0.0, 1.0);
    if rate == R_RATE {
        r
    } else {
        r.powf(R_RATE / rate)
    }
}

/// The LFO's value (0: the bottom of the sweep, 1: its top) at `phase` (cycles, any): a
/// triangle or a raised cosine, at the top at whole cycles.
pub fn lfo(phase: f64, sine: bool) -> f64 {
    let p = phase.rem_euclid(1.0);
    if sine {
        0.5 + 0.5 * (2.0 * PI * p).cos()
    } else {
        (1.0 - 2.0 * p).abs()
    }
}

/// The sweep's frequency (Hz) for the LFO at `l` (0..1), Upper Freq `upper`, Depth `depth`
/// (%) at `rate`: from `max(upper·(1 − depth), 20)` up to `upper` (both under 0.49·fs), on a
/// logarithmic or linear scale.
pub fn sweep_hz(rate: f64, upper: f64, depth: f64, linear: bool, l: f64) -> f64 {
    let top = upper.clamp(LOWEST_HZ, 0.49 * rate);
    let bottom = (top * (1.0 - depth / 100.0)).max(LOWEST_HZ).min(top);
    if linear {
        bottom + (top - bottom) * l
    } else {
        bottom * (top / bottom).powf(l)
    }
}

/// The sweep's range (Hz) for plain `values` at `rate`: its bottom and top.
pub fn sweep_range(rate: f64, values: &[f64]) -> (f64, f64) {
    let linear = values[STEP] >= 0.5;
    (
        sweep_hz(rate, values[UPPER], values[DEPTH], linear, 0.0),
        sweep_hz(rate, values[UPPER], values[DEPTH], linear, 1.0),
    )
}

/// The gain (dB) at `hz` with the sweep held where the LFO is at `l` (0..1), for plain
/// `values` at `rate`: `(1 − 2m) + m·(1 + A)/(1 − fb·z⁻¹·A)` times Output Gain, A the chain.
/// For the editor's curves and the tests.
pub fn response_db(rate: f64, values: &[f64], l: f64, hz: f64) -> f64 {
    let f = sweep_hz(rate, values[UPPER], values[DEPTH], values[STEP] >= 0.5, l);
    let r = radius(rate, values[INTENSITY]);
    let (a1, a2) = (-2.0 * r * (2.0 * PI * f / rate).cos(), r * r);
    let w = 2.0 * PI * hz / rate;
    // z⁻¹ and z⁻² as complex numbers.
    let (c1, s1, c2, s2) = (w.cos(), -w.sin(), (2.0 * w).cos(), -(2.0 * w).sin());
    let mul = |a: (f64, f64), b: (f64, f64)| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    let div = |a: (f64, f64), b: (f64, f64)| {
        let d = b.0 * b.0 + b.1 * b.1;
        ((a.0 * b.0 + a.1 * b.1) / d, (a.1 * b.0 - a.0 * b.1) / d)
    };
    let num = (a2 + a1 * c1 + c2, a1 * s1 + s2);
    let den = (1.0 + a1 * c1 + a2 * c2, a1 * s1 + a2 * s2);
    let section = div(num, den);
    let mut chain = (1.0, 0.0);
    for _ in 0..stages(values) {
        chain = mul(chain, section);
    }
    let m = values[MIX] / 100.0;
    let fb = values[FEEDBACK] / 100.0;
    let loop_gain = mul((fb * c1, fb * s1), chain);
    let wet = div((1.0 + chain.0, chain.1), (1.0 - loop_gain.0, -loop_gain.1));
    let h = (1.0 - 2.0 * m + m * wet.0, m * wet.1);
    let gain = 10f64.powf(values[GAIN] / 20.0);
    20.0 * ((h.0 * h.0 + h.1 * h.1).sqrt() * gain).max(1e-12).log10()
}

/// One second-order allpass section's state on one channel (direct form I).
#[derive(Debug, Clone, Copy, Default)]
struct Section {
    x1: f64,
    x2: f64,
    y1: f64,
    y2: f64,
}

impl Section {
    #[inline]
    fn tick(&mut self, x: f64, a1: f64, a2: f64) -> f64 {
        let y = a2 * (x - self.y2) + a1 * (self.x1 - self.y1) + self.x2;
        // Below about -500 dB a dying chain would run on through denormals (TD128).
        let y = if y.abs() < 1e-25 { 0.0 } else { y };
        *self = Self {
            x1: x,
            x2: self.x1,
            y1: y,
            y2: self.y1,
        };
        y
    }
}

struct Phaser {
    rate: f64,
    values: [f64; COUNT],
    sections: [[Section; MAX_STAGES]; CHANNELS],
    /// Each channel's last chain output, for the feedback.
    last: [f64; CHANNELS],
    /// The LFO's phase (cycles, in [0, 1)).
    phase: f64,
    intensity: Ramp,
    depth: Ramp,
    hz: Ramp,
    lead: Ramp,
    upper: Ramp,
    feedback: Ramp,
    mix: Ramp,
    gain_db: Ramp,
    /// R and the output gain for the values their ramps last stood at.
    r: f64,
    r_at: f32,
    gain: f64,
    gain_at: f32,
    glide: usize,
}

impl Phaser {
    fn new(rate: f64) -> Self {
        let values = PARAMS.map(|p| p.default);
        let plain = |i: usize| Ramp::new(values[i] as f32);
        Self {
            rate,
            sections: [[Section::default(); MAX_STAGES]; CHANNELS],
            last: [0.0; CHANNELS],
            phase: 0.0,
            intensity: plain(INTENSITY),
            depth: plain(DEPTH),
            hz: plain(RATE),
            lead: plain(PHASE),
            upper: plain(UPPER),
            feedback: plain(FEEDBACK),
            mix: plain(MIX),
            gain_db: plain(GAIN),
            r: radius(rate, values[INTENSITY]),
            r_at: values[INTENSITY] as f32,
            gain: 1.0,
            gain_at: 0.0,
            glide: (rate * GLIDE_MS / 1000.0) as usize,
            values,
        }
    }

    fn ramp_of(&mut self, index: usize) -> Option<&mut Ramp> {
        match index {
            INTENSITY => Some(&mut self.intensity),
            DEPTH => Some(&mut self.depth),
            RATE => Some(&mut self.hz),
            PHASE => Some(&mut self.lead),
            UPPER => Some(&mut self.upper),
            FEEDBACK => Some(&mut self.feedback),
            MIX => Some(&mut self.mix),
            GAIN => Some(&mut self.gain_db),
            _ => None,
        }
    }

    /// A stage count change: stages switched in start empty.
    fn stages_changed(&mut self, old: usize) {
        let new = stages(&self.values);
        if new > old {
            for channel in &mut self.sections {
                channel[old..new].fill(Section::default());
            }
        }
    }
}

impl NativeEffect for Phaser {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        let old = stages(&self.values);
        self.values[index] = value;
        let glide = self.glide;
        match self.ramp_of(index) {
            Some(r) => r.to(value as f32, glide),
            None => {
                if index == STAGES {
                    self.stages_changed(old);
                }
            }
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match self.ramp_of(index) {
            Some(r) => {
                r.to(value as f32, samples);
                self.values[index] = value;
            }
            None => self.set(index, value),
        }
    }

    fn snap(&mut self) {
        for i in [INTENSITY, DEPTH, RATE, PHASE, UPPER, FEEDBACK, MIX, GAIN] {
            if let Some(r) = self.ramp_of(i) {
                r.snap();
            }
        }
    }

    /// The chain empty and the LFO at its top, the settings in use at once.
    fn start(&mut self) {
        self.sections = [[Section::default(); MAX_STAGES]; CHANNELS];
        self.last = [0.0; CHANNELS];
        self.phase = 0.0;
        self.snap();
    }

    // A sample at a time across both channels: they share the LFO and the glides.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let n = stages(&self.values);
        let sine = self.values[SHAPE] >= 0.5;
        let linear = self.values[STEP] >= 0.5;
        let sync = self.values[SYNC] >= 0.5;
        for k in from..to {
            let intensity = self.intensity.tick();
            if intensity != self.r_at {
                self.r_at = intensity;
                self.r = radius(self.rate, f64::from(intensity));
            }
            let gain_db = self.gain_db.tick();
            if gain_db != self.gain_at {
                self.gain_at = gain_db;
                self.gain = 10f64.powf(f64::from(gain_db) / 20.0);
            }
            let depth = f64::from(self.depth.tick());
            let hz = f64::from(self.hz.tick());
            let lead = f64::from(self.lead.tick()) / 360.0;
            let upper = f64::from(self.upper.tick());
            let fb = f64::from(self.feedback.tick()) / 100.0;
            let m = f64::from(self.mix.tick()) / 100.0;
            let phases = [
                self.phase,
                if sync { self.phase } else { self.phase + lead },
            ];
            self.phase = (self.phase + hz / self.rate).rem_euclid(1.0);
            let (r, a2) = (self.r, self.r * self.r);
            for c in 0..CHANNELS {
                let f = sweep_hz(self.rate, upper, depth, linear, lfo(phases[c], sine));
                let a1 = -2.0 * r * (2.0 * PI * f / self.rate).cos();
                let x = f64::from(io[c][k]);
                let v = x + fb * self.last[c];
                let mut w = v;
                for section in &mut self.sections[c][..n] {
                    w = section.tick(w, a1, a2);
                }
                self.last[c] = w;
                // The wet: the chain's output and what was fed back into it.
                let y = (1.0 - m) * x + m * (w + v - x);
                io[c][k] = (y * self.gain) as f32;
            }
        }
    }
}

#[cfg(test)]
#[path = "phaser_tests.rs"]
mod tests;
