//! The loudness meter against an independent one and the EBU's own test files (dev plan 6.1).
//!
//! - **ffmpeg's `ebur128` filter**, when ffmpeg is on the PATH, measures the same files: a
//!   recording (mono, so the one-channel rule and the K-weighting at its rate are compared) and
//!   two signals written here from the EBU's definitions (Tech 3341 case 3 for gating, Tech
//!   3342 case 4 for LRA). Integrated loudness must agree within 0.1 LU, LRA within 1 LU, true
//!   peak within 0.3 dB (plus the 0.05 that ffmpeg's one-decimal print can hide).
//! - **The EBU test files** (the zip at tech.ebu.ch/loudness), when they are in the folder
//!   given, are matched to their case by name ("3341-7", "3341-2011-8", "3342-5") and
//!   checked against Tech 3341's and Tech 3342's Table 1. Cases a file-based check can't read
//!   (the 'live' cases 11 and 14) and surround (case 6) are noted, not failed.
//!
//! Prints `RESULT: PASS` or `FAIL:` lines.
//!
//! cargo run --release -p daw-core --example loudness_check -- <recording.wav> [<EBU folder>]

use anyhow::{bail, Context, Result};
use daw_core::loudness::{measure_file, Loudness, LoudnessMeter};
use daw_core::wav;
use std::path::{Path, PathBuf};
use std::process::Command;

const RATE: u32 = 48_000;

/// ffmpeg's summary: integrated loudness, LRA and true peak.
struct Reference {
    integrated: f64,
    range: f64,
    peak: f64,
}

/// Run ffmpeg's EBU R 128 filter on `path`; `None` if ffmpeg isn't there.
fn ffmpeg(path: &Path) -> Result<Option<Reference>> {
    let output = match Command::new("ffmpeg")
        .args(["-nostats", "-hide_banner", "-i"])
        .arg(path)
        .args(["-filter_complex", "ebur128=peak=true", "-f", "null", "-"])
        .output()
    {
        Ok(output) => output,
        Err(_) => return Ok(None),
    };
    let text = String::from_utf8_lossy(&output.stderr);
    let summary = text
        .rsplit("Summary:")
        .next()
        .context("ffmpeg printed no summary")?;
    // "    I:         -27.0 LUFS", "    LRA:         6.8 LU", "    Peak:      -13.0 dBFS"
    let value = |label: &str| -> Result<f64> {
        summary
            .lines()
            .find_map(|line| line.trim().strip_prefix(label))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|v| v.parse().ok())
            .with_context(|| format!("no '{label}' in ffmpeg's summary"))
    };
    Ok(Some(Reference {
        integrated: value("I:")?,
        range: value("LRA:")?,
        peak: value("Peak:")?,
    }))
}

/// A 1 kHz sine with peak level `db` dBFS for `seconds`, as the EBU's tones are.
fn tone(db: f64, seconds: f64) -> Vec<f32> {
    let amplitude = 10f64.powf(db / 20.0);
    let n = (seconds * f64::from(RATE)) as usize;
    (0..n)
        .map(|i| {
            (amplitude * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / f64::from(RATE)).sin())
                as f32
        })
        .collect()
}

/// Write the tones one after another as a stereo WAV in the temp folder.
fn write_sequence(name: &str, parts: &[(f64, f64)]) -> Result<PathBuf> {
    let signal: Vec<f32> = parts.iter().flat_map(|&(db, s)| tone(db, s)).collect();
    let path = std::env::temp_dir().join(name);
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: RATE,
            channels: vec![signal.clone(), signal],
        },
    )?;
    Ok(path)
}

fn show(v: Option<f64>) -> String {
    v.map_or("none".into(), |v| format!("{v:.2}"))
}

