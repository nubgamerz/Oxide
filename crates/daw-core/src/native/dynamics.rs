//! Dynamics: a gain curve driven by a level detector:
//!
//! - a static curve of up to 7 points (input dB, output dB), straight lines between them,
//!   tabulated as 8192 linear gains over −100..0 dB and read with interpolation;
//! - a level detector: Peak, a one-pole on |x| (k = 0.05), or RMS, a
//!   one-pole on x² (k = 0.92, reading a full-scale sine at −1.78 dB);
//! - two smoothers, each reaching 99 % of a step in its time: the level (dB) with the Gain
//!   Processor times, the gain (linear) with the Level Detector times (the
//!   names are swapped against their use);
//! - Link Channels takes the louder channel's level; the audio is delayed by the look-ahead;
//!   input, output and make-up gains; Envelope Only plays the gain envelope instead of the
//!   audio.
//!
//! Not yet (TD103): the band-limiting FIR (Low and High Cutoff), spline curves, noise gating,
//! the side-chain, and above 0 dB the last segment simply carries on.

use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

const LEVEL_ATTACK: usize = 0;
const LEVEL_RELEASE: usize = 1;
const GAIN_ATTACK: usize = 2;
const GAIN_RELEASE: usize = 3;
const LOOK_AHEAD: usize = 4;
const INPUT: usize = 5;
const OUTPUT: usize = 6;
const MAKEUP: usize = 7;
const LINK: usize = 8;
const RMS: usize = 9;
const ENVELOPE_ONLY: usize = 10;
const POINTS: usize = 11;
/// X1, Y1, … X7, Y7 from here.
const FIRST_POINT: usize = 12;
const MAX_POINTS: usize = 7;

const TABLE: usize = 8192;
/// The longest look-ahead, ms (the delay is made this long once).
const MAX_LOOK_AHEAD: f64 = 500.0;

