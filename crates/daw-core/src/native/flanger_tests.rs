//! The Flanger's tests (`flanger.rs`).

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

/// Full feedback on a short, fast triangle sweep: the loop at its loudest.
fn full_feedback() -> [f64; COUNT] {
    with(&[
        (INITIAL, 0.5),
        (FINAL, 1.5),
        (RATE, 8.0),
        (MIX, 50.0),
        (FEEDBACK, 100.0),
        (SINUSOIDAL, 0.0),
    ])
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

fn sine(n: usize, hz: f64, rate: f64, peak: f64) -> Vec<f32> {
    (0..n)
        .map(|i| (peak * (2.0 * PI * hz * i as f64 / rate).sin()) as f32)
        .collect()
}

fn rms(v: &[f32]) -> f64 {
    (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

/// An instance at `rate` with plain `values`, as `Native::new` makes one.
fn flanger(rate: f64, values: &[f64; COUNT]) -> Flanger {
    let mut fx = Flanger::new(rate);
    for (i, v) in values.iter().enumerate() {
        fx.set(i, *v);
    }
    fx.snap();
    fx
}

#[test]
fn the_sweep_s_shape_rate_and_phase_offset() {
    // Sine (Sinusoidal on): P = (sin φ + 1)/2·(final − initial) + initial from φ = -π/2, a
    // period of fs/rate samples; the right leads by Stereo Phasing. Triangle (off): straight
    // lines through the same turning points. Followed sample by sample in the DSP's own state.
    let rate = 48_000.0;
    for sine_mode in [1.0, 0.0] {
        let v = with(&[
            (INITIAL, 1.0),
            (FINAL, 5.0),
            (RATE, 2.0),
            (PHASING, 90.0),
            (SINUSOIDAL, sine_mode),
        ]);
        let mut fx = flanger(rate, &v);
        let (lo, hi) = (48.0, 240.0);
        let mut io = vec![vec![0.0f32; 1]; CHANNELS];
        let period = 24_000;
        let mut left = Vec::new();
        let mut right = Vec::new();
        for n in 0..2 * period {
            left.push(position(rate, &v, fx.phi, 0));
            right.push(position(rate, &v, fx.phi, 1));
            fx.process(&mut io, 0, 1);
            let phi = -PI / 2.0 + 2.0 * PI * 2.0 * (n as f64) / rate;
            let want = if sine_mode > 0.5 {
                (phi.sin() + 1.0) / 2.0 * (hi - lo) + lo
            } else {
                // The triangle: up from lo over half a period, then down.
                let u = (n % period) as f64 / period as f64;
                let up = if u < 0.5 { 2.0 * u } else { 2.0 - 2.0 * u };
                lo + up * (hi - lo)
            };
            assert!(
                (left[n] - want).abs() < 1e-6,
                "{sine_mode}: {n}: {}",
                left[n]
            );
        }
        assert_eq!(left[0], lo);
        assert!((left[period / 2] - hi).abs() < 1e-6);
        // The rate: the same place a period on.
        assert!((left[period] - left[0]).abs() < 1e-6);
        // The right a quarter period ahead (90°).
        for n in 0..period {
            assert!((right[n] - left[n + period / 4]).abs() < 1e-6, "{n}");
        }
    }
}

#[test]
fn the_latency_is_5082_samples_at_any_rate() {
    // The dry sound (Mix 0) comes back exactly the latency late, which `render` takes off;
    // the kind reports it whatever the settings.
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        assert_eq!((KIND.latency.unwrap())(rate, &with(&[])), 5082);
        let n = 12_000;
        let x = noise(n, 3, 0.5);
        let out = render(
            &KIND,
            rate,
            &settings(&with(&[(MIX, 0.0)])),
            std::slice::from_ref(&x),
            256,
        )
        .unwrap();
        assert_eq!(out[0], x, "{rate}");
    }
}

#[test]
fn the_taps_meet_at_the_centre_and_null_through_zero() {
    // Initial = Final = 0: both taps on the centre. At Mix 50 % they cancel it (a 1 kHz tone
    // more than 70 dB down: what's left is the sinc's ripple); at 100 % the centre comes back
    // negated; Inverted at 100 % the taps cancel each other instead.
    let rate = 48_000.0;
    let n = 24_000;
    let x = sine(n, 1000.0, rate, 0.5);
    let run = |v: [f64; COUNT]| {
        render(&KIND, rate, &settings(&v), std::slice::from_ref(&x), 512).unwrap()
    };
    let zero = [(INITIAL, 0.0), (FINAL, 0.0), (FEEDBACK, 0.0)];
    let at = |mix: f64, inverted: f64| {
        let mut v = with(&zero);
        v[MIX] = mix;
        v[INVERTED] = inverted;
        run(v)
    };
    let half = at(50.0, 0.0);
    let db = 20.0 * (rms(&half[0][1000..]) / rms(&x[1000..])).log10();
    assert!(db < -70.0, "{db} dB");
    let full = at(100.0, 0.0);
    for i in 1000..n {
        assert!((full[0][i] + x[i]).abs() < 1e-3, "{i}");
    }
    assert!(rms(&at(100.0, 1.0)[0][1000..]) < 1e-6);
    // Swept the tone isn't nulled: the sweep passes through zero and out again.
    let swept = run(with(&[(FEEDBACK, 0.0), (MIX, 50.0)]));
    assert!(rms(&swept[0][1000..]) > 0.1 * rms(&x));
}

#[test]
fn one_tap_reads_p_behind_the_centre() {
    // Special Effects on, Mix 100 %, a still sweep 2 ms (96 samples at 48 kHz) from the
    // centre: an impulse comes back 96 samples late, nothing ahead of it, inverted when
    // Inverted is on. Two taps: one 96 early (the ahead tap, negated) and one 96 late.
    let rate = 48_000.0;
    let n = 8000;
    let at = 3000;
    let mut x = vec![0.0f32; n];
    x[at] = 0.5;
    let still = [(INITIAL, 2.0), (FINAL, 2.0), (FEEDBACK, 0.0), (MIX, 100.0)];
    let peak_at = |y: &[f32]| {
        y.iter()
            .enumerate()
            .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
            .map(|(i, v)| (i, *v))
            .unwrap()
    };
    for (inverted, sign) in [(0.0, 1.0f32), (1.0, -1.0)] {
        let mut v = with(&still);
        v[SPECIAL] = 1.0;
        v[INVERTED] = inverted;
        let y = render(&KIND, rate, &settings(&v), std::slice::from_ref(&x), 512).unwrap();
        let (i, p) = peak_at(&y[0]);
        assert_eq!(i, at + 96);
        assert!((p * sign - 0.5 * 0.957).abs() < 0.01, "{p}");
        assert!(y[0][..at + 80].iter().all(|s| s.abs() < 1e-3));
    }
    let y = render(
        &KIND,
        rate,
        &settings(&with(&still)),
        std::slice::from_ref(&x),
        512,
    )
    .unwrap();
    let (early, p) = peak_at(&y[0][..at]);
    assert_eq!(early, at - 96);
    assert!(p < -0.2, "{p}");
    let (late, q) = peak_at(&y[0][at + 1..]);
    assert_eq!(late + at + 1, at + 96);
    assert!(q < -0.2, "{q}");
}

#[test]
fn feedback_recirculates_through_the_behind_tap() {
    // One tap, Mix 100 %, still at 2 ms, Feedback 50 %: echoes every 96 samples, each half the
    // last (the energy in each a quarter), positive; Inverted alternates their signs (the tap
    // and the feedback both negated).
    let rate = 48_000.0;
    let n = 4000;
    let at = 500;
    let mut x = vec![0.0f32; n];
    x[at] = 0.5;
    for (inverted, signs) in [(0.0, [1.0, 1.0, 1.0]), (1.0, [-1.0, 1.0, -1.0])] {
        let v = with(&[
            (INITIAL, 2.0),
            (FINAL, 2.0),
            (MIX, 100.0),
            (FEEDBACK, 50.0),
            (SPECIAL, 1.0),
            (INVERTED, inverted),
        ]);
        let y = render(&KIND, rate, &settings(&v), std::slice::from_ref(&x), 128).unwrap();
        let window = |k: usize| &y[0][at + 96 * k - 40..at + 96 * k + 40];
        let energy = |k: usize| window(k).iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
        for k in 1..=3 {
            let w = window(k);
            let big = w
                .iter()
                .copied()
                .fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a });
            assert_eq!(big.signum(), signs[k - 1] as f32, "{inverted}: echo {k}");
        }
        for k in 1..3 {
            let ratio = (energy(k + 1) / energy(k)).sqrt();
            assert!((ratio - 0.5).abs() < 0.02, "{inverted}: {k}: {ratio}");
        }
    }
    // Two taps at Mix 100 %: y = -(T_behind + T_ahead)/2, and the centre rewritten with the
    // behind tap subtracted, so the recirculation comes back with its sign flipped: -T/2 a tap
    // late, then +fb·T/2 two taps late. (At Mix 0 the output is the centre before it is
    // rewritten: the input, untouched.)
    let v = with(&[(INITIAL, 2.0), (FINAL, 2.0), (MIX, 100.0), (FEEDBACK, 50.0)]);
    let y = render(&KIND, rate, &settings(&v), std::slice::from_ref(&x), 128).unwrap();
    let big = |k: usize| {
        y[0][at + 96 * k - 40..at + 96 * k + 40]
            .iter()
            .copied()
            .fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a })
    };
    assert!((big(1) + 0.5 * 0.5 * 0.957).abs() < 0.01, "{}", big(1));
    assert!((big(2) - 0.25 * 0.5 * 0.957).abs() < 0.01, "{}", big(2));
}

