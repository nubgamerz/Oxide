//! Auto Noise Reduction: a real-time noise reducer that learns the noise floor from the signal as it plays, no print.
//!
//! - Framing (normal mode): every hop H (N/4, smaller for large FFTs) a Hann
//!   analysis of the last N samples sets a gain per bin; the frame N/4 samples behind it
//!   (so the gains see a little ahead) is weighed with a window that never reaches zero,
//!   filtered by those gains (DC and Nyquist untouched), its central 2H samples divided by
//!   that window and overlap-added under a Hann of 2H. The delay is N/4 + N/2 + H: 512 samples
//!   at the default 512-point FFT.
//! - The noise model (TD105): each
//!   bin's level is followed up fast and down at the Spectral Decay Rate (ms per 60 dB),
//!   smoothed across bins; the noise floor tracks that level's minimum, rising slowly (faster
//!   the higher the Noisiness); the noise is the floor raised by Noise Floor dB, the signal
//!   threshold a few dB above it (Signal Threshold, Noisiness). A bin at the noise gets the
//!   floor gain 10^(−Reduce By/20); at the threshold, 1; between, a power law of 1.25 in dB. Gains rise at once and fall smoothly, slower for tonal bins when
//!   Broadband Preservation looks around them.
//! - It passes everything until the signal's mean level first exceeds
//!   −110 dB, then starts the model there; `start` (the transport starting) begins again, so
//!   playback and export learn alike. No random dither: it would make every render different.
//! - Not yet (TD105): High Quality mode (FIR filtering).

use super::spectral::{self, Plans, MAX_BITS, MAX_SIZE, MIN_BITS};
use super::{Kind, NativeEffect, ParamSpec};
use rustfft::num_complex::Complex;
use std::f64::consts::PI;

const REDUCE: usize = 0;
const NOISINESS: usize = 1;
const NOISE_FLOOR: usize = 2;
const THRESHOLD: usize = 3;
const DECAY: usize = 4;
const BROADBAND: usize = 5;
const FFT_SIZE: usize = 6;

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
    param("Reduce Noise By", "dB", 0.0, 40.0, 20.0),
    ParamSpec {
        decimals: 0,
        ..param("Noisiness", "%", 0.0, 100.0, 30.0)
    },
    param("Noise Floor", "dB", -10.0, 10.0, 2.0),
    param("Signal Threshold", "dB", -20.0, 20.0, 2.5),
    ParamSpec {
        decimals: 0,
        ..param("Spectral Decay Rate", "ms/60dB", 20.0, 750.0, 140.0)
    },
    ParamSpec {
        decimals: 0,
        ..param("Broadband Preservation", "Hz", 0.0, 500.0, 100.0)
    },
    ParamSpec {
        steps: MAX_BITS - MIN_BITS,
        decimals: 0,
        automatable: false,
        format: Some(spectral::format_size),
        ..param("FFT Size", "", MIN_BITS as f64, MAX_BITS as f64, 9.0)
    },
];

