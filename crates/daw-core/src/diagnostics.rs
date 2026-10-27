//! The Waveform view's Diagnostics (dev plan 11.5b: Click Repair and Clip Repair): scan a file (or
//! part of it) for clicks or clipped runs, list each one, and repair the ones chosen.
//!
//! - **The same repair as the inserts.** The Click Repair runs the Click Remover's own
//!   frames, passes and kernels over the range ([`click_remover::repair_offline`]); the
//!   Clip Repair runs Clip Repair's own tracker and repair ([`declipper::repair_offline`]), with the
//!   clip levels scanned from the range as its Scan does. So "repair all" over a whole file is
//!   what the insert plays (Clip Repair's at Gain 0 dB, under its limiter's ceiling).
//! - **A problem** is a stretch of one channel the repair rebuilt: a click (its padding
//!   included) or a clipped run. Its level is how loud it is in dBFS: a click's the largest
//!   change its repair makes (the click's own size), a clipped run's its highest sample (the
//!   clip level).
//! - **The repair is the scan's.** Each problem keeps the samples the scan rebuilt it with, all
//!   the problems found repaired; repairing a few writes theirs (ours: a chosen repair beside
//!   one left alone was made with its neighbour already repaired, TD125).
//! - **No gain.** Clip Repair repairs at its Gain; here the samples are only rebuilt,
//!   so rebuilt peaks can rise past the old clip level, even past 0 dBFS in a float file
//!   (lower the file's level after, as the effect's Gain would). TD125.

use crate::native::{click_remover, declipper};
use anyhow::{bail, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// What to look for: the two Diagnostics, "click-repair" and "clip-repair". The names before
/// 0.3.1, "declicker" and "declipper", are still read (an agent's call, or anything saved).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProblemKind {
    /// Clicks and pops, by the Click Remover's detector.
    #[default]
    #[serde(alias = "declicker")]
    ClickRepair,
    /// Clipped runs, by Clip Repair's scan.
    #[serde(alias = "declipper")]
    ClipRepair,
}

impl ProblemKind {
    /// "click" / "clipped run", for counts and history steps.
    pub fn noun(self, count: usize) -> String {
        let one = match self {
            ProblemKind::ClickRepair => "click",
            ProblemKind::ClipRepair => "clipped run",
        };
        if count == 1 {
            format!("1 {one}")
        } else {
            format!("{count} {one}s")
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            ProblemKind::ClickRepair => "Click Repair",
            ProblemKind::ClipRepair => "Clip Repair",
        }
    }
}

/// A setting's default from the effect's own parameters.
fn default_of(kind: &crate::native::Kind, name: &str) -> f64 {
    kind.params
        .iter()
        .find(|p| p.name == name)
        .map_or(0.0, |p| p.default)
}

/// The scan's settings: the Click Repair's are the Click Remover's (Threshold and
/// Complexity, 0..100 %), Clip Repair's are Clip Repair's (Tolerance 0..50 %, Min. Clip
/// Size 3..20 samples). Each kind reads only its own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Settings {
    pub threshold: f64,
    pub complexity: f64,
    pub tolerance: f64,
    pub min_clip_size: f64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            threshold: default_of(&click_remover::KIND, "Threshold"),
            complexity: default_of(&click_remover::KIND, "Complexity"),
            tolerance: default_of(&declipper::KIND, "Tolerance"),
            min_clip_size: default_of(&declipper::KIND, "Min. Clip Size"),
        }
    }
}

impl Settings {
    /// Refuse a setting outside its effect's range, by name.
    pub fn check(&self) -> Result<()> {
        let ranges = [
            (&click_remover::KIND, "Threshold", self.threshold),
            (&click_remover::KIND, "Complexity", self.complexity),
            (&declipper::KIND, "Tolerance", self.tolerance),
            (&declipper::KIND, "Min. Clip Size", self.min_clip_size),
        ];
        for (kind, name, value) in ranges {
            if let Some(p) = kind.params.iter().find(|p| p.name == name) {
                if !(p.min..=p.max).contains(&value) {
                    bail!(
                        "{name} {value} is out of range ({} to {} {})",
                        p.min,
                        p.max,
                        p.unit
                    );
                }
            }
        }
        Ok(())
    }
}

/// One problem found, with its id (unique in its file until it is closed).
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Problem {
    pub id: u32,
    pub kind: ProblemKind,
    /// The channel, from 0.
    pub channel: usize,
    /// Its first sample and length, in the file's own samples.
    pub start: u64,
    pub length: u64,
    /// dBFS: a click's size (the largest change its repair makes), a clipped run's highest
    /// sample.
    pub level_db: f64,
}

