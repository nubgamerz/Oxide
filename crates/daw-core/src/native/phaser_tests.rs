//! The Phaser's tests (`phaser.rs`).

use super::super::render;
use super::*;
use std::collections::BTreeMap;

fn settings(values: &[f64; COUNT]) -> BTreeMap<String, f64> {
    PARAMS
        .iter()
        .zip(values)
        .map(|(p, v)| (p.name.to_string(), *v))
        .collect()
}

fn with(pairs: &[(usize, f64)]) -> [f64; COUNT] {
    let mut v = PARAMS.map(|p| p.default);
    for &(i, x) in pairs {
        v[i] = x;
    }
    v
}

/// Uniform white noise in ±`peak` from a fixed seed (xorshift).
fn noise(n: usize, seed: u64, peak: f32) -> Vec<f32> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            (((x >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0) as f32 * peak
        })
        .collect()
}

/// The left channel's gain (dB) at each of `hz` measured from the effect's impulse response
/// (32768 samples at `rate`).
fn measured_db(rate: f64, values: &[f64; COUNT], hz: &[f64]) -> Vec<f64> {
    let n = 32_768;
    let mut x = vec![0.0f32; n];
    x[0] = 1.0;
    let h = render(&KIND, rate, &settings(values), &[x], 512).unwrap();
    hz.iter()
        .map(|f| {
            let w = 2.0 * PI * f / rate;
            let (mut re, mut im) = (0.0, 0.0);
            for (k, v) in h[0].iter().enumerate() {
                re += f64::from(*v) * (w * k as f64).cos();
                im -= f64::from(*v) * (w * k as f64).sin();
            }
            20.0 * (re * re + im * im).sqrt().max(1e-12).log10()
        })
        .collect()
}

/// `count` frequencies from 20 Hz to 20 kHz, evenly on a log scale.
fn log_grid(count: usize) -> Vec<f64> {
    (0..count)
        .map(|k| 20.0 * 1000f64.powf(k as f64 / (count - 1) as f64))
        .collect()
}

/// The local minima of `db` below `under`.
fn notches(hz: &[f64], db: &[f64], under: f64) -> Vec<f64> {
    (1..db.len() - 1)
        .filter(|&k| db[k] < db[k - 1] && db[k] <= db[k + 1] && db[k] < under)
        .map(|k| hz[k])
        .collect()
}

#[test]
fn each_stage_makes_one_notch_round_the_sweep_s_frequency() {
    // Still (Mod Rate 0: the LFO at its top, Upper Freq) at 2 kHz, Intensity 95 %, Mix 50 %:
    // one exact notch a stage, gathered round 2 kHz (an odd count has one on it); the DSP's
    // measured response is the formula's.
    let rate = 48_000.0;
    let grid = log_grid(6000);
    for n in 1..=MAX_STAGES {
        let v = with(&[(STAGES, n as f64), (RATE, 0.0), (UPPER, 2000.0)]);
        let db: Vec<f64> = grid
            .iter()
            .map(|f| response_db(rate, &v, 1.0, *f))
            .collect();
        let found = notches(&grid, &db, -30.0);
        assert_eq!(found.len(), n, "{n} stages: {found:?}");
        assert!(
            found.iter().all(|f| (800.0..5000.0).contains(f)),
            "{found:?}"
        );
        if n % 2 == 1 {
            assert!(
                found.iter().any(|f| (f / 2000.0 - 1.0).abs() < 0.03),
                "{n}: {found:?}"
            );
        }
        let check: Vec<f64> = (0..60).map(|k| grid[k * 100]).collect();
        let measured = measured_db(rate, &v, &check);
        for (f, m) in check.iter().zip(&measured) {
            let want = response_db(rate, &v, 1.0, *f);
            if want > -40.0 {
                assert!(
                    (m - want).abs() < 0.01,
                    "{n} stages at {f} Hz: {m} vs {want}"
                );
            }
        }
    }
    // Mix 0 is the input itself; Mix 100 without feedback the allpass chain, flat.
    let x = noise(20_000, 1, 0.5);
    let dry = render(
        &KIND,
        rate,
        &settings(&with(&[(MIX, 0.0)])),
        std::slice::from_ref(&x),
        512,
    )
    .unwrap();
    assert_eq!(dry[0], x);
    let flat = with(&[(MIX, 100.0), (RATE, 0.0)]);
    for f in [50.0, 1000.0, 9000.0, 14_000.0] {
        assert!(response_db(rate, &flat, 1.0, f).abs() < 1e-9);
    }
}

