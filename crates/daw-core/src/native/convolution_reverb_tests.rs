//! The Convolution Reverb's tests: the impulse's shaping as studied, the effect playing it, and
//! a new impulse taking over while playing.

use super::*;
use crate::graph::{Processor, Transport};
use crate::native::{find, render, render_with_file, Native, NativeInsert};
use crate::plugins::InsertFlags;
use crate::wav;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const RATE: f64 = 48_000.0;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daw-convolution-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

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

/// A decaying noise burst, a room's impulse in miniature.
fn room(n: usize, seed: u64, decay: f32) -> Vec<f32> {
    noise(n, seed)
        .into_iter()
        .enumerate()
        .map(|(i, x)| 0.5 * x * (-(i as f32) / decay).exp())
        .collect()
}

fn write(path: &std::path::Path, rate: u32, channels: Vec<Vec<f32>>) -> PathBuf {
    wav::write_f32(
        path,
        &wav::Audio {
            sample_rate: rate,
            channels,
        },
    )
    .unwrap();
    path.to_path_buf()
}

/// Plain values: the defaults, with `pairs` set.
fn values(pairs: &[(usize, f64)]) -> Vec<f64> {
    let mut v: Vec<f64> = KIND.params.iter().map(|p| p.default).collect();
    for &(i, x) in pairs {
        v[i] = x;
    }
    v
}

/// Settings by name for `render`.
fn named(pairs: &[(usize, f64)]) -> BTreeMap<String, f64> {
    pairs
        .iter()
        .map(|&(i, v)| (KIND.params[i].name.to_string(), v))
        .collect()
}

/// The neutral shaping: Damping 70/70, Width 100, Pre-Delay 0, Room 100.
const NEUTRAL_SHAPE: [(usize, f64); 2] = [(DAMPING_LF, 70.0), (DAMPING_HF, 70.0)];

fn null_db(ours: &[f32], reference: &[f32]) -> f64 {
    let (mut d, mut r) = (0.0f64, 0.0f64);
    for (a, b) in ours.iter().zip(reference) {
        d += (f64::from(*a) - f64::from(*b)).powi(2);
        r += f64::from(*b).powi(2);
    }
    10.0 * (d / r).log10()
}

#[test]
fn the_kind_is_found_with_its_parameters() {
    let kind = find("convolution reverb").expect("registered");
    assert_eq!(kind.id, "convolution-reverb");
    let names: Vec<&str> = kind.params.iter().map(|p| p.name).collect();
    assert_eq!(
        names,
        [
            "Mix",
            "Room Size",
            "Damping LF",
            "Damping HF",
            "Pre-Delay",
            "Width",
            "Gain"
        ]
    );
    let defaults: Vec<f64> = kind.params.iter().map(|p| p.default).collect();
    assert_eq!(defaults, [100.0, 100.0, 70.0, 73.0, 0.0, 100.0, -3.0]);
    // Only Mix and Gain have lanes: the rest prepare the impulse again.
    let lanes: Vec<bool> = kind.params.iter().map(|p| p.automatable).collect();
    assert_eq!(lanes, [true, false, false, false, false, false, true]);
}

#[test]
fn room_size_keeps_round_n_r_over_0_8_with_a_half_cosine_fade() {
    let n = 10_000;
    let h = vec![vec![1.0f32; n]];
    for (room, kept) in [(100.0, n), (10.0, 1250), (55.0, 5625)] {
        let s = shape(
            RATE,
            &values(&[NEUTRAL_SHAPE[0], NEUTRAL_SHAPE[1], (ROOM, room)]),
            &h,
        );
        assert_eq!(s.kept, kept, "Room Size {room}");
        assert_eq!(s.channels[0].len(), kept);
        // A constant impulse shows the fade itself (the neutral damping's split is exact to
        // f32 rounding).
        for i in [0, kept / 3, kept / 2, kept - 1] {
            let w = 0.5 + 0.5 * (std::f64::consts::PI * i as f64 / (kept + 1) as f64).cos();
            let got = f64::from(s.channels[0][i]);
            assert!(
                (got - w).abs() < 1e-6,
                "Room {room}, sample {i}: {got} for {w}"
            );
        }
    }
    // Room Size 100 fades even the full length: the last sample is nearly gone.
    let s = shape(RATE, &values(&NEUTRAL_SHAPE), &h);
    assert!(s.channels[0][n - 1] < 1e-6);
    // Shorter than M: padded with silence.
    let short = shape(RATE, &values(&NEUTRAL_SHAPE), &[vec![1.0; 3]]);
    assert_eq!(short.kept, 3);
}