/// A problem as found, with the samples its repair puts there.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    pub channel: usize,
    pub start: usize,
    pub level_db: f64,
    pub repaired: Vec<f32>,
}

/// A level in dBFS (silence at -200).
fn dbfs(x: f64) -> f64 {
    if x > 0.0 {
        20.0 * x.log10()
    } else {
        -200.0
    }
}

/// Look for `kind`'s problems in `start..end` of `channels` (one buffer per channel, the
/// file's own samples), in order of time, then channel. Channels are scanned in parallel.
pub fn find(
    channels: &[Vec<f32>],
    kind: ProblemKind,
    settings: &Settings,
    (start, end): (usize, usize),
) -> Result<Vec<Found>> {
    settings.check()?;
    let len = channels.first().map_or(0, Vec::len);
    if end <= start || end > len {
        bail!("{start}..{end} isn't a range in the file ({len} samples)");
    }
    let mut found: Vec<Found> = std::thread::scope(|scope| {
        let jobs: Vec<_> = channels
            .iter()
            .enumerate()
            .map(|(c, x)| scope.spawn(move || find_in(c, x, kind, settings, start..end)))
            .collect();
        jobs.into_iter()
            .flat_map(|j| j.join().unwrap_or_default())
            .collect()
    });
    found.sort_by_key(|f| (f.start, f.channel));
    Ok(found)
}

/// One channel's problems.
fn find_in(
    channel: usize,
    x: &[f32],
    kind: ProblemKind,
    settings: &Settings,
    range: std::ops::Range<usize>,
) -> Vec<Found> {
    let (a, b) = (range.start, range.end);
    match kind {
        ProblemKind::ClickRepair => {
            let values = [settings.threshold, settings.complexity];
            let (repaired, rebuilt) = click_remover::repair_offline(x, range, &values);
            let mut out = Vec::new();
            let mut i = 0;
            while i < rebuilt.len() {
                if !rebuilt[i] {
                    i += 1;
                    continue;
                }
                let from = i;
                while i < rebuilt.len() && rebuilt[i] {
                    i += 1;
                }
                // A run at either edge of the range may go on past it: the rest of it is
                // outside what was asked for, so it is left to a scan that covers it.
                if from == 0 || i == rebuilt.len() {
                    continue;
                }
                let size = (from..i)
                    .map(|k| (f64::from(repaired[k]) - f64::from(x[a + k])).abs())
                    .fold(0.0, f64::max);
                out.push(Found {
                    channel,
                    start: a + from,
                    level_db: dbfs(size),
                    repaired: repaired[from..i].to_vec(),
                });
            }
            out
        }
        ProblemKind::ClipRepair => {
            // The clip levels from the range, as Clip Repair's Scan learns them.
            let part = &x[a..b];
            let levels = declipper::Learned {
                top: part.iter().fold(0.0f64, |m, s| m.max(f64::from(*s))),
                bottom: part.iter().fold(0.0f64, |m, s| m.min(f64::from(*s))),
                runs: 0,
            };
            let min = settings.min_clip_size.round() as usize;
            declipper::repair_offline(x, range, &levels, settings.tolerance, min, None)
                .into_iter()
                .map(|(start, repaired)| {
                    let peak = x[start..start + repaired.len()]
                        .iter()
                        .fold(0.0f64, |m, s| m.max(f64::from(s.abs())));
                    Found {
                        channel,
                        start,
                        level_db: dbfs(peak),
                        repaired,
                    }
                })
                .collect()
        }
    }
}

