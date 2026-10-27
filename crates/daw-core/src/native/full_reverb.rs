//! Space Reverb, as studied in the research notes on `full-reverb` (kept outside the
//! repository) and written anew: **a convolution reverb whose impulse is made
//! from the settings**, off the audio thread ([`super::Reads`] with no file), played by the
//! shared [`convolver`](super::convolver).
//!
//! **The impulse** (per impulse channel: two, or four in true stereo) is two buffers, as the
//! study found:
//!
//! - **Early reflections** E: image sources in a shoebox room. Room Size V (m³) and Dimension
//!   A (width/depth) give the height H = (V^0.333 − 3.1)·27.5/43 + 2.5, the depth
//!   D = √(V/(A·H)) and the width W = A·D (m). Listener and source(s) sit where the study puts
//!   them; every image (i, j, k) up to the counts the study gives (about 3300 in all) arrives
//!   (d − d_ref)/1130 ft/s after the shortest direct path d_ref, with amplitude (0.75 or 0.85 per
//!   floor or ceiling bounce)/d² (d in feet, with the study's deterministic jitter), negated for
//!   odd i + j + k, normalised by the largest including the direct path (which is not written:
//!   the dry path carries it). Then a high-pass at "Early Reflections High Pass Cutoff".
//! - **Late reverb** R: N = int(0.6·1.2·Td) positive then N negative pulses at random times,
//!   from a 21-bit LCG seeded by the settings (so the impulse is a pure function
//!   of the settings and the rate), with amplitudes r^γ (γ = 2^(7·Perception/200)) under the
//!   envelope ((1 − t) + 0.2)^2.5·10^(−3.12·t) over the impulse's 1.45·Decay, their times
//!   bunched towards the end by Diffusion (t = ln(r·K + 1)/ln(K + 1), K = 10^(4.5 − 0.0033·
//!   Diffusion)). **Pre-Delay** is a fade-in of R from −96 dB over Pre-Delay ms (an
//!   exponential), not a delay.
//!
//! Both are then **coloured** by three sections (mid peak, low shelf, high shelf) re-applied
//! every Coloration Decay ms from that point to the end, each pass ramping in from flat over
//! one period: a band's gain in dB grows linearly with time, Coloration Decay being the time per
//! full application. The impulse is cut 1000 samples after the last 1000-sample window holding a
//! sample above 2^−20 (searched from a quarter of its length), R's last 5 % faded out, and both
//! put through a fixed DC blocker.
//!
//! **Playing:** per output channel y = g_dry·x + g_e·(x ∗ E) + g_r·(x ∗ R), with
//! g_dry = Dry/100, g_e = 0.5·Early/100 and g_r = Reverb/100, as the reference's convolution job
//! adds them; E and R have convolvers of their own, so the three levels act at the output, glide in
//! straight lines along lanes and never re-make the impulse. Every other setting re-makes it (no
//! lane: a lane can't wait for a worker); the old one rings out its input while the new one
//! takes the input after the switch (the reference re-synthesises inside the audio callback). With
//! **Sum Inputs** on (the default) the stereo input is averaged into one source heard by two
//! ears; off, it is **true stereo**: two speakers, four paths, combined as the study traced:
//! Y_0 = X_0·H_0 + X_1·H_3 and Y_1 = X_1·H_1 + X_0·H_2 (H_c = E_c and R_c; channel 0 is the left
//! speaker to the left ear, 1 left to right, 2 right to left, 3 right to right), which pairs the
//! right input with "right speaker to right ear" in the left output: kept as traced (TD131).
//! **Include Direct** adds the other input to each output, delayed by the difference of the two
//! direct paths' arrivals d and scaled by 1 − min(d/60, 0.7) (the study's interaural delay and
//! head shadow), under the dry level.
//!
//! **Latency:** the convolver's 256 samples at any setting (the reference's is twice its FFT block,
//! about 16 s at the defaults), the dry path delayed with it. Until the first impulse is made,
//! the dry path plays alone. A transport start restarts the convolution from silence.
//!
//! **Parameters:** 29 ids, names and ranges. Ids 22–27 are outputs (Calculating
//! Impulse, Is Processing, Number of Channels, and the room's dimensions; writes are ignored, and
//! the dimensions follow Room Size and Dimension; "Room Height" holds the
//! depth and "Room Depth" the height, as the reference names them). "Set Reverb Based on Room Size" sets
//! Decay and Pre-Delay from the room (Sabine's formula, absorption 0.2) when it is switched on
//! or the room changes while it is on. Frequencies are plain Hz here, so Original Sample Rate
//! only rescales them when it is changed (from the old value to the new; see [`derive`]).

use super::biquad::{self, Coefficients};
use super::convolution_reverb::Glide;
use super::convolver::{Convolver, LATENCY};
use super::{FileBox, Kind, Loaded, NativeEffect, Overview, ParamSpec, Prepared, Reads};
use std::f64::consts::LN_10;

