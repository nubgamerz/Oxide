//! Chorus: a multi-voice chorus (studied in the research notes on `chorus`, kept outside the
//! repository), written from the study's description:
//!
//! - **Up to 16 voices**, each a copy of the input read from a line of 300 ms (and 32 samples) a
//!   channel at a delay that wanders under **filtered random noise**: `d = (n_d + 1)·D/2 + O_v`
//!   samples, D the Delay Time, `n_d` the voice's delay noise (about ±1, clamped to ±3), `O_v`
//!   its random offset within Spread. Read by a Catmull-Rom cubic, or at Highest Quality by the
//!   32-tap windowed sinc of 4000 phases ([`Sinc`]).
//! - **Each voice's level wanders** too: its level noise `n_a` times Modulation Depth, in dB
//!   (10^(dB/20) from a table at 0.001 dB steps over ±30 dB).
//! - **The noises** are uniform random numbers from a generator (an additive lagged
//!   Fibonacci over 55 numbers, seeded through a 21-bit LCG that then draws the offsets), scaled
//!   by amplitude laws (3 and 4.2 dB an octave below half the rate), through Chebyshev
//!   type I low-passes: order 4 with 0.01 dB ripple at Delay Rate for the delay, order 2 with
//!   16 dB ripple (a resonant peak, so the level wobbles at about Modulation Rate) for the
//!   level, each section clamped to ±3. The reference filters the noise at the audio rate; ours at a
//!   control rate of about 375 Hz (every [`control_interval`] samples, 128 at 48 kHz), with
//!   the noise scaled so its spectrum below that is the reference's, joined up by a Catmull-Rom
//!   cubic: the noises' bands end at 12 and 30 Hz, and it makes the warm-up (below) 128 times
//!   cheaper (TD135).
//! - **Spread and pan:** offsets at least `0.65·S/V` apart drawn from the LCG (10·V tries,
//!   then anywhere), and a pan law from Stereo Field: the edge voices hard left and
//!   right at +3 dB at 100 %, the centre at unity. **Add Binaural Cues** puts a floor of 0.17
//!   under the gains and delays the far ear by the path difference from a source 5 units out
//!   around ears ±0.5 apart, read as milliseconds (up to 1 ms). The reference delays the right
//!   ear for every voice (its mirror test is off by a factor of two); ours delays the far ear, the
//!   right for left-hand voices and the left for right-hand ones (TD135).
//! - **Negative feedback:** the line keeps `clip(x − fb·S, ±1)`, S the voices' sum on that
//!   side, so the sound recirculates with its sign flipped. The ±1 clip
//!   applies at Feedback 0 too: a line holds nothing louder than full scale.
//! - **Out:** `wet·S + dry·x`, Dry and Wet up to 200 %, sums in f64. Average Left & Right
//!   channel input feeds both lines the pair's mean (and, as the reference, reads the right side's
//!   voices from the left line unless their binaural delays differ).
//!
//! **Repeatable:** a generator that is global and reseeded only when the effect is built or
//! Voices changes would never render the same twice. Ours keeps a
//! generator per instance and reseeds it at every transport start from a hash of the
//! settings (Voices, Delay Time, Delay Rate, Feedback, Spread, Stereo Field, Modulation Rate
//! and Depth): the offsets drawn and each noise warmed up for 1/(1.5·rate) seconds, as
//! after a redesign. That bank is made **on the main thread** ([`PREPARES`]: by
//! `Native::set` whenever one of those settings changes, and before a render) and reaches the
//! DSP through a mailbox the insert only `try_lock`s; a start, and a Voices change (which
//! reseeds at once, as a rebuild does), only copy it. So the same start gives the same
//! output, an export is what played, and the audio thread never warms anything up. Those eight
//! settings have no lanes (a lane's value couldn't wait for the main thread); Dry and Wet do.
//!
//! Changes by hand: Delay Time, Feedback, Dry, Wet and Modulation Depth glide over 10 ms;
//! Delay Rate and Modulation Rate redesign their filters (the noise runs on); Spread redraws
//! the offsets from the running generator (they jump); Stereo Field and the
//! switches apply at once; each lands in the bank the next start copies. Latency 0. The
//! reference's fade of the input over the first 12 ms after the line is built and its 70 ms dry
//! ramp after a rebuild aren't reproduced (neither runs on an ordinary start).

