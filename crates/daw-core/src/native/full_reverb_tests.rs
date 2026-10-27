//! The Space Reverb's tests: the parameters, the room, the impulse made from the
//! settings as studied (image sources, the late pulses' decay, Pre-Delay's fade, Coloration's
//! growth), and the effect playing it.

use super::*;
use crate::graph::{Processor, Transport};
use crate::native::{find, render, Native};
use crate::plugins::InsertFlags;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const RATE: f64 = 48_000.0;

/// Plain values: the defaults, with `pairs` set.
fn values(pairs: &[(usize, f64)]) -> Vec<f64> {
    let mut v: Vec<f64> = KIND.params.iter().map(|p| p.default).collect();
    for &(i, x) in pairs {
        v[i] = x;
    }
    v
}

/// The same by name, for `render`.
fn named(pairs: &[(usize, f64)]) -> BTreeMap<String, f64> {
    pairs
        .iter()
        .map(|&(i, x)| (KIND.params[i].name.to_string(), x))
        .collect()
}

/// No coloration.
const FLAT: [(usize, f64); 3] = [(LOW_GAIN, 0.0), (MID_GAIN, 0.0), (HIGH_GAIN, 0.0)];

/// Deterministic noise in ±1.
fn noise(n: usize, seed: u64) -> Vec<f32> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            ((x >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0) as f32
        })
        .collect()
}

/// The difference's RMS relative to the reference's, in dB.
fn null_db(ours: &[f32], reference: &[f32]) -> f64 {
    let (mut d, mut r) = (0.0f64, 0.0f64);
    for (a, b) in ours.iter().zip(reference) {
        d += (f64::from(*a) - f64::from(*b)).powi(2);
        r += f64::from(*b).powi(2);
    }
    10.0 * (d / r).log10()
}

fn energy_db(x: &[f32]) -> f64 {
    10.0 * x
        .iter()
        .map(|v| f64::from(*v).powi(2))
        .sum::<f64>()
        .max(1e-300)
        .log10()
}

#[test]
fn the_kind_has_29_parameters() {
    let kind = find("space reverb").expect("registered");
    assert!(std::ptr::eq(kind, &KIND));
    assert_eq!(kind.params.len(), 29);
    for (i, name) in [
        (DECAY, "Reverberation Decay Time"),
        (REVERB, "Output Level (Reverberation)"),
        (ER_HIGH_PASS, "Early Reflections High Pass Cutoff"),
        (MID_FREQ, "Mid Coloration Center Frequency"),
        (SUM_INPUTS, "Sum Inputs"),
        (ROOM_HEIGHT, "Room Height"),
        (ORIGINAL_RATE, "Original Sample Rate"),
    ] {
        assert_eq!(kind.params[i].name, name);
    }
    // Only the output levels have lanes.
    let lanes: Vec<usize> = (0..29).filter(|&i| kind.params[i].automatable).collect();
    assert_eq!(lanes, [DRY, EARLY, REVERB]);
    // Every default is in range; the inputs are summed by default (true stereo is a switch).
    for p in kind.params {
        assert!(
            (p.min..=p.max).contains(&p.default),
            "{} = {}",
            p.name,
            p.default
        );
    }
    assert_eq!(kind.params[SUM_INPUTS].default, 1.0);
}

