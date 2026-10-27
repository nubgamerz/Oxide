//! Noise Reduction: magnitude spectral subtraction against a captured noise
//! print, on [`OverlapSave`] frames.
//!
//! - The print ([`capture`]) holds each bin's RMS, minimum and maximum magnitude over frames of
//!   the noise alone, spaced at least N/3 apart, up to Noise Print Snapshots of them.
//! - Each frame, per channel and bin: a threshold between the print's minimum and maximum (set
//!   by Noise Reduction %), a transition band below it (Smoothing, Transition Width), the
//!   magnitude smoothed across neighbouring bins and over time (Spectral Decay Rate, a fast
//!   attack), and the gain 1 − min(V/E, R'): the threshold subtracted from what is above it,
//!   everything below cut by Reduce By (deeper for steady noise).
//! - Settings take effect from the next frame (every 2⌊N/P⌋ samples); FFT Size and Precision
//!   Factor restart the frames. The delay is the FFT size, compensated like any plug-in's.
//! - Without a print it passes the audio (delayed), as there is nothing to reduce.
//! - It is an insert like the others, not only a process applied to a selection, so the Waveform view's rack previews exactly what Apply writes.
//! - Not yet (TD105): the editable graph (the threshold's curve over frequency: flat here).

use super::spectral::{self, magnitude, OverlapSave, Print, MAX_BITS, MAX_SIZE, MIN_BITS, PAD};
use super::{Kind, NativeEffect, ParamSpec};
use anyhow::{bail, Result};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;

const REDUCTION: usize = 0;
const REDUCE_BY: usize = 1;
const DECAY: usize = 2;
const SMOOTHING: usize = 3;
const PRECISION: usize = 4;
const TRANSITION: usize = 5;
const FFT_SIZE: usize = 6;
const SNAPSHOTS: usize = 7;
const NOISE_ONLY: usize = 8;

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
        decimals: 0,
        automatable: true,
        link: None,
        log_slider: false,
        format: None,
    }
}

static PARAMS: [ParamSpec; 9] = [
    param("Noise Reduction", "%", 0.0, 100.0, 100.0),
    ParamSpec {
        decimals: 1,
        log_slider: true,
        ..param("Reduce By", "dB", 1.0, 100.0, 40.0)
    },
    param("Spectral Decay Rate", "%", 0.0, 100.0, 65.0),
    ParamSpec {
        decimals: 2,
        ..param("Smoothing", "", 0.0, 10.0, 1.0)
    },
    ParamSpec {
        steps: 17,
        automatable: false,
        ..param("Precision Factor", "", 3.0, 20.0, 7.0)
    },
    ParamSpec {
        decimals: 1,
        ..param("Transition Width", "dB", 0.0, 100.0, 0.0)
    },
    ParamSpec {
        steps: MAX_BITS - MIN_BITS,
        automatable: false,
        format: Some(spectral::format_size),
        ..param("FFT Size", "", MIN_BITS as f64, MAX_BITS as f64, 12.0)
    },
    ParamSpec {
        automatable: false,
        ..param("Noise Print Snapshots", "", 8.0, 24_000.0, 4000.0)
    },
    ParamSpec {
        steps: 1,
        automatable: false,
        ..param("Output Noise Only", "", 0.0, 1.0, 0.0)
    },
];

pub static KIND: Kind = Kind {
    id: "noise-reduction",
    name: "Noise Reduction",
    category: "Restoration",
    params: &PARAMS,
    make: |rate| Box::new(NoiseReduction::new(rate)),
    response: None,
    transfer: None,
    latency: Some(|_, v| size_of(v[FFT_SIZE])),
    capture: Some(capture),
    learns: None,
    reads: None,
    prepares: None,
};

/// The FFT size a setting names.
fn size_of(bits: f64) -> usize {
    1 << (bits.round() as u32).clamp(MIN_BITS, MAX_BITS)
}