use super::biquad::Coefficients;
use super::delay_line::{Line, Ramp, Sinc, SINC_TAPS};
use super::{FileBox, Kind, NativeEffect, ParamSpec, Prepares};
use crate::graph::CHANNELS;
use std::f64::consts::PI;
use std::sync::OnceLock;

pub const DELAY_TIME: usize = 0;
pub const DELAY_RATE: usize = 1;
pub const FEEDBACK: usize = 2;
pub const DRY: usize = 3;
pub const WET: usize = 4;
pub const SPREAD: usize = 5;
pub const FIELD: usize = 6;
pub const MOD_RATE: usize = 7;
pub const MOD_DEPTH: usize = 8;
pub const VOICES: usize = 9;
pub const AVERAGE: usize = 10;
pub const BINAURAL: usize = 11;
pub const HIGHEST_QUALITY: usize = 12;
pub const COUNT: usize = 13;

/// The most voices (16 noise slots of each kind).
pub const MAX_VOICES: usize = 16;
/// The sinc's phases at Highest Quality.
pub const SINC_PHASES: usize = 4000;
/// The noises' control rate (Hz), about: a whole number of samples apart.
pub const CONTROL_HZ: f64 = 375.0;
/// The highest rate the lines are sized for up front; an instance made at a higher one sizes
/// for its own.
pub const MAX_RATE: f64 = 192_000.0;
/// A change by hand glides over this long (ms).
const GLIDE_MS: f64 = 10.0;
/// The binaural cues' gain floor.
const BINAURAL_FLOOR: f64 = 0.17;

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "%",
    min: 0.0,
    max: 100.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    // Only Dry and Wet have lanes: the rest make what a start copies (`PREPARED_BY`).
    automatable: false,
    link: None,
    log_slider: false,
    format: None,
};

const SWITCH: ParamSpec = ParamSpec {
    unit: "",
    max: 1.0,
    steps: 1,
    decimals: 0,
    automatable: false,
    ..PARAM
};

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Delay Time",
        unit: "ms",
        min: 0.1,
        max: 50.0,
        default: 31.4,
        decimals: 2,
        ..PARAM
    },
    ParamSpec {
        name: "Delay Rate",
        unit: "Hz",
        min: 0.1,
        max: 12.0,
        default: 0.1,
        decimals: 2,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Feedback",
        max: 50.0,
        ..PARAM
    },
    ParamSpec {
        name: "Dry Output Level",
        max: 200.0,
        default: 70.0,
        automatable: true,
        ..PARAM
    },
    ParamSpec {
        name: "Wet Output Level",
        max: 200.0,
        default: 30.0,
        automatable: true,
        ..PARAM
    },
    ParamSpec {
        name: "Spread",
        unit: "ms",
        max: 200.0,
        default: 30.0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Stereo Field",
        default: 100.0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Modulation Rate",
        unit: "Hz",
        min: 0.1,
        max: 30.0,
        default: 0.1,
        decimals: 2,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Modulation Depth",
        unit: "dB",
        max: 15.0,
        ..PARAM
    },
    ParamSpec {
        name: "Voices",
        unit: "",
        min: 1.0,
        max: MAX_VOICES as f64,
        default: 2.0,
        steps: (MAX_VOICES - 1) as u32,
        decimals: 0,
        automatable: false,
        ..PARAM
    },
    ParamSpec {
        name: "Average Left & Right Channel Input",
        ..SWITCH
    },
    ParamSpec {
        name: "Add Binaural Cues",
        ..SWITCH
    },
    ParamSpec {
        name: "Highest Quality",
        ..SWITCH
    },
];

