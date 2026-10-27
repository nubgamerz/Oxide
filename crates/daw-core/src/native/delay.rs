//! Delay (studied in the research notes on `delay`, kept outside the repository), written from
//! the study's description:
//!
//! - **One tap per channel, no feedback:** `y = (1 − m)·x[n] + ±m·x[n − d]`, m = Mix/100 in f32,
//!   the wet tap negated by Invert; the left settings on the left channel, the right on the
//!   right, independent (no cross-feed).
//! - **Whole-sample delays** from −500 to +500 ms: `d = int(t/1000·fs + 0.5)` (C truncation
//!   toward zero, so −10.3 samples is −9; f32, [`delay_samples`]). No interpolation.
//! - **Negative times** put the copy *ahead* of the dry signal. The reference runs every Delay half a
//!   second late for them (latency ⌊(fs + 200)/2⌋ always); ours runs only as late as the most
//!   negative delay set ([`latency`], 0 when neither is negative), reported through
//!   `Kind::latency` so the graph compensates it, and the DSP reads its dry tap that far back.
//!   Delay times therefore have no lanes: a lane would move the latency while playing.
//! - **Changes crossfade forwards over 100 ms:** a new delay or Invert fades the output from the
//!   old taps to the new in a straight line ([`Crossfade`]), and Mix glides there in the same
//!   time (the same thing for a gain), where the reference's fade runs backwards (it jumps to the new
//!   settings, fades back to the old and snaps to the new again). A Mix lane draws its lines
//!   sample by sample. A transport start empties the lines and starts at the settings, as
//!   the reference's `Resume`.
//! - **Delay Time Units** is a display switch (ms, beats, samples): the DSP always
//!   reads milliseconds. The editor shows the beats through the song's tempo, honestly, where
//!   the reference's "beats" are a fixed 500 ms.
//!
//! The reference's Number of Channels (its id 7) is the host's state, not a setting: the graph is
//! stereo, so it isn't offered.

use super::delay_line::{Crossfade, Line, Ramp};
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

pub const LEFT_DELAY: usize = 0;
pub const LEFT_INVERT: usize = 1;
pub const LEFT_MIX: usize = 2;
pub const RIGHT_DELAY: usize = 3;
pub const RIGHT_INVERT: usize = 4;
pub const RIGHT_MIX: usize = 5;
pub const UNITS: usize = 6;
pub const COUNT: usize = 7;

/// Each channel's delay, invert and mix parameters.
const DELAY: [usize; CHANNELS] = [LEFT_DELAY, RIGHT_DELAY];
const INVERT: [usize; CHANNELS] = [LEFT_INVERT, RIGHT_INVERT];
const MIX: [usize; CHANNELS] = [LEFT_MIX, RIGHT_MIX];

/// The longest delay either way (ms).
pub const MAX_MS: f64 = 500.0;
/// A change's crossfade (ms).
pub const FADE_MS: f64 = 100.0;
/// The highest rate the lines are sized for up front; an instance made at a higher one sizes
/// for its own.
pub const MAX_RATE: f64 = 192_000.0;

fn units(v: f64) -> String {
    match v.round() as i64 {
        1 => "Beats",
        2 => "Samples",
        _ => "Milliseconds",
    }
    .into()
}

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0,
    max: 100.0,
    default: 0.0,
    steps: 0,
    decimals: 0,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

const TIME: ParamSpec = ParamSpec {
    unit: "ms",
    min: -MAX_MS,
    max: MAX_MS,
    decimals: 1,
    // It sets the latency: a lane would move it while playing.
    automatable: false,
    ..PARAM
};

const SWITCH: ParamSpec = ParamSpec {
    max: 1.0,
    steps: 1,
    ..PARAM
};

