//! Vintage Delay (studied in the research notes on `analog-delay`, kept outside the
//! repository), written from the study's description:
//!
//! - **One feedback delay per channel** of 16 s (sized at construction for the highest rate),
//!   read at a fractional position by the common four-point cubic ([`Line::cubic`]), the read
//!   point swaying ±0.1 ms by a slow sine, the "wow": once every 500 ms (Analog), 333 ms
//!   (Tape/Tube) or 750 ms (Tape). The steady delay is `round(fs·D/1000) + 1` samples.
//! - **In the loop**, each sample: `u = tap·f + x·drive` (Feedback f = 0..2; the drive 10^t in
//!   Tape mode, else 1; t = Trash/100), through the mode's shaper, then the mode's low-pass (an
//!   RBJ biquad in double: Analog 5500 Hz Q 0.38, Tape/Tube 4000 Hz Q 0.59, Tape 1500 Hz Q 0.9),
//!   written back. So even the first echo is shaped and filtered.
//!   - **Analog:** `sin(u·a·π/2)/sin(a·π/2)`, a = 1.14t + 0.1 (a quarter sine at t ≈ 0.79,
//!     folding above).
//!   - **Tape/Tube:** the same with b = 0.61t + 0.01, then ×10^(1.8t/20) and the soft clip
//!     `v/(1 + |v|)`.
//!   - **Tape:** `sign(u)·tanh(|u|^c)^(1/c)`, c = 6.5t + 1: a soft clip whose knee hardens with
//!     Trash. The study couldn't name the library call (tanh is its best guess) and read the
//!     sign as flipped on every pass; ours keeps the polarity ([`TAPE_FLIPS`], TD134).
//! - **Out:** the taps' mid/side, the side scaled by Spread/100 (0 % mono, 200 % twice as
//!   wide); above 100 % Feedback each wet tap through a soft limiter (unchanged below 0.95, a
//!   knee to 0.975 at 1, held there); `y = comp·wet·tap' + dry·x`, comp = 1 − t/2 in Tape mode
//!   (the drive's make-up), else 1.
//! - **A delay change glides:** the delay moves toward its target by at most one sample per
//!   sample (the read point stands still, or runs at double speed: a tape machine's pitch bend),
//!   in whole samples, the wow riding on top. A lane moves the target in its straight lines.
//! - **Tempo Sync** (ours to offer: the study found the sync variant in the DLL but couldn't
//!   tell whether the reference's instance has it): the delay is a note value (1/64 dotted to 8 bars,
//!   a bar being 4 quarter notes as the plug-in counts it) at the song's tempo where the audio
//!   is, read from the block's transport (the engine splits blocks at tempo changes); a tempo
//!   change glides like a delay change. Delay is then ignored.
//! - Mode applies at once (the filter's state kept); Dry, Wet, Feedback, Trash and Spread set by
//!   hand glide over 10 ms (the plug-in's callbacks weren't traced); lanes draw lines.
//!
//! Where the study found slips, ours does what was meant (TD134): the read position wraps by the
//! line's length (the plug-in wraps it one sample early, shortening the delay by a sample every
//! 16 s), the wow's phase is kept in double and wrapped (the plug-in's f32 sample counter steps
//! after 6 minutes), and Tape keeps the polarity. A transport start empties the lines and the
//! filters and starts at the settings, as the plug-in's reset. Latency 0.

use super::biquad::{Coefficients, Section};
use super::delay_line::{Line, Ramp};
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::{Transport, CHANNELS};
use std::f64::consts::PI;

pub const MODE: usize = 0;
pub const DRY: usize = 1;
pub const WET: usize = 2;
pub const DELAY: usize = 3;
pub const FEEDBACK: usize = 4;
pub const TRASH: usize = 5;
pub const SPREAD: usize = 6;
pub const SYNC: usize = 7;
pub const NOTE: usize = 8;
pub const COUNT: usize = 9;

pub const ANALOG: usize = 0;
pub const TAPE_TUBE: usize = 1;
pub const TAPE: usize = 2;
pub const MODES: [&str; 3] = ["Analog", "Tape/Tube", "Tape"];