/// Each bin's RMS, minimum and maximum magnitude ([`magnitude`]'s scale) over the noise in
/// `audio`, as Capture Noise Print measures them: frames of the FFT size at least
/// N/3 apart across the selection, at most Noise Print Snapshots of them (8 at least), evenly
/// spread. Statistics in that order (the RMS first, for the editor).
pub fn capture(rate: f64, values: &[f64], audio: &[Vec<f32>]) -> Result<Vec<f32>> {
    let n = size_of(values[FFT_SIZE]);
    let len = audio.first().map_or(0, Vec::len);
    if len <= n + 3 {
        bail!(
            "select more noise: the print needs more than {} samples at this FFT size, not {len}",
            n + 3
        );
    }
    let usable = len - n;
    let snapshots = (values[SNAPSHOTS].round() as usize).max(8);
    let frames = ((usable - 1) / (n / 3)).max(1).min(snapshots);
    let hop = usable / frames;
    if hop < 3 {
        bail!("select more noise: frames would be {hop} samples apart");
    }
    let fft = FftPlanner::new().plan_fft_forward(n);
    let bins = n / 2 + 1;
    let mut work = Vec::with_capacity(n);
    let mut spectra = vec![Vec::with_capacity(bins); audio.len()];
    // Per channel: Σ|X|², min and max of the magnitude.
    let mut power = vec![vec![0.0f64; bins]; audio.len()];
    let mut low = vec![vec![f64::from(f32::MAX); bins]; audio.len()];
    let mut high = vec![vec![0.0f64; bins]; audio.len()];
    for j in 0..frames {
        spectral::spectrum_at(audio, j * hop, n, &fft, &mut work, &mut spectra);
        for (c, spectrum) in spectra.iter().enumerate() {
            for (k, x) in spectrum.iter().enumerate() {
                let a = magnitude(*x, n);
                power[c][k] += x.norm_sqr();
                low[c][k] = low[c][k].min(a);
                high[c][k] = high[c][k].max(a);
            }
        }
    }
    let stats: Vec<Vec<Vec<f64>>> = (0..audio.len())
        .map(|c| {
            let rms = power[c]
                .iter()
                .map(|p| (p / frames as f64).sqrt() * 4.0 / n as f64)
                .collect();
            vec![rms, low[c].clone(), high[c].clone()]
        })
        .collect();
    Ok(Print::write(rate, n, frames, &stats))
}

/// One channel's state: the print for this FFT size and rate, and the magnitude smoother.
struct Channel {
    rms: Vec<f64>,
    min: Vec<f64>,
    max: Vec<f64>,
    smooth: Vec<f64>,
    first: bool,
}

impl Channel {
    fn new() -> Self {
        let bins = MAX_SIZE / 2 + 1;
        Self {
            rms: vec![0.0; bins],
            min: vec![0.0; bins],
            max: vec![0.0; bins],
            smooth: vec![0.0; bins + 2 * PAD],
            first: true,
        }
    }
}

/// The spectral part: what a frame does to each bin.
struct Reducer {
    rate: f64,
    values: [f64; 9],
    /// The print as captured, and whether its per-bin values need reading again (a new print,
    /// FFT size or rate).
    print: Vec<f32>,
    stale: bool,
    has_print: bool,
    size: usize,
    channels: [Channel; 2],
}

impl Reducer {
    /// The print's values for this FFT size and rate. No allocation.
    fn rebuild(&mut self) {
        self.stale = false;
        let Some(print) = Print::read(&self.print) else {
            self.has_print = false;
            return;
        };
        self.has_print = print.stats >= 3;
        let (n, rate) = (self.size, self.rate);
        for (c, ch) in self.channels.iter_mut().enumerate() {
            for k in 0..=n / 2 {
                ch.rms[k] = print.for_bin(c, 0, k, n, rate);
                ch.min[k] = print.for_bin(c, 1, k, n, rate);
                ch.max[k] = print.for_bin(c, 2, k, n, rate);
            }
        }
    }