pub static KIND: Kind = Kind {
    id: "auto-noise-reduction",
    name: "Auto Noise Reduction",
    category: "Restoration",
    params: &PARAMS,
    make: |rate| Box::new(AdaptiveNoise::new(rate)),
    response: None,
    transfer: None,
    latency: Some(|_, v| latency_of(size_of(v[FFT_SIZE]))),
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

fn size_of(bits: f64) -> usize {
    1 << (bits.round() as u32).clamp(MIN_BITS, MAX_BITS)
}

/// The hop: a quarter of the FFT, an eighth at 4096, a sixteenth at 8192, a twentieth above.
fn hop_of(size: usize) -> usize {
    let overlap = match size {
        4096 => 8,
        8192 => 16,
        s if s >= 16_384 => 20,
        _ => 4,
    };
    size / overlap
}

/// The delay in normal mode: N/4 (the look-ahead) + N/2 + H.
fn latency_of(size: usize) -> usize {
    size / 4 + size / 2 + hop_of(size)
}

/// The noise model's state for one channel, a value per bin.
struct Model {
    /// Each bin's level, followed in time (linear magnitude).
    level: Vec<f64>,
    /// That level smoothed across bins: what is judged.
    smooth: Vec<f64>,
    /// The noise in dB: the smoothed level's average while the bin sounds like noise.
    floor: Vec<f64>,
    /// The gain applied, and the bins' local "tonality" for Broadband Preservation.
    gain: Vec<f64>,
    deviation: Vec<f64>,
    /// Whether the model has started (the signal has been heard), and for how long (s).
    ready: bool,
    heard: f64,
}

/// How long the model learns every bin before it tells noise from signal.
const LEARN_SECONDS: f64 = 0.5;

impl Model {
    fn new() -> Self {
        let bins = MAX_SIZE / 2 + 1;
        Self {
            level: vec![0.0; bins],
            smooth: vec![0.0; bins],
            floor: vec![0.0; bins],
            gain: vec![1.0; bins],
            deviation: vec![0.0; bins],
            ready: false,
            heard: 0.0,
        }
    }

    fn restart(&mut self) {
        self.gain.fill(1.0);
        self.ready = false;
        self.heard = 0.0;
    }
}

struct AdaptiveNoise {
    rate: f64,
    values: [f64; 7],
    plans: Plans,
    size: usize,
    hop: usize,
    latency: usize,
    /// The symmetric Hann the analysis uses, and the synthesis window (never zero).
    analysis: Vec<f64>,
    synthesis: Vec<f64>,
    /// The Hann of 2H the kept samples are overlap-added under.
    join: Vec<f64>,
    /// Input by sample time (a ring), and the output being summed (a ring).
    input: [Vec<f64>; 2],
    output: [Vec<f64>; 2],
    work: Vec<Complex<f64>>,
    left: Vec<Complex<f64>>,
    right: Vec<Complex<f64>>,
    scratch: Vec<Complex<f64>>,
    models: [Model; 2],
    /// Samples since the start.
    t: usize,
}

/// The rings: room for the delay and a frame ahead at the largest size.
const RING: usize = 4 * MAX_SIZE;

impl AdaptiveNoise {
    fn new(rate: f64) -> Self {
        let plans = Plans::new();
        let zero = Complex::new(0.0, 0.0);
        let values: Vec<f64> = PARAMS.iter().map(|p| p.default).collect();
        let mut effect = Self {
            rate,
            values: values.try_into().expect("seven parameters"),
            scratch: vec![zero; plans.scratch_len()],
            plans,
            size: 0,
            hop: 0,
            latency: 0,
            analysis: Vec::with_capacity(MAX_SIZE),
            synthesis: Vec::with_capacity(MAX_SIZE),
            join: Vec::with_capacity(MAX_SIZE),
            input: [vec![0.0; RING], vec![0.0; RING]],
            output: [vec![0.0; RING], vec![0.0; RING]],
            work: vec![zero; MAX_SIZE],
            left: vec![zero; MAX_SIZE / 2 + 1],
            right: vec![zero; MAX_SIZE / 2 + 1],
            models: [Model::new(), Model::new()],
            t: 0,
        };
        effect.resize();
        effect
    }

    /// Windows and hop for the FFT size set, starting afresh. No allocation.
    fn resize(&mut self) {
        let n = size_of(self.values[FFT_SIZE]);
        self.size = n;
        self.hop = hop_of(n);
        self.latency = latency_of(n);
        let nf = n as f64;
        self.analysis.clear();
        self.analysis
            .extend((0..n).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f64 / (nf - 1.0)).cos()));
        self.synthesis.clear();
        self.synthesis
            .extend((0..n).map(|i| 0.5001 - 0.4999 * (2.0 * PI * i as f64 / nf).cos()));
        let span = 2 * self.hop;
        self.join.clear();
        self.join
            .extend((0..span).map(|i| 0.5 * (1.0 - (2.0 * PI * i as f64 / span as f64).cos())));
        self.restart();
    }

    fn restart(&mut self) {
        for c in 0..2 {
            self.input[c].fill(0.0);
            self.output[c].fill(0.0);
        }
        for m in &mut self.models {
            m.restart();
        }
        self.t = 0;
    }

    /// Analyse the last N samples into gains, then filter the frame N/4 behind them and add its
    /// middle into the output. `t` samples are in.
    fn frame(&mut self) {
        let (n, h) = (self.size, self.hop);
        let half = n / 2;
        let Some((forward, inverse)) = self.plans.of(n) else {
            return;
        };
        let (forward, inverse) = (forward.clone(), inverse.clone());
        // Analysis: the newest N samples.
        let newest = self.t;
        let work = &mut self.work[..n];
        for (i, w) in work.iter_mut().enumerate() {
            let at = (newest + RING - n + i) % RING;
            let g = self.analysis[i];
            *w = Complex::new(self.input[0][at] * g, self.input[1][at] * g);
        }
        forward.process_with_scratch(work, &mut self.scratch);
        spectral::split(work, &mut self.left[..=half], &mut self.right[..=half]);
        let hop_seconds = h as f64 / self.rate;
        for c in 0..2 {
            let bins = if c == 0 { &self.left } else { &self.right };
            estimate(
                &mut self.models[c],
                &bins[..=half],
                n,
                hop_seconds,
                self.rate,
                &self.values,
            );
        }
        // Synthesis: the N samples ending N/4 earlier, filtered by the gains.
        let ends = newest - n / 4;
        let work = &mut self.work[..n];
        for (i, w) in work.iter_mut().enumerate() {
            let at = (ends + RING - n + i) % RING;
            let g = self.synthesis[i];
            *w = Complex::new(self.input[0][at] * g, self.input[1][at] * g);
        }
        forward.process_with_scratch(work, &mut self.scratch);
        spectral::split(work, &mut self.left[..=half], &mut self.right[..=half]);
        for k in 1..half {
            self.left[k] *= self.models[0].gain[k];
            self.right[k] *= self.models[1].gain[k];
        }
        spectral::join(&self.left[..=half], &self.right[..=half], work);
        inverse.process_with_scratch(work, &mut self.scratch);
        // The middle 2H samples, the window divided out, under the joining Hann: sample i of
        // the frame is time `ends − n + i`.
        let scale = 1.0 / n as f64;
        for i in 0..2 * h {
            let j = half - h + i;
            let Some(time) = (ends + j).checked_sub(n) else {
                continue;
            };
            let g = self.join[i] * scale / self.synthesis[j];
            let slot = time % RING;
            self.output[0][slot] += work[j].re * g;
            self.output[1][slot] += work[j].im * g;
        }
    }
}