#[test]
fn the_lfo_s_shape_rate_and_the_sweep_s_range() {
    // The LFO: at its top at a start, the bottom half a cycle on; a triangle in straight lines,
    // a sine as a raised cosine.
    assert_eq!(
        (lfo(0.0, false), lfo(0.5, false), lfo(0.25, false)),
        (1.0, 0.0, 0.5)
    );
    assert!((lfo(0.125, true) - (0.5 + 0.5 * (PI / 4.0).cos())).abs() < 1e-12);
    assert!((lfo(0.125, false) - 0.75).abs() < 1e-12);
    // The range: Upper Freq·(1 − Depth) to Upper Freq, never under 20 Hz; the middle of a
    // logarithmic sweep is the geometric mean, of a linear one the arithmetic.
    let rate = 48_000.0;
    let (bottom, top) = sweep_range(rate, &with(&[]));
    assert!(
        (bottom - 4200.0).abs() < 1e-9 && top == 14_000.0,
        "{bottom} {top}"
    );
    assert_eq!(sweep_range(rate, &with(&[(DEPTH, 100.0)])).0, 20.0);
    assert!((sweep_hz(rate, 4000.0, 75.0, false, 0.5) - 2000.0).abs() < 1e-9);
    assert!((sweep_hz(rate, 4000.0, 75.0, true, 0.5) - 2500.0).abs() < 1e-9);
    // Heard: one sharp stage swept from 4 kHz down to 1 kHz and back every 2 s (a triangle,
    // logarithmic), Mix 50 %: a 2 kHz tone (the sweep's middle) vanishes when the notch passes
    // it, at 0.5 s and 1.5 s on the left; the right, 90° ahead, at 0 and 1 s.
    let v = with(&[
        (STAGES, 1.0),
        (INTENSITY, 99.0),
        (UPPER, 4000.0),
        (DEPTH, 75.0),
        (RATE, 0.5),
        (PHASE, 90.0),
        (STEP, 0.0),
        (SHAPE, 0.0),
    ]);
    let n = 2 * 48_000;
    let tone: Vec<f32> = (0..n)
        .map(|i| (0.5 * (2.0 * PI * 2000.0 * i as f64 / rate).sin()) as f32)
        .collect();
    let out = render(&KIND, rate, &settings(&v), &[tone.clone(), tone], 512).unwrap();
    let window = 240;
    let level = |c: usize, at: usize| {
        let s = &out[c][at..at + window];
        (s.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / window as f64).sqrt()
    };
    let quietest = |c: usize, from: usize, to: usize| {
        (from..to)
            .step_by(window / 2)
            .min_by(|a, b| level(c, *a).total_cmp(&level(c, *b)))
            .unwrap()
            + window / 2
    };
    for (c, want) in [(0, 24_000), (0, 72_000), (1, 48_000)] {
        let at = quietest(c, want - 12_000, want + 12_000);
        assert!(
            at.abs_diff(want) < 1500,
            "channel {c}: the notch passes at {at}, {want} expected"
        );
        // Down more than 17 dB from the tone (0.35 RMS) in 5 ms windows as the notch passes.
        assert!(
            level(c, at - window / 2) < 0.05,
            "{}",
            level(c, at - window / 2)
        );
    }
    // And between, the tone is there.
    assert!(level(0, 48_000) > 0.2);
    // The DSP's LFO moves rate/fs a sample.
    let mut fx = Phaser::new(rate);
    for (i, x) in v.iter().enumerate() {
        fx.set(i, *x);
    }
    fx.snap();
    let mut io = vec![vec![0.0f32; 12_000]; CHANNELS];
    fx.process(&mut io, 0, 12_000);
    assert!((fx.phase - 12_000.0 * 0.5 / rate).abs() < 1e-9);
}

