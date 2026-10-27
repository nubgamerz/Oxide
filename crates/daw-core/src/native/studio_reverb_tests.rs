//! The Room Reverb's tests (`studio_reverb.rs`).

use super::super::render;
use super::*;
use rustfft::{num_complex::Complex, FftPlanner};
use std::collections::BTreeMap;

fn settings(pairs: &[(&str, f64)]) -> BTreeMap<String, f64> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

fn plain(pairs: &[(&str, f64)]) -> [f64; COUNT] {
    let mut v = PARAMS.map(|p| p.default);
    for (name, value) in pairs {
        v[PARAMS.iter().position(|p| p.name == *name).unwrap()] = *value;
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

/// `n` samples with a 1 at sample 0.
fn impulse(n: usize) -> Vec<f32> {
    let mut v = vec![0.0; n];
    v[0] = 1.0;
    v
}

/// The wet signal alone, each channel its own (no cross-mix), the plainest network: Dry 0, Wet
/// 100, Width 100, Diffusion 0 (the last allpass alone), Damping 0, Early Reflections 0.
const PLAIN_WET: [(&str, f64); 6] = [
    ("Dry", 0.0),
    ("Wet", 100.0),
    ("Width", 100.0),
    ("Diffusion", 0.0),
    ("Damping", 0.0),
    ("Early Reflections", 0.0),
];

/// The defaults with `pairs` (index, plain value) changed.
fn by_index(pairs: &[(usize, f64)]) -> [f64; COUNT] {
    let mut v = PARAMS.map(|p| p.default);
    for &(i, x) in pairs {
        v[i] = x;
    }
    v
}

fn with(base: &[(&'static str, f64)], extra: &[(&'static str, f64)]) -> Vec<(&'static str, f64)> {
    let mut v = base.to_vec();
    v.extend_from_slice(extra);
    v
}

#[test]
fn the_delays_have_the_lengths_the_formulas_give_at_44_1_and_48_khz() {
    // Worked by hand from the study's formula at the defaults (Room 70: s = 1.6; ER Delay 50:
    // s = 2): 50 ms / 1.6 = 31.25 ms = 1378.125 samples at 44.1 kHz, 1500 at 48; the right
    // channel's 31.78 ms = 1401.5 and 1525.4; the last comb 45.279 ms = 1996.8 and 2173.4; the
    // first allpass 12.355 ms = 544.9 and 593.1, the last 5 ms = 220.5 (half up: 221) and 240.
    for (rate, comb_l, comb_r, last_comb, ap_first, ap_last) in [
        (44_100.0, 1378, 1401, 1997, 545, 221),
        (48_000.0, 1500, 1525, 2173, 593, 240),
    ] {
        let left = comb_lengths(rate, 70.0, false);
        let right = comb_lengths(rate, 70.0, true);
        let aps = allpass_lengths(rate, 50.0, false);
        println!("{rate} Hz: combs {left:?} / {right:?}, allpasses {aps:?}");
        assert_eq!((left[0], right[0], left[7]), (comb_l, comb_r, last_comb));
        assert_eq!((aps[0], aps[3]), (ap_first, ap_last));
        // Every length is its time at the rate within half a sample.
        for (k, &ms) in COMB_MS.iter().enumerate() {
            for (odd, l) in [(false, left[k]), (true, right[k])] {
                let exact = (f64::from(ms) / 1.6 + if odd { 0.53 } else { 0.0 }) * rate / 1000.0;
                assert!((l as f64 - exact).abs() <= 0.5 + 1e-3, "{l} vs {exact}");
            }
        }
        // Room Size 1 and 100, ER Delay 0 and 100: the size factors 2.98, 1, 3 and 1.
        let small = comb_lengths(rate, 1.0, false);
        let large = comb_lengths(rate, 100.0, false);
        assert_eq!(large[0], (50.0 * rate / 1000.0 + 0.5) as usize);
        assert_eq!(small[0], (50.0 / 2.98 * rate / 1000.0 + 0.5) as usize);
        assert_eq!(
            allpass_lengths(rate, 100.0, false)[3],
            (10.0 * rate / 1000.0) as usize
        );
        assert_eq!(
            allpass_lengths(rate, 0.0, false)[3],
            (10.0 / 3.0 * rate / 1000.0 + 0.5) as usize
        );

        // The network itself at the defaults: an impulse into a channel's combs comes out of
        // each at its own length first (their sum's first eight echoes)...
        let mut fx = StudioReverb::new(rate);
        let g = fx.gains;
        for (c, odd) in [(0, false), (1, true)] {
            let want = comb_lengths(rate, 70.0, odd);
            let channel = &mut fx.channels[c];
            let mut echoes = Vec::new();
            for n in 0..want[7] + 1 {
                let v = if n == 0 { 1.0 } else { 0.0 };
                let s: f32 = channel.combs.iter_mut().map(|comb| comb.tick(v, &g)).sum();
                if s != 0.0 {
                    echoes.push(n);
                }
            }
            assert_eq!(echoes, want, "channel {c}");
            // ...and each allpass answers at once (−g) and again at its length, whole (1: a
            // true allpass would give 1 − g² there, and keep ringing; the reference's recurrence
            // writes g·d + u back, so the echo returns at g, g², ... times the first).
            let lengths = allpass_lengths(rate, 50.0, odd);
            for (line, &m) in channel.allpasses.iter_mut().zip(&lengths) {
                let out: Vec<f32> = (0..m + 2)
                    .map(|n| allpass(line, if n == 0 { 1.0 } else { 0.0 }, g.allpass))
                    .collect();
                let at: Vec<usize> = (0..out.len()).filter(|&n| out[n] != 0.0).collect();
                assert_eq!(at, [0, m]);
                assert_eq!(out[0], -g.allpass);
                assert_eq!(out[m], 1.0);
            }
        }

        // The whole effect on an impulse: nothing until the first comb's length (no latency,
        // no pre-delay), the right channel 0.53 ms later; the first echo exactly the impulse
        // through both filters' first coefficient, ×0.015, through the allpasses' direct
        // paths (−g⁴ in all, g = 0.3) and ×Wet/20 = 5.
        let s = settings(&PLAIN_WET);
        let audio = [impulse(3000), impulse(3000)];
        let out = render(&KIND, rate, &s, &audio, 512).unwrap();
        let v = plain(&PLAIN_WET);
        let lp = butterworth2(rate, v[HIGH_CUT], false);
        let hp = butterworth2(rate, v[LOW_CUT], true);
        let input = (hp.b0 * f64::from((lp.b0 * 1.0) as f32)) as f32 * INPUT_GAIN;
        let gap = 0.3f32;
        let a4 = -(gap * -(gap * -(gap * -(gap * input))));
        for (c, odd) in [(0, false), (1, true)] {
            let first = out[c].iter().position(|x| *x != 0.0).unwrap();
            let l = comb_lengths(rate, 70.0, odd)[0];
            println!(
                "{rate} Hz, channel {c}: the first echo at {first} ({:.3} ms), {:.6e}",
                first as f64 * 1000.0 / rate,
                out[c][first]
            );
            assert_eq!(first, l);
            assert_eq!(out[c][first], 5.0 * a4);
        }
    }
}

/// The reverberation time (s) of `x` at `rate`: Schroeder's backward integral of its energy,
/// its fall from −5 to −35 dB doubled (T30).
fn t30(x: &[f32], rate: f64) -> f64 {
    let mut tail = 0.0f64;
    let mut integral: Vec<f64> = x
        .iter()
        .rev()
        .map(|s| {
            tail += f64::from(*s).powi(2);
            tail
        })
        .collect();
    integral.reverse();
    let total = integral[0];
    let at = |db: f64| {
        integral
            .iter()
            .position(|e| 10.0 * (e / total).log10() <= db)
            .unwrap()
    };
    2.0 * (at(-35.0) - at(-5.0)) as f64 / rate
}

/// The same for the combs' model: each comb's echoes, every `l` samples, falling by the loop
/// gain each time (equal shares, as their sum is fed), integrated backwards.
fn model_t30(rate: f64, values: &[f64; COUNT]) -> f64 {
    let g = f64::from(feedback(values[DECAY], values[ROOM]));
    let lengths = comb_lengths(rate, values[ROOM], false);
    let n = (rate * 60.0) as usize;
    let mut energy = vec![0.0f64; n];
    for &l in &lengths {
        let mut a = 1.0f64;
        let mut k = l;
        while k < n && a > 1e-30 {
            energy[k] += a;
            a *= g * g;
            k += l;
        }
    }
    let x: Vec<f32> = energy.iter().map(|e| e.sqrt() as f32).collect();
    t30(&x, rate)
}

#[test]
fn the_decay_follows_decay_and_room_size_as_the_feedback_law_says() {
    // The impulse's wet signal with Damping 0 and the cuts wide open: its T30 lies between the
    // shortest and the longest comb's RT60 from the feedback law (tolerance: 5 % beyond either),
    // within 6 % of the combs' model (their echoes summed, integrated the same way), and rises
    // with Decay and with Room Size.
    let rate = 48_000.0;
    let open = [
        ("Low Frequency Cut", 20.0),
        ("High Frequency Cut", 20_000.0),
    ];
    let base = with(&PLAIN_WET, &open);
    let mut last: Option<(f64, f64)> = None;
    for (room, decay) in [
        (70.0, 1000.0),
        (70.0, 2500.0),
        (70.0, 5000.0),
        (70.0, 10_000.0),
        (20.0, 2500.0),
        (100.0, 2500.0),
    ] {
        let pairs = with(&base, &[("Room Size", room), ("Decay", decay)]);
        let v = plain(&pairs);
        let (short, long) = rt60(rate, &v, 0.0);
        let seconds = (2.5 * long + 1.0).ceil() as usize;
        let audio = [impulse(48_000 * seconds), impulse(48_000 * seconds)];
        let out = render(&KIND, rate, &settings(&pairs), &audio, 4096).unwrap();
        let measured = t30(&out[0], rate);
        let model = model_t30(rate, &v);
        let g = feedback(decay, room);
        println!(
            "Room {room}, Decay {decay} ms: g {g:.4}, RT60 measured {measured:.3} s, the combs' \
             model {model:.3}, the combs alone {short:.3} to {long:.3}"
        );
        assert!(
            measured > 0.95 * short && measured < 1.05 * long,
            "{measured} outside {short}..{long}"
        );
        assert!(
            (measured / model - 1.0).abs() < 0.06,
            "{measured} vs {model}"
        );
        if let Some((r, t)) = last {
            if r == room && decay > 1000.0 {
                assert!(measured > t, "Decay {decay}: {measured} after {t}");
            }
        }
        last = Some((room, measured));
    }
    // Room Size at the same Decay: longer combs (and a lower feedback) give a longer tail.
    let at = |room: f64| {
        let v = plain(&with(&base, &[("Room Size", room)]));
        rt60(rate, &v, 0.0).1
    };
    assert!(at(20.0) < at(70.0) && at(70.0) < at(100.0));
    // The cap: Decay past about 7770 ms at Room 70 changes nothing.
    assert_eq!(feedback(7800.0, 70.0), 0.98);
    assert_eq!(feedback(10_000.0, 70.0), 0.98);
    assert!(feedback(7700.0, 70.0) < 0.98);
}

/// The energy of `x` in each octave band centred on `centres` (Hz), from its spectrum.
fn bands(x: &[f32], rate: f64, centres: &[f64]) -> Vec<f64> {
    let n = x.len().next_power_of_two() * 2;
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .map(|s| Complex::new(f64::from(*s), 0.0))
        .chain(std::iter::repeat(Complex::new(0.0, 0.0)))
        .take(n)
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    centres
        .iter()
        .map(|&c| {
            let (lo, hi) = (c / 2f64.sqrt(), c * 2f64.sqrt());
            (0..n / 2)
                .filter(|&k| (lo..hi).contains(&(k as f64 * rate / n as f64)))
                .map(|k| buf[k].norm_sqr())
                .sum()
        })
        .collect()
}

/// The power of each band of `x` weighted by `weight(hz)` (a power gain), over its own power:
/// what the band's energy becomes through a filter of that power response.
fn weighted(x: &[f32], rate: f64, centres: &[f64], weight: impl Fn(f64) -> f64) -> Vec<f64> {
    let n = x.len().next_power_of_two() * 2;
    let mut buf: Vec<Complex<f64>> = x
        .iter()
        .map(|s| Complex::new(f64::from(*s), 0.0))
        .chain(std::iter::repeat(Complex::new(0.0, 0.0)))
        .take(n)
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    centres
        .iter()
        .map(|&c| {
            let (lo, hi) = (c / 2f64.sqrt(), c * 2f64.sqrt());
            let (mut a, mut b) = (0.0, 0.0);
            for (k, bin) in buf[..n / 2].iter().enumerate() {
                let f = k as f64 * rate / n as f64;
                if (lo..hi).contains(&f) {
                    a += bin.norm_sqr() * weight(f);
                    b += bin.norm_sqr();
                }
            }
            a / b
        })
        .collect()
}

const OCTAVES: [f64; 9] = [
    63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16_000.0,
];

#[test]
fn the_cuts_and_the_damping_shape_the_tails_spectrum() {
    let rate = 48_000.0;
    let n = 48_000 * 4;
    let base = with(&PLAIN_WET, &[("Decay", 1000.0)]);
    let tail = |extra: &[(&'static str, f64)]| -> Vec<f32> {
        let s = settings(&with(&base, extra));
        render(&KIND, rate, &s, &[impulse(n), impulse(n)], 4096)
            .unwrap()
            .swap_remove(0)
    };
    let db = |x: f64| 10.0 * x.log10();
    // The cuts filter the input only, so the tail's spectrum is the open one times the filters'
    // power response, band by band (weighted by the open tail's own spectrum within the band).
    let open_pairs = [
        ("Low Frequency Cut", 20.0),
        ("High Frequency Cut", 20_000.0),
    ];
    let open = tail(&open_pairs);
    let open_bands = bands(&open, rate, &OCTAVES);
    for (low, high) in [(880.0, 20_000.0), (20.0, 4000.0), (300.0, 6000.0)] {
        let cut = tail(&[("Low Frequency Cut", low), ("High Frequency Cut", high)]);
        let measured = bands(&cut, rate, &OCTAVES);
        let (hp, lp) = (
            butterworth2(rate, low, true),
            butterworth2(rate, high, false),
        );
        let (hp0, lp0) = (
            butterworth2(rate, 20.0, true),
            butterworth2(rate, 20_000.0, false),
        );
        let expected = weighted(&open, rate, &OCTAVES, |f| {
            (hp.magnitude(f, rate) * lp.magnitude(f, rate)
                / (hp0.magnitude(f, rate) * lp0.magnitude(f, rate)))
            .powi(2)
        });
        for (k, centre) in OCTAVES.iter().enumerate() {
            let m = db(measured[k] / open_bands[k]);
            let e = db(expected[k]);
            println!("cuts {low}/{high} Hz, {centre} Hz band: {m:+.3} dB, the filters {e:+.3}");
            assert!((m - e).abs() < 0.05, "{centre} Hz: {m} vs {e}");
        }
    }
    // Damping: the combs' low-pass lowers their loop gain g·|H(f)| where H = d2/(1 − d1·z⁻¹),
    // so a comb's energy at f falls from 1/(1 − g²) to 1/(1 − g²|H|²) (its resonances
    // averaged): nothing at the bottom, more the higher the band (within 1 dB).
    let undamped = tail(&[("Damping", 0.0)]);
    let before = bands(&undamped, rate, &OCTAVES);
    for damping in [50.0, 100.0] {
        let after = bands(&tail(&[("Damping", damping)]), rate, &OCTAVES);
        let g = f64::from(feedback(1000.0, 70.0));
        let d1 = 0.4 * damping / 100.0;
        let mut last = 1.0;
        for (k, centre) in OCTAVES.iter().enumerate() {
            let m = db(after[k] / before[k]);
            let e = db(weighted(&undamped, rate, &[*centre], |f| {
                let h = damping_gain(rate, d1, f);
                (1.0 - g * g) / (1.0 - g * g * h * h)
            })[0]);
            println!("Damping {damping} %, {centre} Hz band: {m:+.3} dB, predicted {e:+.3}");
            assert!((m - e).abs() < 1.0, "{centre} Hz: {m} vs {e}");
            assert!(m < last + 0.3, "{centre} Hz: {m} after {last}");
            last = m;
        }
    }
}

#[test]
fn width_0_is_one_mono_wet_signal_and_the_levels_are_exact() {
    let rate = 48_000.0;
    let n = 24_000;
    let audio = [noise(n, 1, 0.5), noise(n, 2, 0.5)];
    let run = |pairs: &[(&str, f64)]| render(&KIND, rate, &settings(pairs), &audio, 512).unwrap();
    // Width 0: wet1 = wet2, so both outputs are the same sum of the two channels' reverbs.
    let mono = run(&[("Width", 0.0), ("Dry", 0.0)]);
    assert_eq!(mono[0], mono[1]);
    assert!(mono[0].iter().any(|x| *x != 0.0));
    let wide = run(&[("Width", 100.0), ("Dry", 0.0)]);
    assert_ne!(wide[0], wide[1]);
    // Wet 0: the dry input times Dry/100, bit for bit (the dry path is unfiltered).
    for dry in [100.0, 75.0, 50.0] {
        let out = run(&[("Wet", 0.0), ("Dry", dry)]);
        for (o, x) in out.iter().zip(&audio) {
            let want: Vec<f32> = x.iter().map(|s| (dry / 100.0) as f32 * s).collect();
            assert_eq!(*o, want, "Dry {dry}");
        }
    }
    // Wet 50 is exactly half of Wet 100 (Wet/20: 2.5 and 5); and dry plus wet is the wet
    // alone plus the dry, bit for bit.
    let full = run(&[("Wet", 100.0), ("Dry", 0.0)]);
    let half = run(&[("Wet", 50.0), ("Dry", 0.0)]);
    for (f, h) in full.iter().zip(&half) {
        assert!(f.iter().zip(h).all(|(a, b)| *a == 2.0 * b));
    }
    let wet = run(&[("Dry", 0.0)]);
    let both = run(&[]);
    for ((b, w), x) in both.iter().zip(&wet).zip(&audio) {
        let sum: Vec<f32> = w.iter().zip(x).map(|(w, x)| w + 0.75 * x).collect();
        assert_eq!(*b, sum);
    }
}

#[test]
fn the_same_at_any_block_size() {
    let rate = 44_100.0;
    let n = 44_100 * 2;
    let mut drums = noise(n, 3, 0.4);
    for (i, s) in drums.iter_mut().enumerate() {
        *s *= (-((i % 11_025) as f32) / 900.0).exp();
    }
    let audio = [drums.clone(), drums.iter().rev().copied().collect()];
    for (name, v) in [
        ("defaults", by_index(&[])),
        (
            "a small, bright room",
            by_index(&[(ROOM, 30.0), (DECAY, 900.0), (DAMPING, 20.0), (WIDTH, 60.0)]),
        ),
        (
            "a large, dark, wide space",
            by_index(&[
                (ROOM, 100.0),
                (DECAY, 6000.0),
                (HIGH_CUT, 4000.0),
                (WIDTH, 100.0),
            ]),
        ),
    ] {
        let s: BTreeMap<String, f64> = PARAMS
            .iter()
            .zip(&v)
            .map(|(p, x)| (p.name.to_string(), *x))
            .collect();
        let a = render(&KIND, rate, &s, &audio, 512).unwrap();
        for block in [77, 1, 4096] {
            assert_eq!(a, render(&KIND, rate, &s, &audio, block).unwrap(), "{name}");
        }
    }
}

#[test]
fn a_room_size_change_empties_the_tail_and_other_changes_keep_it() {
    let rate = 48_000.0;
    let ring = |fx: &mut StudioReverb| {
        let mut io = vec![noise(4800, 4, 0.5), noise(4800, 5, 0.5)];
        fx.process(&mut io, 0, 4800);
    };
    let silence = |fx: &mut StudioReverb| -> Vec<Vec<f32>> {
        let mut io = vec![vec![0.0; 4800]; CHANNELS];
        fx.process(&mut io, 0, 4800);
        io
    };
    let quiet = |io: &[Vec<f32>]| io.iter().all(|c| c.iter().all(|x| *x == 0.0));
    for (index, value, empties) in [
        (ROOM, 50.0, true),
        (EARLY_DELAY, 70.0, true),
        (WIDTH, 80.0, false),
        (DECAY, 4000.0, false),
        (HIGH_CUT, 5000.0, false),
        (LOW_CUT, 200.0, false),
    ] {
        let mut fx = StudioReverb::new(rate);
        ring(&mut fx);
        // The same value again changes nothing (the reference ignores it too).
        fx.set(ROOM, 70.0);
        assert!(!quiet(&silence(&mut fx)));
        fx.set(index, value);
        let after = silence(&mut fx);
        assert_eq!(quiet(&after), empties, "{}", PARAMS[index].name);
    }
    // The new lengths are in use: an impulse after Room Size 100 comes back after 50 ms.
    let mut fx = StudioReverb::new(rate);
    ring(&mut fx);
    fx.set(ROOM, 100.0);
    fx.set(DRY, 0.0);
    let mut io = vec![impulse(4800), impulse(4800)];
    fx.process(&mut io, 0, 4800);
    assert_eq!(io[0].iter().position(|x| *x != 0.0), Some(2400));
    // A transport start empties it too.
    ring(&mut fx);
    fx.start();
    assert!(quiet(&silence(&mut fx)));
}

#[test]
fn the_parameters_show_and_the_feedback_grows_with_decay() {
    for p in &PARAMS {
        assert!((p.min..=p.max).contains(&p.default), "{}", p.name);
        assert!(
            (p.plain(p.normalized(p.default)) - p.default).abs() < 1e-9,
            "{}",
            p.name
        );
    }
    // Longer decays feed back more, up to the cap; a bigger room needs less for the same decay.
    let mut last = 0.0;
    for decay in [500.0, 1000.0, 2500.0, 5000.0, 10_000.0] {
        let g = feedback(decay, 70.0);
        assert!(
            g > last && f64::from(g) <= MAX_FEEDBACK + 1e-6,
            "{decay}: {g}"
        );
        last = g;
    }
    assert!(feedback(2500.0, 100.0) < feedback(2500.0, 30.0));
    // The floor: Decay 200 at Room 100.
    assert!((feedback(200.0, 100.0) - 0.1567).abs() < 1e-3);
    assert_eq!(PARAMS[LOW_CUT].display(880.0), "880 Hz");
    assert_eq!(PARAMS[HIGH_CUT].display(13_800.0), "13.80 kHz");
    assert_eq!(PARAMS[DECAY].display(2500.0), "2500 ms");
    assert!(!PARAMS[ROOM].automatable && !PARAMS[EARLY_DELAY].automatable);
}

/// The cost per second of stereo audio in release (`cargo test --release -p daw-core
/// studio_reverb_cost -- --ignored --nocapture`): on noise, and on an impulse followed by long
/// silence (the tail dying away through the smallest values).
#[test]
#[ignore]
fn studio_reverb_cost() {
    let rate = 48_000.0;
    let seconds = 30;
    let n = 48_000 * seconds;
    for (name, audio) in [
        ("noise", [noise(n, 6, 0.5), noise(n, 7, 0.5)]),
        ("an impulse, then silence", [impulse(n), impulse(n)]),
    ] {
        for (setting, v) in [
            ("defaults", by_index(&[])),
            ("a long decay", by_index(&[(ROOM, 100.0), (DECAY, 8000.0)])),
        ] {
            let mut fx = StudioReverb::new(rate);
            for (i, x) in v.iter().enumerate() {
                fx.set(i, *x);
            }
            let mut io: Vec<Vec<f32>> = vec![vec![0.0; 480]; CHANNELS];
            let mut worst = 0.0f64;
            let mut per_second = Vec::new();
            let start = std::time::Instant::now();
            let mut second = std::time::Instant::now();
            for (b, from) in (0..n).step_by(480).enumerate() {
                for (c, buf) in io.iter_mut().enumerate() {
                    buf.copy_from_slice(&audio[c][from..from + 480]);
                }
                let t = std::time::Instant::now();
                fx.process(&mut io, 0, 480);
                worst = worst.max(t.elapsed().as_secs_f64());
                if (b + 1) % 100 == 0 {
                    per_second.push(second.elapsed().as_secs_f64() * 1000.0);
                    second = std::time::Instant::now();
                }
            }
            let total = start.elapsed().as_secs_f64();
            let slowest = per_second.iter().copied().fold(0.0, f64::max);
            println!(
                "{name}, {setting}: {:.2} ms per second of stereo audio ({:.3} % of real \
                 time), the slowest second {slowest:.2} ms, the worst 480-sample block {:.3} ms",
                total * 1000.0 / seconds as f64,
                total * 100.0 / seconds as f64,
                worst * 1000.0
            );
        }
    }
}
