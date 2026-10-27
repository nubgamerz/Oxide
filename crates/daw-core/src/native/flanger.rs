//! Flanger: a through-zero flanger (studied in the research notes on `flanger`, kept outside the
//! repository), written from the study's description:
//!
//! - **A centre 5082 samples back** in a line per channel, at any rate: the effect's latency
//!   ([`LATENCY`], reported through `Kind::latency` so the graph compensates it), so its taps can
//!   read both *behind* the centre (older) and *ahead* of it (newer).
//! - **The sweep** moves P between the Initial and Final Delay Times (in samples, up to 4990):
//!   `P = (w(φ) + 1)/2·(final − initial) + initial`, w a sine or a triangle of the phase φ,
//!   which steps 2π·rate/fs a sample from -π/2 (the sweep starts at Initial); the right
//!   channel's phase leads by Stereo Phasing.
//! - **Two taps through zero** (Special Effects off, the default): one P behind the centre, one
//!   P ahead, both read by the 32-tap windowed sinc of 240 phases ([`Sinc`]);
//!   `y = (1 − m)·c − m/2·T_behind − s·m/2·T_ahead` (s = −1 when Inverted), so where the taps
//!   meet at the centre they cancel it: at Mix 50 % a null, at 100 % the centre negated.
//!   **One tap** (Special Effects on): `y = (1 − m)·c + s·m·T_behind`.
//! - **Feedback rewrites the centre sample**, `c ← c − s·fb·T_behind` (one tap: `+`), so it
//!   recirculates through later behind taps and never reaches earlier ahead ones. The centre
//!   and the output are clipped at ±1.
//!
//! Where the reference evidently means something else, ours does that and says so (TD136):
//! - **The box labelled "Sinusoidal" gives a sine** (the reference's selects the triangle: its
//!   flag is the dialog's `dolinear`).
//! - **The taps are symmetric about the centre**, P behind and P ahead, and P goes down to 0,
//!   so they meet at the centre and null through zero; the reference's behind tap sits one sample
//!   newer (P − 1 behind) with P at least one sample, so its pair centres half a sample ahead
//!   of the dry sound and never meets. [`REFERENCE_TAPS`] brings its back in one line.
//! - **The triangle follows the phase**, as the sine does, so Stereo Phasing offsets it
//!   properly (the reference's reflects a position, starts both sides moving up and restarts
//!   at Initial on any change but the rate's); the phase wraps (the reference's grows without
//!   bound).
//!
//! The reference's Modulation Rate (Beats) (its id 9, 2/rate: beats at a fixed 120 BPM) is the
//! same setting as the rate: it isn't offered, and the editor shows the beats beside the
//! rate.
//! Changes: the times, Stereo Phasing, rate, Mix and Feedback glide over 10 ms by hand (lanes
//! draw lines); the switches apply at once. A transport start empties the lines and starts the
//! sweep at -π/2, as the reference's `Resume`.

use super::delay_line::{Line, Ramp, Sinc, SINC_TAPS};
use super::{Kind, NativeEffect, ParamSpec};
use crate::graph::CHANNELS;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub const INITIAL: usize = 0;
pub const FINAL: usize = 1;
pub const PHASING: usize = 2;
pub const RATE: usize = 3;
pub const MIX: usize = 4;
pub const FEEDBACK: usize = 5;
pub const INVERTED: usize = 6;
pub const SPECIAL: usize = 7;
pub const SINUSOIDAL: usize = 8;
pub const COUNT: usize = 9;

/// The latency at every rate: the centre's age less one.
pub const LATENCY: usize = 5082;
/// The centre's age in the line (1: the sample just written).
const CENTRE: usize = LATENCY + 1;
/// The furthest a tap goes from the centre (samples).
pub const MAX_SWEEP: f64 = 4990.0;
/// The sinc's phases.
pub const SINC_PHASES: usize = 240;
/// Whether the taps sit where the reference's do: the behind tap P − 1 samples behind the centre
/// and P at least 1 (a pair centred half a sample ahead of the dry sound, never meeting). Off:
/// symmetric about the centre, P from 0, meeting there (TD136). Flip it (and
/// `delay_line::SINC_REFERENCE_GAIN`) to bring back the reference's arithmetic and null
/// against the reference renders.
pub const REFERENCE_TAPS: bool = false;
/// A change by hand glides over this long (ms).
const GLIDE_MS: f64 = 10.0;

