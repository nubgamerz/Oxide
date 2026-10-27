//! Echo (studied in the research notes on `echo`, kept outside the repository), written from
//! the study's description:
//!
//! - **A feedback delay per channel:** each sample reads the line `d` samples back (whole
//!   samples, `d = int(t/1000·fs)` truncated, f32), scales it by Feedback (`r = tap·f`), runs it
//!   through the channel's Successive Echo Equalization (`E = EQ(r)`), and plays `y = x + E`: the
//!   dry sound always at unity, no dry or wet control. The line takes `clip(e·x + E)` (e = Echo
//!   Level, the clip at ±1), so Echo Level scales every repeat and the k-th has level e·f^k
//!   before the EQ; high feedback with EQ boost is held by the clip.
//! - **The line** is 200 + 2·fs samples, read before it is written, so a 0 ms delay (typed, or
//!   from a lane) reads what was written a whole line ago: a 2.005 s echo at
//!   44.1 kHz.
//! - **Successive Echo Equalization** ([`Eq`]) is no bank of peaks: seven running averages in
//!   series (boxcars of 2, 4 … 128 samples, each fed by the last, their running sums never
//!   recomputed, all f32) split the echo into eight octave-wide bands by their differences
//!   (B8 = u − m1 … B2 = m6 − m7, B1 = m7), weighted by 10^(dB/20) and summed in the order the
//!   study's decompile shows. It always runs (the reference's bypass test is inverted), so even at 0
//!   dB everywhere it colours the echoes slightly (each band delayed differently); its band edges
//!   are fixed in samples and follow the rate. Each repeat passes it again.
//! - **Echo Bounce** writes each side's line value into the other side's line: repeats
//!   alternate sides (ping-pong). The EQ gains are shared, the EQ state per channel.
//! - **Lock Left & Right Channels** is the editor's (the reference's `SetParameter` copies the left
//!   delay, feedback and level onto the right, and while locked each change to the other side):
//!   the parameters link in pairs (`ParamSpec::link` with a group); the DSP never sees it.
//! - **Delay Time Units** is a display switch: the DSP reads milliseconds.
//!
//! **Changes:** a new delay crossfades forwards over 100 ms from the old tap to the new
//! ([`Crossfade`]); Feedback and Echo Level glide there in the same 100 ms (the same thing for a
//! gain); the EQ's gains change at once. Lanes draw their lines sample by sample.
//! The reference's own crossfade runs backwards and runs the EQ twice a sample while it fades, and
//! the one it starts at every playback or render fades the input into the line from full to
//! nothing over the first 100 ms: neither is reproduced (TD132). A transport start empties the
//! lines and the EQ, as the reference's `Resume`. Latency 0.
//!
//! The reference's Number of Channels (its id 17) is the host's state, not a setting: the graph is
//! stereo, so it isn't offered.

use super::delay_line::{Crossfade, Line, Ramp};
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;

pub const LEFT_DELAY: usize = 0;
pub const RIGHT_DELAY: usize = 1;
pub const LEFT_FEEDBACK: usize = 2;
pub const RIGHT_FEEDBACK: usize = 3;
pub const LEFT_LEVEL: usize = 4;
pub const RIGHT_LEVEL: usize = 5;
/// EQ Band 1 (the lowest) … EQ Band 8 are parameters 6 to 13.
pub const FIRST_BAND: usize = 6;
pub const BANDS: usize = 8;
pub const LOCK: usize = 14;
pub const BOUNCE: usize = 15;
pub const UNITS: usize = 16;
pub const COUNT: usize = 17;

/// The longest delay (ms).
pub const MAX_MS: f64 = 2000.0;
/// A change's crossfade (ms).
pub const FADE_MS: f64 = 100.0;
/// The highest rate the lines are sized for up front; an instance made at a higher one sizes
/// for its own.
pub const MAX_RATE: f64 = 192_000.0;
/// The EQ's band labels in the editor (Hz at 44.1 kHz; the bands follow the rate).
pub const BAND_LABELS_HZ: [f64; BANDS] =
    [86.0, 172.0, 344.0, 689.0, 1400.0, 3000.0, 7400.0, 22000.0];
/// The running averages' lengths, stage 1 to 7.
const STAGES: usize = 7;

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
    decimals: 1,
    automatable: true,
    link: None,
    log_slider: false,
    format: None,
};

