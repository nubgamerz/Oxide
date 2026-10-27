//! Click & Pop Repair's tests (`click_pop.rs`).

use super::super::{render, render_with_print, Native};
use super::*;
use std::collections::BTreeMap;
use std::f64::consts::PI;

const RATE: f64 = 48_000.0;

fn settings(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn defaults() -> [f64; COUNT] {
    std::array::from_fn(|i| PARAMS[i].default)
}

/// A few settings of the effect's own to run the tests over: its defaults; stricter (higher
/// thresholds, two passes, no big-pop pass); and for quiet material in one pass (the levels
/// lower, Multiple Pass off).
fn variants() -> [(&'static str, [f64; COUNT]); 3] {
    let d = defaults();
    let mut strict = d;
    strict[MAX_THRESHOLD] = 40.0;
    strict[MIN_THRESHOLD] = 28.0;
    strict[PASSES] = 2.0;
    strict[BIG_POPS] = 0.0;
    let mut quiet = d;
    quiet[MAX_THRESHOLD] = 40.0;
    quiet[MIN_THRESHOLD] = 25.0;
    quiet[MAX_LEVEL] = -15.0;
    quiet[AVG_LEVEL] = -60.0;
    quiet[MIN_LEVEL] = -36.0;
    quiet[PASSES] = 1.0;
    quiet[MULTIPLE_PASS] = 0.0;
    [
        ("the defaults", d),
        ("stricter, two passes", strict),
        ("quiet, one pass", quiet),
    ]
}

/// Gaussian noise of standard deviation `sigma` from a fixed seed (xorshift, Box-Muller).
fn gauss(n: usize, seed: u64, sigma: f64) -> Vec<f32> {
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut uniform = move || {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        ((x >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            let (u, v) = (uniform(), uniform());
            (sigma * (-2.0 * u.ln()).sqrt() * (2.0 * PI * v).cos()) as f32
        })
        .collect()
}

fn tone(n: usize, hz: f64, amp: f64) -> Vec<f32> {
    (0..n)
        .map(|i| (amp * (2.0 * PI * hz * i as f64 / RATE).sin()) as f32)
        .collect()
}

fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

/// Faded in and out over 10 ms: a sound that starts from digital silence at full tilt is a
/// step, which is rightly taken for a pop.
fn fade(x: Vec<f32>) -> Vec<f32> {
    let ramp = 480;
    let len = x.len();
    x.into_iter()
        .enumerate()
        .map(|(i, s)| {
            let edge = i.min(len - 1 - i);
            if edge < ramp {
                let g = 0.5 - 0.5 * (PI * edge as f64 / ramp as f64).cos();
                (f64::from(s) * g) as f32
            } else {
                s
            }
        })
        .collect()
}

/// Per click: the output's error against `clean` relative to the click's own, in dB, over the
/// click and 8 samples each side.
fn depths(clean: &[f32], clicky: &[f32], out: &[f32], clicks: &[(usize, usize)]) -> Vec<f64> {
    clicks
        .iter()
        .map(|&(at, len)| {
            let err = |x: &[f32]| -> f64 {
                (at - 8..at + len + 8)
                    .map(|i| (f64::from(x[i]) - f64::from(clean[i])).powi(2))
                    .sum()
            };
            10.0 * (err(out) / err(clicky)).max(1e-30).log10()
        })
        .collect()
}

/// Run a fresh instance over `io` (one buffer per channel) in blocks of `block`, plain values
/// set first; returns it for its counters.
fn run(rate: f64, values: &[f64], io: &mut [Vec<f32>], block: usize) -> ClickPop {
    let mut fx = ClickPop::new(rate);
    for (i, v) in values.iter().enumerate() {
        fx.set(i, *v);
    }
    let n = io[0].len();
    let mut at = 0;
    while at < n {
        let to = (at + block).min(n);
        fx.process(io, at, to);
        at = to;
    }
    fx
}

/// `x` (one buffer per channel) through the effect, with the latency taken off: the input's
/// length, plus enough silence after it to come out.
fn through(values: &[f64], x: &[Vec<f32>], block: usize) -> (Vec<Vec<f32>>, ClickPop) {
    through_at(RATE, values, x, block)
}

/// [`through`] at a sample rate.
fn through_at(
    rate: f64,
    values: &[f64],
    x: &[Vec<f32>],
    block: usize,
) -> (Vec<Vec<f32>>, ClickPop) {
    let n = x[0].len();
    let mut io: Vec<Vec<f32>> = (0..CHANNELS)
        .map(|c| {
            let mut v = x[c.min(x.len() - 1)].clone();
            v.resize(n + LATENCY, 0.0);
            v
        })
        .collect();
    let fx = run(rate, values, &mut io, block);
    let out = io.into_iter().map(|v| v[LATENCY..].to_vec()).collect();
    (out, fx)
}

/// `clean` with a click of `len` samples at each of `at`, of `shape`: 0 alternating steps of
/// `amp` (every third sample the other way), 1 a plain step of `amp`, 2 a burst of random
/// signs and sizes from half to all of `amp`; signs alternating click to click.
fn with_shaped(clean: &[f32], clicks: &[(usize, usize)], amp: f32, shape: u8) -> Vec<f32> {
    let mut x = clean.to_vec();
    let burst = gauss(64, 99, 1.0);
    for (k, &(at, len)) in clicks.iter().enumerate() {
        let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
        for i in 0..len {
            let b = burst[(i + 7 * k) % 64];
            x[at + i] += match shape {
                0 => sign * if i % 3 == 1 { -amp } else { amp },
                1 => sign * amp,
                _ => amp * b.signum() * (0.5 + 0.5 * b.abs().min(1.0)),
            };
        }
    }
    x
}

/// Clicks of 1 to 15 samples (the longest every shape is found at, a plain step's: see the
/// next test) every 2311 samples from 6000, so they fall everywhere in the blocks.
fn sizes() -> Vec<(usize, usize)> {
    [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15]
        .iter()
        .enumerate()
        .map(|(k, &len)| (6000 + 2311 * k, len))
        .collect()
}

/// A small click on noise: the error at its own sample against the click's, in dB (over a
/// window the rebuilt noise, which no repair can know, outweighs a click under the noise's
/// level).
fn sample_depth(clean: &[f32], clicky: &[f32], out: &[f32], at: usize) -> f64 {
    let e = |x: &[f32]| (f64::from(x[at]) - f64::from(clean[at])).powi(2);
    10.0 * (e(out) / e(clicky)).max(1e-30).log10()
}

fn counted(fx: &ClickPop, which: usize) -> u64 {
    fx.counters[which].load(Ordering::Relaxed)
}

#[test]
fn clicks_of_1_to_15_samples_on_a_tone_and_on_noise_are_repaired() {
    let n = 48_000;
    let list = sizes();
    // A 440 Hz tone at -12 dBFS over noise at -90 dBFS, and Gaussian noise at -40 dBFS;
    // clicks of 0.5 of every shape.
    let cases = [
        ("tone", mix(&tone(n, 440.0, 0.25), &gauss(n, 3, 3e-5))),
        ("noise", gauss(n, 4, 0.01)),
    ];
    for (name, clean) in &cases {
        for shape in 0..3 {
            let clicky = with_shaped(clean, &list, 0.5, shape);
            let (out, fx) = through(&defaults(), std::slice::from_ref(&clicky), 512);
            let d = depths(clean, &clicky, &out[0], &list);
            let shown: Vec<String> = list
                .iter()
                .zip(&d)
                .map(|((_, len), d)| format!("{len}: {d:.1}"))
                .collect();
            println!(
                "{name}, shape {shape}: {} (corrected {}, passes {})",
                shown.join(", "),
                counted(&fx, CORRECTED),
                fx.passes_run
            );
            for (&(at, len), depth) in list.iter().zip(&d) {
                assert!(
                    *depth < -20.0,
                    "{name}, shape {shape}: a click of {len} at {at}: {depth:.1} dB"
                );
            }
        }
    }
}

#[test]
fn how_long_a_click_the_statistic_finds() {
    // The statistic needs evidence from both directions within 16 samples (as traced), so a
    // long click is no click to it; how long depends on its shape. Measured on the tone and
    // printed; a plain step of 40 samples comes back as it went in.
    let n = 60_000;
    let clean = mix(&tone(n, 440.0, 0.25), &gauss(n, 3, 3e-5));
    let list: Vec<(usize, usize)> = (1..=44).map(|k| (3000 + 1200 * k, k)).collect();
    for shape in 0..3 {
        let clicky = with_shaped(&clean, &list, 0.5, shape);
        let (out, _) = through(&defaults(), std::slice::from_ref(&clicky), 512);
        let d = depths(&clean, &clicky, &out[0], &list);
        let longest = list
            .iter()
            .zip(&d)
            .take_while(|(_, d)| **d < -20.0)
            .last()
            .map_or(0, |(c, _)| c.1);
        println!("shape {shape}: every click up to {longest} samples repaired");
        assert!(longest >= 15, "shape {shape}: {longest}");
    }
    let clicky = with_shaped(&clean, &[(20_000, 40)], 0.5, 1);
    let (out, _) = through(&defaults(), std::slice::from_ref(&clicky), 512);
    let d = depths(&clean, &clicky, &out[0], &[(20_000, 40)])[0];
    println!("a step of 40 samples: {d:.1} dB");
    assert!(d > -1.0, "{d}");
}

#[test]
fn a_click_under_the_detection_curve_for_its_level_is_kept() {
    // On noise at -40 dBFS a single-sample click whose statistic stays under the curve at its
    // level: kept bit for bit; with the curve lowered under it, repaired. (The second level
    // verification off, so only the detection decides.)
    let n = 40_000;
    let clean = gauss(n, 21, 0.01);
    let at = 20_000;
    let mut values = defaults();
    values[SECOND_LEVEL] = 0.0;
    let (amp, peak, threshold) = [0.015f32, 0.02, 0.025, 0.03, 0.035, 0.04, 0.05]
        .iter()
        .rev()
        .find_map(|&a| {
            let x = with_shaped(&clean, &[(at, 1)], a, 1);
            let (stat, level) = statistic(RATE, &x);
            let peak = stat[at - 4..at + 20].iter().fold(0.0f64, |m, v| m.max(*v));
            let threshold = threshold_at(&values, level[at]);
            (peak > 6.5 && peak < 0.7 * threshold).then_some((a, peak, threshold))
        })
        .expect("a click under the curve");
    println!("a click of {amp} on -40 dBFS noise: statistic {peak:.1}, threshold {threshold:.1}");
    let clicky = with_shaped(&clean, &[(at, 1)], amp, 1);
    let (out, _) = through(&values, std::slice::from_ref(&clicky), 512);
    assert!(out[0]
        .iter()
        .zip(&clicky)
        .all(|(a, b)| a.to_bits() == b.to_bits()));
    // The curve at its clamp of 6 everywhere: now the click is over it, and repaired.
    let mut low = values;
    for i in [MAX_THRESHOLD, AVG_THRESHOLD, MIN_THRESHOLD] {
        low[i] = 1.0;
    }
    let (out, _) = through(&low, std::slice::from_ref(&clicky), 512);
    let d = sample_depth(&clean, &clicky, &out[0], at);
    println!("with the curve lowered: the click's sample {d:.1} dB");
    assert!(d < -3.0, "{d}");
}

#[test]
fn audio_without_clicks_comes_back_delayed_bit_for_bit() {
    let n = 3 * 48_000;
    let signals = [
        fade(mix(&tone(n, 440.0, 0.25), &gauss(n, 5, 0.003))),
        fade(gauss(n, 6, 0.05)),
        fade(mix(&tone(n, 1000.0, 0.01), &gauss(n, 7, 1e-4))),
        fade(tone(n, 440.0, 0.5)),
    ];
    for (name, values) in &variants() {
        for (k, x) in signals.iter().enumerate() {
            let mut right = x.clone();
            right.reverse();
            let input = [x.clone(), right];
            let mut io: Vec<Vec<f32>> = input
                .iter()
                .map(|c| {
                    let mut v = c.clone();
                    v.resize(n + LATENCY, 0.0);
                    v
                })
                .collect();
            let fx = run(RATE, values, &mut io, 512);
            for c in 0..CHANNELS {
                assert!(io[c][..LATENCY].iter().all(|s| *s == 0.0));
                let changed = (0..n)
                    .filter(|&i| io[c][LATENCY + i].to_bits() != input[c][i].to_bits())
                    .count();
                println!(
                    "{name}, signal {k}, channel {c}: {changed} changed (corrected {})",
                    counted(&fx, CORRECTED)
                );
                assert_eq!(changed, 0, "{name}, signal {k}, channel {c}");
            }
        }
    }
}

#[test]
fn the_same_at_any_block_size() {
    let n = 40_000;
    let list: Vec<(usize, usize)> = sizes().into_iter().filter(|c| c.0 + 30 < n).collect();
    let left = with_shaped(
        &mix(&tone(n, 440.0, 0.25), &gauss(n, 1, 1e-3)),
        &list,
        0.5,
        2,
    );
    let right = with_shaped(&gauss(n, 8, 0.02), &list[3..], 0.3, 0);
    let audio = [left, right];
    for linked in [0.0, 1.0] {
        let s = settings(&[("Link Channels", linked), ("Pulse Train Verify", 1.0)]);
        let a = render(&KIND, RATE, &s, &audio, 512).unwrap();
        assert_ne!(a[0], audio[0]);
        for block in [77, 1, 4096] {
            assert_eq!(
                a,
                render(&KIND, RATE, &s, &audio, block).unwrap(),
                "{block}"
            );
        }
    }
}

#[test]
fn unlinked_the_channels_are_apart() {
    let n = 40_000;
    let list: Vec<(usize, usize)> = sizes().into_iter().filter(|c| c.0 + 30 < n).collect();
    let left = with_shaped(
        &mix(&tone(n, 440.0, 0.25), &gauss(n, 1, 1e-3)),
        &list,
        0.5,
        0,
    );
    let right = gauss(n, 8, 0.02);
    let s = settings(&[]);
    let a = render(&KIND, RATE, &s, &[left.clone(), right.clone()], 512).unwrap();
    assert_eq!(a[1], right);
    let other = render(&KIND, RATE, &s, &[left, gauss(n, 9, 0.3)], 512).unwrap();
    assert_eq!(a[0], other[0]);
}

#[test]
fn link_channels_couples_the_statistics() {
    // Noise at -40 dBFS on both sides. A click of 0.5 on the left alone at 12 000; at 24 000 a
    // click on the right under the curve, with a click of 0.5 on the left at the same sample.
    let n = 40_000;
    let left_clean = gauss(n, 31, 0.01);
    let right_clean = gauss(n, 32, 0.01);
    let left = with_shaped(&left_clean, &[(12_000, 3), (24_000, 3)], 0.5, 1);
    let right = with_shaped(&right_clean, &[(24_000, 1)], 0.03, 1);
    let mut values = defaults();
    values[SECOND_LEVEL] = 0.0;
    let (stat, level) = statistic(RATE, &right);
    let peak = stat[23_996..24_020].iter().fold(0.0f64, |m, v| m.max(*v));
    let threshold = threshold_at(&values, level[24_000]);
    println!("the right's click alone: statistic {peak:.1}, threshold {threshold:.1}");
    assert!(peak < 0.8 * threshold);
    let audio = [left.clone(), right.clone()];
    let (apart, _) = through(&values, &audio, 512);
    let mut linked_values = values;
    linked_values[LINK] = 1.0;
    let (linked, _) = through(&linked_values, &audio, 512);
    let clicks = [(12_000, 3), (24_000, 3)];
    // Apart: the left's clicks repaired, the right untouched.
    assert_eq!(apart[1], right);
    let d = depths(&left_clean, &left, &apart[0], &clicks);
    println!("apart, the left's clicks: {:.1} and {:.1} dB", d[0], d[1]);
    assert!(d.iter().all(|d| *d < -20.0));
    // Linked: the left's are still repaired; a click there lifts the right's statistic (x1.05
    // to the left's), so the right is rebuilt where the left clicks, its own small click too.
    let d = depths(&left_clean, &left, &linked[0], &clicks);
    println!("linked, the left's clicks: {:.1} and {:.1} dB", d[0], d[1]);
    assert!(d.iter().all(|d| *d < -20.0));
    let changed = |from: usize, to: usize| (from..to).filter(|&i| linked[1][i] != right[i]).count();
    println!(
        "linked, the right rebuilt: {} samples at 12 000, {} at 24 000",
        changed(11_950, 12_100),
        changed(23_950, 24_100)
    );
    assert!(changed(11_950, 12_100) > 0);
    let d = sample_depth(&right_clean, &right, &linked[1], 24_000);
    println!("linked, the right's small click: its sample {d:.1} dB");
    assert!(d < -3.0, "{d}");
    assert_eq!(changed(0, 11_900) + changed(24_200, n), 0);
}

#[test]
fn passes_stop_as_specified() {
    // Two clicks close together on noise: the first (0.5) lifts the means the second (0.08,
    // 40 samples later) is judged by, so a first pass finds only the first; with that and
    // another corrected, a second pass runs on audio without it and finds the second, one new
    // click, so no third. Number of Times 1 (or Multiple Pass off) leaves it.
    let n = 40_000;
    let clean = gauss(n, 41, 0.01);
    // A second big click in the same block, so the first pass corrects two.
    let clicky = with_shaped(&clean, &[(20_000, 2), (25_000, 2)], 0.5, 1);
    let clicky = with_shaped(&clicky, &[(20_040, 1)], 0.08, 1);
    let mut values = defaults();
    values[SECOND_LEVEL] = 0.0;
    let second = |values: &[f64]| {
        let (out, fx) = through(values, std::slice::from_ref(&clicky), 512);
        let d = sample_depth(&clean, &clicky, &out[0], 20_040);
        (d, fx.passes_run, fx.blocks_run)
    };
    let (d8, passes8, blocks) = second(&values);
    let mut once = values;
    once[PASSES] = 1.0;
    let (d1, passes1, _) = second(&once);
    let mut single = values;
    single[MULTIPLE_PASS] = 0.0;
    let (d_off, passes_off, _) = second(&single);
    println!(
        "{blocks} blocks; Number of Times 8: {passes8} passes, the masked click {d8:.1} dB; \
         1: {passes1} passes, {d1:.1} dB; Multiple Pass off: {passes_off}, {d_off:.1} dB"
    );
    // One pass a block where fewer than two are corrected; the clicks' block runs again.
    assert_eq!(passes1, blocks);
    assert_eq!(passes_off, blocks);
    assert_eq!(passes8, blocks + 1);
    assert!(d8 < -10.0 && d1 == 0.0 && d_off == 0.0, "{d8} {d1} {d_off}");
}

#[test]
fn a_regular_train_is_kept_with_pulse_train_verify() {
    // Six single-sample clicks 220 samples apart on a tone of that period (a buzz, a pulse a
    // cycle, so every pulse stands out alike): a train, which Pulse Train Verify leaves in;
    // off, they go.
    let n = 40_000;
    let clean = tone(n, RATE / 220.0, 0.25);
    let list: Vec<(usize, usize)> = (0..6).map(|k| (20_000 + 220 * k, 1)).collect();
    let mut clicky = clean.clone();
    for &(at, _) in &list {
        clicky[at] += 0.3;
    }
    let mut values = defaults();
    let (off, _) = through(&values, std::slice::from_ref(&clicky), 512);
    values[PULSE_TRAIN] = 1.0;
    let (on, _) = through(&values, std::slice::from_ref(&clicky), 512);
    let d_off = depths(&clean, &clicky, &off[0], &list);
    let d_on = depths(&clean, &clicky, &on[0], &list);
    println!("off: {d_off:.1?}\non: {d_on:.1?}");
    assert!(d_off.iter().all(|d| *d < -20.0));
    assert!(d_on.iter().all(|d| *d > -1.0));
}

#[test]
fn the_second_level_verification_rejects_what_its_curve_asks_more_of() {
    // A small click on noise (statistic well under 1000, so the reject curve less 0.15 x it
    // stays positive): with the reject curve at 150 its score falls short, and it is counted
    // rejected and kept; with the verification off it is repaired.
    let n = 40_000;
    let clean = gauss(n, 51, 0.01);
    let (amp, peak) = [0.08f32, 0.1, 0.12, 0.15, 0.2, 0.25]
        .iter()
        .find_map(|&a| {
            let x = with_shaped(&clean, &[(20_000, 2)], a, 1);
            let (stat, level) = statistic(RATE, &x);
            let peak = stat[19_996..20_030].iter().fold(0.0f64, |m, v| m.max(*v));
            (peak > 2.0 * threshold_at(&defaults(), level[20_000]) && peak < 600.0)
                .then_some((a, peak))
        })
        .expect("a click well over the curve with a statistic under 600");
    println!("a click of {amp}: statistic {peak:.0}");
    let clicky = with_shaped(&clean, &[(20_000, 2)], amp, 1);
    let mut values = defaults();
    for i in [MAX_REJECT, AVG_REJECT, MIN_REJECT] {
        values[i] = 150.0;
    }
    let (out, fx) = through(&values, std::slice::from_ref(&clicky), 512);
    let rejected = counted(&fx, REJECTED);
    println!("rejected {rejected}");
    assert!(rejected >= 1);
    assert_eq!(out[0], clicky);
    values[SECOND_LEVEL] = 0.0;
    let (out, fx) = through(&values, std::slice::from_ref(&clicky), 512);
    assert_eq!(counted(&fx, REJECTED), 0);
    let d = sample_depth(&clean, &clicky, &out[0], 20_000);
    println!("not verified: the click's first sample {d:.1} dB");
    assert!(d < -10.0, "{d}");
}

#[test]
fn a_big_pop_is_found_by_the_level_jump() {
    // A pop on quiet noise (-60 dBFS): 120 samples of loud crackle (up to 0.9), rising in 8
    // samples and decaying, longer than the statistic reaches; the jump in the level (which
    // counts high frequencies: the pre-emphasis) finds it.
    let n = 48_000;
    let clean = gauss(n, 61, 0.001);
    let at = 24_000;
    let mut clicky = clean.clone();
    let burst = gauss(120, 62, 0.6);
    for (i, b) in burst.iter().enumerate() {
        let t = i as f64;
        let envelope = (t / 8.0).min(1.0) * (-t / 60.0).exp();
        clicky[at + i] += (f64::from(b.clamp(-0.9, 0.9)) * envelope) as f32;
    }
    let (out, fx) = through(&defaults(), std::slice::from_ref(&clicky), 512);
    let big = counted(&fx, BIG_CORRECTED);
    let d = depths(&clean, &clicky, &out[0], &[(at, 120)])[0];
    println!("big pops corrected {big}; the pop {d:.1} dB");
    assert!(big >= 1, "{big}");
    assert!(d < -20.0, "{d}");
    let mut values = defaults();
    values[BIG_POPS] = 0.0;
    let (_, fx) = through(&values, std::slice::from_ref(&clicky), 512);
    assert_eq!(counted(&fx, BIG_CORRECTED), 0);
}

#[test]
fn ignore_light_crackle_leaves_single_samples_and_too_long_spans() {
    // The rule itself: a detection of one sample is left with Ignore Light Crackle (with the
    // statistic's tail a single-sample click is rarely detected as one: see the note), and a
    // span of 1000 or more always.
    let mut ch = Chan::new();
    for (i, w) in ch.work.iter_mut().enumerate() {
        *w = (i as f64 * 0.05).sin() * 0.3;
    }
    ch.work[5000] += 0.5;
    let mut shared = Shared::new();
    let mut values = defaults();
    values[LIGHT_CRACKLE] = 1.0;
    values[SECOND_LEVEL] = 0.0;
    let s = Settings::of(&values);
    let reject = curve(&values, true);
    let click = |start, end| Click {
        start,
        end,
        peak: 1e4,
        big: false,
    };
    assert!(matches!(
        shared.repair(&mut ch, click(5000, 5000), &s, &reject),
        Outcome::Left
    ));
    assert!(!ch.touched.iter().any(|t| *t));
    assert!(matches!(
        shared.repair(&mut ch, click(5000, 5001), &s, &reject),
        Outcome::Corrected { big: false }
    ));
    assert!(matches!(
        shared.repair(&mut ch, click(2000, 3000), &s, &reject),
        Outcome::Left
    ));
}

#[test]
fn a_transport_start_starts_the_blocks_afresh() {
    let n = 30_000;
    let x = with_shaped(&tone(n, 300.0, 0.3), &[(15_000, 6)], 0.4, 2);
    let input = [x.clone(), x];
    let (first, _) = through(&defaults(), &input, 512);
    let mut fx = ClickPop::new(RATE);
    let mut io = vec![gauss(5000, 1, 0.2); CHANNELS];
    fx.process(&mut io, 0, 5000);
    fx.start();
    let mut io: Vec<Vec<f32>> = input
        .iter()
        .map(|c| {
            let mut v = c.clone();
            v.resize(n + LATENCY, 0.0);
            v
        })
        .collect();
    fx.process(&mut io, 0, n + LATENCY);
    for c in 0..CHANNELS {
        assert_eq!(io[c][LATENCY..], first[c][..]);
    }
}

#[test]
fn the_curves_follow_the_points_and_their_clamps() {
    let values = defaults();
    let detect = curve(&values, false);
    let reject = curve(&values, true);
    assert_eq!(detect.len(), TABLE);
    assert!(detect.iter().chain(&reject).all(|v| v.is_finite()));
    // Near, not through, the points (a B-spline).
    for (level, want, got) in [
        (-50.0, 12.0, detect[entry(-50.0)]),
        (-24.0, 15.0, detect[entry(-24.0)]),
        (-12.0, 30.0, detect[entry(-12.0)]),
        (-50.0, 18.0, reject[entry(-50.0)]),
        (-24.0, 28.0, reject[entry(-24.0)]),
        (-12.0, 40.0, reject[entry(-12.0)]),
    ] {
        println!("at {level} dB: {got:.2} for {want}");
        assert!((got - want).abs() < 2.0, "at {level}: {got} for {want}");
    }
    // The extension points below the low point are floored at 4 and those above the top
    // capped at 60; the spline between them and the low point (here at 1) dips under 4.
    let mut steep = values;
    steep[AVG_THRESHOLD] = 1.0;
    steep[MAX_THRESHOLD] = 150.0;
    let p = points(&steep, false);
    assert_eq!((p[0].1, p[1].1), (4.0, 4.0));
    assert_eq!((p[5].1, p[6].1), (60.0, 60.0));
    let c = curve(&steep, false);
    println!("steep: -100 dB {:.2}, -0.1 dB {:.2}", c[0], c[TABLE - 1]);
    assert!(c[0] >= 1.0 && c[0] <= 4.0 && c[TABLE - 1] <= 150.0);
    // Equal levels: flat lines, no division by zero.
    let mut flat = values;
    flat[MAX_LEVEL] = -30.0;
    flat[AVG_LEVEL] = -30.0;
    flat[MIN_LEVEL] = -30.0;
    assert!(curve(&flat, false).iter().all(|v| v.is_finite()));
    // The fixed order: the "Avg" point is the lowest, "Min" the middle.
    assert_eq!(p[2].0, values[AVG_LEVEL]);
    assert_eq!(p[3].0, values[MIN_LEVEL]);
    assert_eq!(p[4].0, values[MAX_LEVEL]);
    // The effect builds its tables a window at a time with the same code.
    let mut fx = ClickPop::new(RATE);
    let mut io = vec![vec![0.0f32; 2 * HOP]; CHANNELS];
    fx.process(&mut io, 0, 2 * HOP);
    assert!(fx.detect == detect && fx.reject == reject);
}

#[test]
fn the_scans_set_the_points() {
    // Two seconds each of a 1 kHz sine at -6, -20 and -40 dBFS.
    let seg = 96_000;
    let x: Vec<f32> = (0..3 * seg)
        .map(|i| {
            let amp = [0.501, 0.1, 0.01][i / seg];
            (amp * (2.0 * PI * 1000.0 * i as f64 / RATE).sin()) as f32
        })
        .collect();
    let l = levels(RATE, &x);
    let max = l.iter().fold(f64::MIN, |m, v| m.max(*v));
    let min = l.iter().fold(f64::MAX, |m, v| m.min(*v));
    let mean = l.iter().sum::<f64>() / l.len() as f64;
    let floor = |v: f64| ((v * 10.0).floor() / 10.0).clamp(-120.0, 10.0);
    println!("levels: max {max:.2}, min {min:.2}, mean {mean:.2}");
    let mut native = Native::new(&KIND, RATE, None).unwrap();
    native.capture(RATE, std::slice::from_ref(&x)).unwrap();
    let v = native.plain_values();
    let close = |a: f64, b: f64| (a - b).abs() < 1e-4;
    // The maximum to Max Level; the minimum to *Avg* Level and the mean to *Min* Level.
    assert!(close(v[MAX_LEVEL], floor(max)), "{}", v[MAX_LEVEL]);
    assert!(close(v[AVG_LEVEL], floor(min)), "{}", v[AVG_LEVEL]);
    assert!(close(v[MIN_LEVEL], floor(mean)), "{}", v[MIN_LEVEL]);
    let print = scanned(native.print().unwrap()).unwrap();
    assert!(!print.all && print.thresholds.is_none());
    // The thresholds untouched by it.
    assert_eq!(v[MAX_THRESHOLD], 30.0);
    // Saved with the state, the scan's print too.
    let state = native.save_state();
    let back = Native::new(&KIND, RATE, Some(&state)).unwrap();
    assert_eq!(back.plain_values(), native.plain_values());
    assert_eq!(back.print(), native.print());
    // Scan For All Levels on crackle over the sine: thresholds from the trial clicks.
    let mut crackly = x.clone();
    for k in 0..400 {
        crackly[1000 + k * 711] += if k % 2 == 0 { 0.2 } else { -0.15 };
    }
    let all = native.scan_named("all levels").unwrap();
    assert_eq!(all, 1);
    assert_eq!(native.scan_named("Scan For Threshold Levels").unwrap(), 0);
    native
        .scan(RATE, std::slice::from_ref(&crackly), all)
        .unwrap();
    let s = scanned(native.print().unwrap()).unwrap();
    println!("all levels: {s:?}");
    assert!(s.all && s.clicks > 0);
    let v = native.plain_values();
    for i in [
        MAX_THRESHOLD,
        AVG_THRESHOLD,
        MIN_THRESHOLD,
        MAX_REJECT,
        AVG_REJECT,
        MIN_REJECT,
    ] {
        assert!((1.0..=150.0).contains(&v[i]), "{} {}", PARAMS[i].name, v[i]);
    }
    let t = s.thresholds.unwrap();
    assert!(close(v[MAX_THRESHOLD], t[0]) && close(v[AVG_REJECT], t[4]));
    assert!(native.scan_named("nope").is_err());
    assert!(native.scan(RATE, &[Vec::new()], 0).is_err());
    assert!(native.scan(RATE, &[vec![0.0; 1000]], 0).is_err());
    // The capture path's render: the levels learned before the audio runs.
    let out = render_with_print(
        &KIND,
        RATE,
        &settings(&[]),
        Some(std::slice::from_ref(&x)),
        std::slice::from_ref(&x),
        4096,
    )
    .unwrap();
    assert_eq!(out[0].len(), x.len());
}

#[test]
fn the_settings_show_as_expected() {
    assert_eq!(PARAMS.len(), COUNT);
    let names: Vec<&str> = PARAMS.iter().map(|p| p.name).collect();
    assert_eq!(names[AVG_LEVEL], "Avg Level");
    assert_eq!(names[LINK], "Link Channels");
    // The test settings are in range and survive a round trip through the normalised value.
    for (name, values) in &variants() {
        for (p, v) in PARAMS.iter().zip(values) {
            assert!((p.min..=p.max).contains(v), "{name}: {} {v}", p.name);
            assert!(
                (p.plain(p.normalized(*v)) - v).abs() < 1e-9,
                "{name}: {}",
                p.name
            );
        }
    }
    let d = defaults();
    assert_eq!(PARAMS[FFT_SIZE].display(d[FFT_SIZE]), "32 samples");
    assert_eq!(PARAMS[MAX_LEVEL].display(d[MAX_LEVEL]), "-12.0 dB");
    assert_eq!(PARAMS[SECOND_LEVEL].display(d[SECOND_LEVEL]), "On");
    // The context and the order: Auto FFT's rule (16..1024), the order C/2 within 8..32.
    let s = Settings::of(&d);
    assert_eq!(s.context(15, false), 64);
    assert_eq!(s.context(44, false), 256);
    assert_eq!(s.context(200, true), 128);
    assert_eq!(order(64), 32);
    assert_eq!(order(16), 8);
    let mut fixed = d;
    fixed[AUTO_FFT] = 0.0;
    assert_eq!(Settings::of(&fixed).context(15, false), 32);
    let one = Click {
        start: 100,
        end: 100,
        peak: 0.0,
        big: false,
    };
    assert_eq!(s.span(&one), (95, 109));
    assert_eq!(LATENCY, 22_016);
}

/// The cost: run by hand in release (`cargo test --release -p daw-core click_pop_cost --
/// --ignored --nocapture`); also what the test settings change on the DI, if present.
#[test]
#[ignore]
fn click_pop_cost() {
    let n = 48_000 * 20;
    // Crackle: a click of 1 to 8 samples every 300 samples (160 a second) on a tone and noise,
    // and a pop every second.
    let clean = mix(&tone(n, 440.0, 0.25), &gauss(n, 5, 0.003));
    let crackle: Vec<(usize, usize)> = (0..n / 300 - 1)
        .map(|k| (300 * k + 150, 1 + k % 8))
        .collect();
    let mut crackly = with_shaped(&clean, &crackle, 0.2, 2);
    for k in 0..19 {
        let at = 48_000 * k + 24_000;
        for i in 0..60 {
            crackly[at + i] += (0.5 * (PI * i as f64 / 60.0).sin()) as f32;
        }
    }
    for (label, x) in [("clean", &clean), ("crackle", &crackly)] {
        for (name, values) in variants() {
            let mut linked = values;
            linked[LINK] = 1.0;
            linked[PULSE_TRAIN] = 1.0;
            for (what, v) in [("", values), (", linked, pulse trains", linked)] {
                let mut fx = ClickPop::new(RATE);
                for (i, val) in v.iter().enumerate() {
                    fx.set(i, *val);
                }
                let mut io = vec![x.clone(); CHANNELS];
                let mut times = Vec::with_capacity(n / 64);
                for at in (0..n).step_by(64) {
                    let t = std::time::Instant::now();
                    fx.process(&mut io, at, (at + 64).min(n));
                    times.push(t.elapsed().as_secs_f64());
                }
                let total: f64 = times.iter().sum();
                let (at, worst) =
                    times
                        .iter()
                        .enumerate()
                        .fold((0, 0.0f64), |m, (i, t)| if *t > m.1 { (i, *t) } else { m });
                let mut sorted = times.clone();
                sorted.sort_by(f64::total_cmp);
                let top = sorted[sorted.len() * 999 / 1000];
                println!(
                    "{label}, {name}{what}: {:.2} % of real time at 48 kHz (stereo), the worst \
                     block of 64 {:.3} ms (at {} of its hop; 99.9 % under {:.3} ms); corrected \
                     {}, rejected {}, passes {} in {} blocks",
                    100.0 * total / (n as f64 / RATE),
                    worst * 1e3,
                    (at * 64) % HOP,
                    top * 1e3,
                    counted(&fx, CORRECTED),
                    counted(&fx, REJECTED),
                    fx.passes_run,
                    fx.blocks_run
                );
            }
        }
    }
    let di = crate::test_audio();
    if let Ok(audio) = crate::wav::read(&di) {
        let x = &audio.channels[0];
        let rate = f64::from(audio.sample_rate);
        for (name, values) in &variants() {
            let mut fx = ClickPop::new(rate);
            for (i, v) in values.iter().enumerate() {
                fx.set(i, *v);
            }
            let mut io: Vec<Vec<f32>> = (0..CHANNELS)
                .map(|_| {
                    let mut v = x.clone();
                    v.resize(x.len() + LATENCY, 0.0);
                    v
                })
                .collect();
            let t = std::time::Instant::now();
            let len = io[0].len();
            fx.process(&mut io, 0, len);
            let elapsed = t.elapsed().as_secs_f64();
            let changed = (0..x.len())
                .filter(|&i| io[0][LATENCY + i].to_bits() != x[i].to_bits())
                .count();
            println!(
                "DI ({:.1} s), {name}: {changed} samples changed ({:.3} %), corrected {} (big \
                 pops {}), rejected {}, {:.1} ms",
                x.len() as f64 / rate,
                100.0 * changed as f64 / x.len() as f64,
                counted(&fx, CORRECTED),
                counted(&fx, BIG_CORRECTED),
                counted(&fx, REJECTED),
                elapsed * 1e3
            );
        }
        // The levels the scans learn from it, and what the default does with them.
        let changed_with = |values: &[f64]| {
            let (out, fx) = through_at(rate, values, std::slice::from_ref(x), 512);
            let changed = (0..x.len())
                .filter(|&i| out[0][i].to_bits() != x[i].to_bits())
                .count();
            (changed, counted(&fx, CORRECTED), counted(&fx, REJECTED))
        };
        let mut native = Native::new(&KIND, rate, None).unwrap();
        native.capture(rate, std::slice::from_ref(x)).unwrap();
        println!(
            "DI scanned: {:?}; the default with those levels: {:?} (changed, corrected, \
             rejected)",
            scanned(native.print().unwrap()),
            changed_with(&native.plain_values())
        );
        native.scan(rate, std::slice::from_ref(x), 1).unwrap();
        println!(
            "DI all levels: {:?}; with those: {:?}",
            scanned(native.print().unwrap()),
            changed_with(&native.plain_values())
        );
    }
}
