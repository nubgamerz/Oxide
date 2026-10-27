//! The Echo's tests (`echo.rs`).

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

/// The defaults, then `pairs`.
fn with(pairs: &[(usize, f64)]) -> [f64; COUNT] {
    let mut v = PARAMS.map(|p| p.default);
    for &(i, x) in pairs {
        v[i] = x;
    }
    v
}

/// A loop held near full scale: 90 % feedback each side, the full echo level, three bands
/// boosted by 6 dB.
fn hot_loop() -> [f64; COUNT] {
    with(&[
        (LEFT_DELAY, 150.0),
        (RIGHT_DELAY, 220.0),
        (LEFT_FEEDBACK, 90.0),
        (RIGHT_FEEDBACK, 90.0),
        (LEFT_LEVEL, 100.0),
        (RIGHT_LEVEL, 100.0),
        (FIRST_BAND + 2, 6.0),
        (FIRST_BAND + 3, 6.0),
        (FIRST_BAND + 4, 6.0),
    ])
}

/// Short repeats bouncing between the sides.
fn bouncing() -> [f64; COUNT] {
    with(&[
        (LEFT_DELAY, 80.0),
        (RIGHT_DELAY, 80.0),
        (LEFT_FEEDBACK, 70.0),
        (RIGHT_FEEDBACK, 70.0),
        (BOUNCE, 1.0),
    ])
}