#[test]
fn feedback_deepens_the_notches() {
    // Mix 50 %: the notches stay exact at any feedback, and positive feedback raises peaks
    // between them. Mix 40 % (notches 14 dB deep without feedback): deeper with feedback.
    // The DSP's measured response is the formula's with feedback too.
    let rate = 48_000.0;
    let grid = log_grid(4000);
    let still = |mix: f64, fb: f64| {
        with(&[
            (STAGES, 2.0),
            (RATE, 0.0),
            (UPPER, 2000.0),
            (MIX, mix),
            (FEEDBACK, fb),
        ])
    };
    // The response's lowest and highest points: the grid's, each minimum then narrowed down
    // between its neighbours (a notch is far narrower than the grid).
    let range = |v: &[f64; COUNT]| {
        let at = |f: f64| response_db(rate, v, 1.0, f);
        let db: Vec<f64> = grid.iter().map(|f| at(*f)).collect();
        let hi = db.iter().copied().fold(f64::MIN, f64::max);
        let mut lo = f64::MAX;
        for k in 1..grid.len() - 1 {
            if db[k] < db[k - 1] && db[k] <= db[k + 1] {
                let (mut a, mut b) = (grid[k - 1], grid[k + 1]);
                for _ in 0..200 {
                    let (x, y) = (a + (b - a) / 3.0, b - (b - a) / 3.0);
                    if at(x) < at(y) {
                        b = y;
                    } else {
                        a = x;
                    }
                }
                lo = lo.min(at((a + b) / 2.0));
            }
        }
        (lo, hi)
    };
    for fb in [-60.0, 0.0, 60.0] {
        let (lo, _) = range(&still(50.0, fb));
        assert!(lo < -60.0, "feedback {fb}: {lo}");
    }
    let (_, peak0) = range(&still(50.0, 0.0));
    let (_, peak) = range(&still(50.0, 60.0));
    assert!(peak > peak0 + 3.0, "{peak0} → {peak}");
    let (lo0, hi0) = range(&still(40.0, 0.0));
    let (lo, hi) = range(&still(40.0, 60.0));
    assert!(hi - lo > hi0 - lo0 + 6.0, "{} → {}", hi0 - lo0, hi - lo);
    let v = still(40.0, 60.0);
    let check: Vec<f64> = (0..40).map(|k| grid[k * 100]).collect();
    for (f, m) in check.iter().zip(measured_db(rate, &v, &check)) {
        let want = response_db(rate, &v, 1.0, *f);
        assert!((m - want).abs() < 0.01, "{f} Hz: {m} vs {want}");
    }
}

