//! De-esser: a compressor on the sibilance band:
//!
//! - the signal is split at the band's edges (centre ∓ half the bandwidth) into low,
//!   sibilance and high bands, each edge a pair of second-order Butterworth sections
//!   (Linkwitz–Riley 4th order: two identical sections), as a
//!   proper three-way split: the high band passes the lower edge too, and the low band an
//!   allpass at the upper, so the bands add back up flat;
//! - the sibilance band's peak, linked across channels (instant attack, falling to 3 % in
//!   50 ms), drives a hard-knee 8:1 compressor above the threshold, smoothed as the
//!   Compressor smooths (releasing toward unity), with a fixed 100 ms attack and release;
//! - Broadband turns the whole signal down; Multiband only the sibilance band (the output is
//!   then the bands' sum); Output Sibilance Only plays the band alone, for finding it. The
//!   audio is delayed a fixed 199 samples, at every rate.
//!
//! Not yet: a crossfade of the crossover when its edges move; the split's band gains and
//! "CrossOver Q" (TD103).

use super::biquad::{allpass2, butterworth2, Section};
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

const THRESHOLD: usize = 0;
const CENTER: usize = 1;
const BANDWIDTH: usize = 2;
const MULTIBAND: usize = 3;
const SIBILANCE_ONLY: usize = 4;
const ATTACK: usize = 5;
const RELEASE: usize = 6;

/// The fixed delay: 200-sample rings.
const DELAY: usize = 200;
/// The ratio, 8:1, as the exponent 1/8 − 1.
const EXPONENT: f32 = 0.125 - 1.0;
const DB_LN: f32 = 0.115_129_255;

fn mode(v: f64) -> String {
    if v >= 0.5 { "Multiband" } else { "Broadband" }.into()
}

fn hz(v: f64) -> String {
    format!("{v:.0} Hz")
}

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -80.0,
    max: 0.0,
    default: -30.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