pub static KIND: Kind = Kind {
    id: "chorus",
    name: "Chorus",
    category: "Modulation",
    params: &PARAMS,
    make: |rate| Box::new(Chorus::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: None,
    prepares: Some(&PREPARES),
};

/// The voice count the settings ask for (1..16).
pub fn voices(values: &[f64]) -> usize {
    (values[VOICES] + 0.5).clamp(1.0, MAX_VOICES as f64) as usize
}

/// The generator's seed for plain `values`: a hash of the settings: the voice count,
/// XORed with each of Delay Time (ms), Delay Rate, Feedback/100, Spread (ms), Stereo
/// Field/100, Modulation Rate and Modulation Depth rounded to 1/10000, times its own constant
/// (0x70000000 up to 0x76000000), as a 64-bit integer cut to 32 bits.
pub fn seed(values: &[f64]) -> u32 {
    let hashed = [
        (values[DELAY_TIME], 0x7000_0000u32),
        (values[DELAY_RATE], 0x7100_0000),
        (values[FEEDBACK] / 100.0, 0x7200_0000),
        (values[SPREAD], 0x7300_0000),
        (values[FIELD] / 100.0, 0x7400_0000),
        (values[MOD_RATE], 0x7500_0000),
        (values[MOD_DEPTH], 0x7600_0000),
    ];
    hashed
        .iter()
        .fold(voices(values) as u32, |s, &(v, constant)| {
            let rounded = (v * 10_000.0).round() / 10_000.0;
            s ^ ((rounded * f64::from(constant)) as i64) as u32
        })
}

/// The noises' control step at `rate`: the whole number of samples nearest 1/375 s.
pub fn control_interval(rate: f64) -> usize {
    (rate / CONTROL_HZ).round().max(1.0) as usize
}

/// The line's length at `rate`: 300 ms and 32 samples (the longest delay a voice
/// reads), and room for the binaural delay and the interpolators' reach past it.
fn line_length(rate: f64) -> usize {
    longest(rate) + (rate / 1000.0 * 1.1) as usize + 24
}

/// The longest delay a voice reads (samples): the line, less 3.
fn longest(rate: f64) -> usize {
    (0.3 * rate + 32.0) as usize - 3
}

/// The generator: an additive lagged-Fibonacci generator over 55 numbers in [0, 10000),
/// seeded by a 21-bit LCG which then goes on drawing the spread offsets.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Generator {
    table: [u32; 55],
    k: usize,
    lcg: u64,
}

impl Generator {
    fn new(seed: u32) -> Self {
        let mut g = Self {
            table: [0; 55],
            k: 0,
            lcg: u64::from(seed % 412_123),
        };
        for j in 0..55 {
            g.table[j] = ((g.next_lcg() * 10_000) >> 21) as u32;
        }
        g
    }

    /// The LCG's next state (21 bits).
    fn next_lcg(&mut self) -> u64 {
        self.lcg = (self.lcg * 0x80015 + 1) & 0x1F_FFFF;
        self.lcg
    }

    /// A uniform number in [-1, 1).
    #[inline]
    fn next(&mut self) -> f64 {
        self.k = (self.k + 1) % 55;
        let v = (self.table[(self.k + 54) % 55] + self.table[(self.k + 23) % 55]) % 10_000;
        self.table[self.k] = v;
        (f64::from(v) - 5000.0) / 5000.0
    }
}

/// A Chebyshev type I low-pass of `order` (2 or 4) with `ripple_db` in its passband, its edge
/// at `hz` for a sampling rate of `rate`: by the bilinear transform, prewarped there, as
/// conjugate pole pairs in second-order sections (the second unused at order 2); the passband's
/// top at unity, so an even order passes DC at -`ripple_db`. The reference's designer wasn't
/// traced (TD135): this is the textbook one for the type and ripple it was read to use.
pub fn chebyshev(order: usize, ripple_db: f64, hz: f64, rate: f64) -> [Coefficients; 2] {
    let eps = (10f64.powf(ripple_db / 10.0) - 1.0).sqrt();
    let v = (1.0 / eps).asinh() / order as f64;
    let k = (PI * (hz / rate).min(0.49)).tan();
    let mut out = [Coefficients::IDENTITY; 2];
    for (i, c) in out.iter_mut().enumerate().take(order / 2) {
        let theta = (2 * i + 1) as f64 * PI / (2 * order) as f64;
        let re = -v.sinh() * theta.sin() * k;
        let im = v.cosh() * theta.cos() * k;
        let mag2 = re * re + im * im;
        *c = Coefficients::new(
            mag2,
            2.0 * mag2,
            mag2,
            1.0 - 2.0 * re + mag2,
            2.0 * (mag2 - 1.0),
            1.0 + 2.0 * re + mag2,
        );
    }
    let g = 1.0 / (1.0 + eps * eps).sqrt();
    out[0].b0 *= g;
    out[0].b1 *= g;
    out[0].b2 *= g;
    out
}