pub const DECAY: usize = 0;
pub const PRE_DELAY: usize = 1;
pub const DIFFUSION: usize = 2;
pub const PERCEPTION: usize = 3;
pub const DRY: usize = 4;
pub const EARLY: usize = 5;
pub const REVERB: usize = 6;
pub const ROOM_SIZE: usize = 7;
pub const DIMENSION: usize = 8;
pub const LOCATION: usize = 9;
pub const ER_HIGH_PASS: usize = 10;
pub const LOW_GAIN: usize = 11;
pub const MID_GAIN: usize = 12;
pub const HIGH_GAIN: usize = 13;
pub const LOW_FREQ: usize = 14;
pub const HIGH_FREQ: usize = 15;
pub const MID_FREQ: usize = 16;
pub const MID_Q: usize = 17;
pub const COLORATION_DECAY: usize = 18;
pub const SUM_INPUTS: usize = 19;
pub const INCLUDE_DIRECT: usize = 20;
pub const BASED_ON_ROOM: usize = 21;
pub const CALCULATING: usize = 22;
pub const PROCESSING: usize = 23;
pub const CHANNEL_COUNT: usize = 24;
pub const ROOM_WIDTH: usize = 25;
/// Holds the room's depth (the reference's naming).
pub const ROOM_HEIGHT: usize = 26;
/// Holds the room's height (the reference's naming).
pub const ROOM_DEPTH: usize = 27;
pub const ORIGINAL_RATE: usize = 28;
const COUNT: usize = 29;

/// The frequency parameters, which Original Sample Rate rescales.
pub const FREQUENCIES: [usize; 4] = [ER_HIGH_PASS, LOW_FREQ, HIGH_FREQ, MID_FREQ];
/// The settings the impulse is made from: changing one makes it again.
pub const SHAPED_BY: [usize; 18] = [
    DECAY,
    PRE_DELAY,
    DIFFUSION,
    PERCEPTION,
    ROOM_SIZE,
    DIMENSION,
    LOCATION,
    ER_HIGH_PASS,
    LOW_GAIN,
    MID_GAIN,
    HIGH_GAIN,
    LOW_FREQ,
    HIGH_FREQ,
    MID_FREQ,
    MID_Q,
    COLORATION_DECAY,
    SUM_INPUTS,
    INCLUDE_DIRECT,
];

/// Metres to feet, as the reference's constant.
pub const FEET: f64 = 3.280_839_895;
/// The speed of sound the reflections travel at (ft/s).
pub const SOUND_FT_PER_S: f64 = 1130.0;
/// The listener's and the source's heights (ft).
pub const LISTENER_Z_FT: f64 = 3.539_006_974_395_292_5;
pub const SOURCE_Z_FT: f64 = 6.510_214_840_197_056;
/// Half the distance between the ears (m).
pub const EAR_OFFSET_M: f64 = 0.167;
/// Reflection factors of a floor and a ceiling bounce (side walls reflect fully). Which is
/// which was not traced: the floor is taken as the more absorbent.
pub const FLOOR: f64 = 0.75;
pub const CEILING: f64 = 0.85;
/// Coloration passes at most per impulse: Coloration Decay is lengthened to keep to it (Coloration
/// Decay 0 would otherwise never advance).
pub const MAX_PASSES: usize = 64;
/// The impulse's samples at or under this are what the trim calls silence (2^-20).
const TRIM_LEVEL: f32 = 1.0 / 1_048_576.0;
/// The trim's window (samples).
const TRIM_WINDOW: usize = 1000;
/// The loudest either buffer may be, as √Σx² in any channel (+30 dB): coloration gains that
/// grow without bound (positive gains re-applied every period) are scaled down to it instead of
/// overflowing (TD131).
pub const LOUDEST: f64 = 31.622_776_601_683_793;
/// Samples the effect works through at a time (its scratch buffers' size).
const CHUNK: usize = 512;
/// The longest Include Direct delay kept (seconds): two speakers in the widest room the ranges
/// allow are at most 0.0752 s apart.
const LONGEST_CROSS_S: f64 = 0.08;

fn hz(v: f64) -> String {
    if v >= 1000.0 {
        format!("{:.2} kHz", v / 1000.0)
    } else {
        format!("{v:.1} Hz")
    }
}

const PARAM: ParamSpec = ParamSpec {
    name: "",
    unit: "",
    min: 0.0,
    max: 100.0,
    default: 0.0,
    steps: 0,
    decimals: 1,
    // Each change makes the impulse again: no lane (the output levels have one).
    automatable: false,
    link: None,
    log_slider: false,
    format: None,
};

const SWITCH: ParamSpec = ParamSpec {
    min: 0.0,
    max: 1.0,
    steps: 1,
    decimals: 0,
    ..PARAM
};

const FREQUENCY: ParamSpec = ParamSpec {
    unit: "Hz",
    min: 20.0,
    // The reference's range is 20 Hz to half the rate; ours is fixed at the top rate's half and
    // capped at the session's half when the impulse is made.
    max: 96_000.0,
    log_slider: true,
    format: Some(hz),
    ..PARAM
};

const GAIN: ParamSpec = ParamSpec {
    unit: "dB",
    min: -30.0,
    max: 20.0,
    decimals: 2,
    ..PARAM
};

const LEVEL: ParamSpec = ParamSpec {
    unit: "%",
    max: 200.0,
    automatable: true,
    ..PARAM
};

const METRES: ParamSpec = ParamSpec {
    unit: "m",
    max: 1000.0,
    decimals: 2,
    ..PARAM
};

