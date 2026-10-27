//! Convolution Reverb, as studied in the research notes on `convolution-reverb` (kept outside
//! the repository) and written anew: the user's impulse response, shaped
//! by the settings off the audio thread, convolved by the shared [`convolver`](super::convolver).
//!
//! **The impulse** is the user's own file (WAV, AIFF or FLAC, up to [`LONGEST_SECONDS`]),
//! brought into the project (`Session::import_copy`) and saved by its path in the effect's
//! state. No impulse is shipped. It is prepared on a worker of the instance's own
//! ([`super::Reads`]), in the study's order:
//!
//! 1. **Channels:** a stereo file plays left on left and right on right (no cross terms: not
//!    "true stereo"); any other count (mono, 4, 6) is averaged to one impulse used on both
//!    sides. The graph is always stereo, so this is the rule for a stereo instance.
//! 2. **Rate:** resampled to the engine's rate (`media::resample`, the importer's windowed
//!    sinc; the reference's resampler kernel was not traced). Its length is then N.
//! 3. **Room Size** R: r' = 0.1 + 0.7·(R − 10)/90, the impulse cut (or padded) to
//!    M = round(N·r'/0.8) samples (at least 2) and faded by w[i] = 0.5 + 0.5·cos(π·i/(M + 1))
//!    over its whole length, even at 100 % (the reference's, kept). Values under 1e-10 become 0.
//! 4. **Width** W (on the pair): w' = W/100, A = (1 − w')/n, B = (1 + w')/n with
//!    n = √((1 − w')² + (1 + w')²); sample by sample left ← A·right + B·left, then
//!    right ← A·left + B·right with the left already updated (the reference's in-place order, so 0 %
//!    is not quite mono). 100 % is the identity.
//! 5. **Damping LF / HF** (d = D/100): each channel split by a zero-phase smoother
//!    (lp = 0.6·h + 0.4·lp forward from lp[0] = h[0], then the same backward; hp = h − lp,
//!    a crossover near 0.15·fs), and h = E_L·lp + E_H·hp with time envelopes over the M
//!    samples: 1 at d = 0.7; above it, e = d − 0.7, the second half of a Kaiser window of
//!    length 2M with β = 20·e raised to 1 + 1000·e² (a decay, steeper towards 100 %); below
//!    it, k = ((0.7 − d)/0.71)^0.2 and E = 1/(k·e^(−10·i/M) + 1 − k), a rise to about
//!    1/(1 − k). Values under 1e-10 become 0 after each step.
//! 6. **Pre-Delay** P: round(fs·P/1000) zeros in front (the wet is later; no latency).
//! 7. **Normalisation:** k = 0.5/S, S the mean over the two channels of Σ|h|, folded into the
//!    convolver's spectra.
//!
//! **Playing:** y = (x_dry·(100 − m)/100 + wet·m)·g per channel, wet = conv(x)·k with m the Mix
//! in percent (the reference's: m multiplies, not m/100, which with k = 0.5/ΣL1 gives a sensible
//! level) and g = 10^(Gain/20). Mix and Gain act at once (no smoothing) and glide
//! in a straight line along a lane; the other five re-prepare the impulse, so they have no
//! lane. The convolver's [`LATENCY`] (256 samples) is the effect's, the dry path delayed with
//! it.
//!
//! **Without an impulse** (none chosen yet, or one that failed to load) the input passes
//! unchanged, delayed by the latency. The reference plays the dry path alone then
//! (x·(100 − m)/100·g, silence at the default Mix 100) and while it re-prepares; here a new
//! impulse (or new settings) takes over only once prepared, the old one playing until then and
//! then ringing out the input it had while the new one starts (no gap, no cut tail). A
//! transport start restarts the convolution from silence, so playback and export start alike.

use super::convolver::{Convolver, LATENCY};
use super::{FileBox, Kind, Loaded, NativeEffect, Overview, ParamSpec, Prepared, Reads};
use crate::graph::CHANNELS;
use anyhow::{bail, Result};
use std::path::Path;

