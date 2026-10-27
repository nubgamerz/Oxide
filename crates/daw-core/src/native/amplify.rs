//! Amplify: a gain per channel:
//!
//! - the gain is `10^(dB/20)` in double precision, rounded to f32, and each sample is an f32
//!   multiply by it;
//! - a change moves the gain in a straight line (in gain, not dB) over 10 ms, stepping after
//!   each sample and landing exactly on the target; automation draws lines between its points;
//! - a new instance, or a render, starts at its settings with no ramp.
//!
//! Link Sliders makes the editor move both gains together; settings, presets and automation
//! set each channel on its own.

use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

/// How long a gain change by hand takes.
const GLIDE_SECONDS: f64 = 0.01;

const GAIN: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -96.0,
    max: 48.0,
    default: 0.0,
    steps: 0,
    decimals: 2,
    automatable: true,
    link: Some("Link Sliders"),
    log_slider: false,
    format: None,
};

pub static KIND: Kind = Kind {
    id: "amplify",
    name: "Amplify",
    category: "Dynamics",
    params: &[
        ParamSpec {
            name: "Left Gain",
            ..GAIN
        },
        ParamSpec {
            name: "Right Gain",
            ..GAIN
        },
        ParamSpec {
            name: "Link Sliders",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 1.0,
            steps: 1,
            decimals: 0,
            automatable: false,
            link: None,
            log_slider: false,
            format: None,
        },
    ],
    make: |rate| Box::new(Amplify::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

struct Amplify {
    /// The gain each channel is at, where it is going, its per-sample step, and the samples
    /// left until it gets there.
    gain: [f32; CHANNELS],
    target: [f32; CHANNELS],
    step: [f32; CHANNELS],
    left: [usize; CHANNELS],
    glide: usize,
}

impl Amplify {
    fn new(rate: f64) -> Self {
        Self {
            gain: [1.0; CHANNELS],
            target: [1.0; CHANNELS],
            step: [0.0; CHANNELS],
            left: [0; CHANNELS],
            glide: ((rate * GLIDE_SECONDS).round() as usize).max(1),
        }
    }
}

fn db_to_gain(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}

impl NativeEffect for Amplify {
    fn set(&mut self, index: usize, value: f64) {
        let glide = self.glide;
        self.ramp(index, value, glide);
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        // Link Sliders is the editor's business.
        if index >= CHANNELS {
            return;
        }
        self.target[index] = db_to_gain(value);
        self.left[index] = samples;
        if samples == 0 {
            self.gain[index] = self.target[index];
        } else {
            self.step[index] = (self.target[index] - self.gain[index]) / samples as f32;
        }
    }

    fn snap(&mut self) {
        self.gain = self.target;
        self.left = [0; CHANNELS];
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        for (c, buf) in io.iter_mut().take(CHANNELS).enumerate() {
            let mut k = from;
            while self.left[c] > 0 && k < to {
                buf[k] *= self.gain[c];
                self.gain[c] += self.step[c];
                self.left[c] -= 1;
                if self.left[c] == 0 {
                    self.gain[c] = self.target[c];
                }
                k += 1;
            }
            let gain = self.gain[c];
            for s in &mut buf[k..to] {
                *s *= gain;
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
    fn fixed_gains_are_exact_and_the_same_at_any_block_size() {
        let len = 5000;
        let left: Vec<f32> = (0..len).map(|i| ((i as f32) * 0.013).sin() * 0.5).collect();
        let right: Vec<f32> = (0..len).map(|i| ((i as f32) * 0.029).cos() * 0.4).collect();
        let params: BTreeMap<String, f64> =
            [("Left Gain".into(), 6.0), ("Right Gain".into(), -12.5)].into();
        let audio = [left.clone(), right.clone()];
        let a = render(&KIND, 48_000.0, &params, &audio, 512).unwrap();
        let b = render(&KIND, 48_000.0, &params, &audio, 37).unwrap();
        assert_eq!(a, b);
        let (gl, gr) = (db_to_gain(6.0), db_to_gain(-12.5));
        for i in 0..len {
            assert_eq!(a[0][i], left[i] * gl);
            assert_eq!(a[1][i], right[i] * gr);
        }
    }

    #[test]
    fn a_change_ramps_in_gain_over_ten_ms() {
        let mut amp = Amplify::new(48_000.0);
        let mut io = vec![vec![1.0f32; 1000]; CHANNELS];
        amp.set(0, -6.0);
        amp.process(&mut io, 0, 1000);
        // 480 samples: the first at the old gain, halfway down after 240, then the target.
        let target = db_to_gain(-6.0);
        assert_eq!(io[0][0], 1.0);
        assert!(
            (io[0][240] - (1.0 + target) / 2.0).abs() < 1e-4,
            "{}",
            io[0][240]
        );
        assert_eq!(io[0][480], target);
        assert_eq!(io[1][600], 1.0);
    }
}