static PARAMS: [ParamSpec; COUNT] = [
    ParamSpec {
        name: "Reverberation Decay Time",
        unit: "ms",
        min: 20.0,
        max: 6000.0,
        default: 2636.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Reverberation Pre-Delay Time",
        unit: "ms",
        max: 200.0,
        default: 106.0,
        ..PARAM
    },
    ParamSpec {
        name: "Diffusion",
        unit: "ms",
        min: 10.0,
        max: 1000.0,
        default: 823.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Perception",
        max: 200.0,
        default: 16.0,
        ..PARAM
    },
    ParamSpec {
        name: "Output Level (Dry)",
        default: 73.1,
        ..LEVEL
    },
    ParamSpec {
        name: "Output Level (Early Reflections)",
        default: 27.8,
        ..LEVEL
    },
    ParamSpec {
        name: "Output Level (Reverberation)",
        max: 500.0,
        default: 31.5,
        ..LEVEL
    },
    ParamSpec {
        name: "Room Size",
        unit: "m³",
        min: 20.0,
        max: 60_000.0,
        default: 8879.6,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Dimension",
        unit: "width/depth",
        min: 0.05,
        max: 20.0,
        default: 0.736,
        decimals: 3,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Left/Right Location",
        unit: "%",
        min: -100.0,
        max: 100.0,
        default: -3.4,
        ..PARAM
    },
    ParamSpec {
        name: "Early Reflections High Pass Cutoff",
        default: 50.0,
        ..FREQUENCY
    },
    ParamSpec {
        name: "Low Shelf Gain",
        default: -15.71,
        ..GAIN
    },
    ParamSpec {
        name: "Mid Coloration Gain",
        default: -4.29,
        ..GAIN
    },
    ParamSpec {
        name: "High Shelf Gain",
        default: -23.57,
        ..GAIN
    },
    ParamSpec {
        name: "Low Shelf Frequency",
        default: 52.2,
        ..FREQUENCY
    },
    ParamSpec {
        name: "High Shelf Frequency",
        default: 16_950.0,
        ..FREQUENCY
    },
    ParamSpec {
        name: "Mid Coloration Center Frequency",
        default: 3394.6,
        ..FREQUENCY
    },
    ParamSpec {
        name: "Mid Coloration Q",
        min: 0.1,
        max: 20.0,
        default: 1.0,
        decimals: 2,
        log_slider: true,
        ..PARAM
    },
    ParamSpec {
        name: "Coloration Decay",
        unit: "ms",
        max: 500.0,
        default: 200.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Sum Inputs",
        default: 1.0,
        ..SWITCH
    },
    ParamSpec {
        name: "Include Direct",
        default: 1.0,
        ..SWITCH
    },
    ParamSpec {
        name: "Set Reverb Based on Room Size",
        default: 0.0,
        ..SWITCH
    },
    // Outputs: writes are ignored ([`derive`]).
    ParamSpec {
        name: "Calculating Impulse",
        unit: "%",
        default: 100.0,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Is Processing",
        default: 0.0,
        ..SWITCH
    },
    ParamSpec {
        name: "Number of Channels",
        min: 1.0,
        max: 32.0,
        default: 2.0,
        steps: 31,
        decimals: 0,
        ..PARAM
    },
    ParamSpec {
        name: "Room Width",
        default: 21.82,
        ..METRES
    },
    ParamSpec {
        name: "Room Height",
        default: 29.65,
        ..METRES
    },
    ParamSpec {
        name: "Room Depth",
        default: 13.72,
        ..METRES
    },
    ParamSpec {
        name: "Original Sample Rate",
        unit: "Hz",
        max: 192_000.0,
        default: 44_100.0,
        decimals: 0,
        ..PARAM
    },
];

pub static READS: Reads = Reads {
    what: "impulse",
    shaped_by: &SHAPED_BY,
    load: None,
    prepare,
    derive: Some(derive),
    outputs: &[
        CALCULATING,
        PROCESSING,
        CHANNEL_COUNT,
        ROOM_WIDTH,
        ROOM_HEIGHT,
        ROOM_DEPTH,
    ],
};

pub static KIND: Kind = Kind {
    id: "space-reverb",
    name: "Space Reverb",
    category: "Reverb",
    params: &PARAMS,
    make: |rate| Box::new(FullReverb::new(rate)),
    response: None,
    transfer: None,
    latency: None,
    capture: None,
    learns: None,
    reads: Some(&READS),
    prepares: None,
};

// ------------------------------------------------------------------------------- the room

/// The room's width, depth and height (m) for a volume (m³) and a width/depth ratio, as the
/// study found (Dimension clamped to 0.001..1000).
pub fn room(volume: f64, dimension: f64) -> [f64; 3] {
    let a = dimension.clamp(0.001, 1000.0);
    let h = (volume.powf(0.333) - 3.1) * 27.5 / 43.0 + 2.5;
    let d = (volume / (a * h)).sqrt();
    let w = a * d;
    // The reference's round trip through feet.
    [w, d, h].map(|x| x * FEET * 0.3048)
}

/// "Set Reverb Based on Room Size": Decay and Pre-Delay (ms) for a room (m), by Sabine's
/// formula in feet with an absorption of 0.2, capped at 6000 and 200 ms.
pub fn sabine(room: [f64; 3]) -> (f64, f64) {
    let [w, d, h] = room.map(|x| x * FEET);
    let decay = 1000.0 * 5.0 * 0.049 * w * d * h / (2.0 * w * d + 2.0 * w * h + 2.0 * d * h);
    (decay.min(6000.0), ((w + d + h) / 2.0).min(200.0))
}

