//! The Vintage Delay's tests (`analog_delay.rs`).

use super::super::{render, Native};
use super::*;
use crate::graph::{Meter, Processor};
use crate::plugins::InsertFlags;
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

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

/// Wet alone, no feedback: the first echo of everything and nothing more.
fn wet_only(extra: &[(usize, f64)]) -> [f64; COUNT] {
    let mut v = with(&[(DRY, 0.0), (WET, 100.0), (FEEDBACK, 0.0)]);
    for &(i, x) in extra {
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

/// The first echo of `x` from the study's formulas, in double, written here on its own: each
/// input sample shaped (`mode` at Trash `t`, with Tape's drive), through the mode's RBJ
/// low-pass, and read `round(fs·D/1000) + 1 − A·sin(ωn)` samples back by the common cubic, times
/// the wet make-up.
fn first_echo(rate: f64, mode: usize, t: f64, ms: f64, x: &[f32]) -> Vec<f64> {
    let half_pi = std::f64::consts::FRAC_PI_2;
    let tape = mode == TAPE;
    let drive = if tape { 10f64.powf(t) } else { 1.0 };
    let shape = |u: f64| -> f64 {
        match mode {
            ANALOG => {
                let a = 1.14 * t + 0.1;
                (u * a * half_pi).sin() / (a * half_pi).sin()
            }
            TAPE_TUBE => {
                let b = 0.61 * t + 0.01;
                let v = (u * b * half_pi).sin() / (b * half_pi).sin();
                if t > 0.0 {
                    let v = v * 10f64.powf(1.8 * t / 20.0);
                    v / (1.0 + v.abs())
                } else {
                    v
                }
            }
            _ => {
                let c = 6.5 * t + 1.0;
                u.signum() * u.abs().powf(c).tanh().powf(1.0 / c)
            }
        }
    };
    let (f0, q) = FILTERS[mode];
    let w = 2.0 * PI * f0 / rate;
    let alpha = w.sin() / (2.0 * q);
    let a0 = 1.0 + alpha;
    let (b0, b1, b2) = (
        (1.0 - w.cos()) / 2.0 / a0,
        (1.0 - w.cos()) / a0,
        (1.0 - w.cos()) / 2.0 / a0,
    );
    let (a1, a2) = (-2.0 * w.cos() / a0, (1.0 - alpha) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0, 0.0, 0.0, 0.0);
    let line: Vec<f64> = x
        .iter()
        .map(|&s| {
            let u = shape(f64::from(s) * drive);
            let y = b0 * u + b1 * x1 + b2 * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, u, y1, y);
            y
        })
        .collect();
    let whole = (rate * ms / 1000.0 + 0.5).floor() + 1.0;
    let depth = rate * WOW_MS / 1000.0;
    let omega = 2.0 * PI / (rate * WOW_PERIOD_MS[mode] / 1000.0);
    let at = |i: i64| if i >= 0 { line[i as usize] } else { 0.0 };
    let comp = if tape { 1.0 - 0.5 * t } else { 1.0 };
    (0..x.len())
        .map(|n| {
            let age = whole - (omega * n as f64).sin() * depth;
            let k = age.ceil();
            let mu = k - age;
            let i = n as i64 - k as i64;
            let (ym, y0, y1, y2) = (at(i - 1), at(i), at(i + 1), at(i + 2));
            let a = y2 - y1 - ym + y0;
            comp * (((a * mu + (ym - y0 - a)) * mu + (y1 - ym)) * mu + y0)
        })
        .collect()
}

#[test]
fn the_echo_comes_round_plus_one_late_swaying_with_the_wow_through_each_mode() {
    // Noise through each mode at three Trash settings, wet alone with no feedback: the effect's
    // output against the study's formulas worked in double (shaper, low-pass, the delay of
    // round(fs·D/1000) + 1 samples, the ±0.1 ms wow at the mode's period, the cubic read).
    for rate in [44_100.0, 48_000.0] {
        let x = noise(30_000, 1, 0.7);
        for mode in [ANALOG, TAPE_TUBE, TAPE] {
            for trash in [0.0, 50.0, 100.0] {
                let v = wet_only(&[(MODE, mode as f64), (TRASH, trash), (DELAY, 200.0)]);
                let out = render(&KIND, rate, &settings(&v), &[x.clone(), x.clone()], 512).unwrap();
                let want = first_echo(rate, mode, trash / 100.0, 200.0, &x);
                let worst = out[0]
                    .iter()
                    .zip(&want)
                    .map(|(a, b)| (f64::from(*a) - b).abs())
                    .fold(0.0, f64::max);
                assert!(
                    worst < 2e-5,
                    "{rate} Hz, {} at Trash {trash}: {worst}",
                    MODES[mode]
                );
            }
        }
    }
    // An impulse: nothing until the delay less the wow, then the echo.
    let rate = 44_100.0;
    let mut x = vec![0.0f32; 12_000];
    x[0] = 0.5;
    let v = wet_only(&[(TRASH, 0.0)]);
    let out = render(&KIND, rate, &settings(&v), &[x.clone(), x], 512).unwrap();
    let first = out[0].iter().position(|y| *y != 0.0).unwrap();
    assert_eq!(delay_samples(rate, 200.0), 8821);
    assert!((8821 - 7..=8821).contains(&first), "{first}");
}

/// The delay of a 1 kHz sine at each upward zero crossing of the output (samples, against the
/// input's crossing a whole number of periods before).
fn crossing_delays(out: &[f32], rate: f64, from: usize) -> Vec<(f64, f64)> {
    let period = rate / 1000.0;
    let mut found = Vec::new();
    for i in from..out.len() - 1 {
        if out[i] <= 0.0 && out[i + 1] > 0.0 {
            let t = i as f64 + f64::from(out[i] / (out[i] - out[i + 1]));
            // The input crosses upward at whole periods; the delay is the rest, near 8830.
            let periods = ((t - 8830.0) / period).round();
            found.push((t, t - periods * period));
        }
    }
    found
}

#[test]
fn the_wow_sways_the_delay_by_a_tenth_of_a_millisecond_at_each_mode_s_period() {
    // A quiet 1 kHz sine (the shaper nearly straight): its delay swings 2 × 4.41 samples at 44.1
    // kHz, over 500 ms (Analog) or 750 ms (Tape), repeating with that period.
    let rate = 44_100.0;
    let n = 44_100 * 3;
    let sine: Vec<f32> = (0..n)
        .map(|i| (0.05 * (2.0 * PI * 1000.0 * i as f64 / rate).sin()) as f32)
        .collect();
    for (mode, period_ms) in [(ANALOG, 500.0), (TAPE, 750.0)] {
        let v = wet_only(&[(MODE, mode as f64), (TRASH, 0.0)]);
        let out = render(
            &KIND,
            rate,
            &settings(&v),
            &[sine.clone(), sine.clone()],
            512,
        )
        .unwrap();
        let delays = crossing_delays(&out[0], rate, 10_000);
        let (lo, hi) = delays.iter().fold((f64::MAX, f64::MIN), |(lo, hi), d| {
            (lo.min(d.1), hi.max(d.1))
        });
        println!("{}: delay {lo:.2} to {hi:.2} samples", MODES[mode]);
        assert!(
            ((hi - lo) - 8.82).abs() < 0.2,
            "{}: swing {}",
            MODES[mode],
            hi - lo
        );
        // One period later the delay is the same again.
        let period = rate * period_ms / 1000.0;
        let at = |t: f64| {
            delays
                .iter()
                .min_by(|a, b| (a.0 - t).abs().total_cmp(&(b.0 - t).abs()))
                .unwrap()
                .1
        };
        for t in [15_000.0, 20_000.0, 30_000.0] {
            assert!(
                (at(t) - at(t + period)).abs() < 0.15,
                "{} at {t}",
                MODES[mode]
            );
        }
    }
}

#[test]
fn each_mode_s_shaper_and_filter_are_the_study_s() {
    // The sine shapers map 1 to 1; Tape/Tube's soft clip and Tape's knee stay under 1; all are
    // odd (Tape keeps the polarity: TAPE_FLIPS is off).
    for t in [0.0f32, 0.25, 0.79, 1.0] {
        let analog = Shaper::new(ANALOG, t);
        assert!((analog.apply(1.0) - 1.0).abs() < 1e-5, "{t}");
        let a = (1.14 * t + 0.1) * std::f32::consts::FRAC_PI_2;
        assert!((analog.apply(0.5) - (0.5 * a).sin() / a.sin()).abs() < 1e-6);
        let tube = Shaper::new(TAPE_TUBE, t);
        let tape = Shaper::new(TAPE, t);
        for u in [0.1f32, 0.5, 1.0, 3.0] {
            for s in [&analog, &tube, &tape] {
                assert_eq!(s.apply(-u), -s.apply(u), "{t} {u}");
            }
            // Tape/Tube's clip (from any Trash above 0) and Tape's knee never pass 1.
            assert!(t == 0.0 || tube.apply(u).abs() < 1.0, "{t} {u}");
            assert!(tape.apply(u) <= 1.0 && tape.apply(u) > 0.0, "{t} {u}");
        }
        let c = 6.5 * t + 1.0;
        assert!((tape.apply(0.5) - 0.5f32.powf(c).tanh().powf(1.0 / c)).abs() < 1e-6);
        // Tape's drive and make-up; none elsewhere.
        assert!((tape.drive - 10f32.powf(t)).abs() < 1e-5 && tape.comp == 1.0 - 0.5 * t);
        assert_eq!((analog.drive, analog.comp, tube.drive), (1.0, 1.0, 1.0));
    }
    // At t = 0 Tape/Tube is a near-straight sine and no clip (its gain is 1).
    let tube = Shaper::new(TAPE_TUBE, 0.0);
    assert!((tube.apply(0.2) - 0.2).abs() < 1e-4);
    // The low-passes: unity at DC, Q at the corner (the RBJ design), falling beyond.
    for rate in [44_100.0, 48_000.0, 192_000.0] {
        for (mode, (f0, q)) in FILTERS.iter().enumerate() {
            let c = filter(rate, mode);
            assert!((c.magnitude(1.0, rate) - 1.0).abs() < 1e-6);
            assert!(
                (c.magnitude(*f0, rate) - q).abs() < 1e-9,
                "{mode} at {rate}"
            );
            assert!(c.magnitude(4.0 * f0, rate) < 0.1);
        }
    }
}

#[test]
fn a_delay_change_glides_a_sample_a_sample() {
    // 200 → 400 ms at 48 kHz: the delay grows by exactly one sample every sample (the read
    // point stands still) for 9600 samples, then holds; back to 200, it shrinks one a sample.
    let rate = 48_000.0;
    let mut fx = AnalogDelay::new(rate);
    fx.snap();
    assert_eq!(fx.whole, 9601);
    let mut io = vec![vec![0.0f32; 1]; CHANNELS];
    fx.set(DELAY, 400.0);
    let mut seen = Vec::new();
    for _ in 0..9700 {
        fx.process(&mut io, 0, 1);
        seen.push(fx.whole);
    }
    for (k, w) in seen.iter().enumerate() {
        assert_eq!(*w, (9602 + k).min(19_201), "sample {k}");
    }
    fx.set(DELAY, 200.0);
    for k in 0..9700 {
        fx.process(&mut io, 0, 1);
        assert_eq!(fx.whole, (19_200 - k).max(9601), "sample {k}");
    }
    // A small change lands at once (within a sample); a transport start lands with no glide.
    fx.set(DELAY, 200.01);
    fx.process(&mut io, 0, 1);
    assert_eq!(fx.delay_ms, f64::from(200.01f32));
    fx.set(DELAY, 1000.0);
    fx.start();
    assert_eq!(fx.whole, 48_001);
    // A lane moves the target in its line, and the delay follows it.
    fx.ramp(DELAY, 1000.0, 0);
    fx.ramp(DELAY, 1010.0, 480);
    let mut io = vec![vec![0.0f32; 960]; CHANNELS];
    fx.process(&mut io, 0, 960);
    assert_eq!(fx.whole, 48_481);
}

#[test]
fn the_limiter_holds_the_wet_signal_above_100_percent_feedback() {
    assert_eq!(limit(0.9), 0.9);
    assert!((limit(0.97) - (0.95 + 0.02 / (1.0 + 0.16))).abs() < 1e-6);
    assert!((limit(1.0) - 0.975).abs() < 1e-6);
    assert_eq!((limit(3.0), limit(-3.0)), (0.975, -0.975));
    // Driven hard with Feedback 150 % the wet output never passes 0.975; at 100 % the limiter
    // is off and the loop's peaks go past it.
    let rate = 48_000.0;
    let x = noise(48_000 * 2, 2, 0.9);
    for (feedback, limited) in [(150.0, true), (100.0, false)] {
        let v = with(&[
            (DRY, 0.0),
            (WET, 100.0),
            (FEEDBACK, feedback),
            (TRASH, 60.0),
            (DELAY, 30.0),
        ]);
        let out = render(&KIND, rate, &settings(&v), &[x.clone(), x.clone()], 512).unwrap();
        let peak = out.iter().flatten().fold(0.0f32, |m, y| m.max(y.abs()));
        println!("Feedback {feedback} %: the wet peak {peak}");
        assert_eq!(peak <= 0.975 + 1e-6, limited, "{feedback}: {peak}");
    }
}

#[test]
fn spread_scales_the_wet_side() {
    let rate = 48_000.0;
    let audio = [noise(20_000, 3, 0.5), noise(20_000, 4, 0.5)];
    let run = |spread: f64| {
        render(
            &KIND,
            rate,
            &settings(&wet_only(&[(SPREAD, spread), (DELAY, 20.0)])),
            &audio,
            512,
        )
        .unwrap()
    };
    let (zero, unit, double) = (run(0.0), run(100.0), run(200.0));
    // 0 %: mono; 100 %: each side its own; 200 %: the side doubled.
    assert_eq!(zero[0], zero[1]);
    for n in 0..20_000 {
        let (l, r) = (unit[0][n], unit[1][n]);
        let (m, s) = (l + r, l - r);
        assert!((zero[0][n] - m * 0.5).abs() < 1e-6);
        assert!((double[0][n] - (m + 2.0 * s) * 0.5).abs() < 1e-6, "{n}");
        assert!((double[1][n] - (m - 2.0 * s) * 0.5).abs() < 1e-6, "{n}");
    }
    assert_ne!(unit[0], unit[1]);
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100 * 2;
    let audio = [noise(n, 5, 0.5), noise(n, 6, 0.5)];
    for (name, v) in &varied() {
        let s = settings(v);
        let a = render(&KIND, rate, &s, &audio, 512).unwrap();
        for block in [77, 1, 4096] {
            assert_eq!(a, render(&KIND, rate, &s, &audio, block).unwrap(), "{name}");
        }
    }
}

/// A spread of settings: the defaults, each mode with its own delay, feedback and trash
/// (Tape pushed past 100 % feedback), and a short slap with a narrow spread.
fn varied() -> [(&'static str, [f64; COUNT]); 5] {
    [
        ("defaults", PARAMS.map(|p| p.default)),
        (
            "tape and tube",
            with(&[
                (MODE, TAPE_TUBE as f64),
                (DELAY, 350.0),
                (FEEDBACK, 45.0),
                (TRASH, 40.0),
            ]),
        ),
        (
            "long tape, past full feedback",
            with(&[
                (MODE, TAPE as f64),
                (DELAY, 900.0),
                (FEEDBACK, 110.0),
                (WET, 60.0),
            ]),
        ),
        (
            "dark repeats",
            with(&[
                (MODE, TAPE as f64),
                (DELAY, 480.0),
                (FEEDBACK, 70.0),
                (TRASH, 80.0),
            ]),
        ),
        (
            "short and narrow",
            with(&[(DELAY, 60.0), (FEEDBACK, 10.0), (SPREAD, 30.0)]),
        ),
    ]
}

/// A transport at `tempo` from `origin` (samples) at 48 kHz, playing at `position`.
fn transport(tempo: f64, origin: f64, quarters: f64, position: i64) -> Transport {
    Transport {
        playing: true,
        position,
        meter: Meter {
            sample_rate: 48_000,
            tempo,
            origin_sample: origin,
            origin_quarter_notes: quarters,
            ..Meter::default()
        },
    }
}

#[test]
fn tempo_sync_follows_the_tempo_map_across_a_change() {
    // Synced to a quarter note: 500 ms at 120 BPM; at a change to 60 BPM it glides to 1000 ms
    // a sample a sample, as a delay change does; a dotted eighth at 60 is 750 ms.
    let rate = 48_000.0;
    let mut fx = AnalogDelay::new(rate);
    fx.set(SYNC, 1.0);
    fx.transport(&transport(120.0, 0.0, 0.0, 0));
    fx.start();
    assert_eq!(fx.whole, delay_samples(rate, 500.0));
    let mut io = vec![vec![0.0f32; 480]; CHANNELS];
    for b in 0..10 {
        fx.transport(&transport(120.0, 0.0, 0.0, b * 480));
        fx.process(&mut io, 0, 480);
    }
    assert_eq!(fx.whole, 24_001);
    // The change at sample 4800 (quarter note 2 at 120).
    for b in 10..70 {
        fx.transport(&transport(60.0, 4800.0, 2.0, b * 480));
        fx.process(&mut io, 0, 480);
        let done = (b - 9) as usize * 480;
        assert_eq!(fx.whole, (24_001 + done).min(48_001), "block {b}");
    }
    fx.set(NOTE, 9.0);
    assert!((fx.wanted_ms() - 750.0).abs() < 1e-9);
    // Delay is ignored while synced; off again, it is the target.
    fx.set(DELAY, 300.0);
    assert!((fx.wanted_ms() - 750.0).abs() < 1e-9);
    fx.set(SYNC, 0.0);
    assert_eq!(fx.wanted_ms(), 300.0);
    // The same through the insert, which hands each block its transport: an impulse 3 s in,
    // after the change and the glide, comes back 1000 ms (48001 samples, give or take the wow)
    // later.
    let native = Native::new(&KIND, rate, None).unwrap();
    for (name, value) in [
        ("Tempo Sync", 1.0),
        ("Dry Out", 0.0),
        ("Wet Out", 100.0),
        ("Feedback", 0.0),
    ] {
        let i = KIND.params.iter().position(|p| p.name == name).unwrap();
        native
            .set(i as u32, KIND.params[i].normalized(value))
            .unwrap();
    }
    let mut insert = native.insert(Arc::new(AtomicBool::new(false)), InsertFlags::default());
    let block = 512;
    let mut out = Vec::new();
    for b in 0..400usize {
        let position = (b * block) as i64;
        let t = if position < 4800 {
            transport(120.0, 0.0, 0.0, position)
        } else {
            transport(60.0, 4800.0, 2.0, position)
        };
        let mut io: Vec<Vec<f32>> = (0..CHANNELS)
            .map(|_| {
                (0..block)
                    .map(|i| if b * block + i == 144_000 { 1.0 } else { 0.0 })
                    .collect()
            })
            .collect();
        insert.process(&mut io, block, &t);
        out.extend_from_slice(&io[0]);
    }
    let echo = out.iter().position(|y| y.abs() > 1e-4).unwrap();
    assert!(
        (144_000 + 48_001 - 8..=144_000 + 48_001 + 6).contains(&echo),
        "{echo}"
    );
}

#[test]
fn the_parameters_show_as_expected() {
    for p in &PARAMS {
        assert!((p.min..=p.max).contains(&p.default), "{}", p.name);
    }
    assert_eq!(PARAMS[MODE].display(1.0), "Tape/Tube");
    assert_eq!(PARAMS[NOTE].display(13.0), "1/4");
    assert_eq!(PARAMS[NOTE].display(9.0), "1/8 dotted");
    assert_eq!(PARAMS[DELAY].display(200.0), "200.0 ms");
    assert_eq!(NOTE_QUARTERS.len(), NOTE_NAMES.len());
    assert_eq!(line_length(192_000.0), 3_072_001);
}

/// The cost per second of stereo audio in release, and the worst 64-sample block, on noise and
/// on the tail after it (`cargo test --release -p daw-core analog_delay_cost -- --ignored
/// --nocapture`).
#[test]
#[ignore]
fn analog_delay_cost() {
    let rate = 48_000.0;
    let seconds = 30;
    let n = 48_000 * seconds;
    let mut left = noise(n, 6, 0.5);
    let mut right = noise(n, 7, 0.5);
    left[48_000 * 5..].fill(0.0);
    right[48_000 * 5..].fill(0.0);
    let audio = [left, right];
    for (name, v) in &varied()[..4] {
        let mut fx = AnalogDelay::new(rate);
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
            "Vintage Delay, {name}: {:.2} ms per second of stereo audio ({:.3} % of a core), the \
             last 5 s of tail {:.2} ms a second, the worst 64-sample block {:.4} ms (after the first second {:.4}; 99.9th \
             percentile {:.4}) of {:.3}",
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