    /// Start the smoothers again (a new start or new frames).
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
        if !self.has_print {
            return;
        }
        let v = self.values;
        let t = v[REDUCTION] / 100.0;
        let decay = v[DECAY] / 100.0;
        let e = v[SMOOTHING];
        let width = 10f64.powf(v[TRANSITION] / 20.0);
        let r = 1.0 - 10f64.powf(-v[REDUCE_BY] / 20.0);
        let noise_only = v[NOISE_ONLY] >= 0.5;
        let n = self.size;
        for (ch, bins) in self.channels.iter_mut().zip([frame.left, frame.right]) {
            let first = ch.first || decay <= 0.0;
            ch.first = false;
            for (k, bin) in bins.iter_mut().enumerate() {
                let (m, big, rms) = (ch.min[k], ch.max[k], ch.rms[k]);
                // The threshold: a quadratic through the minimum (0 %) and the maximum (100 %).
                let threshold = (m
                    + t * (2.0 * big + 2.0 * m - 4.0 * rms)
                    + t * t.abs() * (4.0 * rms - big - 3.0 * m))
                    .max(0.0);
                let phi = if e != 0.0 && big != 0.0 {
                    width * (rms / big).powf(e)
                } else {
                    width
                };
                let (lo, hi) = if phi < 1.0 {
                    (threshold * phi, threshold)
                } else {
                    (threshold, threshold * phi)
                };
                let a = magnitude(*bin, n);
                let level = if first {
                    ch.smooth[k + PAD] = a;
                    a
                } else {
                    spectral::smooth(&mut ch.smooth, k, a, decay, 0.93)
                };
                if level == 0.0 {
                    if noise_only {
                        *bin = Complex::new(0.0, 0.0);
                    }
                    continue;
                }
                let knee = if level < lo {
                    level
                } else if level < hi {
                    level + (level - lo) / (hi - lo) * (hi - level)
                } else {
                    hi
                };
                let limit = if m != 0.0 && big != 0.0 {
                    r + 5.0 * (m / big) * (1.0 - r)
                } else {
                    r
                };
                let gain = 1.0 - (knee / level).min(limit);
                *bin *= if noise_only { 1.0 - gain } else { gain };
            }
        }
    }
}

struct NoiseReduction {
    frames: OverlapSave,
    reducer: Reducer,
}

impl NoiseReduction {
    fn new(rate: f64) -> Self {
        let values: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let size = size_of(values[FFT_SIZE]);
        Self {
            frames: OverlapSave::new(size, values[PRECISION] as usize),
            reducer: Reducer {
                rate,
                values: values.try_into().expect("nine parameters"),
                print: Vec::new(),
                stale: false,
                has_print: false,
                size,
                channels: [Channel::new(), Channel::new()],
            },
        }
    }

    /// New frames for the FFT size and precision set: they start afresh.
    fn reframe(&mut self) {
        let v = &self.reducer.values;
        let size = size_of(v[FFT_SIZE]);
        let precision = v[PRECISION].round() as usize;
        self.frames.resize(size, precision);
        self.reducer.size = self.frames.size();
        self.reducer.stale = true;
        self.reducer.restart();
    }
}

impl NativeEffect for NoiseReduction {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.reducer.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        if index == FFT_SIZE || index == PRECISION {
            self.reframe();
        }
    }

    /// Automation lands at the next frame, as a setting does.
    fn ramp(&mut self, index: usize, value: f64, _samples: usize) {
        self.set(index, value);
    }

    fn start(&mut self) {
        self.frames.reset();
        self.reducer.restart();
    }

    fn latency(&self) -> usize {
        self.frames.size()
    }

    fn take_print(&mut self, print: &mut Vec<f32>) {
        std::mem::swap(&mut self.reducer.print, print);
        self.reducer.stale = true;
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let [left, right] = io else {
            return;
        };
        let reducer = &mut self.reducer;
        self.frames
            .process(left, right, from, to, &mut |frame| reducer.frame(frame));
    }
}

#[cfg(test)]
mod tests {
    use super::super::{render_with_print, Native};
    use super::*;
    use std::collections::BTreeMap;

    const RATE: f64 = 48_000.0;

    /// White noise at about −35 dBFS RMS (a fixed seed), with a 1 kHz sine at −12 dBFS over it
    /// from `tone_from` on.
    pub(crate) fn noisy_tone(n: usize, tone_from: usize, seed: u64) -> Vec<f32> {
        let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let hiss = ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 0.031;
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

    /// The level of the 1 kHz component (a single-bin DFT over whole cycles).
    fn tone_db(x: &[f32]) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in x.iter().enumerate() {
            let p = 2.0 * std::f64::consts::PI * 1000.0 * i as f64 / RATE;
            re += f64::from(*s) * p.cos();
            im += f64::from(*s) * p.sin();
        }
        20.0 * ((re * re + im * im).sqrt() * 2.0 / x.len() as f64).log10()
    }

