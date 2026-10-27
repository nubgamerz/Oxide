//! Hiss Reduction: a spectral downward expander against a threshold curve, on the same [`OverlapSave`] frames as Noise
//! Reduction.
//!
//! - Each frame, per channel and bin (from bin 3: the lowest three pass untouched): the
//!   smoothed magnitude (Noise Reduction's smoother, a slightly slower attack) against the
//!   threshold T. At or above T the bin passes; below T·10^(−Transition Width/20) it drops to
//!   the floor 10^(−Reduce By/40) (half the dB in amplitude); between, the gain
//!   rises in a straight line.
//! - The threshold: without a captured noise floor, a flat curve, Noise Floor − 70 dB
//!   in its magnitude scale. With one ("Capture noise print" from a selection of hiss), each
//!   bin's captured floor raised by Noise Floor dB, kept bin by bin (TD105).
//! - The window is Noise Reduction's (TD105). Settings take
//!   effect from the next frame; the delay is the FFT size.

use super::spectral::{self, magnitude, OverlapSave, Print, MAX_BITS, MAX_SIZE, MIN_BITS, PAD};
use super::{Kind, NativeEffect, ParamSpec};
use anyhow::{bail, Result};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

const NOISE_FLOOR: usize = 0;
const REDUCE_BY: usize = 1;
const DECAY: usize = 2;
const PRECISION: usize = 3;
const TRANSITION: usize = 4;
const FFT_SIZE: usize = 5;
const HISS_ONLY: usize = 6;

/// Bins below this are never touched.
const FIRST_BIN: usize = 3;
/// The smoother's attack: the new blend's weight when the level rises.
const ATTACK: f64 = 0.92;

const fn param(
    name: &'static str,
    unit: &'static str,
    min: f64,
    max: f64,
    default: f64,
) -> ParamSpec {
    ParamSpec {
        name,
        unit,
        min,
        max,
        default,
        steps: 0,
        decimals: 1,
        automatable: true,
        link: None,
        log_slider: false,
        format: None,
    }
}

static PARAMS: [ParamSpec; 7] = [
    param("Noise Floor", "dB", -40.0, 40.0, 8.0),
    ParamSpec {
        log_slider: true,
        ..param("Reduce By", "dB", 1.0, 100.0, 24.0)
    },
    ParamSpec {
        decimals: 0,
        ..param("Spectral Decay Rate", "%", 0.0, 100.0, 65.0)
    },
    ParamSpec {
        steps: 17,
        decimals: 0,
        automatable: false,
        ..param("Precision Factor", "", 3.0, 20.0, 5.0)
    },
    param("Transition Width", "dB", 0.5, 100.0, 7.0),
    ParamSpec {
        steps: MAX_BITS - MIN_BITS,
        decimals: 0,
        automatable: false,
        format: Some(spectral::format_size),
        ..param("FFT Size", "", MIN_BITS as f64, MAX_BITS as f64, 12.0)
    },
    ParamSpec {
        steps: 1,
        decimals: 0,
        automatable: false,
        ..param("Output Hiss Only", "", 0.0, 1.0, 0.0)
    },
];

pub static KIND: Kind = Kind {
    id: "hiss-reduction",
    name: "Hiss Reduction",
    category: "Restoration",
    params: &PARAMS,
    make: |rate| Box::new(HissReduction::new(rate)),
    response: None,
    transfer: None,
    latency: Some(|_, v| size_of(v[FFT_SIZE])),
    capture: Some(capture),
    learns: None,
    reads: None,
    prepares: None,
};

fn size_of(bits: f64) -> usize {
    1 << (bits.round() as u32).clamp(MIN_BITS, MAX_BITS)
}