/// The noise's scale before its filter: `10^(((log2(fs/2 / r) − 2.94)·slope + 3)/20)`
/// (`slope` 3 for the delay, 4.2 for the level), times √(C/fs) for drawing it at the control
/// rate C rather than fs, so the filtered noise is as strong as at the audio rate.
pub fn noise_scale(rate: f64, control: f64, hz: f64, slope: f64) -> f64 {
    let db = ((rate / 2.0 / hz).log2() - 2.94) * slope + 3.0;
    10f64.powf(db / 20.0) * (control / rate).sqrt()
}

/// One noise filter's design: its sections (`order` 2 or 4) and the noise's scale.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Design {
    sections: [Coefficients; 2],
    order: usize,
    scale: f64,
}

impl Design {
    fn delay(rate: f64, control: f64, hz: f64) -> Self {
        Self {
            sections: chebyshev(4, 0.01, hz, control),
            order: 4,
            scale: noise_scale(rate, control, hz, 3.0),
        }
    }

    fn level(rate: f64, control: f64, hz: f64) -> Self {
        Self {
            sections: chebyshev(2, 16.0, hz, control),
            order: 2,
            scale: noise_scale(rate, control, hz, 4.2),
        }
    }

    /// The warm-up after a design: 1/(1.5·hz) seconds of noise, in control steps.
    fn warm_up(control: f64, hz: f64) -> usize {
        (control / (1.5 * hz)) as usize
    }
}

/// One voice's noise: the filter's state (direct form I, per section: x1, x2, y1, y2), the
/// last four control values and the cubic joining the middle two.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Noise {
    state: [[f64; 4]; 2],
    history: [f64; 4],
    /// The Catmull-Rom cubic from `history[1]` to `history[2]`: a, b, c, d of
    /// `((a·t + b)·t + c)·t + d`.
    cubic: [f64; 4],
}

impl Noise {
    /// One control step: a new value from `generator` through the filter, each section's
    /// output clamped to ±3.
    #[inline]
    fn step(&mut self, generator: &mut Generator, design: &Design) {
        let mut x = generator.next() * design.scale;
        for (s, c) in self
            .state
            .iter_mut()
            .zip(&design.sections)
            .take(design.order / 2)
        {
            let y =
                (c.b0 * x + c.b1 * s[0] + c.b2 * s[1] - c.a1 * s[2] - c.a2 * s[3]).clamp(-3.0, 3.0);
            *s = [x, s[0], y, s[2]];
            x = y;
        }
        let h = &mut self.history;
        *h = [h[1], h[2], h[3], x];
        self.cubic = [
            -0.5 * h[0] + 1.5 * h[1] - 1.5 * h[2] + 0.5 * h[3],
            h[0] - 2.5 * h[1] + 2.0 * h[2] - 0.5 * h[3],
            0.5 * (h[2] - h[0]),
            h[1],
        ];
    }

    /// The noise `t` (0..1) of the way through the control step, clamped to ±3.
    #[inline]
    fn at(&self, t: f64) -> f64 {
        let [a, b, c, d] = self.cubic;
        (((a * t + b) * t + c) * t + d).clamp(-3.0, 3.0)
    }
}

/// Everything a reseed makes and the noise then moves on: the generator, the offsets and each
/// voice's two noises. Plain data, so a start copies the warmed-up one.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Bank {
    generator: Generator,
    /// Each voice's delay offset (samples).
    offsets: [f64; MAX_VOICES],
    delay: [Noise; MAX_VOICES],
    level: [Noise; MAX_VOICES],
}