/// Parameters that follow others ([`Reads::derive`]): the outputs (22 to 27) keep their fixed
/// values or follow the room; "Set Reverb Based on Room Size" sets Decay and Pre-Delay when
/// switched on or when the room changes while it is on; a change of Original Sample Rate from
/// S to S' rescales each frequency f to 20 + (f − 20)·(S'/2 − 20)/(S/2 − 20), capped at the
/// session's half rate. The reference rescales from the bounds of the session's rate (its frequencies
/// are stored normalised to them); ours are plain Hz, so the old Original Sample Rate stands in
/// for those bounds, which keeps settings made at 44100 unchanged at any rate (TD131).
pub fn derive(rate: f64, changed: Option<(usize, f64)>, v: &mut [f64]) {
    let changed_index = changed.map(|(i, _)| i);
    if let Some((ORIGINAL_RATE, old)) = changed {
        let new = v[ORIGINAL_RATE];
        if old > 40.0 && new > 40.0 && old != new {
            for i in FREQUENCIES {
                let f = 20.0 + (v[i] - 20.0) * (new / 2.0 - 20.0) / (old / 2.0 - 20.0);
                v[i] = f.min(rate / 2.0).clamp(PARAMS[i].min, PARAMS[i].max);
            }
        }
    }
    let dims = room(v[ROOM_SIZE], v[DIMENSION]);
    if v[BASED_ON_ROOM] >= 0.5
        && matches!(changed_index, Some(ROOM_SIZE | DIMENSION | BASED_ON_ROOM))
    {
        let (decay, pre) = sabine(dims);
        v[DECAY] = decay.clamp(PARAMS[DECAY].min, PARAMS[DECAY].max);
        v[PRE_DELAY] = pre.clamp(PARAMS[PRE_DELAY].min, PARAMS[PRE_DELAY].max);
    }
    v[CALCULATING] = 100.0;
    v[PROCESSING] = 0.0;
    v[CHANNEL_COUNT] = 2.0;
    v[ROOM_WIDTH] = dims[0];
    v[ROOM_HEIGHT] = dims[1];
    v[ROOM_DEPTH] = dims[2];
}

/// The image counts along x, y and z for a room (any unit): about 3333 images in all, fewer
/// along the longer and the more absorbent axes; each odd, between 5 and 99.
pub fn image_counts(room: [f64; 3]) -> [i64; 3] {
    let [w, d, h] = room;
    const Z_ABSORPTION: f64 = 0.406_406_25;
    let n0 = (((3333.33 / (w / d)) / (Z_ABSORPTION * w / h)).ln() / 3.0).exp() + 2.0;
    let n0 = n0.floor();
    [n0, n0 * w / d, n0 * Z_ABSORPTION * w / h].map(|n| {
        let n = if n.is_finite() { n as i64 } else { 5 };
        n.clamp(5, 99) | 1
    })
}

/// The settings' values as the reference holds them (f32).
fn f32_of(v: f64) -> f64 {
    f64::from(v as f32)
}

/// One impulse channel's path: a source and an ear (ft).
#[derive(Debug, Clone, Copy)]
pub struct Path {
    pub source: [f64; 3],
    pub ear: [f64; 3],
}