pub const MIN_MS: f64 = 10.0;
pub const MAX_MS: f64 = 8000.0;
/// The wow's depth (ms) and each mode's period (ms).
pub const WOW_MS: f64 = 0.1;
pub const WOW_PERIOD_MS: [f64; 3] = [500.0, 333.0, 750.0];
/// Each mode's loop low-pass: frequency (Hz) and Q.
pub const FILTERS: [(f64, f64); 3] = [(5500.0, 0.38), (4000.0, 0.59), (1500.0, 0.9)];
/// The highest rate the lines are sized for up front (16 s at 192 kHz: about 12 MB a channel);
/// an instance made at a higher one sizes for its own.
pub const MAX_RATE: f64 = 192_000.0;
/// A change by hand glides over this long (ms).
const GLIDE_MS: f64 = 10.0;
/// Whether Tape flips the polarity on every pass, as the study read the plug-in's sign. Off:
/// nothing about a tape delay suggests it, and it is untested against a render (TD134). The
/// study's render 4 settles it; flip this if it shows the flip is real.
pub const TAPE_FLIPS: bool = false;

/// The note values Tempo Sync offers, in quarter notes, by index (the plug-in's table).
pub const NOTE_QUARTERS: [f64; 22] = [
    3.0 / 32.0,
    1.0 / 16.0,
    1.0 / 24.0,
    3.0 / 16.0,
    1.0 / 8.0,
    1.0 / 12.0,
    3.0 / 8.0,
    1.0 / 4.0,
    1.0 / 6.0,
    3.0 / 4.0,
    1.0 / 2.0,
    1.0 / 3.0,
    1.5,
    1.0,
    2.0 / 3.0,
    3.0,
    2.0,
    4.0 / 3.0,
    4.0,
    8.0,
    16.0,
    32.0,
];
pub const NOTE_NAMES: [&str; 22] = [
    "1/64 dotted",
    "1/64",
    "1/64 triplet",
    "1/32 dotted",
    "1/32",
    "1/32 triplet",
    "1/16 dotted",
    "1/16",
    "1/16 triplet",
    "1/8 dotted",
    "1/8",
    "1/8 triplet",
    "1/4 dotted",
    "1/4",
    "1/4 triplet",
    "1/2 dotted",
    "1/2",
    "1/2 triplet",
    "1 bar",
    "2 bars",
    "4 bars",
    "8 bars",
];

fn mode_name(v: f64) -> String {
    MODES[(v.round().max(0.0) as usize).min(2)].into()
}

fn note_name(v: f64) -> String {
    NOTE_NAMES[(v.round().max(0.0) as usize).min(NOTE_NAMES.len() - 1)].into()
}

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "%",
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

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Mode",
        unit: "",
        max: 2.0,
        steps: 2,
        decimals: 0,
        format: Some(mode_name),
        ..PARAM
    },
    ParamSpec {
        name: "Dry Out",
        default: 70.0,
        ..PARAM
    },
    ParamSpec {
        name: "Wet Out",
        default: 30.0,
        ..PARAM
    },
    ParamSpec {
        name: "Delay",
        unit: "ms",
        min: MIN_MS,
        max: MAX_MS,
        default: 200.0,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Feedback",
        max: 200.0,
        default: 20.0,
        ..PARAM
    },
    ParamSpec {
        name: "Trash",
        default: 25.0,
        ..PARAM
    },
    ParamSpec {
        name: "Spread",
        max: 200.0,
        default: 100.0,
        ..PARAM
    },
    ParamSpec {
        name: "Tempo Sync",
        unit: "",
        max: 1.0,
        steps: 1,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Sync Note",
        unit: "",
        max: (NOTE_NAMES.len() - 1) as f64,
        default: 13.0,
        steps: (NOTE_NAMES.len() - 1) as u32,
        decimals: 0,
        format: Some(note_name),
        ..PARAM
    },
];