const MS: ParamSpec = ParamSpec {
    name: "",
    unit: "ms",
    min: 0.0,
    max: 500.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

const DB: ParamSpec = ParamSpec {
    unit: "dB",
    min: -48.0,
    max: 48.0,
    ..MS
};

const SWITCH: ParamSpec = ParamSpec {
    unit: "",
    min: 0.0,
    max: 1.0,
    steps: 1,
    decimals: 0,
    automatable: false,
    ..MS
};

const POINT: ParamSpec = ParamSpec {
    unit: "dB",
    min: -100.0,
    max: 0.0,
    automatable: false,
    ..MS
};

fn detector(v: f64) -> String {
    if v >= 0.5 { "RMS" } else { "Peak" }.into()
}

macro_rules! point {
    ($x:literal, $y:literal, $dx:expr, $dy:expr) => {
        [
            ParamSpec {
                name: $x,
                default: $dx,
                ..POINT
            },
            ParamSpec {
                name: $y,
                default: $dy,
                ..POINT
            },
        ]
    };
}

const fn params() -> [ParamSpec; FIRST_POINT + 2 * MAX_POINTS] {
    let head = [
        ParamSpec {
            name: "Level Detector Attack",
            default: 1.0,
            ..MS
        },
        ParamSpec {
            name: "Level Detector Release",
            max: 2000.0,
            default: 100.0,
            ..MS
        },
        ParamSpec {
            name: "Gain Processor Attack",
            default: 24.0,
            ..MS
        },
        ParamSpec {
            name: "Gain Processor Release",
            max: 2000.0,
            default: 100.0,
            ..MS
        },
        ParamSpec {
            name: "Lookahead Time",
            default: 3.0,
            automatable: false,
            ..MS
        },
        ParamSpec {
            name: "Input Gain",
            ..DB
        },
        ParamSpec {
            name: "Output Gain",
            ..DB
        },
        ParamSpec {
            name: "Make-Up Gain",
            min: 0.0,
            max: 100.0,
            ..DB
        },
        ParamSpec {
            name: "Link Channels",
            ..SWITCH
        },
        ParamSpec {
            name: "Detector",
            default: 1.0,
            format: Some(detector),
            ..SWITCH
        },
        ParamSpec {
            name: "Envelope Only",
            ..SWITCH
        },
        ParamSpec {
            name: "Points",
            unit: "",
            min: 2.0,
            max: 7.0,
            default: 3.0,
            steps: 5,
            decimals: 0,
            automatable: false,
            ..MS
        },
    ];
    let points = [
        point!("X1", "Y1", -100.0, -100.0),
        point!("X2", "Y2", -20.0, -20.0),
        point!("X3", "Y3", 0.0, 0.0),
        point!("X4", "Y4", 0.0, 0.0),
        point!("X5", "Y5", 0.0, 0.0),
        point!("X6", "Y6", 0.0, 0.0),
        point!("X7", "Y7", 0.0, 0.0),
    ];
    let mut all = [MS; FIRST_POINT + 2 * MAX_POINTS];
    let mut i = 0;
    while i < FIRST_POINT {
        all[i] = head[i];
        i += 1;
    }
    let mut p = 0;
    while p < MAX_POINTS {
        all[FIRST_POINT + 2 * p] = points[p][0];
        all[FIRST_POINT + 2 * p + 1] = points[p][1];
        p += 1;
    }
    all
}

static PARAMS: [ParamSpec; FIRST_POINT + 2 * MAX_POINTS] = params();

pub static KIND: Kind = Kind {
    id: "dynamics",
    name: "Dynamics",
    category: "Dynamics",
    params: &PARAMS,
    make: |rate| Box::new(Dynamics::new(rate)),
    response: None,
    transfer: Some(transfer),
    latency: Some(look_ahead),
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The audio's delay: the look-ahead, in samples.
fn look_ahead(rate: f64, v: &[f64]) -> usize {
    ((v[LOOK_AHEAD].min(MAX_LOOK_AHEAD) / 1000.0 * f64::from(rate as f32)) as usize)
        .min((MAX_LOOK_AHEAD / 1000.0 * rate) as usize)
}

/// The output level for a steady input level: the curve at the gained input, then the
/// make-up and output gains.
fn transfer(_rate: f64, v: &[f64], input_db: f64) -> f64 {
    curve(v, input_db + v[INPUT]) + v[MAKEUP] + v[OUTPUT]
}

/// The curve at `x` dB: straight lines between the active points, flat beyond the first and
/// the last ones' levels: below the first point its Y, past the last its Y,
/// and above 0 dB the last segment carries on.
fn curve(v: &[f64], x: f64) -> f64 {
    let n = (v[POINTS].round() as usize).clamp(2, MAX_POINTS);
    let point = |i: usize| (v[FIRST_POINT + 2 * i], v[FIRST_POINT + 2 * i + 1]);
    let (x0, y0) = point(0);
    if x <= x0 {
        return y0;
    }
    for i in 1..n {
        let (xa, ya) = point(i - 1);
        let (xb, yb) = point(i);
        if x <= xb {
            if xb == xa {
                return (ya + yb) / 2.0;
            }
            return ya + (x - xa) * (yb - ya) / (xb - xa);
        }
    }
    let (xl, yl) = point(n - 1);
    let (xp, yp) = point(n - 2);
    if x > 0.0 && xl > xp {
        // Above 0 dB: the last segment carries on.
        return yl + (x - xl) * (yl - yp) / (xl - xp);
    }
    yl
}

/// A smoother's coefficient: 99 % of a step in `t` ms; 0 (instant) for t = 0.
fn coefficient(t_ms: f64, rate: f32) -> f32 {
    let n = (t_ms as f32 / 1000.0) * rate;
    if n > 0.0 {
        0.01f32.powf(1.0 / n)
    } else {
        0.0
    }
}

struct Dynamics {
    rate: f32,
    values: [f64; FIRST_POINT + 2 * MAX_POINTS],
    /// Linear gains for input levels 0, −100/8192, … −100 dB.
    table: Vec<f32>,
    level_attack: f32,
    level_release: f32,
    gain_attack: f32,
    gain_release: f32,
    input: f32,
    output: f32,
    makeup: f32,
    look_ahead: usize,
    /// Per channel: the detector's state, the smoothed level (dB) and gain.
    detected: [f32; CHANNELS],
    level: [f32; CHANNELS],
    gain: [f32; CHANNELS],
    delay: [Vec<f32>; CHANNELS],
    at: usize,
}

impl Dynamics {
    fn new(rate: f64) -> Self {
        let length = (MAX_LOOK_AHEAD / 1000.0 * rate) as usize + 1;
        let mut d = Self {
            rate: rate as f32,
            values: [0.0; FIRST_POINT + 2 * MAX_POINTS],
            table: vec![1.0; TABLE],
            level_attack: 0.0,
            level_release: 0.0,
            gain_attack: 0.0,
            gain_release: 0.0,
            input: 1.0,
            output: 1.0,
            makeup: 1.0,
            look_ahead: 0,
            detected: [0.0; CHANNELS],
            level: [-96.0; CHANNELS],
            gain: [1.0; CHANNELS],
            delay: [vec![0.0; length], vec![0.0; length]],
            at: 0,
        };
        for (v, p) in d.values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        d.derive(true);
        d
    }

    fn derive(&mut self, curve_changed: bool) {
        let v = self.values;
        // Crossed: the Gain Processor times smooth the level, the Level
        // Detector times the gain.
        self.level_attack = coefficient(v[GAIN_ATTACK], self.rate);
        self.level_release = coefficient(v[GAIN_RELEASE], self.rate);
        self.gain_attack = coefficient(v[LEVEL_ATTACK], self.rate);
        self.gain_release = coefficient(v[LEVEL_RELEASE], self.rate);
        let gain = |db: f64| 10f32.powf(db as f32 / 20.0);
        self.input = gain(v[INPUT]);
        self.output = gain(v[OUTPUT]) * self.input;
        self.makeup = gain(v[MAKEUP]);
        self.look_ahead = look_ahead(f64::from(self.rate), &v);
        if curve_changed {
            // On the audio thread too (a point moved): writes into the table, no allocation.
            for (k, t) in self.table.iter_mut().enumerate() {
                let x = -100.0 * k as f64 / TABLE as f64;
                *t = ((curve(&v, x) - x) * std::f64::consts::LN_10 / 20.0).exp() as f32;
            }
        }
    }

    /// The gain for a smoothed level (dB).
    fn lookup(&self, level: f32) -> f32 {
        let pos = -81.92 * level;
        if pos < 0.0 {
            let db = f64::from(level);
            return ((curve(&self.values, db) - db) * std::f64::consts::LN_10 / 20.0).exp() as f32;
        }
        let i = (pos as usize).min(TABLE - 2);
        let frac = (pos - i as f32).min(1.0);
        (1.0 - frac) * self.table[i] + frac * self.table[i + 1]
    }
}

impl NativeEffect for Dynamics {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            if *v != value {
                *v = value;
                self.derive(index >= POINTS);
            }
        }
    }

    fn latency(&self) -> usize {
        self.look_ahead
    }

    /// A fresh start when playback or an Apply starts: the detectors quiet, the level
    /// at −96 dB, the gain at unity, the delay empty.
    fn start(&mut self) {
        self.detected = [0.0; CHANNELS];
        self.level = [-96.0; CHANNELS];
        self.gain = [1.0; CHANNELS];
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.at = 0;
    }

    // A sample at a time across both channels: Link Channels shares the level.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let rms = self.values[RMS] >= 0.5;
        let linked = self.values[LINK] >= 0.5;
        let envelope_only = self.values[ENVELOPE_ONLY] >= 0.5;
        let length = self.delay[0].len();
        for k in from..to {
            // The detectors, in a 16-bit scale.
            let mut levels = [0.0f32; CHANNELS];
            for (c, level) in levels.iter_mut().enumerate() {
                let x = io[c][k] * self.input * 32768.0;
                let y = &mut self.detected[c];
                // The detector flushed under -500 dB: it would stick at the smallest
                // denormal (0.92 of it rounds back up) and cost double for ever after the input
                // stops (dev plan 18.1). Its 1e-10 floor below hides the difference.
                *level = if rms {
                    *y = super::flush(0.92 * *y + 0.08 * x * x);
                    10.0 * (*y + 1e-10).log10() - 89.08
                } else {
                    *y = super::flush(0.05 * *y + 0.95 * x.abs());
                    20.0 * ((*y + 1e-10) / 32768.0).log10()
                };
            }
            if linked {
                let loudest = levels.iter().copied().fold(f32::MIN, f32::max);
                levels = [loudest; CHANNELS];
            }
            for c in 0..CHANNELS {
                if linked && c > 0 {
                    self.level[c] = self.level[0];
                    self.gain[c] = self.gain[0];
                    continue;
                }
                let l = &mut self.level[c];
                let coef = if levels[c] > *l {
                    self.level_attack
                } else {
                    self.level_release
                };
                *l = ((1.0 - coef) * (levels[c] - *l) + *l).max(-200.0);
                let g = self.lookup(self.level[c]);
                let gamma = &mut self.gain[c];
                let coef = if *gamma > g {
                    self.gain_attack
                } else {
                    self.gain_release
                };
                *gamma += (1.0 - coef) * (g - *gamma);
            }
            // The audio, delayed by the look-ahead.
            let read = (self.at + length - self.look_ahead) % length;
            for (c, buf) in io.iter_mut().take(CHANNELS).enumerate() {
                self.delay[c][self.at] = buf[k];
                buf[k] = if envelope_only {
                    10f32.powf(self.level[c] / 20.0) * self.gain[c] * self.output
                } else {
                    self.delay[c][read] * self.gain[c] * self.makeup * self.output
                };
            }
            self.at = (self.at + 1) % length;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    fn tone(n: usize, level_db: f64) -> Vec<f32> {
        let a = 10f64.powf(level_db / 20.0);
        (0..n)
            .map(|i| (a * (2.0 * PI * 1000.0 * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

    fn rms_db(v: &[f32]) -> f64 {
        10.0 * (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).log10()
    }

    fn defaults() -> Vec<f64> {
        PARAMS.iter().map(|p| p.default).collect()
    }

    #[test]
    fn the_default_curve_is_unity_and_the_audio_is_only_delayed() {
        let x = tone(9600, -12.0);
        let out = render(
            &KIND,
            48_000.0,
            &BTreeMap::new(),
            &[x.clone(), x.clone()],
            256,
        )
        .unwrap();
        // Unity: the gains are 1 (to f32 rounding of the table), after the 3 ms look-ahead.
        for (a, b) in out[0][4000..].iter().zip(&x[4000..]) {
            assert!((a - b).abs() < 1e-6);
        }
        assert_eq!((KIND.make)(48_000.0).latency(), 144);
        assert!((transfer(48_000.0, &defaults(), -30.0) - -30.0).abs() < 1e-12);
    }

    #[test]
    fn a_compressing_curve_turns_a_loud_tone_down_by_the_curve() {
        // 2:1 above -40 dB: (-100,-100) (-40,-40) (0,-20).
        let settings: BTreeMap<String, f64> = [
            ("X2".to_string(), -40.0),
            ("Y2".to_string(), -40.0),
            ("Y3".to_string(), -20.0),
            ("Detector".to_string(), 1.0),
        ]
        .into();
        let x = tone(48_000, -10.0);
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        // RMS detector: the sine reads -10 − 1.78 dB; the curve gives half of that above -40.
        let level = -10.0 - 3.01 + 1.23;
        let expected = (-40.0 + (level + 40.0) / 2.0) - level;
        let got = rms_db(&out[0][30_000..]) - rms_db(&x[30_000..]);
        assert!((got - expected).abs() < 0.3, "{got:.2} vs {expected:.2} dB");
        let a = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 61).unwrap();
        assert_eq!(a, out);
    }

    #[test]
    fn the_curve_is_straight_lines_between_points() {
        let mut v = defaults();
        v[FIRST_POINT + 2] = -40.0;
        v[FIRST_POINT + 3] = -40.0;
        v[FIRST_POINT + 5] = -20.0;
        assert!((curve(&v, -20.0) - -30.0).abs() < 1e-12);
        assert!((curve(&v, -70.0) - -70.0).abs() < 1e-12);
        assert!((curve(&v, -120.0) - -100.0).abs() < 1e-12);
    }

    #[test]
    fn the_detectors_fall_to_zero_after_the_input_stops() {
        // RMS and peak: 0.92 of the smallest denormal rounds back to it, so without the flush
        // the RMS detector would sit there, a denormal operation every sample, for ever.
        for rms in [1.0, 0.0] {
            let mut d = Dynamics::new(48_000.0);
            d.set(RMS, rms);
            let x = tone(4800, -6.0);
            let mut io = vec![x.clone(), x];
            d.process(&mut io, 0, 4800);
            let mut silence = vec![vec![0.0f32; 48_000]; CHANNELS];
            d.process(&mut silence, 0, 48_000);
            assert_eq!(d.detected, [0.0; CHANNELS], "rms {rms}");
        }
    }
}