#[test]
fn the_output_and_the_centre_clip_at_full_scale() {
    let rate = 48_000.0;
    let n = 12_000;
    let loud = vec![3.0f32; n];
    let y = render(&KIND, rate, &settings(&with(&[(MIX, 0.0)])), &[loud], 512).unwrap();
    assert!(y[0].iter().all(|s| s.abs() <= 1.0));
    assert_eq!(y[0][n - 1], 1.0);
    // Infinite feedback stays within ±1.
    let v = full_feedback();
    let x = noise(n, 5, 0.9);
    let y = render(&KIND, rate, &settings(&v), &[x], 512).unwrap();
    assert!(y.iter().all(|c| c.iter().all(|s| s.abs() <= 1.0)));
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100;
    let audio = [noise(n, 4, 0.5), noise(n, 5, 0.5)];
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        (
            "slow and wide, inverted",
            with(&[
                (INITIAL, 2.0),
                (FINAL, 12.0),
                (RATE, 0.1),
                (PHASING, 180.0),
                (INVERTED, 1.0),
            ]),
        ),
        (
            "special effects",
            with(&[(SPECIAL, 1.0), (FEEDBACK, 80.0), (RATE, 2.5)]),
        ),
        (
            "fast triangle",
            with(&[(SINUSOIDAL, 0.0), (RATE, 12.0), (PHASING, 90.0)]),
        ),
        ("full feedback", full_feedback()),
    ] {
        let s = settings(&v);
        let a = render(&KIND, rate, &s, &audio, 512).unwrap();
        for block in [77, 1, 4096] {
            assert_eq!(a, render(&KIND, rate, &s, &audio, block).unwrap(), "{name}");
        }
    }
}