/// Compare our meter with ffmpeg's on `path`.
fn against_ffmpeg(path: &Path, label: &str, failures: &mut Vec<String>) -> Result<bool> {
    let ours = measure_file(path)?;
    let Some(theirs) = ffmpeg(path)? else {
        return Ok(false);
    };
    println!(
        "{label}: ours I {} LRA {} TP {} | ffmpeg I {:.1} LRA {:.1} TP {:.1}",
        show(ours.integrated),
        show(ours.range),
        show(ours.true_peak),
        theirs.integrated,
        theirs.range,
        theirs.peak
    );
    // ffmpeg prints one decimal, which can hide 0.05 either way.
    for (what, ours, theirs, tolerance) in [
        ("integrated", ours.integrated, theirs.integrated, 0.15),
        ("LRA", ours.range, theirs.range, 1.05),
        ("true peak", ours.true_peak, theirs.peak, 0.35),
    ] {
        match ours {
            Some(v) if (v - theirs).abs() <= tolerance => {}
            _ => failures.push(format!(
                "{label}: {what} {} vs ffmpeg's {theirs:.1}",
                show(ours)
            )),
        }
    }
    Ok(true)
}

/// The Tech 3341 and 3342 case numbers in a file name.
fn cases(name: &str) -> (Option<u32>, Option<u32>) {
    let number = |key: &str| -> Option<u32> {
        let at = name.find(key)? + key.len();
        let rest = name[at..].strip_prefix("2011-").unwrap_or(&name[at..]);
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        digits.parse().ok()
    };
    (number("3341-"), number("3342-"))
}

/// Meter a file in 10 ms steps, calling `each` with the readings and the time.
fn walk(path: &Path, mut each: impl FnMut(f64, &Loudness)) -> Result<()> {
    let audio = wav::read(path)?;
    let mut meter = LoudnessMeter::new(audio.sample_rate);
    let step = (audio.sample_rate / 100) as usize;
    let left = &audio.channels[0];
    let right = audio.channels.get(1);
    for (k, chunk) in left.chunks(step).enumerate() {
        let r = right.map(|r| &r[k * step..(k * step + chunk.len()).min(r.len())]);
        meter.process(chunk, r);
        each((k + 1) as f64 / 100.0, &meter.readings());
    }
    Ok(())
}