#[test]
fn the_room_follows_room_size_and_dimension() {
    // The defaults' room: 21.8 x 29.7 x 13.7 m.
    let [w, d, h] = room(8879.6, 0.736);
    assert!((w - 21.8).abs() < 0.05 && (d - 29.7).abs() < 0.05 && (h - 13.7).abs() < 0.05);
    let native = Native::new(&KIND, RATE, None).unwrap();
    let plain = |n: &Native, i: usize| n.plain_values()[i];
    let dims = |n: &Native| [ROOM_WIDTH, ROOM_HEIGHT, ROOM_DEPTH].map(|i| plain(n, i));
    let close = |a: [f64; 3], b: [f64; 3]| a.iter().zip(&b).all(|(x, y)| (x - y).abs() < 1e-9);
    assert!(close(dims(&native), [w, d, h]), "{:?}", dims(&native));
    let set = |n: &Native, i: usize, v: f64| n.set(i as u32, KIND.params[i].normalized(v)).unwrap();
    set(&native, ROOM_SIZE, 20_000.0);
    assert!(close(dims(&native), room(20_000.0, 0.736)));
    // Outputs keep their values whatever is written.
    set(&native, ROOM_WIDTH, 3.0);
    set(&native, CALCULATING, 10.0);
    set(&native, CHANNEL_COUNT, 7.0);
    assert!(close(dims(&native), room(20_000.0, 0.736)));
    assert_eq!(plain(&native, CALCULATING), 100.0);
    assert_eq!(plain(&native, CHANNEL_COUNT), 2.0);
    // "Set Reverb Based on Room Size": Sabine's decay (feet, absorption 0.2) and half the
    // dimensions' sum (feet) in ms, when switched on and when the room changes.
    let sabine_by_hand = |room: [f64; 3]| {
        let [w, d, h] = room.map(|x| x * 3.280_839_895);
        let volume = w * d * h;
        let surface = 2.0 * (w * d + w * h + d * h);
        (
            (0.049 * volume / (0.2 * surface) * 1000.0).min(6000.0),
            ((w + d + h) / 2.0).min(200.0),
        )
    };
    let decay_before = plain(&native, DECAY);
    set(&native, BASED_ON_ROOM, 1.0);
    let (decay, pre) = sabine_by_hand(room(20_000.0, 0.736));
    assert!((plain(&native, DECAY) - decay).abs() < 1e-6, "{decay}");
    assert!((plain(&native, PRE_DELAY) - pre).abs() < 1e-6, "{pre}");
    assert_ne!(decay, decay_before);
    set(&native, DIMENSION, 2.0);
    let (decay, _) = sabine_by_hand(room(20_000.0, 2.0));
    assert!((plain(&native, DECAY) - decay).abs() < 1e-6);
    // A Decay set by hand afterwards stays.
    set(&native, DECAY, 1234.0);
    assert!((plain(&native, DECAY) - 1234.0).abs() < 1e-9);
    // Original Sample Rate 44100 -> 48000 moves each frequency in proportion above 20 Hz; back
    // to 44100 puts it back.
    let before: Vec<f64> = FREQUENCIES.iter().map(|&i| plain(&native, i)).collect();
    set(&native, ORIGINAL_RATE, 48_000.0);
    for (k, &i) in FREQUENCIES.iter().enumerate() {
        let want = (20.0 + (before[k] - 20.0) * (23_980.0 / 22_030.0)).min(24_000.0);
        assert!(
            (plain(&native, i) - want).abs() < 1e-6,
            "{}",
            KIND.params[i].name
        );
    }
    set(&native, ORIGINAL_RATE, 44_100.0);
    for (k, &i) in FREQUENCIES.iter().enumerate() {
        assert!((plain(&native, i) - before[k]).abs() < 1e-6);
    }
}

#[test]
fn the_seed_wraps_a_negative_location() {
    // A saturating cast would make every negative Location's product 0.
    let (td, tp) = (126_528, 5088);
    let at = |location: f64| seed(&values(&[(LOCATION, location)]), td, tp);
    assert_ne!(at(-3.4), at(-50.0));
    assert_ne!(at(-3.4), at(0.0));
    let product = (-3.4f32 * 1.000_000_1e8_f32) as i64;
    assert!(product < 0);
    assert_eq!(
        at(-3.4) ^ at(0.0),
        product as u32,
        "only Location's term differs"
    );
}

#[test]
fn the_impulse_is_a_pure_function_of_the_settings_and_the_rate() {
    let base = values(&[(DECAY, 400.0)]);
    let a = synthesize(RATE, &base);
    let b = synthesize(RATE, &base);
    assert_eq!(a.late, b.late);
    assert_eq!(a.early, b.early);
    assert_eq!(a.direct, b.direct);
    assert!(a.late[0] != a.late[1], "each ear its own pulses");
    for changed in [
        values(&[(DECAY, 400.0), (DIFFUSION, 700.0)]),
        values(&[(DECAY, 400.0), (PERCEPTION, 100.0)]),
        values(&[(DECAY, 410.0)]),
    ] {
        let c = synthesize(RATE, &changed);
        assert!(c.late != a.late);
    }
    let other_rate = synthesize(44_100.0, &base);
    assert!(other_rate.late[0].len() != a.late[0].len() || other_rate.late != a.late);
    // A room change moves the reflections, not the late pulses' times (their seed has the room
    // in it, so those change too).
    let moved = synthesize(RATE, &values(&[(DECAY, 400.0), (LOCATION, 50.0)]));
    assert!(moved.early != a.early);
}