/// The hiss's floor in each bin, as Capture Noise Floor measures it: consecutive
/// frames across the selection (every 2⌊N/P⌋), each bin's magnitude through the same smoother
/// as processing, and the highest it reached. One statistic.
pub fn capture(rate: f64, values: &[f64], audio: &[Vec<f32>]) -> Result<Vec<f32>> {
    let n = size_of(values[FFT_SIZE]);
    let len = audio.first().map_or(0, Vec::len);
    if len < n {
        bail!("select more hiss: the floor needs at least {n} samples at this FFT size, not {len}");
    }
    let hop = 2 * (n / (values[PRECISION].round() as usize).max(3));
    let decay = values[DECAY] / 100.0;
    let fft = FftPlanner::new().plan_fft_forward(n);
    let bins = n / 2 + 1;
    let mut work = Vec::with_capacity(n);
    let mut spectra = vec![Vec::with_capacity(bins); audio.len()];
    let mut smooth = vec![vec![0.0f64; bins + 2 * PAD]; audio.len()];
    let mut floor = vec![vec![0.0f64; bins]; audio.len()];
    let mut frames = 0;
    let mut at = 0;
    while at + n <= len {
        spectral::spectrum_at(audio, at, n, &fft, &mut work, &mut spectra);
        for (c, spectrum) in spectra.iter().enumerate() {
            for (k, x) in spectrum.iter().enumerate() {
                let a = magnitude(*x, n);
                let level = if frames == 0 {
                    smooth[c][k + PAD] = a;
                    a
                } else {
                    spectral::smooth(&mut smooth[c], k, a, decay, ATTACK)
                };
                floor[c][k] = floor[c][k].max(level);
            }
        }
        frames += 1;
        at += hop;
    }
    let stats: Vec<Vec<Vec<f64>>> = floor.into_iter().map(|f| vec![f]).collect();
    Ok(Print::write(rate, n, frames, &stats))
}

struct Channel {
    /// The captured floor at this FFT size and rate.
    floor: Vec<f64>,
    smooth: Vec<f64>,
    first: bool,
}

struct Expander {
    rate: f64,
    values: [f64; 7],
    print: Vec<f32>,
    stale: bool,
    has_print: bool,
    size: usize,
    channels: [Channel; 2],
}

impl Expander {
    fn rebuild(&mut self) {
        self.stale = false;
        let Some(print) = Print::read(&self.print) else {
            self.has_print = false;
            return;
        };
        self.has_print = true;
        let (n, rate) = (self.size, self.rate);
        for (c, ch) in self.channels.iter_mut().enumerate() {
            for k in 0..=n / 2 {
                ch.floor[k] = print.for_bin(c, 0, k, n, rate);
            }
        }
    }

    fn restart(&mut self) {
        for ch in &mut self.channels {
            ch.smooth.fill(0.0);
            ch.first = true;
        }
    }

    fn frame(&mut self, frame: spectral::Frame<'_>) {
        if self.stale {
            self.rebuild();
        }
        let v = self.values;
        let raise = 10f64.powf(v[NOISE_FLOOR] / 20.0);
        let flat = 10f64.powf((v[NOISE_FLOOR] - 70.0) / 20.0);
        let width = 10f64.powf(-v[TRANSITION] / 20.0);
        let floor_gain = 10f64.powf(-v[REDUCE_BY] / 40.0);
        let decay = v[DECAY] / 100.0;
        let hiss_only = v[HISS_ONLY] >= 0.5;
        let n = self.size;
        let has_print = self.has_print;
        for (ch, bins) in self.channels.iter_mut().zip([frame.left, frame.right]) {
            let first = ch.first || decay <= 0.0;
            ch.first = false;
            for (k, bin) in bins.iter_mut().enumerate().skip(FIRST_BIN) {
                let a = magnitude(*bin, n);
                let level = if first {
                    ch.smooth[k + PAD] = a;
                    a
                } else {
                    spectral::smooth(&mut ch.smooth, k, a, decay, ATTACK)
                };
                let hi = if has_print { ch.floor[k] * raise } else { flat };
                let lo = hi * width;
                let gain = if level >= hi {
                    1.0
                } else if level >= lo {
                    floor_gain + (1.0 - floor_gain) * (level - lo) / (hi - lo)
                } else {
                    floor_gain
                };
                *bin *= if hiss_only { 1.0 - gain } else { gain };
            }
            if hiss_only {
                for bin in &mut bins[..FIRST_BIN] {
                    *bin = Complex::new(0.0, 0.0);
                }
            }
        }
    }
}

struct HissReduction {
    frames: OverlapSave,
    expander: Expander,
}

impl HissReduction {
    fn new(rate: f64) -> Self {
        let values: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let size = size_of(values[FFT_SIZE]);
        let channel = || Channel {
            floor: vec![0.0; MAX_SIZE / 2 + 1],
            smooth: vec![0.0; MAX_SIZE / 2 + 1 + 2 * PAD],
            first: true,
        };
        Self {
            frames: OverlapSave::new(size, values[PRECISION] as usize),
            expander: Expander {
                rate,
                values: values.try_into().expect("seven parameters"),
                print: Vec::new(),
                stale: false,
                has_print: false,
                size,
                channels: [channel(), channel()],
            },
        }
    }
}