pub const MIX: usize = 0;
pub const ROOM: usize = 1;
pub const DAMPING_LF: usize = 2;
pub const DAMPING_HF: usize = 3;
pub const PRE_DELAY: usize = 4;
pub const WIDTH: usize = 5;
pub const GAIN: usize = 6;
pub(crate) const COUNT: usize = 7;

/// The longest impulse file read (seconds at its own rate). The reference refuses files over 7.5 M
/// samples (all channels); ours is by time, enough for 20 s at 192 kHz in stereo.
pub const LONGEST_SECONDS: f64 = 30.0;
/// Values under this are flushed to zero while shaping.
const TINY: f32 = 1e-10;
/// The neutral damping.
const NEUTRAL: f64 = 0.7;
/// Samples the effect works through at a time (its scratch buffers' size).
const CHUNK: usize = 512;

const PERCENT: ParamSpec = ParamSpec {
    name: "",
    unit: "%",
    min: 10.0,
    max: 100.0,
    default: 100.0,
    steps: 0,
    decimals: 0,
    // Each change prepares the impulse again: no lane.
    automatable: false,
    link: None,
    log_slider: false,
    format: None,
};

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Mix",
        min: 0.0,
        automatable: true,
        ..PERCENT
    },
    ParamSpec {
        name: "Room Size",
        ..PERCENT
    },
    ParamSpec {
        name: "Damping LF",
        default: 70.0,
        ..PERCENT
    },
    ParamSpec {
        name: "Damping HF",
        default: 73.0,
        ..PERCENT
    },
    ParamSpec {
        name: "Pre-Delay",
        unit: "ms",
        min: 0.0,
        default: 0.0,
        decimals: 1,
        ..PERCENT
    },
    ParamSpec {
        name: "Width",
        min: 0.0,
        max: 300.0,
        ..PERCENT
    },
    ParamSpec {
        name: "Gain",
        unit: "dB",
        min: -18.0,
        max: 18.0,
        default: -3.0,
        decimals: 1,
        automatable: true,
        ..PERCENT
    },
];

pub static READS: Reads = Reads {
    what: "impulse",
    shaped_by: &[ROOM, DAMPING_LF, DAMPING_HF, PRE_DELAY, WIDTH],
    load: Some(load),
    prepare,
    derive: None,
    outputs: &[],
};

pub static KIND: Kind = Kind {
    id: "convolution-reverb",
    name: "Convolution Reverb",
    category: "Reverb",
    params: &PARAMS,
    make: |_| Box::new(ConvolutionReverb::new()),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: Some(&READS),
    prepares: None,
};

/// Read an impulse file and bring it to `rate`: its channels mapped to the stereo graph (a
/// stereo file as it is, any other count averaged to one), resampled.
fn load(path: &Path, rate: f64) -> Result<Loaded> {
    let audio = crate::media::read_audio(path)?;
    let frames = audio.len();
    if frames == 0 || audio.channels.is_empty() {
        bail!("{} holds no audio", path.display());
    }
    let seconds = frames as f64 / f64::from(audio.sample_rate);
    if seconds > LONGEST_SECONDS {
        bail!(
            "{} is {seconds:.1} s long: impulses up to {LONGEST_SECONDS} s are read",
            path.display()
        );
    }
    let file_channels = audio.channels.len();
    let mapped = if file_channels == CHANNELS {
        audio.channels
    } else {
        let scale = 1.0 / file_channels as f32;
        vec![(0..frames)
            .map(|i| audio.channels.iter().map(|c| c[i]).sum::<f32>() * scale)
            .collect()]
    };
    let to = rate.round() as u32;
    let channels = mapped
        .iter()
        .map(|c| crate::media::resample(c, audio.sample_rate, to))
        .collect();
    Ok(Loaded {
        channels,
        file_channels,
        file_rate: audio.sample_rate,
        file_frames: frames,
    })
}

