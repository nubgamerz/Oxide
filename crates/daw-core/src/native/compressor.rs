//! Compressor: a single-band compressor, all in f32:
//!
//! - a peak detector linked across the channels: instant attack, falling to 3 % in 50 ms;
//! - a hard knee in the linear domain: `(level / threshold)^(1/ratio − 1)` above the threshold;
//! - two smoothers in series: the first attacks toward that gain with the attack time and
//!   releases toward unity (not toward the gain) with the release time (one-poles of time
//!   constant t/2), the second follows it with a quarter of the shorter time;
//! - the audio delayed by 1 ms less a sample (the gain looks that far ahead), and an output
//!   gain; Auto Makeup Gain adds back the average gain reduction of the last 2 s.
//!
//! No ±2e-10 anti-denormal offset is added to the right channel (−194 dB, TD103).

use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

const OUTPUT: usize = 0;
const THRESHOLD: usize = 1;
const RATIO: usize = 2;
const ATTACK: usize = 3;
const RELEASE: usize = 4;
const MAKEUP: usize = 5;

/// dB to natural log units: `exp(dB · 0.115129255)`.
const DB_LN: f32 = 0.115_129_255;

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -30.0,
    max: 30.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

fn ratio(v: f64) -> String {
    format!("{v:.1}:1")
}

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "Output Gain",
        ..PARAM
    },
    ParamSpec {
        name: "Threshold",
        min: -60.0,
        max: 0.0,
        ..PARAM
    },
    ParamSpec {
        name: "Ratio",
        unit: "",
        min: 1.0,
        max: 30.0,
        default: 1.0,
        format: Some(ratio),
        ..PARAM
    },
    ParamSpec {
        name: "Attack",
        unit: "ms",
        min: 0.0,
        max: 500.0,
        default: 10.0,
        ..PARAM
    },
    ParamSpec {
        name: "Release",
        unit: "ms",
        min: 0.0,
        max: 5000.0,
        default: 100.0,
        ..PARAM
    },
    ParamSpec {
        name: "Auto Makeup Gain",
        unit: "",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        steps: 1,
        decimals: 0,
        automatable: false,
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "compressor",
    name: "Compressor",
    category: "Dynamics",
    params: &PARAMS,
    make: |rate| Box::new(Compressor::new(rate)),
    response: None,
    transfer: Some(transfer),
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The static curve: above the threshold, the ratio; then the output gain.
fn transfer(_rate: f64, v: &[f64], input_db: f64) -> f64 {
    let t = v[THRESHOLD];
    let out = if input_db > t {
        t + (input_db - t) / v[RATIO]
    } else {
        input_db
    };
    out + v[OUTPUT]
}

/// A one-pole coefficient with time constant t/2 (t in ms); 0 for an instant one.
fn one_pole(t_ms: f32, rate: f32) -> f32 {
    if t_ms > 0.0 {
        ((-1000.0 / t_ms) / (rate * 0.5)).exp()
    } else {
        0.0
    }
}

struct Compressor {
    rate: f32,
    values: [f64; 6],
    threshold: f32,
    /// 1/ratio, and its exponent 1/ratio − 1.
    inverse: f32,
    exponent: f32,
    attack: f32,
    release: f32,
    follow: f32,
    output: f32,
    makeup: bool,
    /// The detector's fall per sample (to 3 % in 50 ms).
    fall: f32,
    level: f32,
    gain1: f32,
    gain2: f32,
    /// The look-ahead delay, one ring per channel, and where it is.
    delay: [Vec<f32>; CHANNELS],
    at: usize,
    /// Gain reduction (dB) over the last 2 s, for Auto Makeup Gain.
    history: Vec<f32>,
    history_at: usize,
    history_sum: f32,
}

impl Compressor {
    fn new(rate: f64) -> Self {
        let rate = rate as f32;
        let length = (0.5 + rate * 0.001) as usize;
        let window = (2.0 * rate) as usize;
        let mut c = Self {
            rate,
            values: [0.0; 6],
            threshold: 1.0,
            inverse: 1.0,
            exponent: 0.0,
            attack: 0.0,
            release: 0.0,
            follow: 1.0,
            output: 1.0,
            makeup: false,
            fall: 0.03f32.powf(1.0 / (rate * 0.05)),
            level: 0.0,
            gain1: 1.0,
            gain2: 1.0,
            delay: [vec![0.0; length], vec![0.0; length]],
            at: 0,
            history: vec![0.0; window.max(1)],
            history_at: 0,
            history_sum: 0.0,
        };
        for (v, p) in c.values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        c.derive();
        c
    }

    fn derive(&mut self) {
        let v = &self.values;
        self.output = (v[OUTPUT] as f32 * DB_LN).exp();
        self.threshold = (v[THRESHOLD] as f32 * DB_LN).exp();
        self.inverse = (1.0 / v[RATIO]) as f32;
        self.exponent = (f64::from(self.inverse) - 1.0) as f32;
        let (attack, release) = (v[ATTACK] as f32, v[RELEASE] as f32);
        self.attack = one_pole(attack, self.rate);
        self.release = one_pole(release, self.rate);
        let shortest = attack.min(release) * 0.5;
        self.follow = if shortest <= 0.0 {
            1.0
        } else {
            1.0 - ((-1000.0 / shortest) / (self.rate * 0.5)).exp()
        };
        self.makeup = v[MAKEUP] >= 0.5;
    }
}

impl NativeEffect for Compressor {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            *v = value;
            self.derive();
        }
    }

    fn latency(&self) -> usize {
        self.delay[0].len() - 1
    }

    /// A fresh start when playback or an Apply starts.
    fn start(&mut self) {
        self.level = 0.0;
        self.gain1 = 1.0;
        self.gain2 = 1.0;
        for d in &mut self.delay {
            d.fill(0.0);
        }
        self.at = 0;
        self.history.fill(0.0);
        self.history_at = 0;
        self.history_sum = 0.0;
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let length = self.delay[0].len();
        let window = self.history.len();
        let [left, right] = io else {
            return;
        };
        for k in from..to {
            // The linked peak detector.
            let m = (f64::from(left[k]).abs().max(f64::from(right[k]).abs())) as f32;
            if m > self.level {
                self.level = m;
            } else {
                self.level *= self.fall;
                if f64::from(self.level) < 1e-15 {
                    self.level = 0.0;
                }
            }
            let e = self.level.max(1e-5);
            let g = if e < self.threshold || self.inverse == 1.0 {
                1.0
            } else {
                (e / self.threshold).powf(self.exponent)
            };
            // Attack toward the gain; release toward unity.
            if self.gain1 > g {
                self.gain1 += (g - self.gain1) * (1.0 - self.attack);
            } else {
                self.gain1 = self.gain1 * self.release + (1.0 - self.release);
            }
            self.gain2 += (self.gain1 - self.gain2) * self.follow;
            let reduction = if self.gain2 < 1e-20 {
                400.0
            } else {
                -(self.gain2.ln() * 8.685_889)
            };
            self.history_sum += reduction - self.history[self.history_at];
            self.history[self.history_at] = reduction;
            self.history_at = (self.history_at + 1) % window;
            let mut gain = self.gain2 * self.output;
            if self.makeup {
                gain *= ((1.0 / window as f32) * self.history_sum * DB_LN).exp();
            }
            self.delay[0][self.at] = left[k];
            self.delay[1][self.at] = right[k];
            self.at = (self.at + 1) % length;
            left[k] = gain * self.delay[0][self.at];
            right[k] = gain * self.delay[1][self.at];
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

    fn peak_db(v: &[f32]) -> f64 {
        20.0 * f64::from(v.iter().fold(0.0f32, |m, s| m.max(s.abs()))).log10()
    }

    #[test]
    fn a_steady_tone_settles_on_the_curve() {
        // -6 dBFS peaks, threshold -20, 4:1: -20 + 14/4 = -16.5 dB, plus 3 dB of output.
        let settings: BTreeMap<String, f64> = [
            ("Threshold".to_string(), -20.0),
            ("Ratio".to_string(), 4.0),
            ("Output Gain".to_string(), 3.0),
            ("Attack".to_string(), 5.0),
            ("Release".to_string(), 50.0),
        ]
        .into();
        let x = tone(48_000, -6.0);
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 512).unwrap();
        let settled = peak_db(&out[0][30_000..]);
        // The detector holds peaks; between them it sags a little, so allow 0.5 dB.
        assert!((settled - -13.5).abs() < 0.5, "{settled:.2} dB");
        let v: Vec<f64> = settings_values(&settings);
        assert!((transfer(48_000.0, &v, -6.0) - -13.5).abs() < 1e-9);
    }

    fn settings_values(settings: &BTreeMap<String, f64>) -> Vec<f64> {
        PARAMS
            .iter()
            .map(|p| *settings.get(p.name).unwrap_or(&p.default))
            .collect()
    }

    #[test]
    fn ratio_one_and_quiet_audio_pass_delayed_by_the_look_ahead() {
        let x = tone(4800, -12.0);
        let out = render(
            &KIND,
            48_000.0,
            &BTreeMap::new(),
            &[x.clone(), x.clone()],
            100,
        )
        .unwrap();
        // render drops the latency (47 samples at 48 kHz): unity gain, the same samples.
        assert_eq!(out[0], x);
        assert_eq!((KIND.make)(48_000.0).latency(), 47);
        assert_eq!((KIND.make)(44_100.0).latency(), 43);
    }

    #[test]
    fn the_same_at_any_block_size_and_auto_makeup_brings_the_level_back() {
        let mut settings: BTreeMap<String, f64> =
            [("Threshold".to_string(), -30.0), ("Ratio".to_string(), 3.0)].into();
        let x = tone(3 * 48_000, -6.0);
        let a = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        let b = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 61).unwrap();
        assert_eq!(a, b);
        settings.insert("Auto Makeup Gain".into(), 1.0);
        let made_up = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 512).unwrap();
        // After 2 s of steady reduction, makeup restores the input's level (within a dB).
        let level = peak_db(&made_up[0][2 * 48_000 + 10_000..]);
        assert!((level - -6.0).abs() < 1.0, "{level:.2} dB");
    }
}