#[test]
fn pre_delay_puts_zeros_in_front() {
    let h = vec![room(4000, 1, 800.0)];
    let plain = shape(RATE, &values(&NEUTRAL_SHAPE), &h);
    let late = shape(
        RATE,
        &values(&[NEUTRAL_SHAPE[0], NEUTRAL_SHAPE[1], (PRE_DELAY, 50.0)]),
        &h,
    );
    // 50 ms at 48 kHz: 2400 samples, the impulse after them unchanged, and the gain too.
    assert_eq!(late.channels[0].len(), plain.channels[0].len() + 2400);
    assert!(late.channels[0][..2400].iter().all(|x| *x == 0.0));
    assert_eq!(late.channels[0][2400..], plain.channels[0][..]);
    assert_eq!(late.gain, plain.gain);
    // 19.4 ms at 44.1 kHz rounds to 856 samples.
    let odd = shape(
        44_100.0,
        &values(&[NEUTRAL_SHAPE[0], NEUTRAL_SHAPE[1], (PRE_DELAY, 19.4)]),
        &h,
    );
    assert_eq!(odd.channels[0].len() - plain.channels[0].len(), 856);
}

#[test]
fn width_cross_mixes_the_pair_in_place() {
    // A left-only impulse: what reaches the right is the cross-mix.
    let left = room(3000, 2, 600.0);
    let h = vec![left.clone(), vec![0.0; 3000]];
    let base = shape(RATE, &values(&NEUTRAL_SHAPE), &h);
    for (width, a, b) in [
        (100.0, 0.0, 1.0),
        (
            0.0,
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ),
        (300.0, -2.0 / 20f64.sqrt(), 4.0 / 20f64.sqrt()),
    ] {
        let s = shape(
            RATE,
            &values(&[NEUTRAL_SHAPE[0], NEUTRAL_SHAPE[1], (WIDTH, width)]),
            &h,
        );
        // Left ← A·0 + B·left; right ← A·(new left) + B·0 = A·B·left (before damping, which
        // is linear and the same on both).
        for i in [0, 10, 500] {
            let l = f64::from(base.channels[0][i]);
            let want = [b * l, a * b * l];
            for (c, want) in want.iter().enumerate() {
                let got = f64::from(s.channels[c][i]);
                assert!(
                    (got - want).abs() < 1e-5 * l.abs().max(1e-3),
                    "Width {width}, channel {c}, sample {i}: {got} for {want}"
                );
            }
        }
    }
}

#[test]
fn the_gain_is_half_over_the_mean_l1() {
    let h = vec![room(5000, 3, 900.0), room(5000, 4, 700.0)];
    let s = shape(RATE, &values(&[]), &h);
    let l1: f64 = s
        .channels
        .iter()
        .map(|c| c.iter().map(|x| f64::from(x.abs())).sum::<f64>())
        .sum::<f64>()
        / 2.0;
    assert!((s.gain - 0.5 / l1).abs() < 1e-12 * s.gain);
    // A single unit sample: k = 0.5.
    let unit = shape(RATE, &values(&NEUTRAL_SHAPE), &[vec![1.0]]);
    assert_eq!(unit.kept, 2);
    assert!(
        (unit.gain - 0.5 / f64::from(unit.channels[0].iter().map(|x| x.abs()).sum::<f32>())).abs()
            < 1e-12
    );
}

#[test]
fn damping_envelopes_rise_below_70_and_fall_above() {
    let m = 1000;
    assert!(envelope(0.7, m).iter().all(|e| *e == 1.0));
    // 10 %: a rise to 1/(1 − k), k = (0.6/0.71)^0.2, about +29.6 dB at the end.
    let rise = envelope(0.1, m);
    let k = (0.6f64 / 0.71).powf(0.2);
    assert!((f64::from(rise[0]) - 1.0).abs() < 1e-6);
    let end = 1.0 / (k * (-10.0 * 999.0 / 1000.0f64).exp() + 1.0 - k);
    assert!((f64::from(rise[m - 1]) - end).abs() < 1e-4 * end);
    assert!(
        (20.0 * end.log10() - 29.6).abs() < 0.3,
        "{}",
        20.0 * end.log10()
    );
    assert!(rise.windows(2).all(|w| w[1] >= w[0]));
    // 100 %: a decay from about 1, steep: under -100 dB well before the end.
    let fall = envelope(1.0, m);
    assert!(fall[0] > 0.9 && fall.windows(2).all(|w| w[1] <= w[0]));
    assert!(fall[m / 2] < 1e-5, "{}", fall[m / 2]);
    // 80 %: gentler than 100 % everywhere.
    let gentle = envelope(0.8, m);
    assert!(gentle.iter().zip(&fall).all(|(g, f)| g >= f));
}