/// The impulse as the settings shape it, before it is partitioned.
#[derive(Debug, Clone)]
pub struct Shaped {
    /// Left and right, at the engine's rate, with the pre-delay in front.
    pub channels: [Vec<f32>; 2],
    /// The normalisation k = 0.5/S (S the mean of the two channels' Σ|h|); 0 for silence.
    pub gain: f64,
    /// The kept length M (Room Size), before the pre-delay.
    pub kept: usize,
}

/// Shape `channels` (one, played on both sides, or two; at `rate`) for plain `values`, steps 3
/// to 7 of the module's notes.
pub fn shape(rate: f64, values: &[f64], channels: &[Vec<f32>]) -> Shaped {
    let left = channels.first().cloned().unwrap_or_default();
    let right = channels.get(1).cloned().unwrap_or_else(|| left.clone());
    let mut h = [left, right];
    // Room Size: cut to M and faded over all of it.
    let n = h[0].len().max(h[1].len());
    let r = 0.1 + 0.7 * (values[ROOM] - 10.0) / 90.0;
    let kept = ((n as f64 * r / 0.8).round() as usize).max(2);
    for c in &mut h {
        c.resize(kept, 0.0);
        for (i, x) in c.iter_mut().enumerate() {
            let w =
                (0.5 + 0.5 * (std::f64::consts::PI * i as f64 / (kept + 1) as f64).cos()) as f32;
            *x = flush(*x * w);
        }
    }
    // Width, in place in the reference's order.
    let w = values[WIDTH] / 100.0;
    let (a, b) = (1.0 - w, 1.0 + w);
    let norm = (a * a + b * b).sqrt();
    let (a, b) = ((a / norm) as f32, (b / norm) as f32);
    {
        let [l, r] = &mut h;
        for (x0, x1) in l.iter_mut().zip(r.iter_mut()) {
            *x0 = a * *x1 + b * *x0;
            *x1 = a * *x0 + b * *x1;
        }
    }
    // Damping: the band split and its two envelopes.
    let low = envelope(values[DAMPING_LF] / 100.0, kept);
    let high = envelope(values[DAMPING_HF] / 100.0, kept);
    for c in &mut h {
        let lp = low_band(c);
        for i in 0..kept {
            let hp = flush(c[i] - lp[i]);
            c[i] = flush(low[i] * lp[i] + high[i] * hp);
        }
    }
    // Pre-Delay: zeros in front.
    let pre = (rate * values[PRE_DELAY] * 0.001).round() as usize;
    if pre > 0 {
        for c in &mut h {
            c.splice(0..0, std::iter::repeat_n(0.0, pre));
        }
    }
    let sum = h
        .iter()
        .map(|c| c.iter().map(|x| f64::from(x.abs())).sum::<f64>())
        .sum::<f64>()
        / 2.0;
    Shaped {
        channels: h,
        gain: if sum > 0.0 { 0.5 / sum } else { 0.0 },
        kept,
    }
}

/// The damping's low band of `h`: a one-pole smoother (0.6 of the sample, 0.4 of the last)
/// run forward from `h[0]`, then backward, so it adds no delay. Its response is
/// 0.36/(1.16 − 0.8·cos ω): half at about 0.157·fs.
pub fn low_band(h: &[f32]) -> Vec<f32> {
    let mut lp = h.to_vec();
    for i in 1..lp.len() {
        lp[i] = flush(0.6 * h[i] + 0.4 * lp[i - 1]);
    }
    for i in (0..lp.len().saturating_sub(1)).rev() {
        lp[i] = flush(0.6 * lp[i] + 0.4 * lp[i + 1]);
    }
    lp
}

fn flush(x: f32) -> f32 {
    if x.abs() < TINY {
        0.0
    } else {
        x
    }
}