/// The spread offsets (samples) of `voices` voices for a Spread of `spread_ms` at `rate`,
/// drawn from the generator's LCG: at least `0.65·S/V` apart, each anywhere in [gap, S) (one
/// voice: 0), until 10·V draws are spent; then every slot anywhere in [0, S).
fn draw_offsets(
    generator: &mut Generator,
    voices: usize,
    spread_ms: f64,
    rate: f64,
) -> [f64; MAX_VOICES] {
    let mut offsets = [0.0; MAX_VOICES];
    if voices <= 1 {
        return offsets;
    }
    let s = (spread_ms / 1000.0 * rate) as u64;
    let s1 = s.max(1);
    let gap = (s as f64 / voices as f64 * 0.65) as u64;
    let mut drawn = [0u64; MAX_VOICES];
    let mut tries = 0;
    for v in 0..voices {
        loop {
            if tries >= 10 * voices {
                // Given up: everything anywhere (all 32 slots filled).
                for k in 0..2 * MAX_VOICES {
                    let c = (generator.next_lcg() * s1) >> 21;
                    if let Some(o) = offsets.get_mut(k) {
                        *o = c as f64;
                    }
                }
                return offsets;
            }
            tries += 1;
            let c = gap + ((generator.next_lcg() * (s1 - gap)) >> 21);
            if drawn[..v].iter().all(|&o| c.abs_diff(o) >= gap) {
                drawn[v] = c;
                break;
            }
        }
    }
    for (o, d) in offsets.iter_mut().zip(drawn) {
        *o = d as f64;
    }
    offsets
}

/// A voice's place in the stereo field: its gains on each side and its binaural delays
/// (whole samples) on each side.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pan {
    pub gains: [f64; CHANNELS],
    pub delays: [usize; CHANNELS],
}

/// Every voice's pan for plain `values` at `rate`: the pan law. For `V` > 1 voice `v` sits
/// `u = |200v/(V − 1) − 100|/150` from the centre; `k = (field/100)^(ln u / ln 0.5)`, the angle
/// `θ = π/2·(1 − k)` and `φ = π/4·(1 − k)`; the gains `cos φ·(√2 − b) + b` and
/// `sin φ·(√2 − b) + b` (b 0.17 with binaural cues), the larger on the voice's side (the first
/// half of the voices on the left). The binaural delay is the path difference from a source 5
/// units out at θ to ears ±0.5 apart, read as ms, on the far ear.
pub fn pans(values: &[f64], rate: f64) -> [Pan; MAX_VOICES] {
    let n = voices(values);
    let binaural = values[BINAURAL] >= 0.5;
    let b = if binaural { BINAURAL_FLOOR } else { 0.0 };
    let field = (values[FIELD] / 100.0).clamp(0.0, 1.0);
    let mut out = [Pan {
        gains: [1.0, 1.0],
        delays: [0, 0],
    }; MAX_VOICES];
    if n <= 1 {
        return out;
    }
    for (v, pan) in out.iter_mut().enumerate().take(n) {
        let u = ((200.0 * v as f64 / (n - 1) as f64 - 100.0) / 150.0).abs();
        let (theta, phi) = if u == 0.0 {
            (PI / 2.0, PI / 4.0)
        } else {
            let k = field.powf(u.ln() / 0.5f64.ln());
            (PI / 2.0 - k * PI / 2.0, PI / 4.0 - k * PI / 4.0)
        };
        let near = phi.cos() * (2f64.sqrt() - b) + b;
        let far = phi.sin() * (2f64.sqrt() - b) + b;
        let left = 2 * v < n;
        pan.gains = if left { [near, far] } else { [far, near] };
        if binaural {
            let (x, y) = (5.0 * theta.cos(), 5.0 * theta.sin());
            let difference =
                ((x - 0.5).powi(2) + y * y).sqrt() - ((x + 0.5).powi(2) + y * y).sqrt();
            let itd = (difference * rate / 1000.0) as i64;
            let late = itd.unsigned_abs() as usize;
            // The far ear hears it late: the right for a voice on the left, and the other way.
            pan.delays = if left { [0, late] } else { [late, 0] };
        }
    }
    out
}

/// What the editor shows of one voice: its offset and pan, all in ms where they are times.
#[derive(Debug, Clone, Copy)]
pub struct VoiceLayout {
    pub offset_ms: f64,
    pub gains: [f64; CHANNELS],
    pub binaural_ms: [f64; CHANNELS],
}

/// The voices the settings make (as a start draws them), for the editor: no DSP state needed.
pub fn layout(rate: f64, values: &[f64]) -> Vec<VoiceLayout> {
    let n = voices(values);
    let mut generator = Generator::new(seed(values));
    let offsets = draw_offsets(&mut generator, n, values[SPREAD], rate);
    let pans = pans(values, rate);
    (0..n)
        .map(|v| VoiceLayout {
            offset_ms: offsets[v] * 1000.0 / rate,
            gains: pans[v].gains,
            binaural_ms: pans[v].delays.map(|d| d as f64 * 1000.0 / rate),
        })
        .collect()
}

