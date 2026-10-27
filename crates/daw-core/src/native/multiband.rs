//! Multiband Compressor: four bands, each compressed, and a limiter after them:
//!
//! - **Crossover** ("Hybrid"): three edges, cascaded. At each edge the low band is a
//!   Linkwitz–Riley 4th-order low-pass (a bilinear Butterworth section, coefficients rounded to
//!   f32, run twice) and the high band is the edge's input *delayed* minus that low band, so
//!   the two always add back to the delayed input; each delay is set from the low-pass's group
//!   delay, and the first edge is padded to at least 8 ms. The four bands sum to a pure delay.
//! - **Bands**: the Compressor's core in each (a linked peak detector, a hard knee,
//!   two smoothers releasing toward unity, 1 ms of look-ahead), its band gain and the output
//!   gain; Bypass leaves only the delay, Solo mutes the bands not soloed.
//! - **Limiter** (a maximizer): 63 samples, then a peak follower, Soft (a fixed 20:1
//!   knee, the Attack and Release times, then a 1 ms smoother) or Brickwall (the lowest target
//!   over 200 samples, never exceeded), applied 199 samples later; Margin is the ceiling.
//!
//! The latency is the crossover's (it follows the crossovers), the bands' 1 ms, and the
//! limiter's 262 samples unless it is bypassed. Not yet (TD104): a crossfade when the
//! crossovers move, and ±2e-10 anti-denormal offsets.

// Frame by frame across both channels throughout: the detectors are linked and the rings
// shared, so channel indices are the natural loop.
#![allow(clippy::needless_range_loop)]

use super::biquad::{butterworth2, Coefficients, Section};
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

const OUTPUT: usize = 0;
const CROSSOVERS: usize = 1;
/// A band's parameters from its first: threshold, ratio, attack, release, gain, bypass, solo.
const BANDS: usize = 4;
const THRESHOLD: usize = 0;
const RATIO: usize = 1;
const ATTACK: usize = 2;
const RELEASE: usize = 3;
const GAIN: usize = 4;
const BYPASS: usize = 5;
const SOLO: usize = 6;
const BAND_PARAMS: usize = 7;
const BAND_COUNT: usize = 4;
const LIMITER: usize = BANDS + BAND_PARAMS * BAND_COUNT;
/// The limiter's from its first: bypass, threshold, margin, attack, release, brickwall.
const LIM_BYPASS: usize = 0;
const LIM_THRESHOLD: usize = 1;
const LIM_MARGIN: usize = 2;
const LIM_ATTACK: usize = 3;
const LIM_RELEASE: usize = 4;
const LIM_BRICKWALL: usize = 5;
const PARAM_COUNT: usize = LIMITER + 6;

/// The limiter's two delays: before the detector, and from it to the output.
const LIMITER_IN: usize = 63;
const LIMITER_AHEAD: usize = 199;
const LIMITER_WINDOW: usize = 200;
/// The longest crossover delay these rings hold: 0.25 s at 192 kHz.
const MAX_DELAY: usize = 48_000;

const DB_LN: f32 = 0.115_129_255;

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} kHz", v / 1000.0)
    } else {
        format!("{v:.0} Hz")
    }
}

fn ratio(v: f64) -> String {
    format!("{v:.1}:1")
}