#[test]
fn damping_splits_near_0_157_fs_and_weights_each_band_by_its_envelope() {
    // The low band of a long sine keeps 0.36/(1.16 − 0.8·cos ω) of it: 0.90 at 0.05·fs, half
    // at 0.157·fs, 0.18 at 0.45·fs.
    for f in [0.05, 0.157, 0.45] {
        let w = 2.0 * std::f64::consts::PI * f;
        let sine: Vec<f32> = (0..4000).map(|i| (w * i as f64).sin() as f32).collect();
        let low = low_band(&sine);
        let got = low[1500..2500].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        let want = 0.36 / (1.16 - 0.8 * w.cos());
        assert!(
            (f64::from(got) - want).abs() < 0.01,
            "{f}·fs: {got} for {want}"
        );
    }
    // The shaped impulse is E_L·low + E_H·(h − low) of the faded one.
    let h = vec![room(3000, 5, 1000.0)];
    let faded = shape(RATE, &values(&NEUTRAL_SHAPE), &h).channels[0].clone();
    let s = shape(RATE, &values(&[(DAMPING_LF, 20.0), (DAMPING_HF, 90.0)]), &h);
    let low = low_band(&faded);
    let (el, eh) = (envelope(0.2, 3000), envelope(0.9, 3000));
    let peak = faded.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    for i in [0, 1, 100, 1500, 2999] {
        let want = el[i] * low[i] + eh[i] * (faded[i] - low[i]);
        assert!(
            (s.channels[0][i] - want).abs() < 1e-5 * peak,
            "sample {i}: {} for {want}",
            s.channels[0][i]
        );
    }
    // Late in the tail Damping HF 90 has taken the highs: what is left is nearly all low band.
    let late = &s.channels[0][2500..];
    let lows = &low[2500..];
    let ratio = late.iter().map(|x| x * x).sum::<f32>()
        / lows
            .iter()
            .zip(&el[2500..])
            .map(|(x, e)| (x * e).powi(2))
            .sum::<f32>();
    assert!((ratio - 1.0).abs() < 0.01, "{ratio}");
}

#[test]
fn files_map_to_the_stereo_graph_and_the_engine_s_rate() {
    let dir = temp_dir("load");
    let a = room(4410, 6, 900.0);
    let b = room(4410, 7, 900.0);
    let stereo = write(&dir.join("stereo.wav"), 48_000, vec![a.clone(), b.clone()]);
    let loaded = load(&stereo, RATE).unwrap();
    assert_eq!(loaded.channels, vec![a.clone(), b.clone()]);
    assert_eq!((loaded.file_channels, loaded.file_rate), (2, 48_000));
    // Mono: one impulse, played on both sides.
    let mono = write(&dir.join("mono.wav"), 48_000, vec![a.clone()]);
    assert_eq!(load(&mono, RATE).unwrap().channels, vec![a.clone()]);
    // Four channels: averaged to one.
    let quad = write(
        &dir.join("quad.wav"),
        48_000,
        vec![a.clone(), b.clone(), a.clone(), b.clone()],
    );
    let avg = load(&quad, RATE).unwrap();
    assert_eq!(avg.channels.len(), 1);
    assert!((avg.channels[0][5] - (a[5] + b[5]) / 2.0).abs() < 1e-7);
    // 44.1 kHz to 48: resampled, 4410 samples become 4800.
    let cd = write(&dir.join("cd.wav"), 44_100, vec![a]);
    let at_48 = load(&cd, RATE).unwrap();
    assert_eq!(at_48.channels[0].len(), 4800);
    assert_eq!(at_48.file_rate, 44_100);
    // Too long, and not audio: refused with the reason.
    let long = write(&dir.join("long.wav"), 1000, vec![vec![0.1; 31_000]]);
    let e = load(&long, RATE).unwrap_err().to_string();
    assert!(e.contains("31.0 s") && e.contains("30 s"), "{e}");
    std::fs::write(dir.join("text.wav"), "not audio").unwrap();
    assert!(load(&dir.join("text.wav"), RATE).is_err());
}