/// The switch Lock is, by name; each pair of parameters links through it in its own group.
const LOCK_NAME: &str = "Lock Left & Right Channels";

const TIME: ParamSpec = ParamSpec {
    unit: "ms",
    max: MAX_MS,
    link: Some("Lock Left & Right Channels:delay"),
    ..PARAM
};

const FEEDBACK: ParamSpec = ParamSpec {
    unit: "%",
    min: 0.1,
    default: 64.0,
    link: Some("Lock Left & Right Channels:feedback"),
    ..PARAM
};

const LEVEL: ParamSpec = ParamSpec {
    unit: "%",
    min: 0.1,
    default: 85.0,
    link: Some("Lock Left & Right Channels:level"),
    ..PARAM
};

const BAND: ParamSpec = ParamSpec {
    unit: "dB",
    min: -15.0,
    max: 15.0,
    ..PARAM
};

const SWITCH: ParamSpec = ParamSpec {
    max: 1.0,
    steps: 1,
    decimals: 0,
    ..PARAM
};

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Delay Time (Left Channel)",
        default: 348.8,
        ..TIME
    },
    ParamSpec {
        name: "Delay Time (Right Channel)",
        default: 232.6,
        ..TIME
    },
    ParamSpec {
        name: "Feedback (Left Channel)",
        ..FEEDBACK
    },
    ParamSpec {
        name: "Feedback (Right Channel)",
        ..FEEDBACK
    },
    ParamSpec {
        name: "Echo Level (Left Channel)",
        ..LEVEL
    },
    ParamSpec {
        name: "Echo Level (Right Channel)",
        ..LEVEL
    },
    ParamSpec {
        name: "EQ Band 1",
        default: -9.8,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 2",
        default: -9.9,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 3",
        default: -9.6,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 4",
        default: -9.5,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 5",
        default: -2.4,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 6",
        default: -7.0,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 7",
        default: -1.1,
        ..BAND
    },
    ParamSpec {
        name: "EQ Band 8",
        default: 0.0,
        ..BAND
    },
    // The editor's: it links the pairs above.
    ParamSpec {
        name: LOCK_NAME,
        automatable: false,
        ..SWITCH
    },
    ParamSpec {
        name: "Echo Bounce",
        ..SWITCH
    },
    // How the editor shows the delay times: a display switch.
    ParamSpec {
        name: "Delay Time Units",
        max: 2.0,
        steps: 2,
        decimals: 0,
        automatable: false,
        format: Some(units),
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "echo",
    name: "Echo",
    category: "Delay",
    params: &PARAMS,
    make: |rate| Box::new(Echo::new(rate)),
    response: Some(response),
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// A delay time (ms) in whole samples at `rate`: `int(t/1000·fs)` in f32, truncated.
pub fn delay_samples(rate: f64, ms: f64) -> usize {
    ((ms as f32 / 1000.0) * rate as f32).max(0.0) as usize
}

/// The line's length at `rate`: 200 + 2·fs samples.
pub fn line_length(rate: f64) -> usize {
    200 + (MAX_MS / 1000.0 * rate).ceil() as usize
}

/// How far back a delay of `samples` reads: the line is read before it is written, so 0 reads
/// what was written a whole line ago.
pub fn age_of(rate: f64, samples: usize) -> usize {
    if samples == 0 {
        line_length(rate)
    } else {
        samples
    }
}

/// A band's label for the editor at `rate` (the dialog's, which are 44.1 kHz's).
pub fn band_label_hz(rate: f64, band: usize) -> f64 {
    BAND_LABELS_HZ[band] * rate / 44_100.0
}

/// A gain of `db` (f32, as the reference stores it): 10^(dB/20) in double, kept in f32.
fn gain(db: f32) -> f32 {
    10f64.powf(f64::from(db) / 20.0) as f32
}

/// The EQ's response for one pass (dB) at `hz`, for plain `values` at `rate`: the eight bands'
/// differences of cascaded boxcars, weighted and summed (the editor's curve).
pub fn response(rate: f64, values: &[f64], hz: f64) -> f64 {
    let w = 2.0 * std::f64::consts::PI * hz / rate;
    // A boxcar of length l at w: (1/l)·Σ e^(−jwn).
    let boxcar = |l: f64| -> (f64, f64) {
        let (num_re, num_im) = (1.0 - (w * l).cos(), (w * l).sin());
        let (den_re, den_im) = (1.0 - w.cos(), w.sin());
        let d = den_re * den_re + den_im * den_im;
        if d < 1e-18 {
            return (1.0, 0.0);
        }
        (
            (num_re * den_re + num_im * den_im) / d / l,
            (num_im * den_re - num_re * den_im) / d / l,
        )
    };
    let mul = |a: (f64, f64), b: (f64, f64)| (a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
    // m[0] = 1 (the input), m[k] the k-th average.
    let mut m = [(1.0, 0.0); STAGES + 1];
    for k in 1..=STAGES {
        m[k] = mul(m[k - 1], boxcar(f64::from(1u32 << k)));
    }
    let g = |band: usize| 10f64.powf(values[FIRST_BAND + band] / 20.0);
    // Band 8 is u − m1 … band 2 is m6 − m7; band 1 is m7.
    let mut h = (g(0) * m[STAGES].0, g(0) * m[STAGES].1);
    for band in 1..BANDS {
        let (hi, lo) = (m[BANDS - 1 - band], m[BANDS - band]);
        h.0 += g(band) * (hi.0 - lo.0);
        h.1 += g(band) * (hi.1 - lo.1);
    }
    20.0 * (h.0 * h.0 + h.1 * h.1).sqrt().max(1e-12).log10()
}

/// Where each stage's ring starts in [`Eq::ring`] (the rings of 2, 4 … 128 samples end to end).
const OFFSETS: [usize; STAGES] = [0, 2, 6, 14, 30, 62, 126];
/// The rings' total length.
const RINGS: usize = 254;

/// One channel's Successive Echo Equalization: seven running averages in series and their
/// running sums.
#[derive(Clone)]
pub struct Eq {
    ring: [f32; RINGS],
    pos: [usize; STAGES],
    sum: [f32; STAGES],
}

impl Default for Eq {
    fn default() -> Self {
        Self {
            ring: [0.0; RINGS],
            pos: [0; STAGES],
            sum: [0.0; STAGES],
        }
    }
}

impl Eq {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// The echo `u` through the bands weighted by `g` (band 1 first).
    #[inline]
    pub fn tick(&mut self, u: f32, g: &[f32; BANDS]) -> f32 {
        let mut m = [0.0f32; STAGES + 1];
        m[0] = u;
        for k in 0..STAGES {
            let len = 2usize << k;
            let slot = OFFSETS[k] + self.pos[k];
            self.sum[k] = (self.sum[k] - self.ring[slot]) + m[k];
            self.ring[slot] = m[k];
            self.pos[k] = (self.pos[k] + 1) & (len - 1);
            // 2^-(k+1): exact.
            m[k + 1] = self.sum[k] * (1.0 / len as f32);
        }
        let b8 = m[0] - m[1];
        let b7 = m[1] - m[2];
        let b6 = m[2] - m[3];
        let b5 = m[3] - m[4];
        let b4 = m[4] - m[5];
        let b3 = m[5] - m[6];
        let b2 = m[6] - m[7];
        let b1 = m[7];
        // In the order the study's decompile adds them.
        g[1] * b2
            + g[7] * b8
            + g[6] * b7
            + g[5] * b6
            + g[4] * b5
            + g[3] * b4
            + g[2] * b3
            + g[0] * b1
    }
}

/// Both channels' tap ages.
type Ages = [usize; CHANNELS];

fn ages(rate: f64, values: &[f64; COUNT]) -> Ages {
    [LEFT_DELAY, RIGHT_DELAY].map(|i| age_of(rate, delay_samples(rate, values[i])))
}

struct Echo {
    rate: f64,
    values: [f64; COUNT],
    lines: [Line; CHANNELS],
    eq: [Eq; CHANNELS],
    delays: Crossfade<Ages>,
    /// f = Feedback/100 and e = Echo Level/100 per channel (f32).
    feedback: [Ramp; CHANNELS],
    level: [Ramp; CHANNELS],
    /// The bands' dB (f32) and their gains.
    db: [Ramp; BANDS],
    gains: [f32; BANDS],
    fade: usize,
}

impl Echo {
    fn new(rate: f64) -> Self {
        let values = PARAMS.map(|p| p.default);
        let capacity = line_length(rate.max(MAX_RATE));
        let mut lines = [Line::new(capacity), Line::new(capacity)];
        for line in &mut lines {
            line.reset(line_length(rate));
        }
        let fade = (rate * FADE_MS / 1000.0) as usize;
        let db: [Ramp; BANDS] = std::array::from_fn(|b| Ramp::new(values[FIRST_BAND + b] as f32));
        Self {
            rate,
            lines,
            eq: [Eq::default(), Eq::default()],
            delays: Crossfade::new(ages(rate, &values), fade),
            feedback: [LEFT_FEEDBACK, RIGHT_FEEDBACK].map(|i| Ramp::new(values[i] as f32 / 100.0)),
            level: [LEFT_LEVEL, RIGHT_LEVEL].map(|i| Ramp::new(values[i] as f32 / 100.0)),
            gains: db.map(|d| gain(d.value())),
            db,
            values,
            fade,
        }
    }

    fn update_gains(&mut self) {
        for (g, d) in self.gains.iter_mut().zip(&self.db) {
            *g = gain(d.value());
        }
    }
}

impl NativeEffect for Echo {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        match index {
            LEFT_DELAY | RIGHT_DELAY => self.delays.change(ages(self.rate, &self.values)),
            LEFT_FEEDBACK | RIGHT_FEEDBACK => {
                self.feedback[index - LEFT_FEEDBACK].to(value as f32 / 100.0, self.fade)
            }
            LEFT_LEVEL | RIGHT_LEVEL => {
                self.level[index - LEFT_LEVEL].to(value as f32 / 100.0, self.fade)
            }
            i if (FIRST_BAND..FIRST_BAND + BANDS).contains(&i) => {
                self.db[i - FIRST_BAND].to(value as f32, 0);
                self.update_gains();
            }
            _ => {}
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match index {
            LEFT_FEEDBACK | RIGHT_FEEDBACK => {
                self.values[index] = value;
                self.feedback[index - LEFT_FEEDBACK].to(value as f32 / 100.0, samples);
            }
            LEFT_LEVEL | RIGHT_LEVEL => {
                self.values[index] = value;
                self.level[index - LEFT_LEVEL].to(value as f32 / 100.0, samples);
            }
            i if (FIRST_BAND..FIRST_BAND + BANDS).contains(&i) => {
                self.values[index] = value;
                self.db[i - FIRST_BAND].to(value as f32, samples);
                self.update_gains();
            }
            // A delay from a lane crossfades as a change by hand does; switches act at once.
            _ => self.set(index, value),
        }
    }

    fn snap(&mut self) {
        self.delays.snap();
        for r in self
            .feedback
            .iter_mut()
            .chain(&mut self.level)
            .chain(&mut self.db)
        {
            r.snap();
        }
        self.update_gains();
    }

    /// As the reference's `Resume`: the lines and the EQ empty, the settings in use at once.
    fn start(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
        for eq in &mut self.eq {
            eq.clear();
        }
        self.snap();
    }

    // A sample at a time across both channels: they share the fade, glide and wow.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let bounce = self.values[BOUNCE] >= 0.5;
        let eq_moving = self.db.iter().any(Ramp::moving);
        for k in from..to {
            let (now, next) = self.delays.step();
            if eq_moving {
                for (g, d) in self.gains.iter_mut().zip(&mut self.db) {
                    if d.moving() {
                        d.tick();
                        *g = gain(d.value());
                    }
                }
            }
            let mut into = [0.0f32; CHANNELS];
            for c in 0..CHANNELS {
                let line = &self.lines[c];
                let mut tap = line.back(now[c]);
                if let Some((ages, w)) = next {
                    tap = (1.0 - w) * tap + w * line.back(ages[c]);
                }
                let r = tap * self.feedback[c].tick();
                let echo = self.eq[c].tick(r, &self.gains);
                let x = io[c][k];
                io[c][k] = x + echo;
                into[c] = (self.level[c].tick() * x + echo).clamp(-1.0, 1.0);
            }
            if bounce {
                into.swap(0, 1);
            }
            for (line, v) in self.lines.iter_mut().zip(into) {
                line.write(v);
            }
        }
    }
}

#[cfg(test)]
#[path = "echo_tests.rs"]
mod tests;