/// A damping's time envelope over `len` samples, for d = Damping/100 (the module's notes,
/// step 5).
pub fn envelope(d: f64, len: usize) -> Vec<f32> {
    if (d - NEUTRAL).abs() < 1e-9 {
        return vec![1.0; len];
    }
    let m = len as f64;
    if d > NEUTRAL {
        // The second half of a Kaiser window of length 2M, sharpened.
        let e = d - NEUTRAL;
        let beta = 20.0 * e;
        let power = 1.0 + 1000.0 * e * e;
        let scale = bessel_i0(beta);
        (0..len)
            .map(|i| {
                let t = (2.0 * i as f64 + 1.0) / (2.0 * m - 1.0);
                let w = bessel_i0(beta * (1.0 - t * t).max(0.0).sqrt()) / scale;
                let v = w.powf(power) as f32;
                if v < TINY {
                    0.0
                } else {
                    v
                }
            })
            .collect()
    } else {
        let k = ((NEUTRAL - d) / 0.71).powf(0.2);
        (0..len)
            .map(|i| (1.0 / (k * (-10.0 * i as f64 / m).exp() + 1.0 - k)) as f32)
            .collect()
    }
}

/// The modified Bessel function of the first kind, order 0 (its power series).
fn bessel_i0(x: f64) -> f64 {
    let q = x * x / 4.0;
    let (mut sum, mut term) = (1.0, 1.0);
    for k in 1..60 {
        term *= q / (k * k) as f64;
        sum += term;
        if term < sum * 1e-17 {
            break;
        }
    }
    sum
}

/// The impulse ready to play: shaped, partitioned, with its gain in the spectra.
fn prepare(rate: f64, values: &[f64], loaded: &Loaded) -> Prepared {
    let shaped = shape(rate, values, &loaded.channels);
    let convolver = Convolver::new(&shaped.channels, shaped.gain as f32);
    Prepared {
        overview: Overview::of(&shaped.channels, rate, shaped.gain as f32, loaded),
        dsp: Box::new(convolver),
    }
}

/// A value that jumps when set and moves in a straight line along a lane.
#[derive(Debug, Clone, Copy)]
pub(super) struct Glide {
    value: f64,
    step: f64,
    left: usize,
}

impl Glide {
    pub(super) fn new(value: f64) -> Self {
        Self {
            value,
            step: 0.0,
            left: 0,
        }
    }

    pub(super) fn set(&mut self, value: f64) {
        *self = Self::new(value);
    }

    pub(super) fn ramp(&mut self, value: f64, samples: usize) {
        if samples == 0 {
            self.set(value);
        } else {
            self.step = (value - self.value) / samples as f64;
            self.left = samples;
        }
    }

    /// The value for this sample, then a step on.
    pub(super) fn next(&mut self) -> f64 {
        let v = self.value;
        if self.left > 0 {
            self.value += self.step;
            self.left -= 1;
        }
        v
    }
}

pub struct ConvolutionReverb {
    /// The impulse playing.
    current: Option<Box<Convolver>>,
    /// The impulse it replaced, ringing out the input it had.
    draining: Option<Box<Convolver>>,
    /// The dry path's delay, the convolver's latency, per channel.
    dry: [Vec<f32>; 2],
    dry_at: usize,
    wet: [Vec<f32>; 2],
    tail: [Vec<f32>; 2],
    mix: Glide,
    gain_db: Glide,
    /// `gain_db` as a factor, and the dB it was made from.
    gain: (f64, f32),
}

impl ConvolutionReverb {
    fn new() -> Self {
        Self {
            current: None,
            draining: None,
            dry: [vec![0.0; LATENCY], vec![0.0; LATENCY]],
            dry_at: 0,
            wet: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            tail: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            mix: Glide::new(PARAMS[MIX].default),
            gain_db: Glide::new(PARAMS[GAIN].default),
            gain: (f64::NAN, 1.0),
        }
    }