/// 10^(dB/20) at 0.001 dB steps over ±30 dB, as a table (built once, off the audio
/// thread: the first instance asks for it).
fn gain_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        (0..=60_000)
            .map(|i| 10f64.powf((f64::from(i) / 1000.0 - 30.0) / 20.0) as f32)
            .collect()
    })
}

/// The gain of `db` from the table: truncated to its 0.001 dB step, the ends beyond ±30 dB.
#[inline]
fn gain_of(table: &[f32], db: f64) -> f32 {
    let i = ((db + 30.0) * 1000.0).max(0.0) as usize;
    table[i.min(table.len() - 1)]
}

/// The Highest Quality interpolator, built once.
fn sinc() -> &'static Sinc {
    static SINC: OnceLock<Sinc> = OnceLock::new();
    SINC.get_or_init(|| Sinc::new(SINC_PHASES))
}

/// The parameters a start's state is made from (the hash, and the rates its noises
/// are filtered at): none has a lane, so it is made on the main thread before it is needed.
pub const PREPARED_BY: [usize; 8] = [
    DELAY_TIME, DELAY_RATE, FEEDBACK, SPREAD, FIELD, MOD_RATE, MOD_DEPTH, VOICES,
];

/// The bits of [`PREPARED_BY`]'s plain values: which settings a warmed-up bank is for.
type Key = [u64; 8];

fn key(values: &[f64]) -> Key {
    PREPARED_BY.map(|i| values[i].to_bits())
}

/// A start's state as the main thread makes it ([`PREPARES`]), on its way to the DSP.
struct Warm {
    key: Key,
    bank: Bank,
}