#[test]
fn the_notches_keep_their_width_at_any_rate() {
    // R from Intensity at 44.1 kHz, scaled so -ln(R)·fs (the poles' decay a second, which
    // sets a notch's width in Hz) stays put.
    for percent in [25.0, 95.0, 100.0] {
        let width = |rate: f64| -radius(rate, percent).ln() * rate;
        let w = width(44_100.0);
        for rate in [48_000.0, 96_000.0, 192_000.0] {
            assert!((width(rate) / w - 1.0).abs() < 0.02, "{percent}: {rate}");
        }
    }
    assert!((radius(44_100.0, 95.0) - 0.9491).abs() < 1e-9);
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100;
    let audio = [noise(n, 4, 0.5), noise(n, 5, 0.5)];
    for (name, v) in [
        ("defaults", with(&[])),
        (
            "six stages, fast, full feedback",
            with(&[
                (STAGES, 6.0),
                (RATE, 4.0),
                (FEEDBACK, 90.0),
                (INTENSITY, 100.0),
            ]),
        ),
        (
            "one stage, negative feedback, low ceiling",
            with(&[
                (STAGES, 1.0),
                (FEEDBACK, -60.0),
                (UPPER, 2000.0),
                (MIX, 100.0),
            ]),
        ),
        (
            "two stages, channels apart",
            with(&[(STAGES, 2.0), (PHASE, 90.0), (DEPTH, 100.0), (GAIN, -6.0)]),
        ),
        (
            "a slow sine, logarithmic",
            with(&[(STAGES, 4.0), (RATE, 0.1), (SHAPE, 1.0), (STEP, 0.0)]),
        ),
    ] {
        let s = settings(&v);
        let a = render(&KIND, rate, &s, &audio, 512).unwrap();
        for block in [77, 1, 4096] {
            assert_eq!(a, render(&KIND, rate, &s, &audio, block).unwrap(), "{name}");
        }
    }
}

#[test]
fn the_parameters_show_and_automate() {
    for p in &PARAMS {
        assert!((p.min..=p.max).contains(&p.default), "{}", p.name);
        assert!(
            (p.plain(p.normalized(p.default)) - p.default).abs() < 1e-9,
            "{}",
            p.name
        );
    }
    assert_eq!(PARAMS[SHAPE].display(1.0), "Sine");
    assert_eq!(PARAMS[STEP].display(0.0), "Logarithmic");
    assert_eq!(PARAMS[STAGES].display(3.0), "3");
    assert_eq!(PARAMS[GAIN].display(-4.8), "-4.8 dB");
    assert!(PARAMS.iter().all(|p| p.automatable));
}

/// The cost per second of stereo audio in release, and the worst 64-sample block (`cargo test
/// --release -p daw-core phaser_cost -- --ignored --nocapture`).
#[test]
#[ignore]
fn phaser_cost() {
    let rate = 48_000.0;
    let seconds = 20;
    let n = 48_000 * seconds;
    let audio = [noise(n, 6, 0.5), noise(n, 7, 0.5)];
    for (name, v) in [
        ("defaults", with(&[])),
        (
            "six stages, fast, full feedback",
            with(&[(STAGES, 6.0), (RATE, 4.0), (FEEDBACK, 90.0)]),
        ),
        ("a slow sine", with(&[(STAGES, 4.0), (SHAPE, 1.0)])),
    ] {
        let mut fx = Phaser::new(rate);
        for (i, x) in v.iter().enumerate() {
            fx.set(i, *x);
        }
        fx.snap();
        let block = 64;
        let mut io: Vec<Vec<f32>> = vec![vec![0.0; block]; CHANNELS];
        let mut worst = 0.0f64;
        let mut times = Vec::with_capacity(n / block);
        let start = std::time::Instant::now();
        for from in (0..n).step_by(block) {
            for (c, buf) in io.iter_mut().enumerate() {
                buf.copy_from_slice(&audio[c][from..from + block]);
            }
            let t = std::time::Instant::now();
            fx.process(&mut io, 0, block);
            let e = t.elapsed().as_secs_f64();
            worst = worst.max(e);
            times.push(e);
        }
        let total = start.elapsed().as_secs_f64();
        times.sort_by(f64::total_cmp);
        println!(
            "Phaser, {name}: {:.2} ms per second of stereo audio ({:.3} % of a core), the worst \
             64-sample block {:.4} ms (99.9th percentile {:.4}) of {:.3}",
            total * 1000.0 / seconds as f64,
            total * 100.0 / seconds as f64,
            worst * 1000.0,
            times[times.len() * 999 / 1000] * 1000.0,
            64.0 / rate * 1000.0
        );
    }
}