#[test]
fn without_an_impulse_the_input_passes_unchanged() {
    let input = vec![noise(5000, 8), noise(5000, 9)];
    let out = render(
        &KIND,
        RATE,
        &named(&[(MIX, 60.0), (GAIN, 6.0)]),
        &input,
        333,
    )
    .unwrap();
    assert_eq!(out, input);
}

#[test]
fn an_impulse_through_the_effect_is_the_shaped_impulse_at_mix_and_gain() {
    let dir = temp_dir("through");
    let h = vec![room(6000, 10, 1500.0), room(6000, 11, 1000.0)];
    let file = write(&dir.join("room.wav"), 48_000, h.clone());
    // A unit impulse in: out comes the prepared impulse, times k·Mix·g.
    let mut unit = vec![vec![0.0f32; 12_000]; 2];
    unit[0][0] = 1.0;
    unit[1][0] = 1.0;
    for settings in [
        vec![(MIX, 100.0), (GAIN, 0.0)],
        vec![(MIX, 100.0), (GAIN, -3.0), (ROOM, 40.0), (WIDTH, 250.0)],
        vec![
            (MIX, 30.0),
            (GAIN, 6.0),
            (PRE_DELAY, 20.0),
            (DAMPING_LF, 20.0),
            (DAMPING_HF, 95.0),
        ],
    ] {
        let params = named(&settings);
        let out = render_with_file(&KIND, RATE, &params, None, Some(&file), &unit, 512).unwrap();
        let plain = values(&settings);
        let s = shape(RATE, &plain, &h);
        let (m, g) = (plain[MIX], 10f64.powf(plain[GAIN] / 20.0));
        for (c, (out, shaped)) in out.iter().zip(&s.channels).enumerate() {
            let want: Vec<f32> = (0..12_000)
                .map(|i| {
                    let wet = f64::from(shaped.get(i).copied().unwrap_or(0.0)) * s.gain;
                    let dry = if i == 0 { (100.0 - m) / 100.0 } else { 0.0 };
                    ((dry + wet * m) * g) as f32
                })
                .collect();
            let db = null_db(out, &want);
            println!("{settings:?}, channel {c}: {db:.1} dB");
            assert!(db < -100.0, "{settings:?}, channel {c}: {db:.1} dB");
        }
    }
}

#[test]
fn blocks_of_any_size_play_the_same() {
    let dir = temp_dir("blocks");
    let file = write(
        &dir.join("room.wav"),
        48_000,
        vec![room(20_000, 12, 4000.0), room(20_000, 13, 3000.0)],
    );
    let input = vec![noise(30_000, 14), noise(30_000, 15)];
    let params = named(&[(MIX, 70.0), (DAMPING_HF, 85.0)]);
    let reference = render_with_file(&KIND, RATE, &params, None, Some(&file), &input, 512).unwrap();
    for block in [77, 1, 4096] {
        let out = render_with_file(&KIND, RATE, &params, None, Some(&file), &input, block).unwrap();
        assert!(out == reference, "blocks of {block}");
    }
}

#[test]
fn the_impulse_is_saved_by_its_path_with_the_settings() {
    let a = Native::new(&KIND, RATE, None).unwrap();
    let mut a = a;
    a.set_file(PathBuf::from("Imported/room.wav")).unwrap();
    a.set(ROOM as u32, 0.5).unwrap();
    let state = a.save_state();
    let b = Native::new(&KIND, RATE, Some(&state)).unwrap();
    assert_eq!(b.file(), Some(std::path::Path::new("Imported/room.wav")));
    assert_eq!(b.plain_values(), a.plain_values());
    // Another effect ignores a file in its settings, and refuses one.
    let mut amp = Native::new(find("amplify").unwrap(), RATE, Some(&state)).unwrap();
    assert_eq!(amp.file(), None);
    assert!(amp.set_file(PathBuf::from("x.wav")).is_err());
}