/// The line: the centre, the furthest tap behind it and the sinc's reach past that.
const LINE: usize = CENTRE + MAX_SWEEP as usize + 2 * SINC_TAPS + 8;

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

const SWITCH: ParamSpec = ParamSpec {
    unit: "",
    max: 1.0,
    steps: 1,
    decimals: 0,
    ..PARAM
};

const TIME: ParamSpec = ParamSpec {
    unit: "ms",
    max: 20.0,
    decimals: 2,
    ..PARAM
};

/// The default rate: 8.621 beats at 120 BPM (2/rate).
const DEFAULT_RATE: f64 = 2.0 / 8.621;

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Initial Delay Time",
        ..TIME
    },
    ParamSpec {
        name: "Final Delay Time",
        default: 4.16,
        ..TIME
    },
    ParamSpec {
        name: "Stereo Phasing",
        unit: "°",
        max: 360.0,
        default: 28.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Modulation Rate",
        unit: "Hz",
        min: 0.001,
        max: 60.0,
        default: DEFAULT_RATE,
        decimals: 3,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Mix",
        default: 40.0,
        ..PARAM
    },
    ParamSpec {
        name: "Feedback",
        default: 60.0,
        ..PARAM
    },
    ParamSpec {
        name: "Inverted Mode",
        ..SWITCH
    },
    ParamSpec {
        name: "Special Effects Mode",
        ..SWITCH
    },
    // On: a sine (the reference's box of that name selects the triangle).
    ParamSpec {
        name: "Sinusoidal Mode",
        default: 1.0,
        ..SWITCH
    },
];

pub static KIND: Kind = Kind {
    id: "flanger",
    name: "Flanger",
    category: "Modulation",
    params: &PARAMS,
    make: |rate| Box::new(Flanger::new(rate)),
    response: None,
    transfer: None,
    latency: Some(|_, _| LATENCY),
    capture: None,
    learns: None,
    reads: None,
    prepares: None,
};

/// A delay time (ms) as the sweep's distance from the centre at `rate` (samples): from 0 (1 with
/// the reference's taps) to 4990.
pub fn sweep_samples(rate: f64, ms: f64) -> f64 {
    let lowest = if REFERENCE_TAPS { 1.0 } else { 0.0 };
    (rate * ms / 1000.0).clamp(lowest, MAX_SWEEP)
}

/// The sweep's waveform at phase `phi` (radians, any): -1 at -π/2, +1 at π/2; a sine, or a
/// triangle through the same points.
pub fn wave(phi: f64, sine: bool) -> f64 {
    if sine {
        return phi.sin();
    }
    let p = (phi + PI).rem_euclid(2.0 * PI) - PI;
    if p > PI / 2.0 {
        2.0 - 2.0 * p / PI
    } else if p < -PI / 2.0 {
        -2.0 - 2.0 * p / PI
    } else {
        2.0 * p / PI
    }
}

/// The sweep's position (samples from the centre) for plain `values` at `rate`, at the left
/// channel's phase `phi` (the right leads by Stereo Phasing), for the editor and tests.
pub fn position(rate: f64, values: &[f64], phi: f64, channel: usize) -> f64 {
    let (lo, hi) = (
        sweep_samples(rate, values[INITIAL]),
        sweep_samples(rate, values[FINAL]),
    );
    let lead = if channel == 1 {
        values[PHASING] * PI / 180.0
    } else {
        0.0
    };
    (wave(phi + lead, values[SINUSOIDAL] >= 0.5) + 1.0) * (hi - lo) * 0.5 + lo
}

/// The interpolator, built once.
fn sinc() -> &'static Sinc {
    static SINC: OnceLock<Sinc> = OnceLock::new();
    SINC.get_or_init(|| Sinc::new(SINC_PHASES))
}

struct Flanger {
    rate: f64,
    values: [f64; COUNT],
    lines: [Line; CHANNELS],
    sinc: &'static Sinc,
    /// The left channel's phase (radians, in [-π, π)).
    phi: f64,
    initial: Ramp,
    end: Ramp,
    /// Stereo Phasing (degrees), the rate (Hz), Mix and Feedback (fractions).
    phasing: Ramp,
    hz: Ramp,
    mix: Ramp,
    feedback: Ramp,
    glide: usize,
}