/// The defaults with every EQ band at 0 dB, then `pairs`.
fn flat(pairs: &[(usize, f64)]) -> [f64; COUNT] {
    let mut v = PARAMS.map(|p| p.default);
    for b in 0..BANDS {
        v[FIRST_BAND + b] = 0.0;
    }
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

fn impulse(n: usize, at: usize, height: f32) -> Vec<f32> {
    let mut v = vec![0.0; n];
    v[at] = height;
    v
}

/// The EQ's impulse response (`n` samples) in f64, from its definition: m_k the input through
/// boxcars of 2, 4 … 2^k in turn, the bands their differences, weighted by `g` (band 1 first).
fn eq_reference(g: &[f64; BANDS], n: usize) -> Vec<f64> {
    let mut m = vec![vec![0.0f64; n]; 8];
    m[0][0] = 1.0;
    for k in 1..8 {
        let len = 1usize << k;
        for i in 0..n {
            let from = i.saturating_sub(len - 1);
            m[k][i] = m[k - 1][from..=i].iter().sum::<f64>() / len as f64;
        }
    }
    (0..n)
        .map(|i| {
            let mut y = g[0] * m[7][i];
            for band in 1..BANDS {
                y += g[band] * (m[BANDS - 1 - band][i] - m[BANDS - band][i]);
            }
            y
        })
        .collect()
}

fn convolve(a: &[f64], b: &[f64], n: usize) -> Vec<f64> {
    (0..n)
        .map(|i| {
            (0..=i)
                .map(|k| a.get(k).unwrap_or(&0.0) * b.get(i - k).unwrap_or(&0.0))
                .sum()
        })
        .collect()
}

#[test]
fn echoes_come_at_the_delay_and_decay_by_feedback() {
    // The study's first render: 100 / 150 ms at 44.1 kHz (4410 and 6615 samples), Feedback 50
    // %, Echo Level 100 %, every band at 0 dB (which passes the echo within f32 rounding): the
    // dry impulse, then 0.5^k of it at k·d.
    let rate = 44_100.0;
    let v = flat(&[
        (LEFT_DELAY, 100.0),
        (RIGHT_DELAY, 150.0),
        (LEFT_FEEDBACK, 50.0),
        (RIGHT_FEEDBACK, 50.0),
        (LEFT_LEVEL, 100.0),
        (RIGHT_LEVEL, 100.0),
    ]);
    let n = 40_000;
    let audio = [impulse(n, 0, 0.5), impulse(n, 0, 0.5)];
    let out = render(&KIND, rate, &settings(&v), &audio, 512).unwrap();
    for (c, d) in [(0, 4410), (1, 6615)] {
        for (i, y) in out[c].iter().enumerate() {
            let want = if i % d == 0 {
                0.5 * 0.5f32.powi((i / d) as i32)
            } else {
                0.0
            };
            assert!(
                (y - want).abs() < 1e-6,
                "channel {c} sample {i}: {y} vs {want}"
            );
        }
    }
    // Echo Level scales every repeat, Feedback sets the first one too: e·f^k.
    let v = flat(&[
        (LEFT_DELAY, 100.0),
        (LEFT_FEEDBACK, 25.0),
        (LEFT_LEVEL, 40.0),
    ]);
    let out = render(&KIND, rate, &settings(&v), &audio, 512).unwrap();
    for k in 1..4 {
        let want = 0.5 * 0.4 * 0.25f32.powi(k);
        let got = out[0][4410 * k as usize];
        assert!((got - want).abs() < 1e-6, "echo {k}: {got} vs {want}");
    }
    // Whole samples, truncated: 100.01 ms at 44.1 kHz is 4410.44, so 4410.
    assert_eq!(delay_samples(rate, 100.01), 4410);
    assert_eq!(delay_samples(48_000.0, 348.8), 16_742);
}

#[test]
fn the_eq_per_pass_is_the_bands_of_the_cascaded_averages() {
    // The EQ's impulse response against its definition in f64, one band at +15 dB at a time
    // and at the defaults; the editor's curve against the response's spectrum.
    let n = 600;
    let mut sets: Vec<[f64; BANDS]> = (0..BANDS)
        .map(|b| {
            let mut db = [0.0; BANDS];
            db[b] = 15.0;
            db
        })
        .collect();
    sets.push(std::array::from_fn(|b| PARAMS[FIRST_BAND + b].default));
    for db in sets {
        let g32: [f32; BANDS] = db.map(|d| gain(d as f32));
        let g64: [f64; BANDS] = g32.map(f64::from);
        let want = eq_reference(&g64, n);
        let mut eq = Eq::default();
        let got: Vec<f32> = (0..n)
            .map(|i| eq.tick(if i == 0 { 1.0 } else { 0.0 }, &g32))
            .collect();
        for i in 0..n {
            assert!(
                (f64::from(got[i]) - want[i]).abs() < 1e-6,
                "{db:?} sample {i}: {} vs {}",
                got[i],
                want[i]
            );
        }
        // The curve: the response's DFT at a few frequencies (the response has died by 600).
        let mut values = [0.0; COUNT];
        values[FIRST_BAND..FIRST_BAND + BANDS].copy_from_slice(&db.map(|d| f64::from(d as f32)));
        for hz in [30.0, 200.0, 1000.0, 5000.0, 15_000.0] {
            let w = 2.0 * std::f64::consts::PI * hz / 48_000.0;
            let (re, im) = want
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(re, im), (i, h)| {
                    (re + h * (w * i as f64).cos(), im - h * (w * i as f64).sin())
                });
            let measured = 10.0 * (re * re + im * im).log10();
            let curve = response(48_000.0, &values, hz);
            assert!(
                (measured - curve).abs() < 0.01,
                "{db:?} at {hz}: {measured} vs {curve}"
            );
        }
    }
    // At 0 dB everywhere the bands add back to the input (within f32 rounding).
    let mut eq = Eq::default();
    let x = noise(2000, 9, 0.5);
    for s in &x {
        assert!((eq.tick(*s, &[1.0; BANDS]) - s).abs() < 1e-6);
    }
    // Inside the loop: each pass goes through it again. Band 1 at +15 dB, Feedback 50 %, Level
    // 100 %, 100 ms: the first echo is 0.5·h, the second 0.25·(h∗h).
    let rate = 48_000.0;
    let mut v = flat(&[
        (LEFT_DELAY, 100.0),
        (LEFT_FEEDBACK, 50.0),
        (LEFT_LEVEL, 100.0),
    ]);
    v[FIRST_BAND] = 15.0;
    let len = 20_000;
    let out = render(
        &KIND,
        rate,
        &settings(&v),
        &[impulse(len, 0, 1.0), vec![0.0; len]],
        512,
    )
    .unwrap();
    let mut g = [1.0f64; BANDS];
    g[0] = f64::from(gain(15.0));
    let h = eq_reference(&g, 4800);
    let twice = convolve(&h, &h, 4800);
    for i in 0..4800 {
        let first = f64::from(out[0][4800 + i]);
        let second = f64::from(out[0][9600 + i]);
        assert!((first - 0.5 * h[i]).abs() < 1e-6, "first echo at {i}");
        assert!(
            (second - 0.25 * twice[i]).abs() < 1e-5,
            "second echo at {i}"
        );
    }
}

#[test]
fn bounce_alternates_sides() {
    // An impulse on the left alone, 100 / 100 ms, Bounce on: the first echo on the right, the
    // next on the left, and so on; never two in a row on one side.
    let rate = 48_000.0;
    let v = flat(&[
        (LEFT_DELAY, 100.0),
        (RIGHT_DELAY, 100.0),
        (LEFT_FEEDBACK, 50.0),
        (RIGHT_FEEDBACK, 50.0),
        (LEFT_LEVEL, 100.0),
        (RIGHT_LEVEL, 100.0),
        (BOUNCE, 1.0),
    ]);
    let n = 48_000;
    let out = render(
        &KIND,
        rate,
        &settings(&v),
        &[impulse(n, 0, 1.0), vec![0.0; n]],
        256,
    )
    .unwrap();
    for k in 1..9usize {
        let at = 4800 * k;
        let level = 0.5f32.powi(k as i32);
        let (here, there) = if k % 2 == 1 { (1, 0) } else { (0, 1) };
        assert!((out[here][at] - level).abs() < 1e-6, "echo {k}");
        assert!(out[there][at].abs() < 1e-6, "echo {k} on the other side");
    }
    // Off, the right stays silent.
    let off = flat(&[(LEFT_DELAY, 100.0), (RIGHT_DELAY, 100.0)]);
    let out = render(
        &KIND,
        rate,
        &settings(&off),
        &[impulse(n, 0, 1.0), vec![0.0; n]],
        256,
    )
    .unwrap();
    assert!(out[1].iter().all(|v| *v == 0.0));
}