pub static KIND: Kind = Kind {
    id: "vintage-delay",
    name: "Vintage Delay",
    category: "Delay",
    params: &PARAMS,
    make: |rate| Box::new(AnalogDelay::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// The line's length at `rate`: 16 s and one sample, as the plug-in's.
pub fn line_length(rate: f64) -> usize {
    (2.0 * MAX_MS * rate / 1000.0 + 0.5) as usize + 1
}

/// The steady delay in whole samples for `ms` at `rate`: `round(fs·D/1000) + 1`.
pub fn delay_samples(rate: f64, ms: f64) -> usize {
    (rate * ms / 1000.0 + 0.5) as usize + 1
}

/// A quarter note's length (ms) at the tempo of `meter`'s segment: laid out there in samples
/// by the meter, which does the tempo's arithmetic.
pub fn quarter_ms(meter: &crate::graph::Meter) -> f64 {
    let samples =
        meter.sample_at_quarter_notes(meter.origin_quarter_notes + 1.0) - meter.origin_sample;
    samples / f64::from(meter.sample_rate.max(1)) * 1000.0
}

/// Note value `note`'s length (ms) for a quarter note of `quarter_ms`, in the plug-in's range.
pub fn note_ms(quarter_ms: f64, note: usize) -> f64 {
    (NOTE_QUARTERS[note.min(NOTE_QUARTERS.len() - 1)] * quarter_ms).clamp(MIN_MS, MAX_MS)
}

/// The loop's low-pass for `mode` at `rate`: the RBJ cookbook's (in double); none above half
/// the rate.
pub fn filter(rate: f64, mode: usize) -> Coefficients {
    let (f, q) = FILTERS[mode.min(2)];
    if f > rate / 2.0 {
        return Coefficients::IDENTITY;
    }
    let w = 2.0 * PI * f / rate;
    let (cos, alpha) = (w.cos(), w.sin() / (2.0 * q));
    Coefficients::new(
        (1.0 - cos) / 2.0,
        1.0 - cos,
        (1.0 - cos) / 2.0,
        1.0 + alpha,
        -2.0 * cos,
        1.0 - alpha,
    )
}

/// The loop's shaper for a mode and t = Trash/100, with the drive into the loop and the wet
/// make-up that go with it.
#[derive(Debug, Clone, Copy)]
pub struct Shaper {
    mode: usize,
    /// The sine shapers' a·π/2 (or b·π/2) and 1/sin of it.
    k: f32,
    norm: f32,
    /// Tape/Tube's gain before its soft clip.
    tube: f32,
    /// Tape's exponent c and 1/c.
    power: f32,
    root: f32,
    /// Tape's drive into the loop, and the wet make-up.
    pub drive: f32,
    pub comp: f32,
}

impl Shaper {
    pub fn new(mode: usize, t: f32) -> Self {
        let half_pi = std::f32::consts::FRAC_PI_2;
        let k = match mode {
            ANALOG => (1.14 * t + 0.1) * half_pi,
            _ => (0.61 * t + 0.01) * half_pi,
        };
        let power = 6.5 * t + 1.0;
        let tape = mode == TAPE;
        Self {
            mode,
            k,
            norm: 1.0 / k.sin(),
            tube: 10f32.powf(1.8 * t / 20.0),
            power,
            root: 1.0 / power,
            drive: if tape { 10f32.powf(t) } else { 1.0 },
            comp: if tape { 1.0 - 0.5 * t } else { 1.0 },
        }
    }

    /// The shaped sample.
    #[inline]
    pub fn apply(&self, u: f32) -> f32 {
        match self.mode {
            ANALOG => (u * self.k).sin() * self.norm,
            TAPE_TUBE => {
                let mut v = (u * self.k).sin() * self.norm;
                if self.tube != 1.0 {
                    v *= self.tube;
                    v = if v > 0.0 {
                        v / (1.0 + v)
                    } else {
                        v / (1.0 - v)
                    };
                }
                v
            }
            _ => {
                let v = u.abs().powf(self.power).tanh().powf(self.root);
                let positive = (u >= 0.0) != TAPE_FLIPS;
                if positive {
                    v
                } else {
                    -v
                }
            }
        }
    }
}

/// The soft limiter on the wet taps above 100 % Feedback.
#[inline]
pub fn limit(x: f32) -> f32 {
    let a = x.abs();
    if a < 0.95 {
        x
    } else if a <= 1.0 {
        let d = a - 0.95;
        (0.95 + d / (1.0 + (d / 0.05) * (d / 0.05))).copysign(x)
    } else {
        0.975f32.copysign(x)
    }
}

struct AnalogDelay {
    rate: f64,
    values: [f64; COUNT],
    lines: [Line; CHANNELS],
    /// The loop's low-pass, with each channel's state.
    filter: Section,
    shaper: Shaper,
    /// Where the delay is heading (ms) and where it is, and the latter in whole samples.
    target: Ramp,
    delay_ms: f64,
    whole: usize,
    /// The wow's phase (radians), its step a sample, and its depth (samples).
    phase: f64,
    omega: f64,
    depth: f64,
    /// Dry, Wet, Feedback, Trash and Spread as fractions (/100).
    dry: Ramp,
    wet: Ramp,
    feedback: Ramp,
    trash: Ramp,
    spread: Ramp,
    /// A quarter note (ms) at the tempo the last block's transport gave.
    quarter_ms: f64,
    glide: usize,
}

impl AnalogDelay {
    fn new(rate: f64) -> Self {
        let values = PARAMS.map(|p| p.default);
        let capacity = line_length(rate.max(MAX_RATE));
        let mut lines = [Line::new(capacity), Line::new(capacity)];
        for line in &mut lines {
            line.reset(line_length(rate));
        }
        let frac = |i: usize| Ramp::new(values[i] as f32 / 100.0);
        let mut fx = Self {
            rate,
            lines,
            filter: Section::new(Coefficients::IDENTITY),
            shaper: Shaper::new(ANALOG, 0.25),
            target: Ramp::new(values[DELAY] as f32),
            delay_ms: values[DELAY],
            whole: delay_samples(rate, values[DELAY]),
            phase: 0.0,
            omega: 0.0,
            depth: rate * WOW_MS / 1000.0,
            dry: frac(DRY),
            wet: frac(WET),
            feedback: frac(FEEDBACK),
            trash: frac(TRASH),
            spread: frac(SPREAD),
            // Until a transport says otherwise: 120 quarter notes a minute (`Meter::default`).
            quarter_ms: 500.0,
            glide: (rate * GLIDE_MS / 1000.0) as usize,
            values,
        };
        fx.mode_changed();
        fx
    }

    fn mode(&self) -> usize {
        (self.values[MODE].round().max(0.0) as usize).min(2)
    }

    /// The mode's wow period, filter and shaper; the filters keep their state.
    fn mode_changed(&mut self) {
        let mode = self.mode();
        self.omega = 2.0 * PI / (self.rate * WOW_PERIOD_MS[mode] / 1000.0);
        self.filter.c = filter(self.rate, mode);
        self.shaper = Shaper::new(mode, self.trash.value());
    }

    fn synced(&self) -> bool {
        self.values[SYNC] >= 0.5
    }

    /// The delay the settings ask for (ms): the note at the tempo, or Delay.
    fn wanted_ms(&self) -> f64 {
        if self.synced() {
            note_ms(self.quarter_ms, self.values[NOTE].round().max(0.0) as usize)
        } else {
            self.values[DELAY]
        }
    }

    fn retarget(&mut self) {
        let ms = self.wanted_ms() as f32;
        if ms != self.target.target() {
            self.target.to(ms, 0);
        }
    }

    /// Trash moved: the shaper follows (a glide updates it every sample).
    fn trash_changed(&mut self) {
        if !self.trash.moving() {
            self.shaper = Shaper::new(self.mode(), self.trash.value());
        }
    }

    fn fraction(&mut self, index: usize) -> Option<&mut Ramp> {
        match index {
            DRY => Some(&mut self.dry),
            WET => Some(&mut self.wet),
            FEEDBACK => Some(&mut self.feedback),
            TRASH => Some(&mut self.trash),
            SPREAD => Some(&mut self.spread),
            _ => None,
        }
    }
}

impl NativeEffect for AnalogDelay {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        let glide = self.glide;
        match index {
            MODE => self.mode_changed(),
            DELAY | SYNC | NOTE => self.retarget(),
            _ => {
                if let Some(r) = self.fraction(index) {
                    r.to(value as f32 / 100.0, glide);
                }
                if index == TRASH {
                    self.trash_changed();
                }
            }
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match index {
            // The target moves in the lane's line; the glide follows it.
            DELAY => {
                self.values[DELAY] = value;
                if !self.synced() {
                    self.target.to(value as f32, samples);
                }
            }
            _ => match self.fraction(index) {
                Some(r) => {
                    r.to(value as f32 / 100.0, samples);
                    self.values[index] = value;
                    if index == TRASH {
                        self.trash_changed();
                    }
                }
                None => self.set(index, value),
            },
        }
    }

    fn snap(&mut self) {
        for r in [
            &mut self.target,
            &mut self.dry,
            &mut self.wet,
            &mut self.feedback,
            &mut self.trash,
            &mut self.spread,
        ] {
            r.snap();
        }
        self.delay_ms = f64::from(self.target.value());
        self.whole = delay_samples(self.rate, self.delay_ms);
        self.shaper = Shaper::new(self.mode(), self.trash.value());
    }

    /// The tempo where the audio is: a synced delay heads for its note there.
    fn transport(&mut self, transport: &Transport) {
        let quarter = quarter_ms(&transport.meter);
        if quarter != self.quarter_ms {
            self.quarter_ms = quarter;
            if self.synced() {
                self.retarget();
            }
        }
    }

    /// As the plug-in's reset: the lines and filters empty, the wow from its start, the delay
    /// where the settings put it (no glide).
    fn start(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
        self.filter = Section::new(filter(self.rate, self.mode()));
        self.phase = 0.0;
        self.snap();
    }

    // A sample at a time across both channels: they share the fade, glide and wow.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let step = 1000.0 / self.rate;
        for k in from..to {
            // The delay heads for its target at most a sample a sample.
            let target = f64::from(self.target.tick());
            if self.delay_ms != target {
                let delta = target - self.delay_ms;
                self.delay_ms = if (delta / step).abs() > 1.0 {
                    self.delay_ms + step.copysign(delta)
                } else {
                    target
                };
                self.whole = delay_samples(self.rate, self.delay_ms);
            }
            let wow = self.phase.sin() * self.depth;
            self.phase += self.omega;
            if self.phase >= 2.0 * PI {
                self.phase -= 2.0 * PI;
            }
            let age = self.whole as f64 - wow;
            if self.trash.moving() {
                self.trash.tick();
                self.shaper = Shaper::new(self.mode(), self.trash.value());
            }
            let (dry, wet, f, spread) = (
                self.dry.tick(),
                self.wet.tick(),
                self.feedback.tick(),
                self.spread.tick(),
            );
            let taps = [self.lines[0].cubic(age), self.lines[1].cubic(age)];
            let x = [io[0][k], io[1][k]];
            for c in 0..CHANNELS {
                let u = taps[c] * f + x[c] * self.shaper.drive;
                let v = self.shaper.apply(u);
                let v = self.filter.tick(c, f64::from(v)) as f32;
                // Below about -500 dB a dying loop would run on through denormals (TD128).
                self.lines[c].write(super::flush(v));
            }
            let mid = taps[0] + taps[1];
            let side = (taps[0] - taps[1]) * spread;
            let mut out = [(mid + side) * 0.5, (mid - side) * 0.5];
            if f > 1.0 {
                out = out.map(limit);
            }
            let gain = self.shaper.comp * wet;
            for c in 0..CHANNELS {
                io[c][k] = gain * out[c] + dry * x[c];
            }
        }
    }
}

#[cfg(test)]
#[path = "analog_delay_tests.rs"]
mod tests;