/// The study's geometry, written out again here: the first-order image of the source in each
/// wall (the floor's is left out) for each ear, their arrivals after the shortest direct path at
/// 1130 ft/s and their amplitudes (−1)·(0.85 for the ceiling)/d² over the direct path's 1/d².
fn first_order_by_hand(v: &[f64], rate: f64) -> Vec<Vec<(usize, f64)>> {
    let ft = 3.280_839_895;
    let [w, d, h] = room(
        f64::from(v[ROOM_SIZE] as f32),
        f64::from(v[DIMENSION] as f32),
    )
    .map(|x| x * ft);
    let lambda = f64::from(v[LOCATION] as f32) / 100.0;
    let x_l = w / 2.0 - lambda * w * 0.1;
    let ears = [x_l - 0.167 * ft, x_l + 0.167 * ft]
        .map(|x| [x, d * 0.618_033_988_7 * 4.0 / 3.0, 3.539_006_974_395_292_5]);
    let source = [w / 2.0 + lambda * w * 0.42, 0.22 * d, 6.510_214_840_197_056];
    let dist = |a: [f64; 3], b: [f64; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let direct = ears.map(|e| dist(source, e));
    let d_ref = direct[0].min(direct[1]);
    // (i, j, k, image position, reflection factor)
    let walls = [
        (-1i64, 0i64, 0i64, [-source[0], source[1], source[2]], 1.0),
        (1, 0, 0, [2.0 * w - source[0], source[1], source[2]], 1.0),
        (0, -1, 0, [source[0], -source[1], source[2]], 1.0),
        (0, 1, 0, [source[0], 2.0 * d - source[1], source[2]], 1.0),
        (0, 0, 1, [source[0], source[1], 2.0 * h - source[2]], 0.85),
    ];
    ears.iter()
        .zip(direct)
        .map(|(ear, d0)| {
            let mut taps: Vec<(usize, f64)> = walls
                .iter()
                .map(|&(i, j, k, pos, factor)| {
                    let jitter = ((((41 * k + 29 + 17 * i + j) * 1933) % 59) as f64 / 59.0 * 2.0
                        - 1.0)
                        / 48.0;
                    let dd = dist(pos, *ear) + jitter;
                    let arrival = ((dd - d_ref) / 1130.0 * rate) as usize;
                    (arrival, -factor / (dd * dd) * d0 * d0)
                })
                .collect();
            taps.sort_by_key(|t| t.0);
            taps
        })
        .collect()
}

#[test]
fn early_reflections_arrive_at_the_image_source_delays_with_their_gains() {
    // No coloration, the high-pass at 20 Hz: each reflection is one sample at its arrival,
    // 0.024 dB lower for the high-pass's and the DC blocker's first samples.
    for (location, dimension) in [(-3.4, 0.736), (60.0, 1.5)] {
        let v = values(&[
            FLAT[0],
            FLAT[1],
            FLAT[2],
            (ER_HIGH_PASS, 20.0),
            (DECAY, 1000.0),
            (LOCATION, location),
            (DIMENSION, dimension),
        ]);
        let made = synthesize(RATE, &v);
        let expected = first_order_by_hand(&v, RATE);
        for (c, taps) in expected.iter().enumerate() {
            let e = &made.early[c];
            for &(at, gain) in taps.iter().take(4) {
                let got = f64::from(e[at]);
                let db = 20.0 * (got / gain).abs().log10();
                println!(
                    "location {location}, dimension {dimension}, ear {c}: sample {at}, {:.2} dB \
                     (expected {:.2} dB), {db:+.3} dB",
                    20.0 * got.abs().log10(),
                    20.0 * gain.abs().log10()
                );
                assert!(got * gain > 0.0, "ear {c}, sample {at}: the sign");
                assert!(db.abs() < 0.1, "ear {c}, sample {at}: {db:.3} dB");
                // On that sample, not the next or the one before.
                assert!(
                    got.abs() > f64::from(e[at - 1].abs())
                        && got.abs() > f64::from(e[at + 1].abs())
                );
            }
        }
        // The direct paths: the nearer ear at 0.
        assert_eq!(*made.direct.iter().min().unwrap(), 0);
    }
}

/// T60 (s) from the Schroeder integral of `energy` (per sample at `rate`), fitted between -5 and
/// -35 dB.
fn t60(energy: &[f64], rate: f64) -> f64 {
    let mut edc = vec![0.0; energy.len()];
    let mut sum = 0.0;
    for n in (0..energy.len()).rev() {
        sum += energy[n];
        edc[n] = sum;
    }
    let total = edc[0];
    let points: Vec<(f64, f64)> = edc
        .iter()
        .enumerate()
        .map(|(n, e)| (n as f64 / rate, 10.0 * (e / total).log10()))
        .filter(|(_, db)| (-35.0..=-5.0).contains(db))
        .collect();
    let k = points.len() as f64;
    let (mx, my) = points
        .iter()
        .fold((0.0, 0.0), |(a, b), (x, y)| (a + x / k, b + y / k));
    let (num, den) = points.iter().fold((0.0, 0.0), |(a, b), (x, y)| {
        (a + (x - mx) * (y - my), b + (x - mx) * (x - mx))
    });
    -60.0 / (num / den)
}

/// The late reverb's expected energy per sample: pulse density (K + 1)^t times the envelope
/// squared, ((1.2 − t)^2.5·10^(slope·t))², t = n/(L − 2); with R's end fade over `trimmed`.
fn modelled_energy(v: &[f64], rate: f64, trimmed: usize) -> Vec<f64> {
    let (td, _, ld, len) = sizes(rate, v);
    let x = 4.5 - f64::from(v[DIFFUSION] as f32) * 3300.0 / 1e6;
    let k = 10f64.powf(x);
    let slope = -2.6 * ld as f64 / td as f64;
    let fade = (0.05 * trimmed as f64) as usize;
    (0..trimmed)
        .map(|n| {
            let t = n as f64 / (len - 2) as f64;
            let e = (1.2 - t).powf(2.5) * 10f64.powf(slope * t);
            let f = if n >= trimmed - fade {
                (trimmed - n) as f64 / fade as f64
            } else {
                1.0
            };
            (k + 1.0).powf(t) * e * e * f * f
        })
        .collect()
}

#[test]
fn the_late_reverb_decays_as_its_envelope_and_follows_decay() {
    // No coloration and no pre-delay: the late reverb's T60 (from T30) against the envelope's,
    // and in proportion to Decay. Both ears, two diffusions.
    let mut measured = Vec::new();
    for (decay, diffusion) in [(800.0, 823.0), (1600.0, 823.0), (1600.0, 300.0)] {
        let v = values(&[
            FLAT[0],
            FLAT[1],
            FLAT[2],
            (DECAY, decay),
            (DIFFUSION, diffusion),
            (PRE_DELAY, 0.0),
        ]);
        let made = synthesize(RATE, &v);
        let model = t60(&modelled_energy(&v, RATE, made.late[0].len()), RATE);
        for (c, r) in made.late.iter().enumerate() {
            let energy: Vec<f64> = r.iter().map(|x| f64::from(*x).powi(2)).collect();
            let t = t60(&energy, RATE);
            println!(
                "Decay {decay} ms, Diffusion {diffusion}: ear {c} T60 {:.1} ms, the envelope's \
                 {:.1} ms ({:+.1} %), {:.3} x Decay",
                t * 1e3,
                model * 1e3,
                (t / model - 1.0) * 100.0,
                t * 1e3 / decay
            );
            assert!(
                (t / model - 1.0).abs() < 0.05,
                "ear {c}: {t} s, the model {model} s"
            );
            if diffusion == 823.0 {
                measured.push(t);
            }
        }
    }
    // Twice the Decay, twice the T60.
    let ratio = (measured[2] + measured[3]) / (measured[0] + measured[1]);
    assert!((ratio - 2.0).abs() < 0.1, "{ratio}");
}

#[test]
fn pre_delay_fades_the_late_reverb_in_from_minus_96_db() {
    // The fade on its own: 2^-16 times 65536^((n + 1)/Tp) until it reaches 1.
    let tp = 4800;
    let mut ones = vec![1.0f32; 10_000];
    fade_in(&mut ones, tp);
    for n in [0, 1, 1199, 2399, 3599, 4798] {
        let want = -96.329_598_612_473_98 * (1.0 - (n + 1) as f64 / tp as f64);
        let got = 20.0 * f64::from(ones[n]).log10();
        assert!(
            (got - want).abs() < 0.01,
            "sample {n}: {got:.3} dB, {want:.3}"
        );
    }
    assert!(ones[tp..].iter().all(|&x| x == 1.0));
    assert!(ones.windows(2).all(|w| w[1] >= w[0]));
    // In the impulse: the first 5 ms against 100..150 ms, with Pre-Delay 100 ms and 0.
    for (pre, at_most, at_least) in [(100.0, -80.0, f64::NEG_INFINITY), (0.0, 20.0, -10.0)] {
        let v = values(&[FLAT[0], FLAT[1], FLAT[2], (DECAY, 1000.0), (PRE_DELAY, pre)]);
        let made = synthesize(RATE, &v);
        let r = &made.late[0];
        let start = energy_db(&r[..240]) - energy_db(&r[4800..7200]) + 10.0 * 10f64.log10();
        println!(
            "Pre-Delay {pre} ms: the first 5 ms at {start:.1} dB against 100..150 ms (per ms)"
        );
        assert!(start < at_most && start > at_least, "{start}");
    }
}

/// `x` through a fourth-order Butterworth low-pass (`high` false) or high-pass at `f`.
fn band(x: &[f32], f: f64, high: bool) -> Vec<f32> {
    let c = biquad::butterworth2(RATE, f, high);
    let (mut a, mut b) = (biquad::Section::new(c), biquad::Section::new(c));
    x.iter()
        .map(|&v| b.tick(0, a.tick(0, f64::from(v))) as f32)
        .collect()
}

#[test]
fn coloration_grows_with_time_as_specified() {
    // A low shelf of -6 dB at 1 kHz re-applied every 500 ms. A pass starts every 500 ms and has
    // reached the full shelf when the next starts, so just after 1 s the low band has been
    // through two whole passes (-12 dB) and just after 2 s through four (-24 dB); above the
    // shelf it loses only the shelf's little there. The same pulses either way (coloration
    // isn't in the seed). In between, a pass's coefficients move in a straight line from flat,
    // which in dB is mostly near the end of the period: the 100 ms before each mark shows it.
    let flat = values(&[FLAT[0], FLAT[1], FLAT[2], (DECAY, 2000.0), (PRE_DELAY, 0.0)]);
    let mut shelved = flat.clone();
    shelved[LOW_GAIN] = -6.0;
    shelved[LOW_FREQ] = 1000.0;
    shelved[COLORATION_DECAY] = 500.0;
    let (a, b) = (synthesize(RATE, &flat), synthesize(RATE, &shelved));
    let shelf = super::super::parametric_eq::shelf(RATE, 1000.0, -6.0, false, false).unwrap();
    let per_pass = |hz: f64| 20.0 * shelf.magnitude(hz, RATE).log10();
    // Energy of `x` from `from` to `to` seconds.
    let window =
        |x: &[f32], from: f64, to: f64| energy_db(&x[(from * RATE) as usize..(to * RATE) as usize]);
    for (name, hz, fa, fb) in [
        (
            "low",
            50.0,
            band(&a.late[0], 100.0, false),
            band(&b.late[0], 100.0, false),
        ),
        (
            "high",
            12_000.0,
            band(&a.late[0], 8000.0, true),
            band(&b.late[0], 8000.0, true),
        ),
    ] {
        for passes in [2.0, 4.0] {
            let mark = passes * 0.5;
            let got = window(&fb, mark, mark + 0.05) - window(&fa, mark, mark + 0.05);
            let want = per_pass(hz) * passes;
            let ramping = window(&fb, mark - 0.1, mark) - window(&fa, mark - 0.1, mark);
            println!(
                "{name} band just after {mark} s: {got:+.2} dB ({passes} passes of {:.2} dB: {want:+.2}); the 100 ms before it {ramping:+.2} dB",
                per_pass(hz)
            );
            assert!((got - want).abs() < 0.5, "{got:.2} dB, {want:.2}");
        }
    }
}

/// The impulse `made` gives one channel for a unit sample on input `from` (0 left, 1 right) at
/// plain `v`'s levels: the impulse channels the traced matrix routes there.
fn expected_wet(made: &Made, v: &[f64], from: usize, to: usize) -> Vec<f32> {
    let (ge, gr) = ((v[EARLY] * 0.005) as f32, (v[REVERB] * 0.01) as f32);
    let len = made.late[0].len();
    let add = |out: &mut Vec<f32>, c: usize, scale: f32| {
        for (n, o) in out.iter_mut().enumerate() {
            let e = made.early[c].get(n).copied().unwrap_or(0.0);
            *o += scale * (ge * e + gr * made.late[c][n]);
        }
    };
    let mut out = vec![0.0f32; len];
    if made.true_stereo {
        // Y_0 = X_0 H_0 + X_1 H_3, Y_1 = X_1 H_1 + X_0 H_2.
        let c = match (from, to) {
            (0, 0) => 0,
            (1, 0) => 3,
            (1, 1) => 1,
            _ => 2,
        };
        add(&mut out, c, 1.0);
    } else {
        // Summed: half of the input into each ear's own paths.
        add(&mut out, to, 0.5);
    }
    out
}

#[test]
fn a_unit_sample_plays_the_dry_path_and_the_made_impulse_at_its_levels() {
    for (sum, direct) in [(1.0, 0.0), (0.0, 0.0), (1.0, 1.0), (0.0, 1.0)] {
        let pairs = [
            (DECAY, 400.0),
            (SUM_INPUTS, sum),
            (INCLUDE_DIRECT, direct),
            (LOCATION, 40.0),
            (EARLY, 150.0),
            (REVERB, 80.0),
            (DRY, 60.0),
        ];
        let v = values(&pairs);
        let made = synthesize(RATE, &v);
        let len = made.late[0].len();
        let at = 1000;
        for from in 0..2 {
            let mut input = vec![vec![0.0f32; at + len], vec![0.0f32; at + len]];
            input[from][at] = 1.0;
            let out = render(&KIND, RATE, &named(&pairs), &input, 512).unwrap();
            let cross = cross_feed(&made);
            for to in 0..2 {
                let mut want = vec![0.0f32; at + len];
                for (n, w) in expected_wet(&made, &v, from, to).into_iter().enumerate() {
                    want[at + n] = w;
                }
                let dry = (v[DRY] * 0.01) as f32;
                if from == to {
                    want[at] += dry;
                } else if direct >= 0.5 {
                    let (d, k) = cross[to];
                    want[at + d] += dry * k;
                }
                let db = null_db(&out[to], &want);
                println!(
                    "Sum Inputs {sum}, Include Direct {direct}: input {from} to output {to}: \
                     {db:.1} dB"
                );
                assert!(db < -120.0, "input {from} to output {to}: {db:.1} dB");
            }
        }
    }
}

#[test]
fn blocks_of_any_size_play_the_same() {
    for sum in [1.0, 0.0] {
        let params = named(&[(DECAY, 300.0), (SUM_INPUTS, sum), (LOCATION, -30.0)]);
        let input = [noise(30_000, 3), noise(30_000, 4)];
        let reference = render(&KIND, RATE, &params, &input, 512).unwrap();
        assert!(energy_db(&reference[0]) > 0.0);
        for block in [77, 1, 4096] {
            let out = render(&KIND, RATE, &params, &input, block).unwrap();
            assert!(out == reference, "Sum Inputs {sum}, blocks of {block}");
        }
    }
}

#[test]
fn a_settings_change_while_playing_takes_over_without_a_gap() {
    // Noise played through the effect in blocks; at block 30 Decay changes and the impulse is
    // made again: the old impulse rings out the input before the switch while the new one plays
    // the input after it, with nothing missed and no quiet stretch.
    let (block, blocks, swap) = (512, 80, 30);
    let mut native = Native::new(&KIND, RATE, None).unwrap();
    let set = |n: &Native, i: usize, v: f64| n.set(i as u32, KIND.params[i].normalized(v)).unwrap();
    set(&native, DECAY, 300.0);
    native.prepare_now(RATE, None).unwrap();
    let before = native.plain_values();
    let flags = InsertFlags::default();
    let mut insert = native.insert(Arc::new(AtomicBool::new(false)), flags.clone());
    let input = [noise(block * blocks, 5), noise(block * blocks, 6)];
    let mut out = [Vec::new(), Vec::new()];
    let mut after = before.clone();
    for b in 0..blocks {
        if b == swap {
            set(&native, DECAY, 500.0);
            native.prepare_now(RATE, None).unwrap();
            after = native.plain_values();
        }
        let mut io: Vec<Vec<f32>> = (0..2)
            .map(|c| input[c][b * block..(b + 1) * block].to_vec())
            .collect();
        let transport = Transport {
            playing: true,
            position: (b * block) as i64,
            ..Transport::default()
        };
        insert.process(&mut io, block, &transport);
        for c in 0..2 {
            out[c].extend_from_slice(&io[c]);
        }
    }
    assert_eq!(flags.misses.load(Ordering::Relaxed), 0);
    // What it should be, made offline with the impulses' own convolvers (checked against the
    // made impulse by `a_unit_sample_plays_the_dry_path_and_the_made_impulse_at_its_levels`).
    let total = block * blocks;
    let s = swap * block;
    let wet = |v: &[f64], x: [&[f32]; 2]| -> [Vec<f32>; 2] {
        let made = synthesize(RATE, v);
        let mut imp = Impulse::new(&made, v[INCLUDE_DIRECT] >= 0.5);
        let n = x[0].len();
        let mut early = [vec![0.0; n], vec![0.0; n]];
        let mut late = [vec![0.0; n], vec![0.0; n]];
        let mut sum = vec![0.0; n];
        let mut scratch = [vec![0.0; n], vec![0.0; n]];
        imp.process(Some(x), n, &mut early, &mut late, &mut sum, &mut scratch);
        let (ge, gr) = ((v[EARLY] * 0.005) as f32, (v[REVERB] * 0.01) as f32);
        [0, 1].map(|c| (0..n).map(|i| ge * early[c][i] + gr * late[c][i]).collect())
    };
    let pre: Vec<Vec<f32>> = (0..2)
        .map(|c| {
            (0..total)
                .map(|i| if i < s { input[c][i] } else { 0.0 })
                .collect()
        })
        .collect();
    let a = wet(&before, [&pre[0], &pre[1]]);
    let b = wet(&after, [&input[0][s..], &input[1][s..]]);
    let made = synthesize(RATE, &after);
    let (d, k) = cross_feed(&made)[0];
    for c in 0..2 {
        let dry = (before[DRY] * 0.01) as f32;
        let want: Vec<f32> = (0..total)
            .map(|i| {
                // The dry path, delayed by the latency, with Include Direct's cross-feed.
                let x = |ch: usize, back: usize| {
                    if i >= LATENCY + back {
                        input[ch][i - LATENCY - back]
                    } else {
                        0.0
                    }
                };
                let dry_part = dry * (x(c, 0) + k * x(1 - c, d));
                // The convolvers' outputs carry the latency already.
                let w = a[c][i] + if i >= s { b[c][i - s] } else { 0.0 };
                dry_part + w
            })
            .collect();
        let db = null_db(&out[c], &want);
        println!("a switch while playing, channel {c}: {db:.1} dB");
        assert!(db < -120.0, "channel {c}: {db:.1} dB");
        let quiet = out[c][s..]
            .chunks(256)
            .position(|w| w.iter().map(|x| x * x).sum::<f32>() < 1e-6);
        assert_eq!(quiet, None, "channel {c}: a gap after the switch");
    }
}

#[test]
fn without_an_impulse_yet_only_the_dry_path_plays() {
    let native = Native::new(&KIND, RATE, None).unwrap();
    let flags = InsertFlags::default();
    let mut insert = native.insert(Arc::new(AtomicBool::new(false)), flags);
    assert_eq!(insert.latency(), LATENCY);
    let input = noise(2048, 9);
    let mut io = vec![input.clone(), vec![0.0; 2048]];
    insert.process(&mut io, 2048, &Transport::default());
    let dry = (73.1f64 * 0.01) as f32;
    for n in LATENCY..2048 {
        assert_eq!(io[0][n], dry * input[n - LATENCY], "sample {n}");
    }
    assert!(native.file_pending(), "the impulse is still to make");
}

/// Release timing and a survey of settings (`cargo test -p daw-core --release -- --ignored
/// full_reverb_cost --nocapture`): making the impulse (the defaults at 44.1 and 48 kHz, the
/// longest Decay at 48 and 192 kHz, true stereo), a few settings' impulses at 44.1 kHz (length,
/// energy, whether the loudness cap acted), and the worst 64-sample block while playing the
/// defaults at 48 kHz.
#[test]
#[ignore]
fn full_reverb_cost() {
    let time = |rate: f64, v: &[f64]| {
        let started = std::time::Instant::now();
        let prepared = prepare(rate, v, &Loaded::default());
        let took = started.elapsed().as_secs_f64() * 1e3;
        let bytes = prepared
            .dsp
            .downcast_ref::<Impulse>()
            .map_or(0, Impulse::bytes);
        (took, prepared.overview.samples, bytes)
    };
    for (label, rate, v) in [
        ("defaults", 44_100.0, values(&[])),
        ("defaults", 48_000.0, values(&[])),
        ("Decay 6000 ms", 48_000.0, values(&[(DECAY, 6000.0)])),
        ("Decay 6000 ms", 192_000.0, values(&[(DECAY, 6000.0)])),
        (
            "a small room in true stereo",
            48_000.0,
            values(&[
                (SUM_INPUTS, 0.0),
                (ROOM_SIZE, 60.0),
                (DECAY, 900.0),
                (PRE_DELAY, 5.0),
            ]),
        ),
    ] {
        let (ms, samples, bytes) = time(rate, &v);
        println!(
            "{label} at {:.1} kHz: made and partitioned in {ms:.0} ms; {:.2} s, {:.1} MB",
            rate / 1000.0,
            samples as f64 / rate,
            bytes as f64 / 1e6
        );
    }
    for (name, v) in [
        ("defaults", values(&[])),
        ("a small room", values(&[(ROOM_SIZE, 60.0), (DECAY, 900.0)])),
        (
            "a long hall",
            values(&[(ROOM_SIZE, 40_000.0), (DECAY, 5000.0), (PRE_DELAY, 60.0)]),
        ),
        (
            "true stereo",
            values(&[(SUM_INPUTS, 0.0), (INCLUDE_DIRECT, 0.0)]),
        ),
    ] {
        let started = std::time::Instant::now();
        let made = synthesize(44_100.0, &v);
        let ms = started.elapsed().as_secs_f64() * 1e3;
        let rms = |x: &[Vec<f32>]| {
            x.iter()
                .map(|c| energy_db(c))
                .fold(f64::NEG_INFINITY, f64::max)
        };
        println!(
            "{name:30} {:.2} s (early {:.2} s), late {:+.1} dB, early {:+.1} dB, capped {:.1} dB, \
             {ms:.0} ms",
            made.late[0].len() as f64 / 44_100.0,
            made.early[0].len() as f64 / 44_100.0,
            rms(&made.late),
            rms(&made.early),
            made.limited_db
        );
    }
    // Playing: 64-sample blocks of noise through the defaults' impulse.
    let mut native = Native::new(&KIND, RATE, None).unwrap();
    native.prepare_now(RATE, None).unwrap();
    let mut insert = native.insert(Arc::new(AtomicBool::new(false)), InsertFlags::default());
    let input = [noise(48_000, 1), noise(48_000, 2)];
    let mut io = vec![vec![0.0f32; 64]; 2];
    let mut run = |b: usize| {
        let at = (b * 64) % (48_000 - 64);
        for c in 0..2 {
            io[c].copy_from_slice(&input[c][at..at + 64]);
        }
        let transport = Transport {
            playing: true,
            position: (b * 64) as i64,
            ..Transport::default()
        };
        let t = std::time::Instant::now();
        insert.process(&mut io, 64, &transport);
        t.elapsed()
    };
    for b in 0..750 {
        run(b);
    }
    let started = std::time::Instant::now();
    let mut times: Vec<std::time::Duration> = (750..750 + 7500).map(&mut run).collect();
    let per_second = started.elapsed().as_secs_f64() / 10.0;
    times.sort_unstable();
    println!(
        "playing the defaults at 48 kHz: {:.2} % of a core; 64-sample blocks worst {:.3} ms, \
         99.9th percentile {:.3} ms (a block lasts 1.333 ms)",
        per_second * 100.0,
        times[times.len() - 1].as_secs_f64() * 1e3,
        times[times.len() * 999 / 1000].as_secs_f64() * 1e3
    );
}