    fn reduce(reduce_by: f64, block: usize) -> (Vec<f32>, Vec<f32>) {
        let n = 6 * 48_000;
        let signal = vec![noisy_tone(n, 2 * 48_000, 3), noisy_tone(n, 2 * 48_000, 4)];
        let print: Vec<Vec<f32>> = signal.iter().map(|c| c[..72_000].to_vec()).collect();
        let settings: BTreeMap<String, f64> = [("Reduce By".to_string(), reduce_by)].into();
        let out = render_with_print(&KIND, RATE, &settings, Some(&print), &signal, block).unwrap();
        (signal[0].clone(), out[0].clone())
    }

    #[test]
    fn a_print_reduces_steady_noise_by_its_setting_and_keeps_a_tone() {
        for reduce_by in [6.0, 12.0, 24.0, 40.0] {
            let (dry, wet) = reduce(reduce_by, 512);

            // Noise alone, past the first frames: down by the setting, and a few dB more, as
            // steady noise (its minimum not far below its maximum) deepens the cut: the
            // gain floor is (1 − R)(1 − 5·min/max), about 3 dB lower for white noise.
            let noise = &(10_000..90_000);
            let cut = rms_db(&dry[noise.clone()]) - rms_db(&wet[noise.clone()]);
            assert!(
                cut >= reduce_by && cut <= reduce_by + 3.5,
                "Reduce By {reduce_by}: noise down {cut:.2} dB"
            );
            // The tone keeps its level (whole cycles of 1 kHz at 48 kHz: 48 samples).
            let tone = &(144_000..240_000);
            let kept = tone_db(&wet[tone.clone()]) - tone_db(&dry[tone.clone()]);
            assert!(
                kept.abs() < 0.5,
                "Reduce By {reduce_by}: tone moved {kept:.2} dB"
            );
        }
    }

    #[test]
    fn the_same_at_any_block_size() {
        assert_eq!(reduce(18.0, 512), reduce(18.0, 333));
        assert_eq!(reduce(18.0, 512).1, reduce(18.0, 64).1);
    }

    #[test]
    fn without_a_print_it_passes_the_audio() {
        let n = 30_000;
        let signal = vec![noisy_tone(n, 0, 5)];
        let out = render_with_print(&KIND, RATE, &BTreeMap::new(), None, &signal, 512).unwrap();
        let worst = out[0]
            .iter()
            .zip(&signal[0])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn output_noise_only_is_what_was_taken_out() {
        let n = 4 * 48_000;
        let signal = vec![noisy_tone(n, 48_000, 6)];
        let print = vec![signal[0][..40_000].to_vec()];
        let run = |only: f64| {
            let settings: BTreeMap<String, f64> = [("Output Noise Only".to_string(), only)].into();
            render_with_print(&KIND, RATE, &settings, Some(&print), &signal, 512).unwrap()
        };
        let (kept, removed) = (run(0.0), run(1.0));
        let worst = (0..n)
            .map(|i| (kept[0][i] + removed[0][i] - signal[0][i]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn the_print_is_saved_with_the_settings() {
        let audio = vec![noisy_tone(20_000, 20_000, 9)];
        let mut a = Native::new(&KIND, RATE, None).unwrap();
        assert!(a.print().is_none());
        a.capture(RATE, &audio).unwrap();
        let print = Print::read(a.print().unwrap()).unwrap();
        assert_eq!((print.size, print.channels, print.stats), (4096, 1, 3));
        // 20 000 samples: (15 904 − 1) / 1365 = 11 frames, 1445 apart.
        assert_eq!(print.frames, 11);
        let b = Native::new(&KIND, RATE, Some(&a.save_state())).unwrap();
        assert_eq!(a.print(), b.print());
        assert!(a.capture(RATE, &[vec![0.0; 4000]]).is_err());
    }
}