static PARAMS: [ParamSpec; 7] = [
    ParamSpec {
        name: "Threshold",
        ..PARAM
    },
    ParamSpec {
        name: "Center Frequency",
        unit: "Hz",
        min: 500.0,
        max: 12_000.0,
        default: 5000.0,
        log_slider: true,
        format: Some(hz),
        ..PARAM
    },
    ParamSpec {
        name: "Bandwidth",
        unit: "Hz",
        min: 100.0,
        max: 7500.0,
        default: 3000.0,
        format: Some(hz),
        ..PARAM
    },
    ParamSpec {
        name: "Mode",
        unit: "",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        steps: 1,
        decimals: 0,
        automatable: false,
        format: Some(mode),
        ..PARAM
    },
    ParamSpec {
        name: "Output Sibilance Only",
        unit: "",
        min: 0.0,
        max: 1.0,
        default: 0.0,
        steps: 1,
        decimals: 0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Attack",
        unit: "ms",
        min: 0.1,
        max: 500.0,
        default: 100.0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Release",
        unit: "ms",
        min: 10.0,
        max: 500.0,
        default: 100.0,
        automatable: false,
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "de-esser",
    name: "De-esser",
    category: "Dynamics",
    params: &PARAMS,
    make: |rate| Box::new(DeEsser::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

fn one_pole(t_ms: f32, rate: f32) -> f32 {
    if t_ms > 0.0 {
        ((-1000.0 / t_ms) / (rate * 0.5)).exp()
    } else {
        0.0
    }
}

struct DeEsser {
    rate: f64,
    values: [f64; 7],
    threshold: f32,
    attack: f32,
    release: f32,
    fall: f32,
    /// Low: two low-passes at the lower edge and the allpass at the upper; band: two
    /// high-passes at the lower edge and two low-passes at the upper; high: two high-passes at
    /// each edge.
    low: [Section; 3],
    band: [Section; 4],
    high: [Section; 4],
    level: f32,
    gain: f32,
    /// Rings of the dry signal and of each band, per channel.
    dry: [[f32; DELAY]; CHANNELS],
    lows: [[f32; DELAY]; CHANNELS],
    bands: [[f32; DELAY]; CHANNELS],
    highs: [[f32; DELAY]; CHANNELS],
    at: usize,
}

impl DeEsser {
    fn new(rate: f64) -> Self {
        let section = Section::new(super::biquad::Coefficients::IDENTITY);
        let mut d = Self {
            rate,
            values: [0.0; 7],
            threshold: 1.0,
            attack: 0.0,
            release: 0.0,
            fall: 0.03f32.powf(1.0 / (rate as f32 * 0.001 * 50.0)),
            low: [section; 3],
            band: [section; 4],
            high: [section; 4],
            level: 0.0,
            gain: 1.0,
            dry: [[0.0; DELAY]; CHANNELS],
            lows: [[0.0; DELAY]; CHANNELS],
            bands: [[0.0; DELAY]; CHANNELS],
            highs: [[0.0; DELAY]; CHANNELS],
            at: 0,
        };
        for (v, p) in d.values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        d.derive(true);
        d
    }

    /// Recompute what the settings give; the crossover only when its edges moved (keeping the
    /// filters' state, so a move doesn't click).
    fn derive(&mut self, edges: bool) {
        let v = &self.values;
        self.threshold = (v[THRESHOLD] as f32 * DB_LN).exp();
        let rate = self.rate as f32;
        self.attack = one_pole(v[ATTACK] as f32, rate);
        self.release = one_pole(v[RELEASE] as f32, rate);
        if edges {
            let lo = (v[CENTER] - v[BANDWIDTH] / 2.0).clamp(20.0, 20_000.0);
            let hi = (v[CENTER] + v[BANDWIDTH] / 2.0).clamp(20.0, 20_000.0);
            let (lp_lo, hp_lo) = (
                butterworth2(self.rate, lo, false),
                butterworth2(self.rate, lo, true),
            );
            let (lp_hi, hp_hi) = (
                butterworth2(self.rate, hi, false),
                butterworth2(self.rate, hi, true),
            );
            self.low[0].c = lp_lo;
            self.low[1].c = lp_lo;
            self.low[2].c = allpass2(self.rate, hi);
            self.band[0].c = hp_lo;
            self.band[1].c = hp_lo;
            self.band[2].c = lp_hi;
            self.band[3].c = lp_hi;
            self.high[0].c = hp_lo;
            self.high[1].c = hp_lo;
            self.high[2].c = hp_hi;
            self.high[3].c = hp_hi;
        }
    }
}

impl NativeEffect for DeEsser {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            *v = value;
            self.derive(index == CENTER || index == BANDWIDTH);
        }
    }

    fn latency(&self) -> usize {
        DELAY - 1
    }

    /// A fresh start when playback or an Apply starts.
    fn start(&mut self) {
        for s in self
            .low
            .iter_mut()
            .chain(&mut self.band)
            .chain(&mut self.high)
        {
            *s = Section::new(s.c);
        }
        self.level = 0.0;
        self.gain = 1.0;
        for rings in [
            &mut self.dry,
            &mut self.lows,
            &mut self.bands,
            &mut self.highs,
        ] {
            for r in rings.iter_mut() {
                r.fill(0.0);
            }
        }
        self.at = 0;
    }

    // A sample at a time across both channels: the band's level is linked.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let multiband = self.values[MULTIBAND] >= 0.5;
        let sibilance_only = self.values[SIBILANCE_ONLY] >= 0.5;
        for k in from..to {
            let mut m = 0.0f32;
            let mut parts = [[0.0f32; 3]; CHANNELS];
            for (c, part) in parts.iter_mut().enumerate() {
                let x = f64::from(io[c][k]);
                let low = self.low.iter_mut().fold(x, |s, f| f.tick(c, s));
                let band = self.band.iter_mut().fold(x, |s, f| f.tick(c, s));
                let high = self.high.iter_mut().fold(x, |s, f| f.tick(c, s));
                *part = [low as f32, band as f32, high as f32];
                m = m.max(part[1].abs());
            }
            // The band's linked peak, an 8:1 hard knee, smoothed (releasing toward unity).
            if m > self.level {
                self.level = m;
            } else {
                self.level *= self.fall;
                if self.level < 1e-15 {
                    self.level = 0.0;
                }
                self.level = self.level.max(m);
            }
            let g = if self.level < self.threshold {
                1.0
            } else {
                (self.level * (1.0 / self.threshold)).powf(EXPONENT)
            };
            if g < self.gain {
                self.gain = self.gain * self.attack + g * (1.0 - self.attack);
            } else {
                self.gain = self.gain * self.release + (1.0 - self.release);
            }
            let p = self.at;
            let next = (p + 1) % DELAY;
            for (c, part) in parts.iter().enumerate() {
                self.dry[c][p] = io[c][k];
                self.lows[c][p] = part[0];
                self.bands[c][p] = part[1];
                self.highs[c][p] = part[2];
                io[c][k] = if sibilance_only {
                    self.bands[c][next]
                } else if multiband {
                    self.lows[c][next] + self.bands[c][next] * self.gain + self.highs[c][next]
                } else {
                    self.dry[c][next] * self.gain
                };
            }
            self.at = next;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    fn tones(n: usize, parts: &[(f64, f64)]) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = i as f64 / 48_000.0;
                parts
                    .iter()
                    .map(|&(hz, db)| 10f64.powf(db / 20.0) * (2.0 * PI * hz * t).sin())
                    .sum::<f64>() as f32
            })
            .collect()
    }

    /// The level of the `hz` part of `v`, by correlation.
    fn level_db(v: &[f32], hz: f64) -> f64 {
        let (mut s, mut c) = (0.0, 0.0);
        for (i, x) in v.iter().enumerate() {
            let p = 2.0 * PI * hz * i as f64 / 48_000.0;
            s += f64::from(*x) * p.sin();
            c += f64::from(*x) * p.cos();
        }
        20.0 * (2.0 * (s * s + c * c).sqrt() / v.len() as f64).log10()
    }

    #[test]
    fn multiband_turns_down_the_sibilance_and_leaves_the_voice() {
        // A 300 Hz "voice" and a loud 6 kHz "ess" (centre 6 kHz, 3 kHz wide).
        let x = tones(48_000, &[(300.0, -12.0), (6000.0, -10.0)]);
        let settings: BTreeMap<String, f64> = [
            ("Mode".to_string(), 1.0),
            ("Center Frequency".to_string(), 6000.0),
            ("Threshold".to_string(), -30.0),
        ]
        .into();
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 512).unwrap();
        let settled = &out[0][24_000..];
        let ess = level_db(settled, 6000.0);
        let voice = level_db(settled, 300.0);
        // 8:1 above -30 on the band: the ess comes down (by 5 dB here: with its edges 6 dB
        // down, part of a 6 kHz tone is in the high band, which stays); the voice stays.
        assert!(ess < -14.0, "ess at {ess:.1} dB");
        assert!((voice - -12.0).abs() < 0.5, "voice at {voice:.2} dB");
    }

    #[test]
    fn broadband_turns_down_everything_and_sibilance_only_plays_the_band() {
        let x = tones(48_000, &[(300.0, -12.0), (6000.0, -10.0)]);
        let mut settings: BTreeMap<String, f64> = [("Center Frequency".to_string(), 6000.0)].into();
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        let voice = level_db(&out[0][24_000..], 300.0);
        assert!(
            voice < -15.0,
            "the voice ducked with the ess: {voice:.1} dB"
        );
        settings.insert("Output Sibilance Only".into(), 1.0);
        let band = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 512).unwrap();
        let settled = &band[0][24_000..];
        // The band alone: the 6 kHz ess through both edges' filters (4500 and 7500 Hz).
        let through: f64 = [
            (4500.0, true),
            (4500.0, true),
            (7500.0, false),
            (7500.0, false),
        ]
        .iter()
        .map(|&(f, high)| {
            20.0 * butterworth2(48_000.0, f, high)
                .magnitude(6000.0, 48_000.0)
                .log10()
        })
        .sum();
        let ess = level_db(settled, 6000.0);
        assert!(
            (ess - (-10.0 + through)).abs() < 0.2,
            "{ess:.2} vs {:.2}",
            -10.0 + through
        );
        assert!(level_db(settled, 300.0) < -60.0);
        assert_eq!((KIND.make)(48_000.0).latency(), 199);
    }

    #[test]
    fn under_the_threshold_multiband_passes_the_bands_sum() {
        let x = tones(24_000, &[(1000.0, -40.0), (6000.0, -40.0)]);
        let settings: BTreeMap<String, f64> = [("Mode".to_string(), 1.0)].into();
        let a = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        let b = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 37).unwrap();
        assert_eq!(a, b);
        for hz in [1000.0, 6000.0] {
            assert!((level_db(&a[0][12_000..], hz) - -40.0).abs() < 0.5, "{hz}");
        }
    }
}