const MIX_PARAM: ParamSpec = ParamSpec { unit: "%", ..PARAM };

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Left Channel Delay Time",
        default: 120.0,
        ..TIME
    },
    ParamSpec {
        name: "Invert Left Channel",
        ..SWITCH
    },
    ParamSpec {
        name: "Left Channel Mix",
        default: 75.0,
        ..MIX_PARAM
    },
    ParamSpec {
        name: "Right Channel Delay Time",
        default: 180.0,
        ..TIME
    },
    ParamSpec {
        name: "Invert Right Channel",
        ..SWITCH
    },
    ParamSpec {
        name: "Right Channel Mix",
        default: 65.0,
        ..MIX_PARAM
    },
    // How the editor shows the delay times: a display switch.
    ParamSpec {
        name: "Delay Time Units",
        max: 2.0,
        steps: 2,
        automatable: false,
        format: Some(units),
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "delay",
    name: "Delay",
    category: "Delay",
    params: &PARAMS,
    make: |rate| Box::new(Delay::new(rate)),
    response: None,
    transfer: None,
    latency: Some(latency),
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// A delay time (ms) in whole samples at `rate`: `int(t/1000·fs + 0.5)` in f32, truncated
/// toward zero as C does (so negative times round toward zero: −10.3 samples is −9, −10.5 is
/// −10).
pub fn delay_samples(rate: f64, ms: f64) -> i64 {
    ((ms as f32 / 1000.0) * rate as f32 + 0.5) as i64
}

/// The latency for plain `values` at `rate`: as far as the most negative delay reaches ahead,
/// 0 when neither is negative.
pub fn latency(rate: f64, values: &[f64]) -> usize {
    let earliest = DELAY
        .iter()
        .map(|&i| delay_samples(rate, values[i]))
        .min()
        .unwrap_or(0);
    (-earliest).max(0) as usize
}

/// One channel's taps: the ages read back for the dry and the wet signal (1: the sample just
/// written), and the wet tap's sign.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Tap {
    dry: usize,
    wet: usize,
    sign: f32,
}

type Taps = [Tap; CHANNELS];

/// Both channels' taps for plain `values` at `rate`: the dry tap as far back as the latency,
/// the wet tap its delay further (never ahead of the newest sample: the latency covers it).
fn taps(rate: f64, values: &[f64; COUNT]) -> Taps {
    let late = latency(rate, values) as i64;
    std::array::from_fn(|c| Tap {
        dry: (1 + late) as usize,
        wet: (1 + late + delay_samples(rate, values[DELAY[c]])) as usize,
        sign: if values[INVERT[c]] >= 0.5 { -1.0 } else { 1.0 },
    })
}

/// The ages a line must reach at `rate`: the latency plus the longest delay, and the newest.
fn line_length(rate: f64) -> usize {
    2 * delay_samples(rate, MAX_MS).unsigned_abs() as usize + 2
}

struct Delay {
    rate: f64,
    values: [f64; COUNT],
    lines: [Line; CHANNELS],
    taps: Crossfade<Taps>,
    /// Each channel's m = Mix/100 (f32).
    mix: [Ramp; CHANNELS],
    fade: usize,
}

impl Delay {
    fn new(rate: f64) -> Self {
        let values = PARAMS.map(|p| p.default);
        let capacity = line_length(rate.max(MAX_RATE));
        let mut lines = [Line::new(capacity), Line::new(capacity)];
        for line in &mut lines {
            line.reset(line_length(rate));
        }
        let fade = (rate * FADE_MS / 1000.0) as usize;
        Self {
            rate,
            taps: Crossfade::new(taps(rate, &values), fade),
            mix: MIX.map(|i| Ramp::new(values[i] as f32 / 100.0)),
            values,
            lines,
            fade,
        }
    }
}

impl NativeEffect for Delay {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        match index {
            LEFT_MIX | RIGHT_MIX => {
                let c = usize::from(index == RIGHT_MIX);
                self.mix[c].to(value as f32 / 100.0, self.fade);
            }
            UNITS => {}
            _ => self.taps.change(taps(self.rate, &self.values)),
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match index {
            LEFT_MIX | RIGHT_MIX => {
                self.values[index] = value;
                let c = usize::from(index == RIGHT_MIX);
                self.mix[c].to(value as f32 / 100.0, samples);
            }
            // A switch from a lane crossfades as a change by hand does.
            _ => self.set(index, value),
        }
    }

    fn snap(&mut self) {
        self.taps.snap();
        for m in &mut self.mix {
            m.snap();
        }
    }

    /// As the reference's `Resume`: the lines empty, the settings in use at once.
    fn start(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
        self.snap();
    }

    fn latency(&self) -> usize {
        latency(self.rate, &self.values)
    }

    // A sample at a time across both channels: they share the fade, glide and wow.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        for k in from..to {
            let (now, next) = self.taps.step();
            for c in 0..CHANNELS {
                let line = &mut self.lines[c];
                line.write(io[c][k]);
                let m = self.mix[c].tick();
                let out = |t: Tap| (t.sign * m) * line.back(t.wet) + (1.0 - m) * line.back(t.dry);
                let mut y = out(now[c]);
                if let Some((taps, w)) = next {
                    y = (1.0 - w) * y + w * out(taps[c]);
                }
                io[c][k] = y;
            }
        }
    }
}

#[cfg(test)]
#[path = "delay_tests.rs"]
mod tests;