/// Update one channel's noise model from a frame's half spectrum and set its gains.
fn estimate(
    m: &mut Model,
    bins: &[Complex<f64>],
    n: usize,
    hop_seconds: f64,
    rate: f64,
    v: &[f64; 7],
) {
    let half = n / 2;
    let magnitude = |k: usize| spectral::magnitude(bins[k], n);
    // The start-up gate: nothing until the signal's mean level passes −110 dB.
    if !m.ready {
        let mean_db = (1..half)
            .map(|k| 20.0 * magnitude(k).max(1e-15).log10() + 4.6)
            .sum::<f64>()
            / (half - 1).max(1) as f64;
        if mean_db <= -110.0 {
            return;
        }
        for k in 1..half {
            let a = magnitude(k);
            m.level[k] = a;
            m.smooth[k] = a;
            m.floor[k] = 20.0 * a.max(1e-15).log10();
            m.gain[k] = 1.0;
        }
        m.ready = true;
    }
    let hop_ms = hop_seconds * 1000.0;
    let fast = 0.001f64.powf(hop_ms / 10.0);
    let release = 0.001f64.powf(hop_ms / v[DECAY]);
    // The level in time: up fast, down at the decay rate.
    for k in 1..half {
        let a = magnitude(k);
        let keep = if a > m.level[k] { fast } else { release };
        m.level[k] = keep * m.level[k] + (1.0 - keep) * a;
    }
    // Across bins: forward then back, 0.2 of the neighbour.
    m.smooth[1] = m.level[1];
    for k in 2..half {
        m.smooth[k] = 0.8 * m.level[k] + 0.2 * m.smooth[k - 1];
    }
    for k in (1..half - 1).rev() {
        m.smooth[k] = 0.8 * m.smooth[k] + 0.2 * m.smooth[k + 1];
    }
    // The noise (dB): the level's average over half a second while the bin sounds like noise
    // (under the signal threshold); over it, only a slow rise (2 dB/s at the default
    // Noisiness), so a held note isn't taken for noise. Noise Floor shifts it.
    let threshold_db = 6.0 + 0.1 * v[NOISINESS] + v[THRESHOLD];
    let rise = (0.5 + 0.05 * v[NOISINESS]) * hop_seconds;
    // For its first half second the model follows every bin faster, whatever it hears: the
    // gate opens on a frame that is partly the silence before it.
    let learning = m.heard < LEARN_SECONDS;
    m.heard += hop_seconds;
    let follow = 1.0 - (-hop_seconds / if learning { 0.1 } else { 0.5 }).exp();
    for k in 1..half {
        let level = 20.0 * m.smooth[k].max(1e-15).log10();
        let floor = &mut m.floor[k];
        *floor = if learning || level < *floor + threshold_db {
            *floor + (level - *floor) * follow
        } else {
            *floor + rise
        };
    }
    // The level's average in dB sits under its power: judge against 2 dB over it.
    let noise_db = 2.0 + v[NOISE_FLOOR];
    let floor_gain = 10f64.powf(-v[REDUCE] / 20.0);
    let reach = ((v[BROADBAND] * n as f64 / rate).round() as usize).min(half / 2);
    let level_db = |k: usize, m: &Model| 20.0 * m.smooth[k].max(1e-15).log10();
    let noise_at = |k: usize, m: &Model| m.floor[k] + noise_db;
    // How far each bin stands over its threshold (0..3 dB), for the tonality around it.
    for k in 1..half {
        let over = level_db(k, m) - (noise_at(k, m) + threshold_db);
        m.deviation[k] = over.clamp(0.0, 3.0);
    }
    for k in 1..half {
        let level = level_db(k, m);
        let noise = noise_at(k, m);
        let target = if threshold_db <= 0.0 || level >= noise + threshold_db {
            1.0
        } else if level <= noise {
            floor_gain
        } else {
            let x = (level - noise) / threshold_db;
            floor_gain + (1.0 - floor_gain) * x.powf(1.25)
        };
        let gain = if target >= m.gain[k] {
            target
        } else {
            // Falling gains glide; tonal bins (standing out among their neighbours) slower.
            let hold = if reach > 0 {
                let (lo, hi) = (k.saturating_sub(reach).max(1), (k + reach).min(half - 1));
                let sum: f64 = (lo..=hi).map(|j| m.deviation[j].powi(2)).sum();
                let peaky = ((sum / (2 * reach) as f64).sqrt() / 3.0).min(1.0);
                0.7 + 0.2 * peaky
            } else {
                0.7
            };
            hold * m.gain[k] + (1.0 - hold) * target
        };
        m.gain[k] = gain;
    }
}

