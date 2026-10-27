//! Channel Mixer, for stereo: each output is a mix of the inputs. Each coefficient is `percent / 100` in f32,
//! an output is `c0·left + c1·right` in f32 (two multiplies, one add), and a change takes
//! effect at once, with no ramp and no state. Invert is the sign of the percentage.

use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

const COEFFICIENT: ParamSpec = ParamSpec {
    name: "",
    unit: "%",
    min: -100.0,
    max: 100.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

pub static KIND: Kind = Kind {
    id: "channel-mixer",
    name: "Channel Mixer",
    category: "Dynamics",
    // Output-major: an output's inputs together.
    params: &[
        ParamSpec {
            name: "Left from Left",
            default: 100.0,
            ..COEFFICIENT
        },
        ParamSpec {
            name: "Left from Right",
            ..COEFFICIENT
        },
        ParamSpec {
            name: "Right from Left",
            ..COEFFICIENT
        },
        ParamSpec {
            name: "Right from Right",
            default: 100.0,
            ..COEFFICIENT
        },
    ],
    make: |_| Box::new(ChannelMixer::default()),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

#[derive(Default)]
struct ChannelMixer {
    /// `c[out][in]`.
    c: [[f32; CHANNELS]; CHANNELS],
}

impl NativeEffect for ChannelMixer {
    fn set(&mut self, index: usize, value: f64) {
        if index < CHANNELS * CHANNELS {
            self.c[index / CHANNELS][index % CHANNELS] = (value as f32) / 100.0;
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let [l, r] = io else {
            return;
        };
        let [[ll, lr], [rl, rr]] = self.c;
        for (a, b) in l[from..to].iter_mut().zip(&mut r[from..to]) {
            let (x0, x1) = (*a, *b);
            *a = ll * x0 + lr * x1;
            *b = rl * x0 + rr * x1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn identity_swap_and_mid_side() {
        let left: Vec<f32> = (0..300).map(|i| (i as f32 * 0.07).sin()).collect();
        let right: Vec<f32> = (0..300).map(|i| (i as f32 * 0.11).cos() * 0.5).collect();
        let audio = [left.clone(), right.clone()];
        let same = render(&KIND, 48_000.0, &BTreeMap::new(), &audio, 64).unwrap();
        assert_eq!(same, audio);
        let swap: BTreeMap<String, f64> = [
            ("Left from Left".into(), 0.0),
            ("Left from Right".into(), 100.0),
            ("Right from Left".into(), 100.0),
            ("Right from Right".into(), 0.0),
        ]
        .into();
        let swapped = render(&KIND, 48_000.0, &swap, &audio, 64).unwrap();
        assert_eq!(swapped, vec![right.clone(), left.clone()]);
        // A Mid/Side coefficient of 70.7106 %, not exactly 1/sqrt(2).
        let k = 70.7106;
        let ms: BTreeMap<String, f64> = [
            ("Left from Left".into(), k),
            ("Left from Right".into(), k),
            ("Right from Left".into(), k),
            ("Right from Right".into(), -k),
        ]
        .into();
        let out = render(&KIND, 48_000.0, &ms, &audio, 64).unwrap();
        let c = (k as f32) / 100.0;
        assert_eq!(out[0][17], c * left[17] + c * right[17]);
        assert_eq!(out[1][17], c * left[17] + -c * right[17]);
    }
}