impl Flanger {
    fn new(rate: f64) -> Self {
        let values = PARAMS.map(|p| p.default);
        let ramp = |i: usize| Ramp::new(values[i] as f32);
        let frac = |i: usize| Ramp::new((values[i] / 100.0) as f32);
        Self {
            rate,
            lines: [Line::new(LINE), Line::new(LINE)],
            sinc: sinc(),
            phi: -PI / 2.0,
            initial: ramp(INITIAL),
            end: ramp(FINAL),
            phasing: ramp(PHASING),
            hz: ramp(RATE),
            mix: frac(MIX),
            feedback: frac(FEEDBACK),
            glide: (rate * GLIDE_MS / 1000.0) as usize,
            values,
        }
    }

    fn ramp_of(&mut self, index: usize) -> Option<(&mut Ramp, f64)> {
        match index {
            INITIAL => Some((&mut self.initial, 1.0)),
            FINAL => Some((&mut self.end, 1.0)),
            PHASING => Some((&mut self.phasing, 1.0)),
            RATE => Some((&mut self.hz, 1.0)),
            MIX => Some((&mut self.mix, 0.01)),
            FEEDBACK => Some((&mut self.feedback, 0.01)),
            _ => None,
        }
    }

    fn ramps(&mut self) -> [&mut Ramp; 6] {
        [
            &mut self.initial,
            &mut self.end,
            &mut self.phasing,
            &mut self.hz,
            &mut self.mix,
            &mut self.feedback,
        ]
    }

    /// Read `line` at `age` (fractional) by the sinc.
    #[inline]
    fn read(&self, line: &Line, age: f64) -> f32 {
        let mut kernel = [0.0; SINC_TAPS];
        let oldest = self.sinc.kernel(age, &mut kernel);
        line.dot(&kernel, oldest)
    }
}

impl NativeEffect for Flanger {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        let glide = self.glide;
        if let Some((r, scale)) = self.ramp_of(index) {
            r.to((value * scale) as f32, glide);
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match self.ramp_of(index) {
            Some((r, scale)) => {
                r.to((value * scale) as f32, samples);
                self.values[index] = value;
            }
            None => self.set(index, value),
        }
    }

    fn snap(&mut self) {
        for r in self.ramps() {
            r.snap();
        }
    }

    fn latency(&self) -> usize {
        LATENCY
    }

    /// As the reference's `Resume`: the lines empty, the sweep from its start (-π/2: at Initial),
    /// the settings in use at once.
    fn start(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
        self.phi = -PI / 2.0;
        self.snap();
    }

    // A sample at a time across both channels: they share the sweep's phase and glides.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let sine = self.values[SINUSOIDAL] >= 0.5;
        let s = if self.values[INVERTED] >= 0.5 {
            -1.0
        } else {
            1.0
        };
        let special = self.values[SPECIAL] >= 0.5;
        let behind_offset = if REFERENCE_TAPS { -1.0 } else { 0.0 };
        for k in from..to {
            let lo = sweep_samples(self.rate, f64::from(self.initial.tick()));
            let hi = sweep_samples(self.rate, f64::from(self.end.tick()));
            let lead = f64::from(self.phasing.tick()) * PI / 180.0;
            let step = 2.0 * PI * f64::from(self.hz.tick()) / self.rate;
            let m = f64::from(self.mix.tick());
            let fb = f64::from(self.feedback.tick());
            let phases = [self.phi, self.phi + lead];
            self.phi += step;
            if self.phi >= PI {
                self.phi -= 2.0 * PI;
            }
            for (c, phi) in phases.into_iter().enumerate() {
                self.lines[c].write(io[c][k]);
                let p = (wave(phi, sine) + 1.0) * (hi - lo) * 0.5 + lo;
                let line = &self.lines[c];
                let centre = f64::from(line.back(CENTRE));
                let behind = f64::from(self.read(line, CENTRE as f64 + p + behind_offset));
                let (y, back) = if special {
                    (
                        centre * (1.0 - m) + m * s * behind,
                        centre + fb * s * behind,
                    )
                } else {
                    let ahead = f64::from(self.read(line, CENTRE as f64 - p));
                    let g = 0.5 * m;
                    (
                        centre * (1.0 - m) - g * behind - g * s * ahead,
                        centre - fb * s * behind,
                    )
                };
                let back = (back as f32).clamp(-1.0, 1.0);
                // Below about -500 dB a dying loop would run on through denormals (TD128).
                let back = super::flush(back);
                self.lines[c].set_back(CENTRE, back);
                io[c][k] = (y as f32).clamp(-1.0, 1.0);
            }
        }
    }
}

#[cfg(test)]
#[path = "flanger_tests.rs"]
mod tests;