#[test]
fn the_loop_is_clipped_at_full_scale() {
    // Feedback and Echo Level 100 %, the EQ flat, a steady 0.8: the line takes 0.8, then
    // clip(0.8 + 0.8) = 1 and stays there, so the output settles at 0.8 + 1 rather than
    // growing by 0.8 every 10 ms.
    let rate = 48_000.0;
    let v = flat(&[
        (LEFT_DELAY, 10.0),
        (LEFT_FEEDBACK, 100.0),
        (LEFT_LEVEL, 100.0),
    ]);
    let n = 9600;
    let out = render(
        &KIND,
        rate,
        &settings(&v),
        &[vec![0.8; n], vec![0.0; n]],
        512,
    )
    .unwrap();
    // The first echo (480 samples on, past the EQ's 254-sample settling) is 0.8; every later
    // one is the clipped 1.
    assert!((out[0][700] - 1.6).abs() < 1e-5, "{}", out[0][700]);
    for (i, y) in out[0].iter().enumerate().skip(1200) {
        assert!((y - 1.8).abs() < 1e-5, "sample {i}: {y}");
    }
    // A boost the clip holds: a hot loop on loud noise never runs away.
    let v = hot_loop();
    let loud = noise(48_000 * 3, 3, 0.9);
    let out = render(
        &KIND,
        rate,
        &settings(&v),
        &[loud.clone(), loud.clone()],
        512,
    )
    .unwrap();
    let echo_peak = out[0]
        .iter()
        .zip(&loud)
        .fold(0.0f32, |m, (y, x)| m.max((y - x).abs()));
    // The line holds at most 1, so the echo is at most the EQ's impulse response's sum of
    // magnitudes times the feedback: bounded, where without the clip it would grow.
    let g: [f64; BANDS] = std::array::from_fn(|b| f64::from(gain(v[FIRST_BAND + b] as f32)));
    let feedback = v[LEFT_FEEDBACK] / 100.0 + 2e-4;
    let bound = eq_reference(&g, 600).iter().map(|h| h.abs()).sum::<f64>() * feedback;
    println!("A hot loop: the echo's peak {echo_peak:.3}, its bound {bound:.3}");
    assert!(
        f64::from(echo_peak) <= bound + 1e-4,
        "{echo_peak} > {bound}"
    );
}

#[test]
fn a_zero_delay_reads_a_whole_line_back() {
    // Typed 0 ms: the echo after the whole line, 200 + 2·fs samples (88400 at 44.1 kHz).
    let rate = 44_100.0;
    let v = flat(&[
        (LEFT_DELAY, 0.0),
        (LEFT_FEEDBACK, 50.0),
        (LEFT_LEVEL, 100.0),
    ]);
    let n = 90_000;
    let out = render(
        &KIND,
        rate,
        &settings(&v),
        &[impulse(n, 0, 1.0), vec![0.0; n]],
        512,
    )
    .unwrap();
    assert_eq!(line_length(rate), 88_400);
    let echoes: Vec<usize> = (1..n).filter(|&i| out[0][i].abs() > 1e-3).collect();
    assert_eq!(echoes, [88_400]);
}