    /// The gain factor for `db`, made again only when it changes.
    fn gain_of(&mut self, db: f64) -> f32 {
        if self.gain.0 != db {
            self.gain = (db, (db * 0.115_129_254_649_702_28).exp() as f32);
        }
        self.gain.1
    }
}

impl NativeEffect for ConvolutionReverb {
    fn set(&mut self, index: usize, value: f64) {
        match index {
            MIX => self.mix.set(value),
            GAIN => self.gain_db.set(value),
            // The rest shape the impulse, prepared off the audio thread.
            _ => {}
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match index {
            MIX => self.mix.ramp(value, samples),
            GAIN => self.gain_db.ramp(value, samples),
            _ => {}
        }
    }

    fn latency(&self) -> usize {
        LATENCY
    }

    fn start(&mut self) {
        if let Some(current) = &mut self.current {
            current.reset();
        }
        if let Some(draining) = &mut self.draining {
            draining.silence();
        }
        for d in &mut self.dry {
            d.fill(0.0);
        }
        self.dry_at = 0;
    }

    fn take_file(&mut self, fresh: &mut Option<FileBox>, spent: &mut Option<FileBox>) {
        // One that has rung out goes back to be freed.
        if spent.is_none() && self.draining.as_ref().is_some_and(|d| d.drained()) {
            *spent = self.draining.take().map(|d| d as FileBox);
        }
        if fresh.is_none() {
            return;
        }
        if self.draining.is_some() {
            // A third impulse before the last switch has rung out: the oldest's tail is cut
            // (when the main thread has collected the last one given back).
            if spent.is_some() {
                return;
            }
            *spent = self.draining.take().map(|d| d as FileBox);
        }
        let Some(payload) = fresh.take() else {
            return;
        };
        match payload.downcast::<Convolver>() {
            Ok(new) => {
                if let Some(mut old) = self.current.take() {
                    old.quiet();
                    self.draining = Some(old);
                }
                self.current = Some(new);
            }
            // Not ours: left where it was, never taken.
            Err(other) => *fresh = Some(other),
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let [l, r, ..] = io else {
            return;
        };
        let mut at = from;
        while at < to {
            let n = CHUNK.min(to - at);
            let Some(current) = &mut self.current else {
                // No impulse: the input, delayed as the wet would be.
                for i in at..at + n {
                    let k = self.dry_at;
                    l[i] = std::mem::replace(&mut self.dry[0][k], l[i]);
                    r[i] = std::mem::replace(&mut self.dry[1][k], r[i]);
                    self.dry_at = (k + 1) % LATENCY;
                    self.mix.next();
                    self.gain_db.next();
                }
                at += n;
                continue;
            };
            {
                let [wl, wr] = &mut self.wet;
                current.process(
                    Some([&l[at..at + n], &r[at..at + n]]),
                    [&mut wl[..n], &mut wr[..n]],
                );
            }
            if let Some(old) = &mut self.draining {
                let [tl, tr] = &mut self.tail;
                old.process(None, [&mut tl[..n], &mut tr[..n]]);
                for c in 0..2 {
                    for (w, t) in self.wet[c][..n].iter_mut().zip(&self.tail[c][..n]) {
                        *w += *t;
                    }
                }
            }
            for i in 0..n {
                let m = self.mix.next();
                let db = self.gain_db.next();
                let g = self.gain_of(db);
                let dry = ((100.0 - m) * 0.01) as f32;
                let wet = m as f32;
                let k = self.dry_at;
                let x = [
                    std::mem::replace(&mut self.dry[0][k], l[at + i]),
                    std::mem::replace(&mut self.dry[1][k], r[at + i]),
                ];
                self.dry_at = (k + 1) % LATENCY;
                l[at + i] = (x[0] * dry + self.wet[0][i] * wet) * g;
                r[at + i] = (x[1] * dry + self.wet[1][i] * wet) * g;
            }
            at += n;
        }
    }
}

#[cfg(test)]
#[path = "convolution_reverb_tests.rs"]
mod tests;