/// Check one EBU file against its case's expected response.
fn ebu_file(path: &Path, failures: &mut Vec<String>, notes: &mut Vec<String>) -> Result<()> {
    let name = path
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();
    let (tech3341, tech3342) = cases(&name);
    let near = |v: Option<f64>, want: f64, tolerance: f64| {
        v.is_some_and(|v| (v - want).abs() <= tolerance)
    };
    let mut checked = false;
    if let Some(case) = tech3341 {
        checked = true;
        if case == 6 {
            notes.push(format!(
                "{name}: case 6 is 5.0 surround; the master is stereo"
            ));
        } else if matches!(case, 11 | 14) {
            notes.push(format!(
                "{name}: case {case} is for 'live' meters (unit tests cover it)"
            ));
        } else if matches!(case, 9 | 12) {
            // S (9) or M (12) constant at -23 after 3 s (9) or 1 s (12).
            let (settle, short) = if case == 9 { (3.0, true) } else { (1.0, false) };
            let mut worst: f64 = 0.0;
            walk(path, |t, l| {
                let v = if short { l.short_term } else { l.momentary };
                if t >= settle {
                    worst = worst.max(v.map_or(f64::INFINITY, |v| (v + 23.0).abs()));
                }
            })?;
            println!("3341 #{case} ({name}): worst {:.2} LU off -23", worst);
            if worst > 0.1 {
                failures.push(format!("3341 #{case}: off -23 by {worst:.2} LU"));
            }
        } else {
            let l = measure_file(path)?;
            let ok = match case {
                1 => [l.momentary, l.short_term, l.integrated]
                    .iter()
                    .all(|v| near(*v, -23.0, 0.1)),
                2 => [l.momentary, l.short_term, l.integrated]
                    .iter()
                    .all(|v| near(*v, -33.0, 0.1)),
                3 | 4 | 5 | 7 | 8 => near(l.integrated, -23.0, 0.1),
                10 => near(l.max_short_term, -23.0, 0.1),
                13 => near(l.max_momentary, -23.0, 0.1),
                15..=18 => l.true_peak.is_some_and(|v| (-6.4..=-5.8).contains(&v)),
                19 => l.true_peak.is_some_and(|v| (2.6..=3.2).contains(&v)),
                20..=23 => l.true_peak.is_some_and(|v| (-0.4..=0.2).contains(&v)),
                _ => {
                    notes.push(format!("{name}: no expectation for Tech 3341 case {case}"));
                    true
                }
            };
            println!(
                "3341 #{case} ({name}): M {} S {} I {} max M {} max S {} TP {}",
                show(l.momentary),
                show(l.short_term),
                show(l.integrated),
                show(l.max_momentary),
                show(l.max_short_term),
                show(l.true_peak)
            );
            if !ok {
                failures.push(format!("3341 #{case} ({name}) is outside its tolerance"));
            }
        }
    }
    if let Some(case) = tech3342 {
        checked = true;
        let expected = match case {
            1 => Some(10.0),
            2 => Some(5.0),
            3 => Some(20.0),
            4 => Some(15.0),
            5 => Some(5.0),
            6 => Some(15.0),
            _ => None,
        };
        let l = measure_file(path)?;
        println!("3342 #{case} ({name}): LRA {}", show(l.range));
        match expected {
            Some(lra) if !near(l.range, lra, 1.0) => {
                failures.push(format!("3342 #{case}: LRA {} vs {lra} ±1", show(l.range)))
            }
            None => notes.push(format!("{name}: no expectation for Tech 3342 case {case}")),
            _ => {}
        }
    }
    if !checked {
        notes.push(format!("{name}: not an EBU test file name"));
    }
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let recording =
        std::path::absolute(args.next().ok_or_else(|| {
            anyhow::anyhow!("give the audio file (a WAV) as the first argument")
        })?)?;
    let ebu = args.next().map_or(PathBuf::from("out/ebu"), PathBuf::from);
    let mut failures = Vec::new();
    let mut notes = Vec::new();

    // Against ffmpeg: a real recording and two signals from the EBU definitions.
    let gating = write_sequence(
        "daw-loudness-3341-3.wav",
        &[(-36.0, 10.0), (-23.0, 60.0), (-36.0, 10.0)],
    )?;
    let range = write_sequence(
        "daw-loudness-3342-4.wav",
        &[
            (-50.0, 20.0),
            (-35.0, 20.0),
            (-20.0, 20.0),
            (-35.0, 20.0),
            (-50.0, 20.0),
        ],
    )?;
    let mut compared = 0;
    for (path, label) in [
        (recording.as_path(), "recording"),
        (gating.as_path(), "3341 #3 (written here)"),
        (range.as_path(), "3342 #4 (written here)"),
    ] {
        if against_ffmpeg(path, label, &mut failures)? {
            compared += 1;
        }
    }
    if compared == 0 {
        notes.push("ffmpeg isn't on the PATH: no independent comparison".into());
    }

    // The EBU's own files, if they're here.
    let files: Vec<PathBuf> = std::fs::read_dir(&ebu)
        .map(|dir| {
            dir.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("wav")))
                .collect()
        })
        .unwrap_or_default();
    if files.is_empty() {
        notes.push(format!(
            "no EBU test files in {}: the programme-material cases (3341 #7-8, 3342 #5-6) are \
             unchecked (get them from tech.ebu.ch/loudness)",
            ebu.display()
        ));
    }
    for file in &files {
        if let Err(e) = ebu_file(file, &mut failures, &mut notes) {
            failures.push(format!("{}: {e:#}", file.display()));
        }
    }

    for note in &notes {
        println!("note: {note}");
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("loudness check failed")
    }
}