#[test]
fn a_delay_change_crossfades_forwards() {
    // Feedback at its least (0.1 %), Level 100 %, EQ flat: the echo is 0.001 of the tap, which
    // moves from 10 ms back to 20 ms back in a straight line over 4800 samples.
    let rate = 48_000.0;
    let mut fx = Echo::new(rate);
    for (i, x) in flat(&[
        (LEFT_DELAY, 10.0),
        (LEFT_FEEDBACK, 0.1),
        (LEFT_LEVEL, 100.0),
    ])
    .iter()
    .enumerate()
    {
        fx.set(i, *x);
    }
    fx.snap();
    let n = 14_000;
    let x = noise(n, 4, 0.5);
    let mut io = vec![x.clone(), vec![0.0; n]];
    let change = 4000;
    fx.process(&mut io, 0, change);
    fx.set(LEFT_DELAY, 20.0);
    fx.process(&mut io, change, n);
    let back = |i: usize, d: usize| if i >= d { x[i - d] } else { 0.0 };
    for i in 1500..n {
        let tap = if i < change {
            back(i, 480)
        } else if i < change + 4800 {
            let w = (i - change + 1) as f32 / 4800.0;
            (1.0 - w) * back(i, 480) + w * back(i, 960)
        } else {
            back(i, 960)
        };
        let echo = (io[0][i] - x[i]) / 0.001;
        assert!((echo - tap).abs() < 2e-3, "sample {i}: {echo} vs {tap}");
    }
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100 * 2;
    let audio = [noise(n, 5, 0.5), noise(n, 6, 0.5)];
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        ("bouncing", bouncing()),
        ("hot loop", hot_loop()),
        (
            "short repeats",
            with(&[(LEFT_DELAY, 40.0), (RIGHT_DELAY, 40.0), (LOCK, 1.0)]),
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
fn the_parameters_show_and_lock_links_the_pairs() {
    for p in &PARAMS {
        assert!((p.min..=p.max).contains(&p.default), "{}", p.name);
    }
    assert_eq!(PARAMS[LEFT_DELAY].display(348.8), "348.8 ms");
    assert_eq!(PARAMS[FIRST_BAND].display(-9.8), "-9.8 dB");
    assert_eq!(PARAMS[UNITS].display(2.0), "Samples");
    // Lock links the left and right of each pair, by its name and a group per pair.
    for (l, r) in [
        (LEFT_DELAY, RIGHT_DELAY),
        (LEFT_FEEDBACK, RIGHT_FEEDBACK),
        (LEFT_LEVEL, RIGHT_LEVEL),
    ] {
        assert_eq!(PARAMS[l].link, PARAMS[r].link);
        let link = PARAMS[l].link.unwrap();
        assert_eq!(link.split(':').next(), Some(PARAMS[LOCK].name));
    }
    assert_ne!(PARAMS[LEFT_DELAY].link, PARAMS[LEFT_LEVEL].link);
    assert!(!PARAMS[LOCK].automatable && !PARAMS[UNITS].automatable);
    assert_eq!(band_label_hz(48_000.0, 0).round(), 94.0);
}

/// The cost per second of stereo audio in release, and the worst 64-sample block, on noise and
/// on the silent tail after it (`cargo test --release -p daw-core echo_cost -- --ignored
/// --nocapture`).
#[test]
#[ignore]
fn echo_cost() {
    let rate = 48_000.0;
    let seconds = 30;
    let n = 48_000 * seconds;
    let mut left = noise(n, 6, 0.5);
    let mut right = noise(n, 7, 0.5);
    // Noise for 5 s, then 25 s of the tail.
    left[48_000 * 5..].fill(0.0);
    right[48_000 * 5..].fill(0.0);
    let audio = [left, right];
    // The bouncing 80 ms repeats die into the smallest values within the tail.
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        ("hot loop", hot_loop()),
        ("bouncing", bouncing()),
    ] {
        let mut fx = Echo::new(rate);
        for (i, x) in v.iter().enumerate() {
            fx.set(i, *x);
        }
        fx.snap();
        let block = 64;
        let mut io: Vec<Vec<f32>> = vec![vec![0.0; block]; CHANNELS];
        let mut times = Vec::with_capacity(n / block);
        let mut tail = 0.0;
        let start = std::time::Instant::now();
        for from in (0..n).step_by(block) {
            for (c, buf) in io.iter_mut().enumerate() {
                buf.copy_from_slice(&audio[c][from..from + block]);
            }
            let t = std::time::Instant::now();
            fx.process(&mut io, 0, block);
            let e = t.elapsed().as_secs_f64();
            if from >= 48_000 * 25 {
                tail += e;
            }
            times.push(e);
        }
        let total = start.elapsed().as_secs_f64();
        let worst = times.iter().copied().fold(0.0, f64::max);
        // The first blocks touch the lines' fresh pages: the worst after the first second too.
        let settled = times[750..].iter().copied().fold(0.0, f64::max);
        times.sort_by(f64::total_cmp);
        println!(
            "Echo, {name}: {:.2} ms per second of stereo audio ({:.3} % of a core), the last 5 s \
             of tail {:.2} ms a second, the worst 64-sample block {:.4} ms (after the first second {:.4}; 99.9th percentile \
             {:.4}) of {:.3}",
            total * 1000.0 / seconds as f64,
            total * 100.0 / seconds as f64,
            tail * 1000.0 / 5.0,
            worst * 1000.0,
            settled * 1000.0,
            times[times.len() * 999 / 1000] * 1000.0,
            64.0 / rate * 1000.0
        );
    }
}