impl NativeEffect for HissReduction {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.expander.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        if index == FFT_SIZE || index == PRECISION {
            let v = &self.expander.values;
            self.frames
                .resize(size_of(v[FFT_SIZE]), v[PRECISION].round() as usize);
            self.expander.size = self.frames.size();
            self.expander.stale = true;
            self.expander.restart();
        }
    }

    fn ramp(&mut self, index: usize, value: f64, _samples: usize) {
        self.set(index, value);
    }

    fn start(&mut self) {
        self.frames.reset();
        self.expander.restart();
    }

    fn latency(&self) -> usize {
        self.frames.size()
    }

    fn take_print(&mut self, print: &mut Vec<f32>) {
        std::mem::swap(&mut self.expander.print, print);
        self.expander.stale = true;
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let [left, right] = io else {
            return;
        };
        let expander = &mut self.expander;
        self.frames
            .process(left, right, from, to, &mut |frame| expander.frame(frame));
    }
}

#[cfg(test)]
mod tests {
    use super::super::render_with_print;
    use super::*;
    use std::collections::BTreeMap;

    const RATE: f64 = 48_000.0;

    /// Hiss (white noise, uniform ±`peak`) with a 1 kHz sine at −12 dBFS from `tone_from`.
    fn hissy_tone(n: usize, tone_from: usize, peak: f64) -> Vec<f32> {
        let mut x = 0x2545_F491_4F6C_DD1Du64;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let hiss = ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * peak;
                let tone = if i >= tone_from {
                    0.25 * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / RATE).sin()
                } else {
                    0.0
                };
                (hiss + tone) as f32
            })
            .collect()
    }

    fn rms_db(x: &[f32]) -> f64 {
        let p = x.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / x.len() as f64;
        10.0 * p.max(1e-30).log10()
    }

    fn run(settings: &[(&str, f64)], print: bool, block: usize) -> (Vec<f32>, Vec<f32>) {
        run_at(0.01, settings, print, block)
    }

    fn run_at(
        peak: f64,
        settings: &[(&str, f64)],
        print: bool,
        block: usize,
    ) -> (Vec<f32>, Vec<f32>) {
        let n = 4 * 48_000;
        let signal = vec![hissy_tone(n, 2 * 48_000, peak)];
        let settings: BTreeMap<String, f64> =
            settings.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let floor: Vec<Vec<f32>> = vec![signal[0][..72_000].to_vec()];
        let out = render_with_print(
            &KIND,
            RATE,
            &settings,
            print.then_some(&floor[..]),
            &signal,
            block,
        )
        .unwrap();
        (signal[0].clone(), out[0].clone())
    }

    #[test]
    fn hiss_under_the_floor_drops_by_half_the_setting_and_a_tone_passes() {
        for reduce_by in [24.0, 40.0] {
            let (dry, wet) = run(&[("Reduce By", reduce_by)], true, 512);
            let hiss = 10_000..90_000;
            let cut = rms_db(&dry[hiss.clone()]) - rms_db(&wet[hiss]);
            // Under the floor (raised 8 dB) all of it sits at 10^(−dB/40): half the setting.
            assert!(
                (cut - reduce_by / 2.0).abs() < 1.0,
                "Reduce By {reduce_by}: hiss down {cut:.2} dB"
            );
            // The tone, far above the floor, passes; the hiss around it still drops.
            let tone = 120_000..180_000;
            let kept = rms_db(&wet[tone.clone()]) - rms_db(&dry[tone]);
            assert!(kept.abs() < 0.2, "tone moved {kept:.2} dB");
        }
    }

    #[test]
    fn without_a_capture_the_flat_curve_takes_quiet_hiss_and_leaves_loud() {
        // The flat threshold is 8 − 70 = −62 dB in the magnitude scale. White hiss at −45 dBFS
        // RMS reads about −75 dB a bin at 4096 points: under it, down 12 dB. At −15 dBFS, −45
        // dB a bin: over it, untouched.
        let hiss = 10_000..90_000;
        let cut = |peak| {
            let (dry, wet) = run_at(peak, &[], false, 512);
            rms_db(&dry[hiss.clone()]) - rms_db(&wet[hiss.clone()])
        };
        let quiet = cut(0.01);
        assert!((quiet - 12.0).abs() < 0.5, "quiet hiss down {quiet:.2} dB");
        let loud = cut(0.3);
        assert!(loud.abs() < 0.3, "loud hiss down {loud:.2} dB");
    }

    #[test]
    fn the_same_at_any_block_size() {
        assert_eq!(run(&[], true, 512), run(&[], true, 333));
        assert_eq!(run(&[], true, 512).1, run(&[], true, 64).1);
    }
}
