//! Hum Remover: notches at a hum's fundamental and its harmonics:
//!
//! - per channel, a chain of up to 8 peaking sections at the hum's harmonics f0, 2·f0 … N·f0
//!   (N = Number of Harmonics), all with the one Q, each the cookbook peaking EQ (gain law
//!   A = 10^(g/40): exactly g dB at its centre, unity far away), run in harmonic order;
//! - each harmonic's cut is interpolated in dB from Gain at the fundamental to
//!   Gain·(1 − Slope) at the N-th; the harmonic's frequency is an f32 product and
//!   the interpolation f32 arithmetic, the coefficients doubles;
//! - a harmonic at or above Nyquist is left out (exactly at Nyquist the section would be a
//!   double pole on the unit circle);
//! - both channels are filtered alike and independently; no latency, no glide: a change
//!   swaps the sections in at once, keeping their state unless the Frequency jumped by 0.3
//!   or more in log10 (about an octave), which starts them afresh. A
//!   transport start clears them, so playback and export start alike.
//!
//! **Output Hum Only** swaps in a second set of sections: a +60 dB peak at
//! each of the same harmonics with the same Q (Gain and Slope ignored), then a -60 dB trim, so
//! a lone harmonic comes out at unity and everything far from the harmonics 60 dB down. The
//! trim is a guess at the output stage (TD122). The peaks are very narrow (at Q 60 a 60 Hz one rings for seconds)
//! and lift each other's harmonics through their skirts: at the defaults +0.9 dB at 60 Hz, up
//! to +16.0 dB at 420 Hz and +11.4 dB at 480 Hz.

use super::biquad::Coefficients;
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;
use std::f64::consts::PI;

// Parameter indices (the four floats, the switch, the count).
const FREQUENCY: usize = 0;
const Q: usize = 1;
const GAIN: usize = 2;
const SLOPE: usize = 3;
const HUM_ONLY: usize = 4;
const HARMONICS: usize = 5;

/// Sections per channel, always allocated (8 slots).
pub const MAX_HARMONICS: usize = 8;

/// A Frequency change smaller than this in log10 keeps the filters' state.
const KEEP_STATE: f64 = 0.3;

/// Output Hum Only's peaks (dB, every harmonic) and the trim after them (linear, -60 dB).
const HUM_PEAK_DB: f64 = 60.0;
const HUM_TRIM: f64 = 1e-3;

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} kHz", v / 1000.0)
    } else {
        format!("{v:.1} Hz")
    }
}

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0,
    max: 1.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