#[cfg(test)]
thread_local! {
    /// Warm-ups made on this thread, so a test can see the audio thread's path makes none.
    static WARM_UPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// The bank a reseed makes for plain `values` at `rate`: the generator from the hash, the
/// offsets, then every voice's delay noise warmed up, then every voice's level noise, in
/// that order. Up to about a millisecond in release (16 voices with both
/// rates at 0.1 Hz): only ever on the main thread.
fn warm_bank(rate: f64, values: &[f64]) -> Bank {
    #[cfg(test)]
    WARM_UPS.with(|w| w.set(w.get() + 1));
    let n = voices(values);
    let mut generator = Generator::new(seed(values));
    let offsets = draw_offsets(&mut generator, n, values[SPREAD], rate);
    let mut bank = Bank {
        generator,
        offsets,
        delay: [Noise::default(); MAX_VOICES],
        level: [Noise::default(); MAX_VOICES],
    };
    let c = rate / control_interval(rate) as f64;
    let (delay, level) = (
        Design::delay(rate, c, values[DELAY_RATE]),
        Design::level(rate, c, values[MOD_RATE]),
    );
    // The warm-up, and four more steps so the cubic has its points.
    let steps = |hz: f64| Design::warm_up(c, hz) + 4;
    let delay_steps = steps(values[DELAY_RATE]);
    for noise in &mut bank.delay[..n] {
        for _ in 0..delay_steps {
            noise.step(&mut bank.generator, &delay);
        }
    }
    let level_steps = steps(values[MOD_RATE]);
    for noise in &mut bank.level[..n] {
        for _ in 0..level_steps {
            noise.step(&mut bank.generator, &level);
        }
    }
    bank
}

/// The Chorus's start state, made on the main thread whenever one of [`PREPARED_BY`] changes
/// (`Native::set`) and before a render: a start only copies it.
static PREPARES: Prepares = Prepares {
    by: &PREPARED_BY,
    make: |rate, values| {
        Box::new(Warm {
            key: key(values),
            bank: warm_bank(rate, values),
        })
    },
};

struct Chorus {
    rate: f64,
    values: [f64; COUNT],
    lines: [Line; CHANNELS],
    longest: f64,
    sinc: &'static Sinc,
    gains: &'static [f32],
    /// Delay Time (ms), Feedback, Dry and Wet (fractions) and Modulation Depth (dB).
    delay_ms: Ramp,
    feedback: Ramp,
    dry: Ramp,
    wet: Ramp,
    depth: Ramp,
    /// The noise filters' designs, for the delay and the level.
    delay_design: Design,
    level_design: Design,
    control: usize,
    /// Samples into the control step.
    phase: usize,
    bank: Bank,
    /// The bank a start copies, made on the main thread for `warm_key`'s settings.
    warm: Bank,
    warm_key: Key,
    /// A start (or a Voices change) found no bank made for the settings yet: it reseeds when
    /// one arrives (a hand change racing the start; never offline, which waits for it).
    waiting: bool,
    pans: [Pan; MAX_VOICES],
    glide: usize,
}

impl Chorus {
    /// On the main thread (it makes the defaults' bank).
    fn new(rate: f64) -> Self {
        let values = PARAMS.map(|p| p.default);
        let capacity = line_length(rate.max(MAX_RATE));
        let mut lines = [Line::new(capacity), Line::new(capacity)];
        for line in &mut lines {
            line.reset(line_length(rate));
        }
        let control = control_interval(rate);
        let c = rate / control as f64;
        let frac = |i: usize| Ramp::new((values[i] / 100.0) as f32);
        let bank = warm_bank(rate, &values);
        Self {
            rate,
            lines,
            longest: longest(rate) as f64,
            sinc: sinc(),
            gains: gain_table(),
            delay_ms: Ramp::new(values[DELAY_TIME] as f32),
            feedback: frac(FEEDBACK),
            dry: frac(DRY),
            wet: frac(WET),
            depth: Ramp::new(values[MOD_DEPTH] as f32),
            delay_design: Design::delay(rate, c, values[DELAY_RATE]),
            level_design: Design::level(rate, c, values[MOD_RATE]),
            control,
            phase: 0,
            bank,
            warm: bank,
            warm_key: key(&values),
            waiting: false,
            pans: pans(&values, rate),
            glide: (rate * GLIDE_MS / 1000.0) as usize,
            values,
        }
    }

    fn control_rate(&self) -> f64 {
        self.rate / self.control as f64
    }

    fn voices(&self) -> usize {
        voices(&self.values)
    }

    /// Reseed from the settings now: the designs and pans for them, and the bank the main
    /// thread made for them, copied; if it hasn't come yet, as soon as it does. Nothing is
    /// made here: this runs on the audio thread.
    fn reseed(&mut self) {
        let c = self.control_rate();
        self.delay_design = Design::delay(self.rate, c, self.values[DELAY_RATE]);
        self.level_design = Design::level(self.rate, c, self.values[MOD_RATE]);
        self.pans = pans(&self.values, self.rate);
        self.waiting = self.warm_key != key(&self.values);
        if !self.waiting {
            self.bank = self.warm;
            self.phase = 0;
        }
    }

    fn fraction(&mut self, index: usize) -> Option<&mut Ramp> {
        match index {
            FEEDBACK => Some(&mut self.feedback),
            DRY => Some(&mut self.dry),
            WET => Some(&mut self.wet),
            _ => None,
        }
    }

    /// A voice's delay (samples, the age it reads at before its binaural delay) `t` of the way
    /// through the control step, for a Delay Time of `d` samples: within what the interpolator
    /// can read (the sinc reaches 16 samples newer, the cubic 2) and the line.
    #[inline]
    fn voice_delay(&self, v: usize, t: f64, d: f64, hq: bool) -> f64 {
        let lowest = if hq { Sinc::MIN_AGE } else { 2.0 };
        let n = self.bank.delay[v].at(t);
        ((n + 1.0) * d * 0.5 + self.bank.offsets[v]).clamp(lowest, self.longest)
    }
}

impl NativeEffect for Chorus {
    fn set(&mut self, index: usize, value: f64) {
        let Some(v) = self.values.get_mut(index) else {
            return;
        };
        if *v == value {
            return;
        }
        *v = value;
        let glide = self.glide;
        let c = self.control_rate();
        match index {
            DELAY_TIME => self.delay_ms.to(value as f32, glide),
            MOD_DEPTH => self.depth.to(value as f32, glide),
            DELAY_RATE => self.delay_design = Design::delay(self.rate, c, value),
            MOD_RATE => self.level_design = Design::level(self.rate, c, value),
            SPREAD => {
                let n = self.voices();
                self.bank.offsets = draw_offsets(&mut self.bank.generator, n, value, self.rate);
            }
            FIELD | AVERAGE | BINAURAL => self.pans = pans(&self.values, self.rate),
            VOICES => self.reseed(),
            HIGHEST_QUALITY => {}
            _ => {
                if let Some(r) = self.fraction(index) {
                    r.to(value as f32 / 100.0, glide);
                }
            }
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match index {
            // Only Dry and Wet have lanes (the rest make what a start copies).
            DRY | WET => {
                self.values[index] = value;
                if let Some(r) = self.fraction(index) {
                    r.to(value as f32 / 100.0, samples);
                }
            }
            _ => self.set(index, value),
        }
    }

    fn snap(&mut self) {
        for r in [
            &mut self.delay_ms,
            &mut self.feedback,
            &mut self.dry,
            &mut self.wet,
            &mut self.depth,
        ] {
            r.snap();
        }
        self.reseed();
    }

    /// The lines empty and the generator reseeded from the settings (the bank made for them
    /// copied), so every start plays the same.
    fn start(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
        self.snap();
    }

    /// The bank the main thread made for the settings: kept for the next start (and used at
    /// once if a start or a Voices change is waiting for it), its box given back.
    fn take_prepared(&mut self, fresh: &mut Option<FileBox>, spent: &mut Option<FileBox>) {
        if spent.is_some() {
            return;
        }
        let Some(made) = fresh.take() else {
            return;
        };
        if let Some(warm) = made.downcast_ref::<Warm>() {
            self.warm = warm.bank;
            self.warm_key = warm.key;
        }
        *spent = Some(made);
        if self.waiting {
            self.reseed();
        }
    }

    // A sample at a time across both channels: they share the voices' noises and glides.
    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize) {
        let n = self.voices();
        let hq = self.values[HIGHEST_QUALITY] >= 0.5;
        let average = self.values[AVERAGE] >= 0.5;
        let to_samples = self.rate / 1000.0;
        let step = 1.0 / self.control as f64;
        for k in from..to {
            if self.phase == self.control {
                self.phase = 0;
                let bank = &mut self.bank;
                for v in 0..n {
                    bank.delay[v].step(&mut bank.generator, &self.delay_design);
                    bank.level[v].step(&mut bank.generator, &self.level_design);
                }
            }
            let t = self.phase as f64 * step;
            self.phase += 1;
            let d = f64::from(self.delay_ms.tick()) * to_samples;
            let fb = f64::from(self.feedback.tick());
            let dry = f64::from(self.dry.tick());
            let wet = f64::from(self.wet.tick());
            let depth = f64::from(self.depth.tick());
            let mut x = [io[0][k], io[1][k]];
            if average {
                let mean = ((f64::from(x[0]) + f64::from(x[1])) / 2.0) as f32;
                x = [mean, mean];
            }
            self.lines[0].write(x[0]);
            self.lines[1].write(x[1]);
            let mut sum = [0.0f64; CHANNELS];
            for v in (0..n).rev() {
                let age = self.voice_delay(v, t, d, hq);
                let amp = f64::from(gain_of(self.gains, self.bank.level[v].at(t) * depth));
                let pan = &self.pans[v];
                let [bl, br] = pan.delays;
                let (left, right) = if hq {
                    let mut kernel = [0.0; SINC_TAPS];
                    let oldest = self.sinc.kernel(age, &mut kernel);
                    let left = self.lines[0].dot(&kernel, oldest + bl);
                    let right = if average && bl == br {
                        left
                    } else {
                        self.lines[1].dot(&kernel, oldest + br)
                    };
                    (left, right)
                } else {
                    let left = self.lines[0].hermite(age + bl as f64);
                    let right = if average && bl == br {
                        left
                    } else {
                        self.lines[1].hermite(age + br as f64)
                    };
                    (left, right)
                };
                sum[0] += f64::from(left) * amp * pan.gains[0];
                sum[1] += f64::from(right) * amp * pan.gains[1];
            }
            for c in 0..CHANNELS {
                io[c][k] = (wet * sum[c] + dry * f64::from(x[c])) as f32;
                // The voices' sum back into the line, negated; the line holds ±1 at most.
                let back = ((f64::from(x[c]) - fb * sum[c]) as f32).clamp(-1.0, 1.0);
                // Below about -500 dB a dying loop would run on through denormals (TD128).
                let back = super::flush(back);
                self.lines[c].set_back(1, back);
            }
        }
    }
}

#[cfg(test)]
#[path = "chorus_tests.rs"]
mod tests;