#[test]
fn the_parameters_show_and_automate_as_expected() {
    for p in &PARAMS {
        assert!((p.min..=p.max).contains(&p.default), "{}", p.name);
    }
    // The default sweeps a sine.
    assert_eq!(PARAMS[SINUSOIDAL].default, 1.0);
    // The rate from the beats: 8.621 beats, 0.232 Hz.
    assert_eq!(PARAMS[RATE].display(PARAMS[RATE].default), "0.232 Hz");
    assert_eq!(PARAMS[PHASING].display(28.0), "28 °");
    assert_eq!(PARAMS[SINUSOIDAL].display(1.0), "On");
    assert!(PARAMS.iter().all(|p| p.automatable));
}

/// The cost per second of stereo audio in release, and the worst 64-sample block (`cargo test
/// --release -p daw-core flanger_cost -- --ignored --nocapture`).
#[test]
#[ignore]
fn flanger_cost() {
    let rate = 48_000.0;
    let seconds = 20;
    let n = 48_000 * seconds;
    let audio = [noise(n, 6, 0.5), noise(n, 7, 0.5)];
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        ("full feedback", full_feedback()),
    ] {
        let mut fx = flanger(rate, &v);
        fx.start();
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
            "Flanger, {name}: {:.2} ms per second of stereo audio ({:.3} % of a core), the worst \
             64-sample block {:.4} ms (99.9th percentile {:.4}) of {:.3}",
            total * 1000.0 / seconds as f64,
            total * 100.0 / seconds as f64,
            worst * 1000.0,
            times[times.len() * 999 / 1000] * 1000.0,
            64.0 / rate * 1000.0
        );
    }
}