static PARAMS: [ParamSpec; 6] = [
    ParamSpec {
        name: "Frequency",
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        default: 60.0,
        log_slider: true,
        format: Some(hz),
        ..PARAM
    },
    ParamSpec {
        name: "Q",
        unit: "Q",
        min: 30.0,
        max: 3000.0,
        default: 60.0,
        log_slider: true,
        ..PARAM
    },
    // The range tops out at +5 dB (a boost) (TD122).
    ParamSpec {
        name: "Gain",
        unit: "dB",
        min: -80.0,
        max: 5.0,
        default: -60.0,
        ..PARAM
    },
    ParamSpec {
        name: "Harmonic Slope",
        unit: "%",
        min: 0.0,
        max: 100.0,
        default: 50.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Output Hum Only",
        steps: 1,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Number of Harmonics",
        min: 1.0,
        max: 8.0,
        default: 8.0,
        steps: 7,
        decimals: 0,
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "hum-remover",
    name: "Hum Remover",
    category: "Restoration",
    params: &PARAMS,
    make: |rate| Box::new(DeHummer::new(rate)),
    response: Some(response),
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The cut (dB) of harmonic `h` (1-based) of `n`: linear in dB from `gain` at the fundamental
/// to `gain`·(1 − `slope`/100) at the n-th, in f32.
fn harmonic_gain(gain: f32, slope: f32, n: usize, h: usize) -> f32 {
    let k = h - 1;
    if k == 0 || n == 1 {
        return gain;
    }
    let end = gain * (1.0 - slope / 100.0);
    gain + (k as f32 / (n - 1) as f32) * (end - gain)
}

/// The cookbook peaking EQ at `f` Hz: `g_db` at its centre, the width set by `q` at half the
/// gain in dB, unity far away.
fn peak(rate: f64, f: f64, g_db: f64, q: f64) -> Coefficients {
    let a = 10f64.powf(g_db / 40.0);
    let w = 2.0 * PI * f / rate;
    let alpha = w.sin() / (2.0 * q);
    let cos = w.cos();
    Coefficients::new(
        1.0 + alpha * a,
        -2.0 * cos,
        1.0 - alpha * a,
        1.0 + alpha / a,
        -2.0 * cos,
        1.0 - alpha / a,
    )
}

/// The harmonics' count from its plain value.
fn count(v: f64) -> usize {
    (v.round() as usize).clamp(1, MAX_HARMONICS)
}

/// The harmonic's frequency: an f32 product.
fn harmonic_hz(f0: f64, h: usize) -> f64 {
    f64::from(h as f32 * f0 as f32)
}

/// What the settings ask for: the sections that run (the harmonics below Nyquist, a prefix of
/// the N, since they rise; the +60 dB set for the hum alone) and the gain after them.
struct Design {
    sections: [Coefficients; MAX_HARMONICS],
    active: usize,
    /// 1, or Output Hum Only's trim.
    trim: f64,
}

impl Design {
    fn new(rate: f64, v: &[f64]) -> Self {
        let n = count(v[HARMONICS]);
        let (gain, slope) = (v[GAIN] as f32, v[SLOPE] as f32);
        let hum_only = v[HUM_ONLY] >= 0.5;
        let mut sections = [Coefficients::IDENTITY; MAX_HARMONICS];
        let mut active = 0;
        for h in 1..=n {
            let f = harmonic_hz(v[FREQUENCY], h);
            if f >= rate / 2.0 {
                break;
            }
            let g = if hum_only {
                HUM_PEAK_DB
            } else {
                f64::from(harmonic_gain(gain, slope, n, h))
            };
            sections[active] = peak(rate, f, g, v[Q]);
            active += 1;
        }
        Self {
            sections,
            active,
            trim: if hum_only { HUM_TRIM } else { 1.0 },
        }
    }
}

/// Whether plain `values` play the hum alone (Output Hum Only).
pub fn hum_only(values: &[f64]) -> bool {
    values[HUM_ONLY] >= 0.5
}

/// The harmonics that run for plain `values` at `rate`: their frequencies (Hz) and their
/// sections' gains (dB: the cuts, or Output Hum Only's +60 dB peaks), for the editor's curve
/// to mark.
pub fn harmonics(rate: f64, values: &[f64]) -> Vec<(f64, f64)> {
    let n = count(values[HARMONICS]);
    let hum_only = values[HUM_ONLY] >= 0.5;
    (1..=n)
        .map(|h| {
            let g = if hum_only {
                HUM_PEAK_DB
            } else {
                f64::from(harmonic_gain(
                    values[GAIN] as f32,
                    values[SLOPE] as f32,
                    n,
                    h,
                ))
            };
            (harmonic_hz(values[FREQUENCY], h), g)
        })
        .take_while(|(f, _)| *f < rate / 2.0)
        .collect()
}

/// The editor's curve: the chain's response in dB (with Output Hum Only, the +60 dB set's
/// less the trim).
fn response(rate: f64, v: &[f64], hz: f64) -> f64 {
    let d = Design::new(rate, v);
    let gain: f64 = d.sections[..d.active]
        .iter()
        .map(|c| c.magnitude(hz, rate))
        .product();
    20.0 * (gain * d.trim).max(1e-12).log10()
}

struct DeHummer {
    rate: f64,
    values: [f64; 6],
    design: Design,
    /// Per channel and section: the two state values of transposed direct form II.
    state: [[[f64; 2]; MAX_HARMONICS]; CHANNELS],
}

impl DeHummer {
    fn new(rate: f64) -> Self {
        let mut values = [0.0; 6];
        for (v, p) in values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        Self {
            rate,
            design: Design::new(rate, &values),
            values,
            state: [[[0.0; 2]; MAX_HARMONICS]; CHANNELS],
        }
    }

    fn clear(&mut self) {
        self.state = [[[0.0; 2]; MAX_HARMONICS]; CHANNELS];
    }
}

impl NativeEffect for DeHummer {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        let old = *v;
        *v = value;
        if index == FREQUENCY && (value.log10() - old.log10()).abs() >= KEEP_STATE {
            self.clear();
        }
        self.design = Design::new(self.rate, &self.values);
        // Sections that stop running lose their state, so one that comes back starts clean.
        for channel in &mut self.state {
            for z in &mut channel[self.design.active..] {
                *z = [0.0; 2];
            }
        }
    }

    fn start(&mut self) {
        self.clear();
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let sections = &self.design.sections[..self.design.active];
        let trim = self.design.trim;
        for (buf, state) in io.iter_mut().zip(self.state.iter_mut()) {
            for s in &mut buf[from..to] {
                let mut x = f64::from(*s);
                for (c, z) in sections.iter().zip(state.iter_mut()) {
                    let y = c.b0 * x + z[0];
                    z[0] = c.b1 * x - c.a1 * y + z[1];
                    z[1] = c.b2 * x - c.a2 * y;
                    x = y;
                }
                *s = (x * trim) as f32;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;

    const RATE: f64 = 48_000.0;

    fn settings(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    fn plain(pairs: &[(&str, f64)]) -> Vec<f64> {
        let mut v: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        for (name, value) in pairs {
            v[PARAMS.iter().position(|p| p.name == *name).unwrap()] = *value;
        }
        v
    }

    /// A sum of sines (Hz, dBFS, phase) at 48 kHz, `seconds` long.
    fn sines(parts: &[(f64, f64, f64)], seconds: usize) -> Vec<f32> {
        (0..RATE as usize * seconds)
            .map(|i| {
                let t = i as f64 / RATE;
                parts
                    .iter()
                    .map(|(hz, db, phase)| {
                        10f64.powf(db / 20.0) * (2.0 * PI * hz * t + phase).sin()
                    })
                    .sum::<f64>() as f32
            })
            .collect()
    }

    /// The level (dBFS, of a sine's peak) at `hz` over `x`, a whole number of its cycles: a
    /// single bin of the DFT, so the other components (whole cycles too) don't leak in.
    fn level(x: &[f32], hz: f64) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in x.iter().enumerate() {
            let w = 2.0 * PI * hz * i as f64 / RATE;
            re += f64::from(*s) * w.cos();
            im += f64::from(*s) * w.sin();
        }
        20.0 * (2.0 * (re * re + im * im).sqrt() / x.len() as f64).log10()
    }

    /// 60 Hz hum and its seven harmonics (-20 dBFS falling 3 dB a harmonic) under a 1 kHz
    /// tone at -12 dBFS, and `extra` components.
    fn hum_and_tone(extra: &[(f64, f64, f64)], seconds: usize) -> Vec<f32> {
        let mut parts: Vec<(f64, f64, f64)> = (1..=8)
            .map(|h| {
                (
                    60.0 * h as f64,
                    -20.0 - 3.0 * (h - 1) as f64,
                    0.7 * h as f64,
                )
            })
            .collect();
        parts.push((1000.0, -12.0, 0.0));
        parts.extend_from_slice(extra);
        sines(&parts, seconds)
    }

    /// The steady part: the last second of three, after the filters have settled.
    fn steady(x: &[f32]) -> &[f32] {
        &x[2 * RATE as usize..3 * RATE as usize]
    }

    #[test]
    fn each_harmonic_loses_its_cut_and_the_tone_keeps_its_level() {
        // At Q 300 the sections' skirts barely reach their neighbours (0.12 dB at most), so
        // each harmonic loses its own interpolated cut. A tone a third of an octave above the
        // fundamental (76 Hz, at -30 dBFS) and the 1 kHz tone keep their level.
        let input = hum_and_tone(&[(76.0, -30.0, 0.3)], 3);
        for q in [300.0, 60.0] {
            let s = settings(&[("Q", q)]);
            let v = plain(&[("Q", q)]);
            let out = render(&KIND, RATE, &s, std::slice::from_ref(&input), 512).unwrap();
            let (a, b) = (steady(&input), steady(&out[0]));
            let cuts = harmonics(RATE, &v);
            assert_eq!(cuts.len(), 8);
            for (h, &(f, g)) in cuts.iter().enumerate() {
                let measured = level(b, f) - level(a, f);
                let curve = response(RATE, &v, f);
                println!(
                    "Q {q}: harmonic {} ({f} Hz): cut {measured:.3} dB, interpolated {g:.3}, \
                     curve {curve:.3}",
                    h + 1
                );
                // The measurement is the design's curve, at any Q.
                assert!(
                    (measured - curve).abs() < 0.05,
                    "{f} Hz: {measured} vs {curve}"
                );
                if q == 300.0 {
                    assert!((measured - g).abs() < 0.5, "{f} Hz: {measured} vs {g}");
                }
            }
            for hz in [76.0, 1000.0] {
                let kept = level(b, hz) - level(a, hz);
                println!("Q {q}: the {hz} Hz tone: {kept:+.3} dB");
                assert!(
                    (kept - response(RATE, &v, hz)).abs() < 0.05,
                    "{hz} Hz: {kept}"
                );
                // At Q 60 the -60 dB cut's skirts are wide (the cookbook's Q is the width at
                // half the gain in dB, -30 here): 76 Hz loses about 4 dB.
                if q == 300.0 || hz == 1000.0 {
                    assert!(kept.abs() < 0.5, "Q {q}, {hz} Hz: {kept}");
                }
            }
        }
    }

    #[test]
    fn the_slope_and_the_harmonic_count_set_the_cuts() {
        // The defaults: -60 at the fundamental to -30 at the 8th, in equal steps.
        let cuts: Vec<f64> = harmonics(RATE, &plain(&[])).iter().map(|c| c.1).collect();
        let expected = [
            -60.0, -55.714, -51.429, -47.143, -42.857, -38.571, -34.286, -30.0,
        ];
        for (c, e) in cuts.iter().zip(expected) {
            assert!((c - e).abs() < 1e-3, "{cuts:?}");
        }
        // Each section has exactly its cut at its centre.
        let v = plain(&[("Q", 300.0)]);
        let d = Design::new(RATE, &v);
        for (s, (f, g)) in d.sections.iter().zip(harmonics(RATE, &v)) {
            assert!((20.0 * s.magnitude(f, RATE).log10() - g).abs() < 1e-6);
        }
        // Slope 0: all alike; 100 %: the top one untouched.
        let flat = harmonics(RATE, &plain(&[("Harmonic Slope", 0.0)]));
        assert!(flat.iter().all(|c| c.1 == -60.0), "{flat:?}");
        let full = harmonics(RATE, &plain(&[("Harmonic Slope", 100.0)]));
        assert_eq!(full[7].1, 0.0);
        // Four harmonics at 100 %, measured: -60, -40, -20, 0; the fifth (300 Hz) and above
        // are left alone.
        let s = settings(&[
            ("Q", 300.0),
            ("Harmonic Slope", 100.0),
            ("Number of Harmonics", 4.0),
        ]);
        let input = hum_and_tone(&[], 3);
        let out = render(&KIND, RATE, &s, std::slice::from_ref(&input), 512).unwrap();
        let (a, b) = (steady(&input), steady(&out[0]));
        for (h, want) in [(1, -60.0), (2, -40.0), (3, -20.0), (4, 0.0)] {
            let f = 60.0 * f64::from(h);
            let cut = level(b, f) - level(a, f);
            println!("slope 100 %, 4 harmonics: {f} Hz cut {cut:.3} dB (want {want})");
            assert!((cut - want).abs() < 0.5, "{f} Hz: {cut}");
        }
        for h in 5..=8 {
            let f = 60.0 * f64::from(h);
            let cut = level(b, f) - level(a, f);
            assert!(cut.abs() < 0.1, "{f} Hz: {cut}");
        }
    }

    #[test]
    fn harmonics_at_or_past_nyquist_are_skipped() {
        let rate = 44_100.0;
        // 5 kHz: 5, 10, 15 and 20 kHz run; 25 kHz and up don't exist at 44.1 kHz.
        let v = plain(&[("Frequency", 5000.0)]);
        assert_eq!(harmonics(rate, &v).len(), 4);
        assert_eq!(Design::new(rate, &v).active, 4);
        // 2756.25 Hz: its 8th harmonic is exactly Nyquist, left out.
        let v = plain(&[("Frequency", 2756.25)]);
        assert_eq!(Design::new(rate, &v).active, 7);
        // Noise through both: finite, and no louder than it came in.
        let noise: Vec<f32> = (0..44_100u32)
            .map(|i| ((f64::from(i) * 12.9898).sin() * 43_758.547).fract() as f32 * 0.5)
            .collect();
        for f in [5000.0, 2756.25, 20_000.0] {
            let out = render(
                &KIND,
                rate,
                &settings(&[("Frequency", f)]),
                std::slice::from_ref(&noise),
                512,
            )
            .unwrap();
            let peak = out[0].iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak.is_finite() && peak <= 1.0, "{f} Hz: peak {peak}");
        }
    }

    #[test]
    fn output_hum_only_is_the_plus_60_set_trimmed_by_60_db() {
        // At the defaults (60 Hz, Q 60) the curve: each harmonic is the +60 dB peaks' product
        // there less 60 dB, its own peak's +60 cancelled by the trim and its neighbours'
        // skirts lifting it; the 1 kHz tone at least 50 dB down. (A 60 Hz peak at Q 60 rings
        // for seconds, so the renders below use a faster setting.)
        let v = plain(&[("Output Hum Only", 1.0)]);
        let peaks: Vec<Coefficients> = (1..=8)
            .map(|h| peak(RATE, 60.0 * h as f64, 60.0, 60.0))
            .collect();
        for (f, g) in harmonics(RATE, &v) {
            assert_eq!(g, 60.0);
            let expected: f64 = peaks
                .iter()
                .map(|c| 20.0 * c.magnitude(f, RATE).log10())
                .sum::<f64>()
                - 60.0;
            let curve = response(RATE, &v, f);
            println!("hum only, defaults: {f} Hz at {curve:+.3} dB");
            assert!(
                (curve - expected).abs() < 1e-9,
                "{f} Hz: {curve} vs {expected}"
            );
            assert!((0.0..20.0).contains(&curve), "{f} Hz: {curve}");
        }
        let tone = response(RATE, &v, 1000.0);
        println!("hum only, defaults: the 1 kHz tone at {tone:.2} dB");
        assert!(tone < -50.0, "{tone}");
        // Measured: hum at 1 kHz and its harmonics (-20 dBFS falling 3 dB a harmonic) under a
        // 1250 Hz tone at -12 dBFS, Q 30, settled for 9 s: each harmonic and the tone come out
        // as the curve says.
        let mut parts: Vec<(f64, f64, f64)> = (1..=8)
            .map(|h| {
                (
                    1000.0 * h as f64,
                    -20.0 - 3.0 * (h - 1) as f64,
                    0.7 * h as f64,
                )
            })
            .collect();
        parts.push((1250.0, -12.0, 0.0));
        let input = sines(&parts, 10);
        let pairs = [("Output Hum Only", 1.0), ("Frequency", 1000.0), ("Q", 30.0)];
        let out = render(
            &KIND,
            RATE,
            &settings(&pairs),
            std::slice::from_ref(&input),
            512,
        )
        .unwrap();
        let v = plain(&pairs);
        let last = 9 * RATE as usize..10 * RATE as usize;
        let (a, b) = (&input[last.clone()], &out[0][last]);
        for (f, _) in harmonics(RATE, &v) {
            let measured = level(b, f) - level(a, f);
            let curve = response(RATE, &v, f);
            println!("hum only, 1 kHz, Q 30: {f} Hz at {measured:+.3} dB (curve {curve:+.3})");
            assert!(
                (measured - curve).abs() < 0.05,
                "{f} Hz: {measured} vs {curve}"
            );
        }
        let tone = level(b, 1250.0) - level(a, 1250.0);
        let curve = response(RATE, &v, 1250.0);
        println!("hum only, 1 kHz, Q 30: the 1250 Hz tone at {tone:.2} dB (curve {curve:.2})");
        assert!((tone - curve).abs() < 0.1, "{tone} vs {curve}");
        assert!(tone < -30.0, "{tone}");
    }

    #[test]
    fn the_same_at_any_block_size() {
        let left = hum_and_tone(&[], 1);
        let right: Vec<f32> = left.iter().rev().copied().collect();
        let audio = [left, right];
        for s in [
            settings(&[]),
            settings(&[("Output Hum Only", 1.0), ("Q", 3000.0)]),
        ] {
            let a = render(&KIND, RATE, &s, &audio, 512).unwrap();
            for block in [77, 1, 4096] {
                assert_eq!(a, render(&KIND, RATE, &s, &audio, block).unwrap());
            }
            // The channels are filtered independently.
            assert_ne!(a[0], a[1]);
        }
    }

    #[test]
    fn a_frequency_jump_of_an_octave_starts_the_filters_afresh() {
        let mut d = DeHummer::new(RATE);
        let mut io = vec![hum_and_tone(&[], 1); CHANNELS];
        d.process(&mut io, 0, 4800);
        let busy = |d: &DeHummer| d.state[0].iter().any(|z| z[0] != 0.0);
        assert!(busy(&d));
        // 60 to 100 Hz (0.22 in log10): kept. 100 to 200 Hz (0.30): afresh.
        d.set(FREQUENCY, 100.0);
        assert!(busy(&d));
        d.set(FREQUENCY, 200.0);
        assert!(!busy(&d));
        d.process(&mut io, 4800, 9600);
        assert!(busy(&d));
        // Fewer harmonics: the ones dropped are cleared.
        d.set(HARMONICS, 2.0);
        assert!(d.state[0][2..].iter().all(|z| *z == [0.0; 2]));
        // Output Hum Only swaps in its own sections and keeps the state; a transport start
        // clears them all, in either mode.
        d.set(HARMONICS, 8.0);
        d.process(&mut io, 9600, 14_400);
        d.set(HUM_ONLY, 1.0);
        assert!(busy(&d));
        d.process(&mut io, 14_400, 19_200);
        d.start();
        assert!(!busy(&d));
    }

    #[test]
    fn the_defaults_remove_60_hz_and_its_harmonics() {
        let defaults: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        assert_eq!(defaults, [60.0, 60.0, -60.0, 50.0, 0.0, 8.0]);
        for (p, v) in PARAMS.iter().zip(&defaults) {
            assert!((p.min..=p.max).contains(v), "{} {v}", p.name);
            assert!((p.plain(p.normalized(*v)) - v).abs() < 1e-9, "{}", p.name);
        }
        assert_eq!(PARAMS[GAIN].display(-60.0), "-60.0 dB");
        assert_eq!(PARAMS[FREQUENCY].display(60.0), "60.0 Hz");
    }
}