impl NativeEffect for AdaptiveNoise {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        if index == FFT_SIZE {
            self.resize();
        }
    }

    /// Automation lands at the next hop, as a setting does.
    fn ramp(&mut self, index: usize, value: f64, _samples: usize) {
        self.set(index, value);
    }

    fn start(&mut self) {
        self.restart();
    }

    fn latency(&self) -> usize {
        self.latency
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let [left, right] = io else {
            return;
        };
        for k in from..to {
            let slot = self.t % RING;
            self.input[0][slot] = f64::from(left[k]);
            self.input[1][slot] = f64::from(right[k]);
            self.t += 1;
            if self.t.is_multiple_of(self.hop) && self.t >= self.size / 4 {
                self.frame();
            }
            if self.t > self.latency {
                let at = (self.t - 1 - self.latency) % RING;
                left[k] = self.output[0][at] as f32;
                right[k] = self.output[1][at] as f32;
                self.output[0][at] = 0.0;
                self.output[1][at] = 0.0;
            } else {
                left[k] = 0.0;
                right[k] = 0.0;
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

    fn noisy_tone(n: usize, tone_from: usize) -> Vec<f32> {
        let mut x = 0x9E37_79B9_7F4A_7C15u64;
        (0..n)
            .map(|i| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let hiss = ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) * 0.031;
                let tone = if i >= tone_from {
                    0.25 * (2.0 * PI * 1000.0 * i as f64 / RATE).sin()
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

    fn tone_db(x: &[f32]) -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for (i, s) in x.iter().enumerate() {
            let p = 2.0 * PI * 1000.0 * i as f64 / RATE;
            re += f64::from(*s) * p.cos();
            im += f64::from(*s) * p.sin();
        }
        20.0 * ((re * re + im * im).sqrt() * 2.0 / x.len() as f64).log10()
    }

    fn run(settings: &[(&str, f64)], block: usize) -> (Vec<f32>, Vec<f32>) {
        let signal = vec![noisy_tone(6 * 48_000, 3 * 48_000)];
        let settings: BTreeMap<String, f64> =
            settings.iter().map(|(k, v)| (k.to_string(), *v)).collect();
        let out = render(&KIND, RATE, &settings, &signal, block).unwrap();
        (signal[0].clone(), out[0].clone())
    }

    #[test]
    fn the_latency_is_the_references() {
        assert_eq!(
            [512, 1024, 2048, 4096, 8192, 16_384].map(latency_of),
            [512, 1024, 2048, 3584, 6656, 13_107]
        );
    }

    #[test]
    fn it_learns_steady_noise_and_takes_it_down_keeping_a_tone() {
        let mut cuts = Vec::new();
        for reduce in [10.0, 20.0] {
            let (dry, wet) = run(&[("Reduce Noise By", reduce)], 512);
            // After a second to learn it, the noise alone is down by about the setting.
            let noise = 60_000..140_000;
            let cut = rms_db(&dry[noise.clone()]) - rms_db(&wet[noise]);
            assert!(
                cut > reduce - 3.0 && cut <= reduce + 0.5,
                "Reduce Noise By {reduce}: noise down {cut:.2} dB"
            );
            cuts.push(cut);
            let tone = 192_000..288_000;
            let kept = tone_db(&wet[tone.clone()]) - tone_db(&dry[tone]);
            assert!(kept.abs() < 0.5, "tone moved {kept:.2} dB");
        }
        assert!(cuts[1] > cuts[0] + 5.0, "{cuts:?}");
    }

    #[test]
    fn nothing_to_reduce_passes_the_audio() {
        // Reduce Noise By 0: the gains stay 1, and the framing gives the input back.
        let (dry, wet) = run(&[("Reduce Noise By", 0.0)], 512);
        let worst = (2048..dry.len())
            .map(|i| (dry[i] - wet[i]).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn the_same_at_any_block_size() {
        assert_eq!(run(&[], 512), run(&[], 333));
        assert_eq!(run(&[], 512).1, run(&[("FFT Size", 9.0)], 64).1);
    }
}