/// The impulse channels' paths for plain `values`: one source to the left and right ears, or
/// (true stereo) the left speaker to each ear, then the right speaker to each.
pub fn paths(values: &[f64], true_stereo: bool) -> Vec<Path> {
    let [w, d, _] = room(f32_of(values[ROOM_SIZE]), f32_of(values[DIMENSION]));
    let lambda = f32_of(values[LOCATION]) / 100.0;
    let x_l = w / 2.0 - lambda * w * 0.1;
    let y_l = d * 0.618_033_988_7 * 4.0 / 3.0;
    let ear = |side: f64| {
        [
            (x_l + side * EAR_OFFSET_M) * FEET,
            y_l * FEET,
            LISTENER_Z_FT,
        ]
    };
    let (left, right) = (ear(-1.0), ear(1.0));
    if true_stereo {
        let speaker = |side: f64| {
            [
                (w / 2.0 + (lambda + side * 0.23) * w * 0.26) * FEET,
                0.28 * d * FEET,
                SOURCE_Z_FT,
            ]
        };
        let (l, r) = (speaker(-1.0), speaker(1.0));
        vec![
            Path {
                source: l,
                ear: left,
            },
            Path {
                source: l,
                ear: right,
            },
            Path {
                source: r,
                ear: left,
            },
            Path {
                source: r,
                ear: right,
            },
        ]
    } else {
        let source = [
            (w / 2.0 + lambda * w * 0.42) * FEET,
            0.22 * d * FEET,
            SOURCE_Z_FT,
        ];
        vec![Path { source, ear: left }, Path { source, ear: right }]
    }
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// The early reflections of each impulse channel as taps (arrival sample, amplitude), before
/// the high-pass, and each channel's direct-path arrival (samples after the shortest direct
/// path, which arrives at 0).
pub fn early_taps(
    rate: f64,
    values: &[f64],
    true_stereo: bool,
) -> (Vec<Vec<(usize, f32)>>, Vec<usize>) {
    let dims = room(f32_of(values[ROOM_SIZE]), f32_of(values[DIMENSION])).map(|x| x * FEET);
    let counts = image_counts(dims);
    let paths = paths(values, true_stereo);
    let direct: Vec<f64> = paths.iter().map(|p| distance(p.source, p.ear)).collect();
    let d_ref = direct.iter().copied().fold(f64::INFINITY, f64::min);
    let arrival = |d: f64| ((d - d_ref) / SOUND_FT_PER_S * rate).max(0.0) as usize;
    let image = |i: i64, size: f64, p: f64| {
        if i.rem_euclid(2) == 0 {
            i as f64 * size + p
        } else {
            (i + 1) as f64 * size - p
        }
    };
    let bounces = |k: i64| {
        // |k| bounces alternating floor and ceiling, the first on the floor below (k < 0).
        let n = k.unsigned_abs() as i32;
        let (first, second) = if k < 0 {
            (FLOOR, CEILING)
        } else {
            (CEILING, FLOOR)
        };
        first.powi((n + 1) / 2) * second.powi(n / 2)
    };
    let range = |n: i64| -(n / 2)..=(n - 1 - n / 2);
    let taps = paths
        .iter()
        .map(|path| {
            let mut found: Vec<(f64, f64)> =
                Vec::with_capacity((counts[0] * counts[1] * counts[2]) as usize);
            let mut loudest: f64 = 0.0;
            for i in range(counts[0]) {
                let x = image(i, dims[0], path.source[0]);
                for j in range(counts[1]) {
                    let y = image(j, dims[1], path.source[1]);
                    for k in range(counts[2]) {
                        // The floor bounce of the direct path is left out, as the reference does.
                        if (i, j, k) == (0, 0, -1) {
                            continue;
                        }
                        let z = image(k, dims[2], path.source[2]);
                        let order = (i.abs() + j.abs() + k.abs()) as f64;
                        let hash = ((41 * k + 29 + 17 * i + j) * 1933) % 59;
                        let jitter = (hash as f64 / 59.0 * 2.0 - 1.0) * order / 48.0;
                        let d = distance([x, y, z], path.ear) + jitter;
                        let sign = if (i + j + k).rem_euclid(2) == 1 {
                            -1.0
                        } else {
                            1.0
                        };
                        let amplitude = sign * bounces(k) / (d * d);
                        loudest = loudest.max(amplitude.abs());
                        // The direct path counts for the normalisation but is not written.
                        if (i, j, k) != (0, 0, 0) {
                            found.push((d, amplitude));
                        }
                    }
                }
            }
            let loudest = loudest.max(1e-9);
            found
                .into_iter()
                .map(|(d, a)| (arrival(d), (a / loudest) as f32))
                .collect()
        })
        .collect();
    (taps, direct.into_iter().map(arrival).collect())
}

// --------------------------------------------------------------------------- the impulse

/// The random numbers: a 21-bit LCG, re-seeded at each synthesis.
pub struct Lcg(u32);

impl Lcg {
    pub fn new(seed: u32) -> Self {
        Self(seed % 412_123)
    }

    /// The next number in [0, 1).
    pub fn draw(&mut self) -> f64 {
        self.0 = ((524_309u64 * u64::from(self.0) + 1) % (1 << 21)) as u32;
        f64::from(self.0) / 2_097_152.0
    }
}

/// The synthesis's seed: the settings' f32 products truncated to 64 bits, then to 32 (a
/// negative Location wraps, never saturates), xored with Td and Tp.
pub fn seed(values: &[f64], td: usize, tp: usize) -> u32 {
    let wrap = |x: f32| (x as i64) as u32;
    let p = |i: usize| values[i] as f32;
    wrap(p(LOCATION) * 1.000_000_1e8_f32)
        ^ wrap(p(DIMENSION) * 10_007.0)
        ^ wrap(p(PERCEPTION) * 100_003.0)
        ^ wrap(p(DIFFUSION) * 1_000_003.0)
        ^ wrap(p(ROOM_SIZE) * 10_000_019.0)
        ^ td as u32
        ^ tp as u32
}

/// The impulse's sizes in samples at `rate`: Td (Decay), Tp (Pre-Delay, at least 10), Ld
/// (1.2·Td) and L (1.45·Td, the impulse before it is trimmed).
pub fn sizes(rate: f64, values: &[f64]) -> (usize, usize, usize, usize) {
    let td = ((f32_of(values[DECAY]) * rate / 1000.0) as usize).max(1);
    let tp = ((f32_of(values[PRE_DELAY]) * rate / 1000.0) as usize).max(10);
    let ld = (1.2 * td as f64) as usize;
    let len = (1.45 * td as f64) as usize;
    (td, tp, ld, len.max(4))
}

/// One channel's late pulses (before the pre-delay fade), `len` samples, drawing from `rng`.
pub fn late_pulses(values: &[f64], td: usize, ld: usize, len: usize, rng: &mut Lcg) -> Vec<f32> {
    let mut out = vec![0.0f32; len];
    let x = 4.5 - f32_of(values[DIFFUSION]) * 3300.0 / 1e6;
    let k = 10f64.powf(x);
    let level0 = 10f64.powf(0.3 * (x - 2.24));
    let gamma = f64::from(2f64.powf(7.0 * f32_of(values[PERCEPTION]) / 200.0) as f32);
    let sigma: f64 = (0..100).map(|i| (f64::from(i) / 100.0).powf(gamma)).sum();
    let a = 2.0 * level0 / sigma / (gamma / 12.0 + 1.0) / 0.6f64.sqrt();
    let slope = -2.6 * ld as f64 / td as f64;
    let ln_k = (k + 1.0).ln();
    let n = (0.6 * ld as f64) as usize;
    let mut r = rng.draw();
    for sign in [1.0f32, -1.0] {
        for _ in 0..n {
            let amplitude = r.powf(gamma);
            r = rng.draw();
            let t = (r * k + 1.0).ln() / ln_k;
            let envelope = ((1.0 - t) + 0.2).powf(2.5) * (slope * t * LN_10).exp();
            let at = ((len - 2) as f64 * t) as usize;
            out[at] += sign * (envelope * amplitude * a) as f32;
        }
    }
    out
}

/// Pre-Delay: a fade-in from −96 dB, the factor multiplied by 65536^(1/Tp) before each sample
/// from the first while it is under 1.
pub fn fade_in(r: &mut [f32], tp: usize) {
    let g = (65_536f64.ln() / tp as f64).exp() as f32;
    let mut f = 1.0f32 / 65_536.0;
    for x in r {
        f *= g;
        if f >= 1.0 {
            break;
        }
        *x *= f;
    }
}

/// Coloration's three sections for plain `values` at `rate` (mid peak, low shelf, high shelf;
/// a flat one is the identity), frequencies capped at half the rate.
pub fn coloration_sections(rate: f64, values: &[f64]) -> [Coefficients; 3] {
    let f = |i: usize| f32_of(values[i]).min(rate / 2.0);
    let g = |i: usize| f32_of(values[i]);
    [
        super::parametric_eq::peak(rate, f(MID_FREQ), g(MID_GAIN), f32_of(values[MID_Q])),
        super::parametric_eq::shelf(rate, f(LOW_FREQ), g(LOW_GAIN), false, false),
        super::parametric_eq::shelf(rate, f(HIGH_FREQ), g(HIGH_GAIN), false, true),
    ]
    .map(|c| c.unwrap_or(Coefficients::IDENTITY))
}

/// Coloration's period (samples): Coloration Decay, lengthened to keep to [`MAX_PASSES`] over
/// `len` samples.
pub fn coloration_period(rate: f64, values: &[f64], len: usize) -> usize {
    ((rate * f32_of(values[COLORATION_DECAY]) / 1000.0) as usize)
        .max(len.div_ceil(MAX_PASSES))
        .max(1)
}

/// Colour `x` in place: from every multiple of `period`, the sections run from flat state to
/// the end, their coefficients moving in a straight line from the identity to `sections` over
/// the first `period` samples of the pass.
pub fn colour(x: &mut [f64], sections: &[Coefficients; 3], period: usize) {
    if sections.iter().all(|c| *c == Coefficients::IDENTITY) {
        return;
    }
    let lerp = |c: &Coefficients, t: f64| Coefficients {
        b0: 1.0 + (c.b0 - 1.0) * t,
        b1: c.b1 * t,
        b2: c.b2 * t,
        a1: c.a1 * t,
        a2: c.a2 * t,
    };
    let mut start = 0;
    while start < x.len() {
        let mut z = [[0.0f64; 2]; 3];
        for (n, s) in x[start..].iter_mut().enumerate() {
            let ramp = n < period;
            let t = (n + 1) as f64 / period as f64;
            let mut v = *s;
            for (c, z) in sections.iter().zip(z.iter_mut()) {
                let c = if ramp { lerp(c, t) } else { *c };
                let y = c.b0 * v + z[0];
                z[0] = c.b1 * v - c.a1 * y + z[1];
                z[1] = c.b2 * v - c.a2 * y;
                v = y;
            }
            *s = v;
        }
        start += period;
    }
}

/// Where the trim ends `x` (from a quarter in, 1000 samples past the last 1000-sample window
/// with a sample above 2^-20, at most its length): `above` decides which samples count.
fn trimmed_length(x: &[f32], above: impl Fn(f32) -> bool) -> usize {
    let len = x.len();
    let mut last = len / 4;
    let mut at = len / 4;
    while at < len {
        let end = (at + TRIM_WINDOW).min(len);
        if x[at..end].iter().any(|&v| above(v)) {
            last = end;
        }
        at = end;
    }
    (last + TRIM_WINDOW).min(len)
}

/// The fixed DC blocker (the same coefficients at every rate), in place.
pub fn dc_block(x: &mut [f32]) {
    let mut w1 = 0.0f64;
    for s in x {
        let w = f64::from(*s) + 0.998_207_965_664_44 * w1;
        *s = (0.999_107_549_940_81 * w - 0.999_100_415_723_62 * w1) as f32;
        w1 = w;
    }
}

/// An impulse made from the settings.
#[derive(Debug, Clone)]
pub struct Made {
    /// Early reflections per impulse channel (2, or 4 in true stereo), at the rate, as long as
    /// they ring (to the trimmed length at most).
    pub early: Vec<Vec<f32>>,
    /// The late reverb per impulse channel, the trimmed length.
    pub late: Vec<Vec<f32>>,
    /// Each impulse channel's direct-path arrival (samples after the shortest).
    pub direct: Vec<usize>,
    /// The impulse's length before trimming (1.45·Td).
    pub length: usize,
    pub true_stereo: bool,
    /// How much the loudness cap took off (dB; 0 for any preset).
    pub limited_db: f64,
}

/// Make the impulse for plain `values` at `rate`, as the module's notes say.
pub fn synthesize(rate: f64, values: &[f64]) -> Made {
    let true_stereo = values[SUM_INPUTS] < 0.5;
    let (td, tp, ld, len) = sizes(rate, values);
    let mut rng = Lcg::new(seed(values, td, tp));
    let (taps, direct) = early_taps(rate, values, true_stereo);
    let high_pass = biquad::butterworth2(rate, f32_of(values[ER_HIGH_PASS]).min(rate / 2.0), true);
    let sections = coloration_sections(rate, values);
    let period = coloration_period(rate, values, len);
    // f64 while colouring, so gains that grow pass after pass can't overflow; f32 after.
    let to_f32 =
        |x: &[f64]| -> Vec<f32> { x.iter().map(|v| v.clamp(-1e30, 1e30) as f32).collect() };
    let mut early = Vec::with_capacity(taps.len());
    let mut late = Vec::with_capacity(taps.len());
    let mut lengths = Vec::with_capacity(taps.len());
    for channel_taps in &taps {
        let mut r = late_pulses(values, td, ld, len, &mut rng);
        fade_in(&mut r, tp);
        let mut e = vec![0.0f32; len];
        for &(at, a) in channel_taps {
            if at < len {
                e[at] += a;
            }
        }
        let mut hp = biquad::Section::new(high_pass);
        let mut e: Vec<f64> = e.iter().map(|&v| hp.tick(0, f64::from(v))).collect();
        let mut r: Vec<f64> = r.iter().map(|&v| f64::from(v)).collect();
        colour(&mut r, &sections, period);
        colour(&mut e, &sections, period);
        let (r, e) = (to_f32(&r), to_f32(&e));
        let above = |v: f32| v > TRIM_LEVEL;
        lengths.push(trimmed_length(&r, above).max(trimmed_length(&e, above)));
        late.push(r);
        early.push(e);
    }
    let trimmed = lengths.iter().copied().max().unwrap_or(len);
    let fade = (0.05 * trimmed as f64) as usize;
    for (r, e) in late.iter_mut().zip(early.iter_mut()) {
        r.truncate(trimmed);
        e.truncate(trimmed);
        for (n, x) in r.iter_mut().enumerate().skip(trimmed - fade) {
            *x *= (trimmed - n) as f32 / fade as f32;
        }
        dc_block(r);
        dc_block(e);
    }
    // Only as much of the early reflections as rings above the trim's level: their convolver
    // costs less, and what is left out is under -120 dB.
    let early_len = early
        .iter()
        .map(|e| trimmed_length(e, |v| v.abs() > TRIM_LEVEL))
        .max()
        .unwrap_or(0);
    for e in &mut early {
        e.truncate(early_len);
    }
    // The loudness cap.
    let energy = |x: &Vec<f32>| {
        x.iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt()
    };
    let loudest = late.iter().chain(&early).map(energy).fold(0.0f64, |m, e| {
        if e.is_finite() {
            m.max(e)
        } else {
            f64::INFINITY
        }
    });
    let mut limited_db = 0.0;
    if loudest > LOUDEST {
        let scale = if loudest.is_finite() {
            LOUDEST / loudest
        } else {
            0.0
        };
        limited_db = if scale > 0.0 {
            -20.0 * scale.log10()
        } else {
            f64::INFINITY
        };
        for x in late.iter_mut().chain(early.iter_mut()) {
            for v in x.iter_mut() {
                *v = if v.is_finite() {
                    (f64::from(*v) * scale) as f32
                } else {
                    0.0
                };
            }
        }
    }
    Made {
        early,
        late,
        direct,
        length: len,
        true_stereo,
        limited_db,
    }
}

/// Include Direct's cross-feed for each output (delay in samples, gain): the other input
/// delayed by the difference of the two direct arrivals d, scaled by 1 − min(d/60, 0.7). In
/// true stereo the difference is between the two speakers' paths to that ear; with one source,
/// between its paths to the two ears.
pub fn cross_feed(made: &Made) -> [(usize, f32); 2] {
    let a = &made.direct;
    let d = if made.true_stereo {
        [a[2].abs_diff(a[0]), a[1].abs_diff(a[3])]
    } else {
        let d = a[1].abs_diff(a[0]);
        [d, d]
    };
    d.map(|d| (d, (1.0 - (d as f64 / 60.0).min(0.7)) as f32))
}

/// The made impulse ready to play: one convolver for the early reflections and one for the
/// late reverb on each output's own paths (H_0, H_1), and in true stereo another pair for the
/// cross paths (H_3, H_2) on the swapped input.
pub struct Impulse {
    early: Convolver,
    late: Convolver,
    cross: Option<(Convolver, Convolver)>,
    sum_inputs: bool,
    /// Include Direct's cross-feed per output, if on.
    direct: Option<[(usize, f32); 2]>,
}

impl Impulse {
    pub fn new(made: &Made, include_direct: bool) -> Self {
        let pair =
            |x: &[Vec<f32>], a: usize, b: usize| Convolver::new(&[x[a].clone(), x[b].clone()], 1.0);
        Self {
            early: pair(&made.early, 0, 1),
            late: pair(&made.late, 0, 1),
            cross: made
                .true_stereo
                .then(|| (pair(&made.early, 3, 2), pair(&made.late, 3, 2))),
            sum_inputs: !made.true_stereo,
            direct: include_direct.then(|| cross_feed(made)),
        }
    }

    fn convolvers(&mut self) -> impl Iterator<Item = &mut Convolver> {
        let cross = self.cross.as_mut().map(|(e, l)| [e, l]);
        [&mut self.early, &mut self.late]
            .into_iter()
            .chain(cross.into_iter().flatten())
    }

    fn reset(&mut self) {
        self.convolvers().for_each(Convolver::reset);
    }

    fn quiet(&mut self) {
        self.convolvers().for_each(Convolver::quiet);
    }

    fn silence(&mut self) {
        self.convolvers().for_each(Convolver::silence);
    }

    fn drained(&self) -> bool {
        self.early.drained()
            && self.late.drained()
            && self
                .cross
                .as_ref()
                .is_none_or(|(e, l)| e.drained() && l.drained())
    }

    /// Bytes held for the spectra and delay lines.
    pub fn bytes(&self) -> usize {
        self.early.bytes()
            + self.late.bytes()
            + self
                .cross
                .as_ref()
                .map_or(0, |(e, l)| e.bytes() + l.bytes())
    }

    /// Convolve `n` samples of `input` (left, right; `None`: silence) into `early` and `late`
    /// (added to what they hold), using `sum` and `scratch` as work space.
    fn process(
        &mut self,
        input: Option<[&[f32]; 2]>,
        n: usize,
        early: &mut [Vec<f32>; 2],
        late: &mut [Vec<f32>; 2],
        sum: &mut [f32],
        scratch: &mut [Vec<f32>; 2],
    ) {
        let input = match input {
            Some([l, r]) if self.sum_inputs => {
                for i in 0..n {
                    sum[i] = 0.5 * (l[i] + r[i]);
                }
                let s: &[f32] = &sum[..n];
                Some([s, s])
            }
            other => other,
        };
        let swapped = input.map(|[l, r]| [r, l]);
        let mut run = |conv: &mut Convolver, input: Option<[&[f32]; 2]>, to: &mut [Vec<f32>; 2]| {
            let [s0, s1] = &mut *scratch;
            conv.process(input, [&mut s0[..n], &mut s1[..n]]);
            for (o, s) in to.iter_mut().zip(scratch.iter()) {
                for (o, s) in o[..n].iter_mut().zip(&s[..n]) {
                    *o += *s;
                }
            }
        };
        run(&mut self.early, input, early);
        run(&mut self.late, input, late);
        if let Some((e, l)) = &mut self.cross {
            run(e, swapped, early);
            run(l, swapped, late);
        }
    }
}

/// The impulse ready to play, and what the editor shows of it: rows E_0, E_1, R_0, R_1.
fn prepare(rate: f64, values: &[f64], _loaded: &Loaded) -> Prepared {
    let made = synthesize(rate, values);
    let impulse = Impulse::new(&made, values[INCLUDE_DIRECT] >= 0.5);
    let rows = [
        made.early[0].clone(),
        made.early[1].clone(),
        made.late[0].clone(),
        made.late[1].clone(),
    ];
    let about = Loaded {
        channels: Vec::new(),
        file_channels: made.late.len(),
        file_rate: rate.round() as u32,
        file_frames: made.length,
    };
    let mut overview = Overview::of(&rows, rate, 1.0, &about);
    overview.note = (made.limited_db > 0.0).then(|| {
        format!(
            "The coloration grows the impulse without bound: scaled down by {:.1} dB.",
            made.limited_db
        )
    });
    Prepared {
        dsp: Box::new(impulse),
        overview,
    }
}

// ------------------------------------------------------------------------------- playing

pub struct FullReverb {
    current: Option<Box<Impulse>>,
    /// The impulse it replaced, ringing out the input it had.
    draining: Option<Box<Impulse>>,
    /// The dry path: the input's recent past per channel (the latency and Include Direct's
    /// longest delay), written at `at`.
    ring: [Vec<f32>; 2],
    at: usize,
    /// Include Direct's cross-feed now (from the impulse playing).
    cross: Option<[(usize, f32); 2]>,
    early: [Vec<f32>; 2],
    late: [Vec<f32>; 2],
    sum: Vec<f32>,
    scratch: [Vec<f32>; 2],
    dry_level: Glide,
    early_level: Glide,
    late_level: Glide,
}

impl FullReverb {
    fn new(rate: f64) -> Self {
        let longest = LATENCY + (LONGEST_CROSS_S * rate).ceil() as usize + 1;
        let ring = longest.next_power_of_two();
        Self {
            current: None,
            draining: None,
            ring: [vec![0.0; ring], vec![0.0; ring]],
            at: 0,
            cross: None,
            early: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            late: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            sum: vec![0.0; CHUNK],
            scratch: [vec![0.0; CHUNK], vec![0.0; CHUNK]],
            dry_level: Glide::new(PARAMS[DRY].default),
            early_level: Glide::new(PARAMS[EARLY].default),
            late_level: Glide::new(PARAMS[REVERB].default),
        }
    }
}

impl NativeEffect for FullReverb {
    fn set(&mut self, index: usize, value: f64) {
        match index {
            DRY => self.dry_level.set(value),
            EARLY => self.early_level.set(value),
            REVERB => self.late_level.set(value),
            // The rest make the impulse, off the audio thread.
            _ => {}
        }
    }

    fn ramp(&mut self, index: usize, value: f64, samples: usize) {
        match index {
            DRY => self.dry_level.ramp(value, samples),
            EARLY => self.early_level.ramp(value, samples),
            REVERB => self.late_level.ramp(value, samples),
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
        for r in &mut self.ring {
            r.fill(0.0);
        }
        self.at = 0;
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
        match payload.downcast::<Impulse>() {
            Ok(new) => {
                if let Some(mut old) = self.current.take() {
                    old.quiet();
                    self.draining = Some(old);
                }
                self.cross = new.direct;
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
        let mask = self.ring[0].len() - 1;
        let mut at = from;
        while at < to {
            let n = CHUNK.min(to - at);
            for c in 0..2 {
                self.early[c][..n].fill(0.0);
                self.late[c][..n].fill(0.0);
            }
            {
                let Self {
                    current,
                    draining,
                    early,
                    late,
                    sum,
                    scratch,
                    ..
                } = self;
                if let Some(current) = current {
                    let input = [&l[at..at + n], &r[at..at + n]];
                    current.process(Some(input), n, early, late, sum, scratch);
                }
                if let Some(old) = draining {
                    old.process(None, n, early, late, sum, scratch);
                }
            }
            for i in 0..n {
                let dry = (self.dry_level.next() * 0.01) as f32;
                let ge = (self.early_level.next() * 0.005) as f32;
                let gr = (self.late_level.next() * 0.01) as f32;
                let x = [l[at + i], r[at + i]];
                let k = self.at;
                self.ring[0][k] = x[0];
                self.ring[1][k] = x[1];
                let past = |ring: &Vec<f32>, delay: usize| ring[k.wrapping_sub(delay) & mask];
                for c in 0..2 {
                    let mut d = past(&self.ring[c], LATENCY);
                    if let Some(cross) = &self.cross {
                        let (delay, gain) = cross[c];
                        d += gain * past(&self.ring[1 - c], LATENCY + delay.min(mask - LATENCY));
                    }
                    let y = dry * d + ge * self.early[c][i] + gr * self.late[c][i];
                    if c == 0 {
                        l[at + i] = y;
                    } else {
                        r[at + i] = y;
                    }
                }
                self.at = (k + 1) & mask;
            }
            at += n;
        }
    }
}

#[cfg(test)]
#[path = "full_reverb_tests.rs"]
mod tests;