#[test]
fn a_new_impulse_while_playing_takes_over_without_a_gap() {
    // Impulse A plays; B is prepared and handed over between two blocks. The output is A's
    // convolution of the input before the switch (its tail rings on) plus B's of the input
    // after it: nothing dropped, nothing cut, no block skipped.
    let dir = temp_dir("swap");
    let ha = vec![room(9000, 16, 2500.0), room(9000, 17, 2000.0)];
    let hb = vec![room(15_000, 18, 4000.0)];
    let file_a = write(&dir.join("a.wav"), 48_000, ha.clone());
    let file_b = write(&dir.join("b.wav"), 48_000, hb.clone());
    let settings = [(MIX, 100.0), (GAIN, 0.0)];
    let mut native = Native::new(&KIND, RATE, None).unwrap();
    for &(i, v) in &settings {
        native.set(i as u32, KIND.params[i].normalized(v)).unwrap();
    }
    native.prepare_file_now(RATE, &file_a).unwrap();
    let flags = InsertFlags::default();
    let mut insert: NativeInsert = native.insert(Arc::new(AtomicBool::new(false)), flags.clone());
    let block = 300;
    let blocks = 120;
    let swap = 37;
    let input = [noise(block * blocks, 19), noise(block * blocks, 20)];
    let mut out = [Vec::new(), Vec::new()];
    for b in 0..blocks {
        if b == swap {
            native.prepare_file_now(RATE, &file_b).unwrap();
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
    // What it should be: A on the input up to the switch, B on what follows from there.
    let s = swap * block;
    let plain = values(&settings);
    let conv = |h: &[Vec<f32>], x: [&[f32]; 2]| -> [Vec<f32>; 2] {
        let shaped = shape(RATE, &plain, h);
        let mut c = Convolver::new(&shaped.channels, shaped.gain as f32);
        let n = x[0].len();
        let mut o = [vec![0.0; n], vec![0.0; n]];
        let [l, r] = &mut o;
        c.process(Some(x), [&mut l[..], &mut r[..]]);
        o
    };
    let total = block * blocks;
    let pre: Vec<Vec<f32>> = (0..2)
        .map(|c| {
            (0..total)
                .map(|i| if i < s { input[c][i] } else { 0.0 })
                .collect()
        })
        .collect();
    let a = conv(&ha, [&pre[0], &pre[1]]);
    let b = conv(&hb, [&input[0][s..], &input[1][s..]]);
    for c in 0..2 {
        let want: Vec<f32> = (0..total)
            .map(|i| {
                let w = a[c][i] + if i >= s { b[c][i - s] } else { 0.0 };
                w * 100.0
            })
            .collect();
        let db = null_db(&out[c], &want);
        println!("a switch while playing, channel {c}: {db:.1} dB");
        assert!(db < -120.0, "channel {c}: {db:.1} dB");
        // And the wet never stops: every stretch of 256 samples after the switch is loud.
        let quiet = out[c][s..]
            .chunks(256)
            .position(|w| w.iter().map(|x| x * x).sum::<f32>() < 1e-6);
        assert_eq!(quiet, None, "channel {c}: a gap after the switch");
    }
}

/// Release timing (`cargo test -p daw-core --release -- --ignored preparation_cost
/// --nocapture`): reading a 6 s stereo impulse at 44.1 kHz and bringing it to 48 kHz (done once
/// per file), then shaping and partitioning it (each settings change).
#[test]
#[ignore]
fn preparation_cost() {
    let dir = temp_dir("cost");
    let file = write(
        &dir.join("six.wav"),
        44_100,
        vec![
            room(6 * 44_100, 21, 40_000.0),
            room(6 * 44_100, 22, 40_000.0),
        ],
    );
    let started = std::time::Instant::now();
    let loaded = load(&file, RATE).unwrap();
    let read = started.elapsed();
    let mut shaping = Vec::new();
    for settings in [
        vec![],
        vec![(DAMPING_LF, 20.0), (DAMPING_HF, 95.0), (ROOM, 60.0)],
    ] {
        let started = std::time::Instant::now();
        let prepared = prepare(RATE, &values(&settings), &loaded);
        shaping.push(started.elapsed());
        drop(prepared);
    }
    println!(
        "6 s stereo at 44.1 kHz: read and resampled to 48 kHz in {:.0} ms; shaped and \
         partitioned in {:.1} ms (defaults) and {:.1} ms (damping both ways, Room 60)",
        read.as_secs_f64() * 1e3,
        shaping[0].as_secs_f64() * 1e3,
        shaping[1].as_secs_f64() * 1e3
    );
}
