//! The Chorus's tests (`chorus.rs`).

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

/// A spread of settings to run the tests over: the defaults, many voices, a fast and deep
/// wobble with feedback, a narrow field from averaged inputs, and Highest Quality with the
/// binaural cues and a long spread.
fn varied() -> [(&'static str, [f64; COUNT]); 5] {
    [
        ("defaults", PARAMS.map(|p| p.default)),
        ("many voices", with(&[(VOICES, 10.0), (SPREAD, 40.0)])),
        (
            "fast and deep",
            with(&[
                (VOICES, 4.0),
                (DELAY_TIME, 12.0),
                (DELAY_RATE, 3.0),
                (FEEDBACK, 30.0),
                (MOD_RATE, 6.0),
                (MOD_DEPTH, 8.0),
            ]),
        ),
        (
            "narrow averaged",
            with(&[
                (VOICES, 2.0),
                (DELAY_TIME, 5.0),
                (FIELD, 30.0),
                (AVERAGE, 1.0),
            ]),
        ),
        (
            "wide at highest quality",
            with(&[
                (VOICES, 6.0),
                (SPREAD, 120.0),
                (HIGHEST_QUALITY, 1.0),
                (BINAURAL, 1.0),
                (MOD_DEPTH, 4.0),
                (MOD_RATE, 1.5),
            ]),
        ),
    ]
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

/// Hand `fx` the start state the main thread makes for plain `values` (as `Native::set` posts
/// it and the insert delivers it), and check it took it.
fn deliver(fx: &mut Chorus, rate: f64, values: &[f64; COUNT]) {
    let mut fresh = Some((PREPARES.make)(rate, values));
    let mut spent = None;
    fx.take_prepared(&mut fresh, &mut spent);
    assert!(fresh.is_none() && spent.is_some());
}

/// An instance at `rate` with plain `values`, as `Native::new` makes one: the start state made
/// and delivered first, then the settings, then a snap.
fn chorus(rate: f64, values: &[f64; COUNT]) -> Chorus {
    let mut fx = Chorus::new(rate);
    deliver(&mut fx, rate, values);
    for (i, v) in values.iter().enumerate() {
        fx.set(i, *v);
    }
    fx.snap();
    fx
}

/// `n` samples of `left` and `right` through `fx` in one go.
fn run(fx: &mut Chorus, left: f32, right: f32, n: usize) -> Vec<Vec<f32>> {
    let mut io = vec![vec![left; n], vec![right; n]];
    fx.process(&mut io, 0, n);
    io
}

#[test]
fn the_same_start_gives_the_same_output() {
    // Rendered twice, and an instance played, started again and played: the same samples,
    // whatever the settings (the generator reseeds from their hash at every start).
    let rate = 48_000.0;
    let n = 48_000;
    let audio = [noise(n, 1, 0.5), noise(n, 2, 0.5)];
    for (name, v) in &varied() {
        let a = render(&KIND, rate, &settings(v), &audio, 512).unwrap();
        assert_eq!(a, render(&KIND, rate, &settings(v), &audio, 512).unwrap());
        let mut fx = chorus(rate, v);
        let mut first = audio.to_vec();
        fx.process(&mut first, 0, n);
        assert_eq!(first, a, "{name}: a fresh instance is what renders");
        fx.start();
        let mut again = audio.to_vec();
        fx.process(&mut again, 0, n);
        assert_eq!(again, a, "{name}: started again");
        // And it does wander: other settings, another output.
        assert_ne!(
            a,
            render(
                &KIND,
                rate,
                &settings(&with(&[(DELAY_RATE, 0.2)])),
                &audio,
                512
            )
            .unwrap()
        );
    }
    // The seed is the settings' hash: Dry, Wet and the switches aren't in it, the rest are.
    let base = PARAMS.map(|p| p.default);
    for (i, x) in [
        (DRY, 10.0),
        (WET, 90.0),
        (AVERAGE, 1.0),
        (HIGHEST_QUALITY, 1.0),
    ] {
        assert_eq!(seed(&with(&[(i, x)])), seed(&base), "{}", PARAMS[i].name);
    }
    for (i, x) in [
        (DELAY_TIME, 20.0),
        (DELAY_RATE, 0.5),
        (FEEDBACK, 10.0),
        (SPREAD, 40.0),
        (FIELD, 50.0),
        (MOD_RATE, 2.0),
        (MOD_DEPTH, 3.0),
        (VOICES, 3.0),
    ] {
        assert_ne!(seed(&with(&[(i, x)])), seed(&base), "{}", PARAMS[i].name);
    }
}

#[test]
fn a_voices_change_reseeds_at_once() {
    // As a rebuild: the bank a start would make for the new count, there and then
    // (the main thread posts it before the DSP sees the new count).
    let rate = 44_100.0;
    let mut fx = chorus(rate, &with(&[]));
    run(&mut fx, 0.1, 0.1, 20_000);
    deliver(&mut fx, rate, &with(&[(VOICES, 5.0)]));
    fx.set(VOICES, 5.0);
    let fresh = chorus(rate, &with(&[(VOICES, 5.0)]));
    assert!(fx.bank == fresh.bank && fx.phase == 0 && !fx.waiting);
    // A count whose bank hasn't come yet: the old one plays on until it does, then reseeds.
    run(&mut fx, 0.1, 0.1, 1000);
    fx.set(VOICES, 6.0);
    assert!(fx.waiting && fx.bank != fx.warm);
    run(&mut fx, 0.1, 0.1, 64);
    deliver(&mut fx, rate, &with(&[(VOICES, 6.0)]));
    let six = chorus(rate, &with(&[(VOICES, 6.0)]));
    assert!(!fx.waiting && fx.bank == six.bank && fx.phase == 0);
}

#[test]
fn a_start_makes_nothing_on_the_audio_thread() {
    // Through the insert as the engine runs it: settings changed by hand (the main thread
    // makes the start state in `Native::set`), then the transport starts. The insert's blocks
    // warm nothing up (counted on this thread), and what they play is what a render plays.
    use super::super::{Native, Processor};
    use crate::graph::Transport;
    use crate::plugins::InsertFlags;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;
    let rate = 48_000.0;
    let native = Native::new(&KIND, rate, None).unwrap();
    let mut insert = native.insert(Arc::new(AtomicBool::new(false)), InsertFlags::default());
    let block = 64;
    let mut io = vec![vec![0.0f32; block]; CHANNELS];
    insert.process(&mut io, block, &Transport::default());
    let v = with(&[
        (VOICES, 16.0),
        (DELAY_RATE, 0.1),
        (MOD_RATE, 0.1),
        (MOD_DEPTH, 4.0),
        (HIGHEST_QUALITY, 1.0),
    ]);
    let made = WARM_UPS.with(|w| w.get());
    for (i, p) in PARAMS.iter().enumerate() {
        native.set(i as u32, p.normalized(v[i])).unwrap();
    }
    assert!(WARM_UPS.with(|w| w.get()) > made, "the main thread made it");
    let n = 9600;
    let audio = [noise(n, 8, 0.5), noise(n, 9, 0.5)];
    let before = WARM_UPS.with(|w| w.get());
    let mut played = vec![Vec::new(), Vec::new()];
    for from in (0..n).step_by(block) {
        for (c, buf) in io.iter_mut().enumerate() {
            buf.copy_from_slice(&audio[c][from..from + block]);
        }
        insert.process(&mut io, block, &Transport::playing_at(from as u64));
        for c in 0..CHANNELS {
            played[c].extend_from_slice(&io[c]);
        }
    }
    assert_eq!(
        WARM_UPS.with(|w| w.get()),
        before,
        "the audio thread warmed up"
    );
    assert_eq!(
        played,
        render(&KIND, rate, &settings(&v), &audio, 512).unwrap()
    );
}

#[test]
fn every_voice_s_delay_stays_within_its_range() {
    // d = (n + 1)·D/2 + O with the noise n within ±3 (about ±1: an RMS near 0.29 whatever
    // the rate), clamped to what the interpolator can read and the line; offsets at least
    // 0.65·S/V apart within [gap, S).
    let rate = 44_100.0;
    for (name, v) in &varied() {
        let mut fx = chorus(rate, v);
        let voices = fx.voices();
        let s = (v[SPREAD] / 1000.0 * rate) as u64;
        let gap = (s as f64 / voices as f64 * 0.65).floor();
        let offsets = &fx.bank.offsets[..voices];
        if voices > 1 {
            for (i, a) in offsets.iter().enumerate() {
                assert!(*a >= gap && *a < s.max(1) as f64, "{name}: offset {a}");
                for b in &offsets[..i] {
                    assert!(
                        (a - b).abs() >= gap,
                        "{name}: {a} and {b} closer than {gap}"
                    );
                }
            }
        } else {
            assert_eq!(offsets, [0.0]);
        }
        let hq = v[HIGHEST_QUALITY] >= 0.5;
        let lowest = if hq { Sinc::MIN_AGE } else { 2.0 };
        let d = v[DELAY_TIME] / 1000.0 * rate;
        let (mut sum, mut count, mut outside) = (0.0, 0usize, 0usize);
        let mut io = vec![vec![0.0f32; 1]; CHANNELS];
        for _ in 0..(rate as usize * 20) {
            fx.process(&mut io, 0, 1);
            let t = (fx.phase - 1) as f64 / fx.control as f64;
            let d_now = f64::from(fx.delay_ms.value()) / 1000.0 * rate;
            for voice in 0..voices {
                let noise = fx.bank.delay[voice].at(t);
                assert!(noise.abs() <= 3.0);
                sum += noise * noise;
                count += 1;
                outside += usize::from(noise.abs() > 1.0);
                let age = fx.voice_delay(voice, t, d_now, hq);
                let o = fx.bank.offsets[voice];
                let lo = (o - d).max(lowest) - 1e-9;
                let hi = (o + 2.0 * d).min(fx.longest).max(lowest) + 1e-9;
                assert!((lo..=hi).contains(&age), "{name}: voice {voice} at {age}");
            }
        }
        let rms = (sum / count as f64).sqrt();
        assert!(
            (0.15..0.5).contains(&rms),
            "{name}: the delay noise's RMS {rms}"
        );
        assert!(
            (outside as f64) < 0.02 * count as f64,
            "{name}: {outside} of {count} past ±1"
        );
    }
}

#[test]
fn the_delay_noise_is_as_strong_at_any_rate() {
    // 3 dB an octave below half the rate holds the delay noise near 0.29 RMS at
    // every Delay Rate and sample rate; ours draws at the control rate and scales to match.
    for rate in [44_100.0, 48_000.0, 96_000.0] {
        for hz in [0.5, 2.0, 12.0] {
            let mut fx = chorus(
                rate,
                &with(&[(VOICES, 16.0), (DELAY_RATE, hz), (SPREAD, 0.0)]),
            );
            let design = fx.delay_design;
            let bank = &mut fx.bank;
            let mut sum = 0.0;
            let steps = 40_000;
            for _ in 0..steps {
                for noise in &mut bank.delay {
                    noise.step(&mut bank.generator, &design);
                    sum += noise.history[3].powi(2);
                }
            }
            let rms = (sum / (steps * MAX_VOICES) as f64).sqrt();
            assert!((0.22..0.38).contains(&rms), "{rate} Hz, {hz}: {rms}");
        }
    }
}

#[test]
fn levels_feedback_and_the_clip_are_as_specified() {
    let rate = 48_000.0;
    let n = 48_000;
    let one = [(VOICES, 1.0), (SPREAD, 0.0)];
    // One voice, no depth: a steady input comes back at wet + dry once the line is full.
    let mut fx = chorus(rate, &with(&one));
    let out = run(&mut fx, 0.5, 0.25, n);
    for c in 0..CHANNELS {
        let x = [0.5, 0.25][c];
        let want = (0.3 + 0.7) * x;
        assert!((out[c][n - 1] - want).abs() < 1e-6, "{}", out[c][n - 1]);
    }
    // Negative feedback: the line settles at x/(1 + fb), so y = wet·x/(1 + fb) + dry·x.
    let mut fx = chorus(rate, &with(&[one[0], one[1], (FEEDBACK, 50.0)]));
    let out = run(&mut fx, 0.5, 0.5, 2 * n);
    let want = 0.3 * 0.5 / 1.5 + 0.7 * 0.5;
    assert!(
        (out[0][2 * n - 1] - want).abs() < 1e-5,
        "{}",
        out[0][2 * n - 1]
    );
    // Its sign: an impulse comes back negated a recirculation later.
    let mut fx = chorus(
        rate,
        &with(&[one[0], one[1], (FEEDBACK, 50.0), (DRY, 0.0), (WET, 100.0)]),
    );
    let mut io = vec![vec![0.0f32; n], vec![0.0f32; n]];
    io[0][100] = 1.0;
    fx.process(&mut io, 0, n);
    let first = io[0].iter().position(|x| x.abs() > 0.05).unwrap();
    let peak = |range: std::ops::Range<usize>| {
        io[0][range]
            .iter()
            .copied()
            .fold(0.0f32, |a, b| if b.abs() > a.abs() { b } else { a })
    };
    let echo = peak(first..first + 8);
    let next = first
        + 8
        + io[0][first + 8..]
            .iter()
            .position(|x| x.abs() > 0.05)
            .unwrap();
    let again = peak(next..next + 8);
    assert!(echo > 0.5 && again < -0.2, "{echo} then {again}");
    // Two voices at Stereo Field 100: hard left and right at +3 dB (√2), nothing across.
    let mut fx = chorus(rate, &with(&[(SPREAD, 30.0), (DRY, 0.0), (WET, 100.0)]));
    let out = run(&mut fx, 0.5, 0.0, n);
    assert!((f64::from(out[0][n - 1]) - 0.5 * 2f64.sqrt()).abs() < 1e-6);
    assert_eq!(out[1][n - 1], 0.0);
    // The ±1 clip of the line: a steady 2 reads back as 1.
    let mut fx = chorus(rate, &with(&[one[0], one[1], (DRY, 0.0), (WET, 100.0)]));
    let out = run(&mut fx, 2.0, -3.0, n);
    assert_eq!((out[0][n - 1], out[1][n - 1]), (1.0, -1.0));
    // Average: both lines get the mean, and the dry sound is the mean too.
    let mut fx = chorus(rate, &with(&[one[0], one[1], (AVERAGE, 1.0)]));
    let out = run(&mut fx, 0.5, 0.1, n);
    assert!((out[0][n - 1] - 0.3).abs() < 1e-6 && (out[1][n - 1] - 0.3).abs() < 1e-6);
    // Modulation Depth: the level wanders within ±3·depth dB, and does wander.
    let mut fx = chorus(
        rate,
        &with(&[
            one[0],
            one[1],
            (DRY, 0.0),
            (WET, 100.0),
            (MOD_DEPTH, 6.0),
            (MOD_RATE, 5.0),
        ]),
    );
    let out = run(&mut fx, 0.5, 0.5, 4 * n);
    let db: Vec<f64> = out[0][n..]
        .iter()
        .map(|y| 20.0 * (f64::from(*y) / 0.5).log10())
        .collect();
    let (lo, hi) = db
        .iter()
        .fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(lo >= -18.01 && hi <= 18.01 && hi - lo > 2.0, "{lo} .. {hi}");
}

#[test]
fn the_pan_law_and_the_binaural_cues() {
    let rate = 44_100.0;
    // Three voices, the field at 100 %: the edges hard over at √2, the middle at unity.
    let p = pans(&with(&[(VOICES, 3.0)]), rate);
    let s = 2f64.sqrt();
    assert!((p[0].gains[0] - s).abs() < 1e-12 && p[0].gains[1].abs() < 1e-12);
    assert!((p[1].gains[0] - 1.0).abs() < 1e-12 && (p[1].gains[1] - 1.0).abs() < 1e-12);
    assert!(p[2].gains[0].abs() < 1e-12 && (p[2].gains[1] - s).abs() < 1e-12);
    // At 0 % every voice in the middle at unity.
    let p = pans(&with(&[(VOICES, 4.0), (FIELD, 0.0)]), rate);
    for pan in &p[..4] {
        assert!((pan.gains[0] - 1.0).abs() < 1e-12 && (pan.gains[1] - 1.0).abs() < 1e-12);
    }
    // At 50 % the edges (u = 2/3) at k = 0.5^(ln(2/3)/ln 0.5): their pan angle.
    let p = pans(&with(&[(VOICES, 2.0), (FIELD, 50.0)]), rate);
    let k = 0.5f64.powf((2.0f64 / 3.0).ln() / 0.5f64.ln());
    let phi = PI / 4.0 * (1.0 - k);
    assert!((p[0].gains[0] - phi.cos() * s).abs() < 1e-12);
    assert!((p[1].gains[1] - phi.cos() * s).abs() < 1e-12);
    // Binaural cues: a floor of 0.17 and the far ear 1 ms late (44 samples), the right for the
    // left-hand voice and the left for the right-hand one; nothing in the middle.
    let p = pans(&with(&[(VOICES, 3.0), (BINAURAL, 1.0)]), rate);
    assert!((p[0].gains[1] - 0.17).abs() < 1e-12 && (p[0].gains[0] - s).abs() < 1e-12);
    assert_eq!(
        (p[0].delays, p[1].delays, p[2].delays),
        ([0, 44], [0, 0], [44, 0])
    );
    // The editor's layout is what a start draws.
    let v = with(&[(VOICES, 4.0), (SPREAD, 50.0)]);
    let fx = chorus(48_000.0, &v);
    let shown = layout(48_000.0, &v);
    assert_eq!(shown.len(), 4);
    for (k, voice) in shown.iter().enumerate() {
        assert!((voice.offset_ms - fx.bank.offsets[k] / 48.0).abs() < 1e-9);
    }
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100;
    let audio = [noise(n, 4, 0.5), noise(n, 5, 0.5)];
    for (name, v) in &varied() {
        let s = settings(v);
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
    assert_eq!(PARAMS[VOICES].display(10.0), "10");
    assert_eq!(PARAMS[HIGHEST_QUALITY].display(1.0), "On");
    assert_eq!(PARAMS[DELAY_TIME].display(31.4), "31.40 ms");
    // What a start copies is made on the main thread from these: no lanes; nor for the
    // switches. Dry and Wet have them.
    for i in PREPARED_BY
        .into_iter()
        .chain([AVERAGE, BINAURAL, HIGHEST_QUALITY])
    {
        assert!(!PARAMS[i].automatable, "{}", PARAMS[i].name);
    }
    assert!(PARAMS[DRY].automatable && PARAMS[WET].automatable);
}

/// The cost per second of stereo audio in release, the worst 64-sample block, and a start's
/// reseed (`cargo test --release -p daw-core chorus_cost -- --ignored --nocapture`).
#[test]
#[ignore]
fn chorus_cost() {
    let rate = 48_000.0;
    let seconds = 20;
    let n = 48_000 * seconds;
    let audio = [noise(n, 6, 0.5), noise(n, 7, 0.5)];
    let sixteen_hq = with(&[(VOICES, 16.0), (HIGHEST_QUALITY, 1.0), (SPREAD, 60.0)]);
    let sixteen = with(&[(VOICES, 16.0), (SPREAD, 60.0)]);
    for (name, v) in [
        ("defaults", PARAMS.map(|p| p.default)),
        ("10 voices", with(&[(VOICES, 10.0), (SPREAD, 40.0)])),
        (
            "4 voices at Highest Quality",
            with(&[(VOICES, 4.0), (HIGHEST_QUALITY, 1.0)]),
        ),
        ("16 voices", sixteen),
        ("16 voices at Highest Quality", sixteen_hq),
    ] {
        // A start whose settings changed: the main thread makes the bank (timed: the worst case
        // is 16 voices at 0.1 Hz), the audio thread copies it.
        let mut fx = chorus(rate, &with(&[(VOICES, 1.0)]));
        let t = std::time::Instant::now();
        let mut fresh = Some((PREPARES.make)(rate, &v));
        let made = t.elapsed().as_secs_f64();
        let mut spent = None;
        fx.take_prepared(&mut fresh, &mut spent);
        for (i, x) in v.iter().enumerate() {
            fx.set(i, *x);
        }
        fx.start();
        let block = 64;
        let mut io: Vec<Vec<f32>> = vec![vec![0.0; block]; CHANNELS];
        let mut worst = 0.0f64;
        let mut times = Vec::with_capacity(n / block);
        let start = std::time::Instant::now();
        let mut first = 0.0;
        for from in (0..n).step_by(block) {
            for (c, buf) in io.iter_mut().enumerate() {
                buf.copy_from_slice(&audio[c][from..from + block]);
            }
            let t = std::time::Instant::now();
            fx.process(&mut io, 0, block);
            let e = t.elapsed().as_secs_f64();
            if from == 0 {
                first = e;
            } else {
                worst = worst.max(e);
                times.push(e);
            }
        }
        let total = start.elapsed().as_secs_f64();
        // A start with the settings unchanged: the warmed-up bank copied.
        fx.start();
        let t = std::time::Instant::now();
        fx.process(&mut io, 0, block);
        let copied = t.elapsed().as_secs_f64();
        times.sort_by(f64::total_cmp);
        println!(
            "Chorus, {name}: {:.2} ms per second of stereo audio ({:.3} % of a core), the worst \
             64-sample block {:.4} ms (99.9th percentile {:.4}) of {:.3}; the start state made on \
             the main thread in {:.3} ms; the first block after a start with new settings \
             {:.4} ms, with the same settings {:.4} ms",
            total * 1000.0 / seconds as f64,
            total * 100.0 / seconds as f64,
            worst * 1000.0,
            times[times.len() * 999 / 1000] * 1000.0,
            64.0 / rate * 1000.0,
            made * 1000.0,
            first * 1000.0,
            copied * 1000.0,
        );
    }
}

#[test]
fn the_level_noise_is_stronger_at_slower_rates() {
    // 4.2 dB an octave more makes up for the narrower band: the level noise's
    // RMS is about 0.85 at 0.1 Hz, 0.6 at 1 Hz and 0.31 at 30 Hz (at 48 kHz; 1.5 % less at
    // 44.1 kHz), so Modulation Depth swings a slow wobble further.
    let mut last = f64::MAX;
    for (hz, lo, hi) in [
        (0.1, 0.7, 1.0),
        (1.0, 0.5, 0.7),
        (5.0, 0.38, 0.5),
        (30.0, 0.26, 0.36),
    ] {
        let mut fx = chorus(
            48_000.0,
            &with(&[(VOICES, 16.0), (MOD_RATE, hz), (SPREAD, 0.0)]),
        );
        let design = fx.level_design;
        let bank = &mut fx.bank;
        let mut sum = 0.0;
        let steps = 40_000;
        for _ in 0..steps {
            for noise in &mut bank.level {
                noise.step(&mut bank.generator, &design);
                sum += noise.history[3].powi(2);
            }
        }
        let rms = (sum / (steps * MAX_VOICES) as f64).sqrt();
        assert!((lo..hi).contains(&rms) && rms < last, "{hz} Hz: {rms}");
        last = rms;
    }
}