/// `channels` with `fixes` written in.
pub fn write(channels: &mut [Vec<f32>], fixes: &[&Found]) {
    for f in fixes {
        if let Some(c) = channels.get_mut(f.channel) {
            let end = (f.start + f.repaired.len()).min(c.len());
            c[f.start..end].copy_from_slice(&f.repaired[..end - f.start]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::native::{render, render_with_print};
    use std::collections::BTreeMap;
    use std::f64::consts::PI;

    const RATE: f64 = 48_000.0;

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

    fn sine(n: usize, hz: f64, amp: f64, phase: f64) -> Vec<f32> {
        (0..n)
            .map(|i| (amp * (2.0 * PI * hz * i as f64 / RATE + phase).sin()) as f32)
            .collect()
    }

    /// `x` faded in and out over 10 ms: a sound that starts at full tilt against silence is a
    /// step, which a click detector rightly takes for a click.
    fn faded(x: Vec<f32>) -> Vec<f32> {
        let ramp = 480;
        let len = x.len();
        x.into_iter()
            .enumerate()
            .map(|(i, s)| {
                let edge = i.min(len - 1 - i);
                if edge < ramp {
                    (f64::from(s) * (0.5 - 0.5 * (PI * edge as f64 / ramp as f64).cos())) as f32
                } else {
                    s
                }
            })
            .collect()
    }

    fn mix(a: &[f32], b: &[f32]) -> Vec<f32> {
        a.iter().zip(b).map(|(x, y)| x + y).collect()
    }

    /// Clicks of `sizes` samples into `x` from `first`, 2500 apart: bursts of samples of
    /// varying size, all one way, of alternating sign, at about `amp`. Where each one is.
    fn add_clicks(x: &mut [f32], first: usize, sizes: &[usize], amp: f32) -> Vec<(usize, usize)> {
        sizes
            .iter()
            .enumerate()
            .map(|(k, &len)| {
                let at = first + 2500 * k;
                let sign = if k % 2 == 0 { 1.0 } else { -1.0 };
                for (i, s) in x[at..at + len].iter_mut().enumerate() {
                    *s += sign * amp * (0.6 + 0.4 * ((i * 7 % 5) as f32 / 4.0));
                }
                (at, len)
            })
            .collect()
    }

    fn all(found: &[Found]) -> Vec<&Found> {
        found.iter().collect()
    }

    /// A clean stereo file: a faded 440 Hz tone with a little noise on the left, faded noise
    /// on the right.
    fn clean(n: usize) -> Vec<Vec<f32>> {
        vec![
            faded(mix(&sine(n, 440.0, 0.25, 0.0), &gauss(n, 3, 3e-5))),
            faded(gauss(n, 4, 0.01)),
        ]
    }

    #[test]
    fn clicks_are_found_where_they_are_and_repaired_at_least_20_db() {
        let n = 2 * RATE as usize;
        let clean = clean(n);
        let mut x = clean.clone();
        let sizes = [1, 2, 3, 5, 8, 12, 20, 30];
        let left = add_clicks(&mut x[0], 6000, &sizes, 0.5);
        let right = add_clicks(&mut x[1], 7300, &sizes[..5], 0.4);
        let settings = Settings::default();
        let found = find(&x, ProblemKind::ClickRepair, &settings, (0, n)).unwrap();
        for f in &found {
            println!(
                "channel {} at {} for {}: {:.1} dBFS",
                f.channel,
                f.start,
                f.repaired.len(),
                f.level_db
            );
        }
        // Each click once, within a few samples of where it is (the repair pads it by 2), and
        // nothing else; in time order.
        let expected: Vec<(usize, usize, usize)> = left
            .iter()
            .map(|&(at, len)| (0, at, len))
            .chain(right.iter().map(|&(at, len)| (1, at, len)))
            .collect();
        assert_eq!(found.len(), expected.len(), "{found:?}");
        for &(c, at, len) in &expected {
            let f = found
                .iter()
                .find(|f| f.channel == c && f.start <= at && f.start + f.repaired.len() >= at + len)
                .unwrap_or_else(|| panic!("channel {c}: no click found at {at} ({len})"));
            let (early, late) = (at - f.start, f.start + f.repaired.len() - (at + len));
            assert!(
                early <= 4 && late <= 4,
                "channel {c} at {at}: {early} {late}"
            );
            assert!(f.level_db > -20.0, "{}", f.level_db);
        }
        assert!(found.windows(2).all(|w| w[0].start <= w[1].start));

        // Repaired, each click's error at least 20 dB down; a second scan finds none.
        let mut fixed = x.clone();
        write(&mut fixed, &all(&found));
        for &(c, at, len) in &expected {
            let err = |y: &[f32]| -> f64 {
                (at - 8..at + len + 8)
                    .map(|i| (f64::from(y[i]) - f64::from(clean[c][i])).powi(2))
                    .sum()
            };
            let depth = 10.0 * (err(&fixed[c]) / err(&x[c])).log10();
            println!("channel {c}, {len} samples: {depth:.1} dB");
            assert!(depth < -20.0, "channel {c} at {at}: {depth:.1} dB");
        }
        let again = find(&fixed, ProblemKind::ClickRepair, &settings, (0, n)).unwrap();
        assert!(again.is_empty(), "{again:?}");
        // Nothing outside the clicks moved.
        for (c, (a, b)) in fixed.iter().zip(&x).enumerate() {
            let moved = (0..n).filter(|&i| a[i] != b[i]).count();
            let rebuilt: usize = found
                .iter()
                .filter(|f| f.channel == c)
                .map(|f| f.repaired.len())
                .sum();
            assert!(moved <= rebuilt, "{moved} {rebuilt}");
        }
    }

    #[test]
    fn nothing_is_found_on_clean_audio() {
        let n = 2 * RATE as usize;
        let mut x = clean(n);
        x.push(faded(sine(n, 1000.0, 0.01, 0.3)));
        let none = find(&x, ProblemKind::ClickRepair, &Settings::default(), (0, n)).unwrap();
        assert!(none.is_empty(), "{none:?}");
        // Clip Repair takes a scan's loudest peaks for clipping (its bands sit under them):
        // clean audio is clean to it under -24 dBFS, and a pure tone at
        // Tolerance 0 (its peak is one sample, never three).
        let quiet: Vec<Vec<f32>> = vec![sine(n, 440.0, 0.06, 0.0), gauss(n, 5, 0.01)];
        let none = find(
            &quiet,
            ProblemKind::ClipRepair,
            &Settings::default(),
            (0, n),
        )
        .unwrap();
        assert!(none.is_empty(), "{none:?}");
        let tight = Settings {
            tolerance: 0.0,
            ..Settings::default()
        };
        let tone = [faded(sine(n, 440.0, 0.8, 0.1))];
        let none = find(&tone, ProblemKind::ClipRepair, &tight, (0, n)).unwrap();
        assert!(none.is_empty(), "{none:?}");
    }

    #[test]
    fn clipped_runs_are_found_where_they_are_and_rebuilt() {
        // 480 Hz (a cycle every 100 samples) at -3 dBFS, clipped at ±0.5: every peak a run.
        let n = RATE as usize;
        let amp = 10f64.powf(-3.0 / 20.0);
        let clean = sine(n, 480.0, amp, 0.3);
        let clipped: Vec<f32> = clean.iter().map(|s| s.clamp(-0.5, 0.5)).collect();
        let x = vec![clipped.clone()];
        let found = find(&x, ProblemKind::ClipRepair, &Settings::default(), (0, n)).unwrap();
        // Where the clipped stretches are.
        let mut flats = Vec::new();
        let mut i = 0;
        while i < n {
            if clipped[i].abs() == 0.5 {
                let from = i;
                while i < n && clipped[i].abs() == 0.5 {
                    i += 1;
                }
                flats.push((from, i - from));
            } else {
                i += 1;
            }
        }
        assert_eq!(found.len(), flats.len(), "{} {}", found.len(), flats.len());
        for (f, &(at, len)) in found.iter().zip(&flats) {
            // The band (10 % under the level) starts a sample or two before the flat top.
            assert!(f.start <= at && at - f.start <= 3, "{} {at}", f.start);
            assert!(f.start + f.repaired.len() >= at + len);
            assert!((f.level_db - 20.0 * 0.5f64.log10()).abs() < 1e-6);
        }
        // Rebuilt: the peaks within 1 dB of the clean ones, no flat top left.
        let mut fixed = x.clone();
        write(&mut fixed, &all(&found));
        let mut worst = 0.0f64;
        for k in 20..n / 100 - 20 {
            let peak = |y: &[f32]| {
                y[k * 100..(k + 1) * 100]
                    .iter()
                    .fold(0.0f32, |m, s| m.max(*s))
            };
            worst = worst.max((20.0 * f64::from(peak(&fixed[0]) / peak(&clean)).log10()).abs());
        }
        println!("rebuilt peaks within {worst:.3} dB");
        assert!(worst < 1.0, "{worst}");
        let flat_left = fixed[0]
            .windows(3)
            .filter(|w| w.iter().all(|s| s.abs() == 0.5))
            .count();
        assert_eq!(flat_left, 0);
    }

    #[test]
    fn repairing_all_is_what_the_inserts_play() {
        // The Click Repair over a whole file: the Click Remover's output (its 4096
        // samples of latency dropped), bit for bit.
        let n = 60_000;
        let mut x = clean(n);
        add_clicks(&mut x[0], 5000, &[1, 4, 9, 16], 0.5);
        add_clicks(&mut x[1], 9100, &[2, 6], 0.3);
        let found = find(&x, ProblemKind::ClickRepair, &Settings::default(), (0, n)).unwrap();
        assert_eq!(found.len(), 6, "{found:?}");
        let mut fixed = x.clone();
        write(&mut fixed, &all(&found));
        let played = render(&click_remover::KIND, RATE, &BTreeMap::new(), &x, 512).unwrap();
        for c in 0..2 {
            assert!(played[c] == fixed[c], "channel {c}");
        }

        // Clip Repair at Gain 0 dB (rebuilt peaks under its limiter's -0.1 dBFS), its levels
        // scanned from the whole file: its output (its latency dropped), bit for bit.
        let clipped: Vec<f32> = sine(n, 480.0, 0.8, 0.3)
            .iter()
            .map(|s| s.clamp(-0.5, 0.45))
            .collect();
        let x = vec![clipped.clone(), clipped.iter().map(|s| -s * 0.9).collect()];
        let found = find(&x, ProblemKind::ClipRepair, &Settings::default(), (0, n)).unwrap();
        assert!(found.len() > 2 * 900, "{}", found.len());
        let mut fixed = x.clone();
        write(&mut fixed, &all(&found));
        assert!(fixed.iter().flatten().all(|s| s.abs() < 0.95));
        let gain: BTreeMap<String, f64> = [("Gain".to_string(), 0.0)].into();
        for c in 0..2 {
            // The scan learns per channel; each alone (a mono buffer feeds both).
            let one = vec![x[c].clone()];
            let played =
                render_with_print(&declipper::KIND, RATE, &gain, Some(&one), &one, 512).unwrap();
            assert!(played[0] == fixed[c], "channel {c}");
        }
    }

    #[test]
    fn a_range_scan_finds_only_inside_it_and_settings_are_checked() {
        let n = RATE as usize;
        let mut x = clean(n);
        let at = add_clicks(&mut x[0], 4000, &[3, 3, 3, 3, 3, 3], 0.5);
        // From just after the second click to just before the fifth: the third and fourth.
        let range = (at[1].0 + 100, at[4].0 - 100);
        let found = find(&x, ProblemKind::ClickRepair, &Settings::default(), range).unwrap();
        let starts: Vec<usize> = found.iter().map(|f| f.start + 2).collect();
        assert_eq!(starts, [at[2].0, at[3].0], "{found:?}");
        assert!(find(&x, ProblemKind::ClickRepair, &Settings::default(), (5, 5)).is_err());
        assert!(find(
            &x,
            ProblemKind::ClickRepair,
            &Settings::default(),
            (0, n + 1)
        )
        .is_err());
        let bad = Settings {
            min_clip_size: 2.0,
            ..Settings::default()
        };
        let e = find(&x, ProblemKind::ClipRepair, &bad, (0, n)).unwrap_err();
        assert!(e.to_string().contains("Min. Clip Size"), "{e}");
        assert_eq!(ProblemKind::ClickRepair.noun(1), "1 click");
        assert_eq!(ProblemKind::ClipRepair.noun(7), "7 clipped runs");
    }

    /// The cost on the DI, if present: run by hand in release (`cargo test --release -p
    /// daw-core diagnostics_cost -- --ignored --nocapture`).
    #[test]
    #[ignore]
    fn diagnostics_cost() {
        let di = crate::test_audio();
        let Ok(audio) = crate::wav::read(&di) else {
            return;
        };
        let n = audio.channels[0].len();
        let seconds = n as f64 / f64::from(audio.sample_rate);
        for (name, threshold, complexity) in [
            ("default", 29.0 / 0.99, 15.0 / 0.99),
            ("Heavy Reduction", 12.0 / 0.99, 94.0 / 0.99),
        ] {
            let settings = Settings {
                threshold,
                complexity,
                ..Settings::default()
            };
            let t = std::time::Instant::now();
            let found = find(&audio.channels, ProblemKind::ClickRepair, &settings, (0, n)).unwrap();
            let took = t.elapsed().as_secs_f64();
            println!(
                "Click Repair ({name}) on the DI ({seconds:.1} s, {} channel): {} found in {:.3} s, {:.1} ms per second of audio a channel",
                audio.channels.len(),
                found.len(),
                took,
                1e3 * took / seconds / audio.channels.len() as f64
            );
        }
        let t = std::time::Instant::now();
        let found = find(
            &audio.channels,
            ProblemKind::ClipRepair,
            &Settings::default(),
            (0, n),
        )
        .unwrap();
        println!(
            "Clip Repair on the DI: {} found in {:.3} s",
            found.len(),
            t.elapsed().as_secs_f64()
        );
    }
}
