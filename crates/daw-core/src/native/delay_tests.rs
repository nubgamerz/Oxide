//! The Delay's tests (`delay.rs`).

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

/// The widest setting: the left copy 500 ms early and inverted, the right 500 ms late, half
/// each.
fn widest() -> [f64; COUNT] {
    with(&[
        (LEFT_DELAY, -500.0),
        (LEFT_INVERT, 1.0),
        (LEFT_MIX, 50.0),
        (RIGHT_DELAY, 500.0),
        (RIGHT_MIX, 50.0),
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

/// `n` samples with a 1 at `at`.
fn impulse(n: usize, at: usize) -> Vec<f32> {
    let mut v = vec![0.0; n];
    v[at] = 1.0;
    v
}

/// The non-zero samples of `x`: (position, value).
fn taps_of(x: &[f32]) -> Vec<(usize, f32)> {
    x.iter()
        .enumerate()
        .filter(|(_, v)| **v != 0.0)
        .map(|(i, v)| (i, *v))
        .collect()
}

#[test]
fn each_channel_delays_by_its_own_whole_samples() {
    // The defaults: left 120 ms at 75 %, right 180 ms at 65 %. At 44.1 kHz 5292 and 7938
    // samples, at 48 kHz 5760 and 8640; dry 1 - m and wet m, both in f32. No latency.
    for (rate, left, right) in [(44_100.0, 5292, 7938), (48_000.0, 5760, 8640)] {
        assert_eq!(latency(rate, &PARAMS.map(|p| p.default)), 0);
        let n = 20_000;
        let audio = [impulse(n, 100), impulse(n, 300)];
        let out = render(&KIND, rate, &settings(&with(&[])), &audio, 512).unwrap();
        let (m_l, m_r) = (0.75f32, 0.65f32);
        assert_eq!(
            taps_of(&out[0]),
            [(100, 1.0 - m_l), (100 + left, m_l)],
            "{rate}"
        );
        assert_eq!(
            taps_of(&out[1]),
            [(300, 1.0 - m_r), (300 + right, m_r)],
            "{rate}"
        );
    }
    // Rounding: 126.7 ms is 5587.47 samples at 44.1 kHz (5587) and
    // 6081.6 at 48 kHz (6082); 0.5 rounds up.
    assert_eq!(delay_samples(44_100.0, 126.7), 5587);
    assert_eq!(delay_samples(48_000.0, 126.7), 6082);
    // Negative times truncate toward zero after the half: -10.3 samples is -9, -10.5 is -10.
    assert_eq!(delay_samples(1000.0, -10.3), -9);
    assert_eq!(delay_samples(1000.0, -10.5), -10);
}

#[test]
fn negative_times_come_early_by_exactly_their_amount_against_the_latency() {
    // Left -200 ms, right +200 ms, 60 % each. At 44.1 kHz -8820 + 0.5 truncates toward zero
    // to -8819: the latency, and the left copy that much before the dry sound.
    let rate = 44_100.0;
    let v = with(&[
        (LEFT_DELAY, -200.0),
        (LEFT_MIX, 60.0),
        (RIGHT_DELAY, 200.0),
        (RIGHT_MIX, 60.0),
    ]);
    assert_eq!(latency(rate, &v), 8819);
    let n = 40_000;
    let at = 20_000;
    let audio = [impulse(n, at), impulse(n, at)];
    // `render` drains the reported latency, as the graph compensates it.
    let out = render(&KIND, rate, &settings(&v), &audio, 512).unwrap();
    let m = 0.6f32;
    assert_eq!(taps_of(&out[0]), [(at - 8819, m), (at, 1.0 - m)]);
    assert_eq!(taps_of(&out[1]), [(at, 1.0 - m), (at + 8820, m)]);
    // The widest: -500 ms rounds toward zero to -22049, inverted; +500 to +22050.
    let v = widest();
    assert_eq!(latency(rate, &v), 22_049);
    let n = 60_000;
    let at = 25_000;
    let audio = [impulse(n, at), impulse(n, at)];
    let out = render(&KIND, rate, &settings(&v), &audio, 333).unwrap();
    assert_eq!(taps_of(&out[0]), [(at - 22_049, -0.5), (at, 0.5)]);
    assert_eq!(taps_of(&out[1]), [(at, 0.5), (at + 22_050, 0.5)]);
    // Both negative: the most negative sets the latency; the other is early by its own amount.
    let v = with(&[(LEFT_DELAY, -100.0), (RIGHT_DELAY, -10.0)]);
    let rate = 48_000.0;
    assert_eq!(latency(rate, &v), 4799);
    let audio = [impulse(n, at), impulse(n, at)];
    let out = render(&KIND, rate, &settings(&v), &audio, 512).unwrap();
    assert_eq!(out[0][at - 4799], 0.75);
    assert_eq!(out[1][at - 479], 0.65);
    assert_eq!((out[0][at], out[1][at]), (1.0 - 0.75f32, 1.0 - 0.65f32));
    // The instance reports the same latency the kind does.
    let mut fx = Delay::new(rate);
    fx.set(LEFT_DELAY, -100.0);
    assert_eq!(fx.latency(), 4799);
}

#[test]
fn mix_and_inversion_are_exact() {
    let rate = 48_000.0;
    let n = 30_000;
    let audio = [noise(n, 1, 0.5), noise(n, 2, 0.5)];
    let run = |v: [f64; COUNT]| render(&KIND, rate, &settings(&v), &audio, 512).unwrap();
    // Mix 0: the input itself; Mix 100: the delayed copy alone.
    let dry = run(with(&[(LEFT_MIX, 0.0), (RIGHT_MIX, 0.0)]));
    assert_eq!(dry, audio.to_vec());
    let wet = run(with(&[
        (LEFT_MIX, 100.0),
        (RIGHT_MIX, 100.0),
        (RIGHT_INVERT, 1.0),
    ]));
    for i in 0..n {
        let l = if i >= 5760 { audio[0][i - 5760] } else { 0.0 };
        let r = if i >= 8640 { -audio[1][i - 8640] } else { 0.0 };
        assert_eq!((wet[0][i], wet[1][i]), (l, r), "sample {i}");
    }
    // Any mix: wet·m (negated when inverted) plus dry·(1 - m), in f32, exactly.
    for (mix, invert) in [(37.0, 0.0), (81.5, 1.0)] {
        let out = run(with(&[(LEFT_MIX, mix), (LEFT_INVERT, invert)]));
        let m = mix as f32 / 100.0;
        let g = if invert > 0.5 { -m } else { m };
        for i in 0..n {
            let w = if i >= 5760 { audio[0][i - 5760] } else { 0.0 };
            assert_eq!(out[0][i], g * w + (1.0 - m) * audio[0][i], "sample {i}");
        }
    }
}

#[test]
fn a_change_crossfades_forwards_over_100_ms() {
    // Left wet only at 10 ms; then 20 ms. The output moves from the 10 ms copy to the 20 ms one
    // in a straight line over 4800 samples, the weight on the new one rising 1/4800 ... 1: at
    // once afterwards the new copy alone, with no snap back.
    let rate = 48_000.0;
    let n = 20_000;
    let x = noise(n, 3, 0.5);
    let mut fx = Delay::new(rate);
    for (i, v) in with(&[(LEFT_MIX, 100.0), (LEFT_DELAY, 10.0)])
        .iter()
        .enumerate()
    {
        fx.set(i, *v);
    }
    fx.snap();
    let mut io = vec![x.clone(), x.clone()];
    let change = 6000;
    fx.process(&mut io, 0, change);
    fx.set(LEFT_DELAY, 20.0);
    fx.process(&mut io, change, n);
    let back = |i: usize, d: usize| if i >= d { x[i - d] } else { 0.0 };
    for (i, y) in io[0].iter().enumerate() {
        let want = if i < change {
            back(i, 480)
        } else if i < change + 4800 {
            let w = (i - change + 1) as f32 / 4800.0;
            (1.0 - w) * back(i, 480) + w * back(i, 960)
        } else {
            back(i, 960)
        };
        assert!((y - want).abs() < 1e-6, "sample {i}");
    }
    // The first faded sample is nearly the old copy, not the new (the reference's runs backwards).
    let old = back(change, 480);
    assert!((io[0][change] - old).abs() < 1e-3);
    // Mix by hand glides there in the same 100 ms, in a straight line.
    let mut fx = Delay::new(rate);
    fx.set(LEFT_DELAY, 0.0);
    fx.snap();
    let mut io = vec![vec![1.0; 6000]; CHANNELS];
    fx.set(LEFT_MIX, 25.0);
    fx.process(&mut io, 0, 6000);
    // At 0 ms dry and wet are the same sample: 1 throughout, whatever the mix.
    assert!(io[0].iter().all(|v| (v - 1.0).abs() < 1e-6));
    let mut fx = Delay::new(rate);
    fx.set(LEFT_INVERT, 1.0);
    fx.set(LEFT_DELAY, 0.0);
    fx.snap();
    let mut io = vec![vec![1.0; 6000]; CHANNELS];
    fx.set(LEFT_MIX, 25.0);
    fx.process(&mut io, 0, 6000);
    // Inverted at 0 ms: 1 - 2m, m gliding 0.75 → 0.25 over 4800 samples.
    for (i, y) in io[0].iter().enumerate() {
        let m = if i < 4800 {
            0.75 - 0.5 * i as f32 / 4800.0
        } else {
            0.25
        };
        assert!((y - (1.0 - 2.0 * m)).abs() < 5e-4, "sample {i}: {y}");
    }
    // A transport start empties the lines and lands on the settings.
    let mut io = vec![vec![0.0; 100]; CHANNELS];
    fx.start();
    fx.process(&mut io, 0, 100);
    assert!(io.iter().all(|c| c.iter().all(|v| *v == 0.0)));
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100;
    let audio = [noise(n, 4, 0.5), noise(n, 5, 0.5)];
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        (
            "uneven, one side inverted",
            with(&[
                (LEFT_DELAY, -37.3),
                (RIGHT_DELAY, 251.7),
                (RIGHT_INVERT, 1.0),
            ]),
        ),
        ("widest", widest()),
        (
            "short and quiet",
            with(&[
                (LEFT_DELAY, 12.0),
                (LEFT_MIX, 40.0),
                (RIGHT_DELAY, 17.0),
                (RIGHT_MIX, 35.0),
            ]),
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
fn the_parameters_show_and_automate_as_expected() {
    for p in &PARAMS {
        assert!((p.min..=p.max).contains(&p.default), "{}", p.name);
    }
    assert_eq!(PARAMS[LEFT_DELAY].display(-482.0), "-482.0 ms");
    assert_eq!(PARAMS[UNITS].display(1.0), "Beats");
    assert_eq!(PARAMS[LEFT_INVERT].display(1.0), "On");
    // Delay times move the latency, so a lane can't play them; mix and invert can.
    assert!(!PARAMS[LEFT_DELAY].automatable && !PARAMS[RIGHT_DELAY].automatable);
    assert!(PARAMS[LEFT_MIX].automatable && PARAMS[RIGHT_INVERT].automatable);
}

/// The cost per second of stereo audio in release, and the worst 64-sample block (`cargo test
/// --release -p daw-core delay_cost -- --ignored --nocapture`).
#[test]
#[ignore]
fn delay_cost() {
    let rate = 48_000.0;
    let seconds = 30;
    let n = 48_000 * seconds;
    let audio = [noise(n, 6, 0.5), noise(n, 7, 0.5)];
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        ("widest", widest()),
    ] {
        let mut fx = Delay::new(rate);
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
        // The first blocks touch the lines' fresh pages: the worst after the first second too.
        let settled = times[750..].iter().copied().fold(0.0, f64::max);
        times.sort_by(f64::total_cmp);
        println!(
            "Delay, {name}: {:.2} ms per second of stereo audio ({:.3} % of a core), the worst \
             64-sample block {:.4} ms (after the first second {:.4}; 99.9th percentile {:.4}) \
             of {:.3}",
            total * 1000.0 / seconds as f64,
            total * 100.0 / seconds as f64,
            worst * 1000.0,
            settled * 1000.0,
            times[times.len() * 999 / 1000] * 1000.0,
            64.0 / rate * 1000.0
        );
    }
}