const P: ParamSpec = ParamSpec {
    name: "",
    unit: "dB",
    min: -18.0,
    max: 18.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

const SWITCH: ParamSpec = ParamSpec {
    unit: "",
    min: 0.0,
    max: 1.0,
    steps: 1,
    decimals: 0,
    automatable: false,
    ..P
};

macro_rules! band {
    ($n:literal) => {
        [
            ParamSpec {
                name: concat!("Band ", $n, " Threshold"),
                min: -60.0,
                max: 0.0,
                ..P
            },
            ParamSpec {
                name: concat!("Band ", $n, " Ratio"),
                unit: "",
                min: 1.0,
                max: 30.0,
                default: 2.0,
                format: Some(ratio),
                ..P
            },
            ParamSpec {
                name: concat!("Band ", $n, " Attack"),
                unit: "ms",
                min: 0.0,
                max: 500.0,
                default: 10.0,
                ..P
            },
            ParamSpec {
                name: concat!("Band ", $n, " Release"),
                unit: "ms",
                min: 0.0,
                max: 5000.0,
                default: 100.0,
                ..P
            },
            ParamSpec {
                name: concat!("Band ", $n, " Gain"),
                ..P
            },
            ParamSpec {
                name: concat!("Band ", $n, " Bypass"),
                default: 0.0,
                ..SWITCH
            },
            ParamSpec {
                name: concat!("Band ", $n, " Solo"),
                default: 0.0,
                ..SWITCH
            },
        ]
    };
}

const fn params() -> [ParamSpec; PARAM_COUNT] {
    let mut all = [P; PARAM_COUNT];
    all[OUTPUT] = ParamSpec {
        name: "Output Gain",
        ..P
    };
    let crossover = ParamSpec {
        unit: "Hz",
        min: 20.0,
        max: 20_000.0,
        automatable: false,
        log_slider: true,
        format: Some(hz),
        ..P
    };
    all[CROSSOVERS] = ParamSpec {
        name: "Crossover 1",
        default: 120.0,
        ..crossover
    };
    all[CROSSOVERS + 1] = ParamSpec {
        name: "Crossover 2",
        default: 2000.0,
        ..crossover
    };
    all[CROSSOVERS + 2] = ParamSpec {
        name: "Crossover 3",
        default: 10_000.0,
        ..crossover
    };
    let bands = [band!("1"), band!("2"), band!("3"), band!("4")];
    let mut b = 0;
    while b < BAND_COUNT {
        let mut k = 0;
        while k < BAND_PARAMS {
            all[BANDS + b * BAND_PARAMS + k] = bands[b][k];
            k += 1;
        }
        b += 1;
    }
    all[LIMITER + LIM_BYPASS] = ParamSpec {
        name: "Limiter Bypass",
        default: 0.0,
        ..SWITCH
    };
    all[LIMITER + LIM_THRESHOLD] = ParamSpec {
        name: "Limiter Threshold",
        min: -20.0,
        max: 0.0,
        ..P
    };
    all[LIMITER + LIM_MARGIN] = ParamSpec {
        name: "Limiter Margin",
        min: -5.0,
        max: 0.0,
        ..P
    };
    all[LIMITER + LIM_ATTACK] = ParamSpec {
        name: "Limiter Attack",
        unit: "ms",
        min: 1.0,
        max: 5.0,
        default: 4.0,
        ..P
    };
    all[LIMITER + LIM_RELEASE] = ParamSpec {
        name: "Limiter Release",
        unit: "ms",
        min: 10.0,
        max: 500.0,
        default: 500.0,
        ..P
    };
    all[LIMITER + LIM_BRICKWALL] = ParamSpec {
        name: "Brickwall Limiter",
        default: 0.0,
        ..SWITCH
    };
    all
}

static PARAMS: [ParamSpec; PARAM_COUNT] = params();

pub static KIND: Kind = Kind {
    id: "multiband-compressor",
    name: "Multiband Compressor",
    category: "Dynamics",
    params: &PARAMS,
    make: |rate| Box::new(Multiband::new(rate)),
    response: None,
    transfer: None,
    latency: Some(latency),
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The whole delay: the crossover's, the bands' look-ahead, and the limiter's unless bypassed.
fn latency(rate: f64, v: &[f64]) -> usize {
    let (pad, delays) = crossover_delays(rate, v);
    let limiter = if v[LIMITER + LIM_BYPASS] >= 0.5 {
        0
    } else {
        LIMITER_IN + LIMITER_AHEAD
    };
    pad + delays.iter().sum::<usize>() + band_delay(rate) + limiter
}

/// The bands' look-ahead: 1 ms less a sample.
fn band_delay(rate: f64) -> usize {
    (0.5 + rate as f32 as f64 * 0.001) as usize - 1
}

/// The crossover frequencies, each kept at or above the one below.
fn edges(v: &[f64]) -> [f64; 3] {
    let a = v[CROSSOVERS];
    let b = v[CROSSOVERS + 1].max(a);
    let c = v[CROSSOVERS + 2].max(b);
    [a, b, c]
}

/// An edge's low-pass section: Butterworth, its coefficients rounded to f32.
fn section(rate: f64, f: f64) -> Coefficients {
    let c = butterworth2(rate, f, false);
    let r = |x: f64| f64::from(x as f32);
    Coefficients {
        b0: r(c.b0),
        b1: r(c.b1),
        b2: r(c.b2),
        a1: r(c.a1),
        a2: r(c.a2),
    }
}

/// The phase of a section at `w` radians per sample, as a unit complex number's angle.
fn response(c: &Coefficients, w: f64) -> (f64, f64) {
    let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
    let (nr, ni) = (c.b0 + c.b1 * c1 + c.b2 * c2, -c.b1 * s1 - c.b2 * s2);
    let (dr, di) = (1.0 + c.a1 * c1 + c.a2 * c2, -c.a1 * s1 - c.a2 * s2);
    let d = dr * dr + di * di;
    ((nr * dr + ni * di) / d, (ni * dr - nr * di) / d)
}

/// Each edge's delay (samples) and the padding before the first: an edge's delay is the
/// low-pass pair's group delay where its band sits, scaled by 0.95 − r/2 (r: that frequency
/// over Nyquist); the total is padded to at least 8 ms.
fn crossover_delays(rate: f64, v: &[f64]) -> (usize, [usize; 3]) {
    let f = edges(v);
    let mut delays = [0usize; 3];
    for (k, d) in delays.iter_mut().enumerate() {
        let at = if k == 0 {
            f[0] / 2.0
        } else {
            (f[k - 1] * f[k]).sqrt()
        };
        let r = (at / (rate / 2.0)).clamp(0.0, 1.0);
        let c = section(rate, f[k]);
        let w = 2.0 * std::f64::consts::PI * at / rate;
        let dw = 1e-4;
        let (ar, ai) = response(&c, w);
        let (br, bi) = response(&c, w + dw);
        // The phase step from w to w + dw, without wrapping: the angle of b·conj(a).
        let step = (bi * ar - br * ai).atan2(br * ar + bi * ai);
        let tau = 2.0 * -step / dw;
        *d = (tau * (0.95 - 0.5 * r)).round().max(0.0) as usize;
    }
    let total: usize = delays.iter().sum();
    let pad = ((0.008 * rate).round() as usize).saturating_sub(total);
    (pad, delays)
}

/// A delay of up to `MAX_DELAY` samples on both channels.
struct Delay {
    ring: [Vec<f64>; CHANNELS],
    at: usize,
    samples: usize,
}

impl Delay {
    fn new() -> Self {
        Self {
            ring: [vec![0.0; MAX_DELAY + 1], vec![0.0; MAX_DELAY + 1]],
            at: 0,
            samples: 0,
        }
    }

    /// Put in a frame, take out the one `samples` ago.
    fn tick(&mut self, x: [f64; CHANNELS]) -> [f64; CHANNELS] {
        let n = self.ring[0].len();
        let read = (self.at + n - self.samples) % n;
        let mut out = [0.0; CHANNELS];
        for c in 0..CHANNELS {
            self.ring[c][self.at] = x[c];
            out[c] = self.ring[c][read];
        }
        self.at = (self.at + 1) % n;
        out
    }

    fn clear(&mut self) {
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.at = 0;
    }
}

fn one_pole(t_ms: f32, rate: f32) -> f32 {
    if t_ms > 0.0 {
        ((-1000.0 / t_ms) / (rate * 0.5)).exp()
    } else {
        0.0
    }
}

/// One band's compressor: the Compressor's core.
#[derive(Clone)]
struct Band {
    threshold: f32,
    exponent: f32,
    unity: bool,
    attack: f32,
    release: f32,
    follow: f32,
    gain: f32,
    bypass: bool,
    solo: bool,
    level: f32,
    gain1: f32,
    gain2: f32,
    ring: [Vec<f32>; CHANNELS],
    at: usize,
}

impl Band {
    fn new(rate: f64) -> Self {
        let length = band_delay(rate) + 1;
        Self {
            threshold: 1.0,
            exponent: 0.0,
            unity: true,
            attack: 0.0,
            release: 0.0,
            follow: 1.0,
            gain: 1.0,
            bypass: false,
            solo: false,
            level: 0.0,
            gain1: 1.0,
            gain2: 1.0,
            ring: [vec![0.0; length], vec![0.0; length]],
            at: 0,
        }
    }

    fn derive(&mut self, v: &[f64], rate: f32) {
        self.threshold = (v[THRESHOLD] as f32 * DB_LN).exp();
        let inverse = (1.0 / v[RATIO]) as f32;
        self.unity = inverse == 1.0;
        self.exponent = (f64::from(inverse) - 1.0) as f32;
        let (attack, release) = (v[ATTACK] as f32, v[RELEASE] as f32);
        self.attack = one_pole(attack, rate);
        self.release = one_pole(release, rate);
        let shortest = attack.min(release) * 0.5;
        self.follow = if shortest <= 0.0 {
            1.0
        } else {
            1.0 - ((-1000.0 / shortest) / (rate * 0.5)).exp()
        };
        self.gain = (v[GAIN] as f32 * DB_LN).exp();
        self.bypass = v[BYPASS] >= 0.5;
        self.solo = v[SOLO] >= 0.5;
    }

    /// One frame of the band's signal: its compressed, delayed output (before the output gain).
    fn tick(&mut self, s: [f32; CHANNELS], fall: f32) -> [f32; CHANNELS] {
        let m = (f64::from(s[0]).abs().max(f64::from(s[1]).abs())) as f32;
        if m > self.level {
            self.level = m;
        } else {
            if self.level.abs() < 1e-10 {
                self.level = 0.0;
            }
            self.level *= fall;
        }
        let e = self.level.max(1e-5);
        let g = if e < self.threshold || self.unity {
            1.0
        } else {
            (e / self.threshold).powf(self.exponent)
        };
        if self.gain1 > g {
            self.gain1 += (g - self.gain1) * (1.0 - self.attack);
        } else {
            self.gain1 = self.gain1 * self.release + (1.0 - self.release);
        }
        self.gain2 += (self.gain1 - self.gain2) * self.follow;
        let gain = if self.bypass {
            1.0
        } else {
            self.gain2 * self.gain
        };
        let n = self.ring[0].len();
        let mut out = [0.0; CHANNELS];
        for c in 0..CHANNELS {
            self.ring[c][self.at] = s[c];
        }
        self.at = (self.at + 1) % n;
        for (c, o) in out.iter_mut().enumerate() {
            *o = gain * self.ring[c][self.at];
        }
        out
    }

    fn reset(&mut self) {
        self.level = 0.0;
        self.gain1 = 1.0;
        self.gain2 = 1.0;
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.at = 0;
    }
}

/// The output limiter (a maximizer).
struct Limiter {
    bypass: bool,
    brickwall: bool,
    threshold: f32,
    post: f32,
    attack: f32,
    release: f32,
    smooth: f32,
    brick: f32,
    level: f32,
    gain1: f32,
    gain2: f32,
    window: [f32; LIMITER_WINDOW],
    window_at: usize,
    input: [[f32; LIMITER_IN + 1]; CHANNELS],
    input_at: usize,
    ahead: [[f32; LIMITER_AHEAD + 1]; CHANNELS],
    ahead_at: usize,
}

impl Limiter {
    fn new(rate: f32) -> Self {
        Self {
            bypass: false,
            brickwall: false,
            threshold: 1.0,
            post: 1.0,
            attack: 0.0,
            release: 0.0,
            smooth: 1.0 - (-1.0 / (0.001 * rate)).exp(),
            brick: ((-1000.0 / (200_000.0 / rate)) / (rate * 0.5)).exp(),
            level: 0.0,
            gain1: 1.0,
            gain2: 1.0,
            window: [1.0; LIMITER_WINDOW],
            window_at: 0,
            input: [[0.0; LIMITER_IN + 1]; CHANNELS],
            input_at: 0,
            ahead: [[0.0; LIMITER_AHEAD + 1]; CHANNELS],
            ahead_at: 0,
        }
    }

    fn derive(&mut self, v: &[f64], rate: f32) {
        let l = &v[LIMITER..];
        self.bypass = l[LIM_BYPASS] >= 0.5;
        self.brickwall = l[LIM_BRICKWALL] >= 0.5;
        self.threshold = (l[LIM_THRESHOLD] as f32 * DB_LN).exp();
        self.post = ((l[LIM_MARGIN] - l[LIM_THRESHOLD]) as f32 * DB_LN).exp();
        self.attack = one_pole(l[LIM_ATTACK] as f32, rate);
        self.release = one_pole(l[LIM_RELEASE] as f32, rate);
    }

    fn tick(&mut self, x: [f32; CHANNELS], fall: f32) -> [f32; CHANNELS] {
        // 63 samples in, then the detector.
        let mut a = [0.0f32; CHANNELS];
        let n = LIMITER_IN + 1;
        for c in 0..CHANNELS {
            self.input[c][self.input_at] = x[c];
        }
        self.input_at = (self.input_at + 1) % n;
        for (c, v) in a.iter_mut().enumerate() {
            *v = self.input[c][self.input_at];
        }
        let m = (f64::from(a[0]).abs().max(f64::from(a[1]).abs())) as f32;
        if m < self.level {
            // Flushed under -500 dB: 0.9985 of the smallest denormal rounds back to it, so it
            // would stick there, at a denormal's cost a sample, for ever after the input stops
            // (dev plan 18.1). Only a level over the threshold acts.
            self.level = super::flush(self.level * fall).max(m);
        } else {
            self.level = m;
        }
        let e = self.level;
        let gain = if self.brickwall {
            let target = if e < self.threshold {
                1.0
            } else {
                self.threshold / e
            };
            // The lowest target over the next 200 frames' worth of the window.
            for w in &mut self.window {
                *w = w.min(target);
            }
            let low = self.window[self.window_at];
            self.window[self.window_at] = 1.0;
            self.window_at = (self.window_at + 1) % LIMITER_WINDOW;
            if self.gain1 <= low {
                self.gain1 = self.gain1 * self.release + (1.0 - self.release);
            } else {
                self.gain1 = self.gain1 * self.brick + low * (1.0 - self.brick);
            }
            self.gain1 = self.gain1.min(low);
            self.gain1
        } else {
            let target = if e < self.threshold {
                1.0
            } else {
                (e / self.threshold).powf(0.05 - 1.0)
            };
            if self.gain1 <= target {
                self.gain1 = self.gain1 * self.release + (1.0 - self.release);
            } else {
                self.gain1 = self.gain1 * self.attack + target * (1.0 - self.attack);
            }
            if (self.gain1 - self.gain2).abs() <= 1e-9 {
                self.gain2 = self.gain1;
            } else {
                self.gain2 += (self.gain1 - self.gain2) * self.smooth;
            }
            self.gain2
        };
        // 199 samples later, the gain worked out from them.
        let n = LIMITER_AHEAD + 1;
        for c in 0..CHANNELS {
            self.ahead[c][self.ahead_at] = a[c];
        }
        self.ahead_at = (self.ahead_at + 1) % n;
        let mut y = [0.0f32; CHANNELS];
        for (c, v) in y.iter_mut().enumerate() {
            *v = gain * self.post * self.ahead[c][self.ahead_at];
        }
        y
    }

    fn reset(&mut self) {
        self.level = 0.0;
        self.gain1 = 1.0;
        self.gain2 = 1.0;
        self.window = [1.0; LIMITER_WINDOW];
        self.window_at = 0;
        self.input = [[0.0; LIMITER_IN + 1]; CHANNELS];
        self.ahead = [[0.0; LIMITER_AHEAD + 1]; CHANNELS];
        self.input_at = 0;
        self.ahead_at = 0;
    }
}

struct Multiband {
    rate: f64,
    values: [f64; PARAM_COUNT],
    output: f32,
    fall: f32,
    /// Per edge: its low-pass pair, the remainder's delay; the padding before the first edge,
    /// and the delays lining the low bands up.
    lows: [[Section; 2]; 3],
    edge_delays: [Delay; 3],
    pad: Delay,
    line_up: [Delay; 2],
    bands: [Band; BAND_COUNT],
    limiter: Limiter,
}

impl Multiband {
    fn new(rate: f64) -> Self {
        let identity = Section::new(Coefficients::IDENTITY);
        let mut m = Self {
            rate,
            values: [0.0; PARAM_COUNT],
            output: 1.0,
            fall: 0.03f32.powf(1.0 / (rate as f32 * 0.05)),
            lows: [[identity; 2]; 3],
            edge_delays: [Delay::new(), Delay::new(), Delay::new()],
            pad: Delay::new(),
            line_up: [Delay::new(), Delay::new()],
            bands: [
                Band::new(rate),
                Band::new(rate),
                Band::new(rate),
                Band::new(rate),
            ],
            limiter: Limiter::new(rate as f32),
        };
        for (v, p) in m.values.iter_mut().zip(PARAMS.iter()) {
            *v = p.default;
        }
        m.derive(true);
        m
    }

    fn derive(&mut self, crossover: bool) {
        let rate = self.rate as f32;
        self.output = (self.values[OUTPUT] as f32 * DB_LN).exp();
        for (b, band) in self.bands.iter_mut().enumerate() {
            band.derive(&self.values[BANDS + b * BAND_PARAMS..], rate);
        }
        self.limiter.derive(&self.values, rate);
        if crossover {
            let f = edges(&self.values);
            for (k, pair) in self.lows.iter_mut().enumerate() {
                let c = section(self.rate, f[k]);
                for s in pair.iter_mut() {
                    s.c = c;
                }
            }
            let (pad, d) = crossover_delays(self.rate, &self.values);
            self.pad.samples = pad.min(MAX_DELAY);
            for (delay, &n) in self.edge_delays.iter_mut().zip(&d) {
                delay.samples = n.min(MAX_DELAY);
            }
            self.line_up[0].samples = (d[1] + d[2]).min(MAX_DELAY);
            self.line_up[1].samples = d[2].min(MAX_DELAY);
        }
    }
}

impl NativeEffect for Multiband {
    fn set(&mut self, index: usize, value: f64) {
        if let Some(v) = self.values.get_mut(index) {
            if *v != value {
                *v = value;
                self.derive((CROSSOVERS..CROSSOVERS + 3).contains(&index));
            }
        }
    }

    fn latency(&self) -> usize {
        latency(self.rate, &self.values)
    }

    fn start(&mut self) {
        for pair in &mut self.lows {
            for s in pair.iter_mut() {
                *s = Section::new(s.c);
            }
        }
        for d in self
            .edge_delays
            .iter_mut()
            .chain(std::iter::once(&mut self.pad))
            .chain(self.line_up.iter_mut())
        {
            d.clear();
        }
        for b in &mut self.bands {
            b.reset();
        }
        self.limiter.reset();
    }

    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let soloing = self.bands.iter().any(|b| b.solo);
        for k in from..to {
            // The cascaded split: each edge's low band, and the delayed remainder less it.
            let mut rest = self.pad.tick([f64::from(io[0][k]), f64::from(io[1][k])]);
            let mut split = [[0.0f64; CHANNELS]; BAND_COUNT];
            for e in 0..3 {
                let mut low = [0.0; CHANNELS];
                for c in 0..CHANNELS {
                    let s = self.lows[e][0].tick(c, rest[c]);
                    low[c] = self.lows[e][1].tick(c, s);
                }
                let delayed = self.edge_delays[e].tick(rest);
                split[e] = low;
                for c in 0..CHANNELS {
                    rest[c] = delayed[c] - low[c];
                }
            }
            split[3] = rest;
            split[0] = self.line_up[0].tick(split[0]);
            split[1] = self.line_up[1].tick(split[1]);
            // Each band compressed, the output gain, the solos; summed.
            let mut sum = [0.0f32; CHANNELS];
            for (b, band) in self.bands.iter_mut().enumerate() {
                let s = [split[b][0] as f32, split[b][1] as f32];
                let y = band.tick(s, self.fall);
                if !soloing || band.solo {
                    for c in 0..CHANNELS {
                        sum[c] += y[c] * self.output;
                    }
                }
            }
            let out = if self.limiter.bypass {
                sum
            } else {
                self.limiter.tick(sum, self.fall)
            };
            io[0][k] = out[0];
            io[1][k] = out[1];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::render;
    use super::*;
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    fn defaults() -> Vec<f64> {
        PARAMS.iter().map(|p| p.default).collect()
    }

    fn tone(n: usize, hz: f64, db: f64) -> Vec<f32> {
        let a = 10f64.powf(db / 20.0);
        (0..n)
            .map(|i| (a * (2.0 * PI * hz * i as f64 / 48_000.0).sin()) as f32)
            .collect()
    }

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
    fn the_crossover_delays_match_the_reference_and_the_latency_follows_them() {
        // The study's numbers: 120/2000/10000 Hz give 185, 10, 2 at 44.1 kHz (353 with the
        // 8 ms floor) and 201, 11, 2 at 48 kHz (384).
        let v = defaults();
        assert_eq!(crossover_delays(44_100.0, &v), (156, [185, 10, 2]));
        assert_eq!(crossover_delays(48_000.0, &v), (170, [201, 11, 2]));
        assert_eq!(latency(44_100.0, &v), 353 + 43 + 262);
        assert_eq!(latency(48_000.0, &v), 384 + 47 + 262);
        let mut bypassed = v.clone();
        bypassed[LIMITER + LIM_BYPASS] = 1.0;
        assert_eq!(latency(48_000.0, &bypassed), 384 + 47);
    }

    #[test]
    fn with_nothing_to_do_the_bands_sum_back_to_the_input() {
        // Ratios of 1, the limiter bypassed: the split sums to a pure delay, so the render
        // (with that delay dropped) is the input, to the f32 rounding of the bands.
        let mut settings: BTreeMap<String, f64> =
            (1..=4).map(|b| (format!("Band {b} Ratio"), 1.0)).collect();
        settings.insert("Limiter Bypass".into(), 1.0);
        let x: Vec<f32> = (0..20_000)
            .map(|i| (((i as f32) * 0.011).sin() + ((i as f32) * 0.37).sin()) * 0.3)
            .collect();
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        let worst = out[0][2000..]
            .iter()
            .zip(&x[2000..])
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{worst}");
    }

    #[test]
    fn a_band_compresses_its_own_range_and_solo_isolates_it() {
        // A loud 60 Hz and a quiet 1 kHz; band 1 (below 120 Hz) at -30 dB, 10:1, the limiter
        // off. The bass comes down; the 1 kHz tone, in band 2, stays.
        let x: Vec<f32> = tone(48_000, 60.0, -6.0)
            .iter()
            .zip(tone(48_000, 1000.0, -20.0))
            .map(|(a, b)| a + b)
            .collect();
        let mut settings: BTreeMap<String, f64> = [
            ("Band 1 Threshold".to_string(), -30.0),
            ("Band 1 Ratio".to_string(), 10.0),
            ("Limiter Bypass".to_string(), 1.0),
        ]
        .into();
        let out = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        let settled = &out[0][24_000..];
        assert!(
            level_db(settled, 60.0) < -20.0,
            "{}",
            level_db(settled, 60.0)
        );
        assert!((level_db(settled, 1000.0) - -20.0).abs() < 0.5);
        settings.insert("Band 2 Solo".into(), 1.0);
        let solo = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 512).unwrap();
        // Band 2 alone: the 1 kHz tone; the bass only as the split leaks it (an upper band is
        // the delayed input less the low band, so it isn't a steep high-pass): over 20 dB down.
        assert!((level_db(&solo[0][24_000..], 1000.0) - -20.0).abs() < 1.0);
        assert!(level_db(&solo[0][24_000..], 60.0) < -26.0);
    }

    #[test]
    fn the_limiter_holds_its_margin_and_everything_is_the_same_at_any_block_size() {
        let x = tone(48_000, 500.0, 0.0);
        let settings: BTreeMap<String, f64> = [
            ("Limiter Threshold".to_string(), -12.0),
            ("Limiter Margin".to_string(), -1.0),
            ("Brickwall Limiter".to_string(), 1.0),
        ]
        .into();
        let a = render(&KIND, 48_000.0, &settings, &[x.clone(), x.clone()], 512).unwrap();
        let b = render(&KIND, 48_000.0, &settings, &[x.clone(), x], 41).unwrap();
        assert_eq!(a, b);
        // Brickwall: no sample past the margin, once the band ratios' compression is in.
        let margin = 10f32.powf(-1.0 / 20.0);
        let peak = a[0][10_000..].iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak <= margin * 1.0001, "{peak} vs {margin}");
        assert!(peak > margin * 0.9, "{peak}");
    }

    #[test]
    fn the_limiter_and_the_crossovers_fall_to_zero_after_the_input_stops() {
        // The limiter's level falls by 0.9985 a sample, which rounds the smallest denormal
        // back to itself: without the flush it would stay there for ever. The crossovers'
        // double state would decay through f64 denormals.
        let mut m = Multiband::new(48_000.0);
        let x = tone(9600, 1000.0, -3.0);
        let mut io = vec![x.clone(), x];
        m.process(&mut io, 0, 9600);
        let mut silence = vec![vec![0.0f32; 48_000]; CHANNELS];
        for _ in 0..3 {
            silence.iter_mut().for_each(|c| c.fill(0.0));
            m.process(&mut silence, 0, 48_000);
        }
        assert_eq!(m.limiter.level, 0.0);
        for edge in &m.lows {
            for section in edge {
                assert!(section.is_silent(), "{section:?}");
            }
        }
        assert!(silence.iter().flatten().all(|s| *s == 0.0));
    }
}
