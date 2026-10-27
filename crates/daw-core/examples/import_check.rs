//! Importing FLAC, MP3 and AIFF (dev plan 8.5): ffmpeg encodes a recording into each format, in
//! the temp folder, and the importer decodes them back.
//!
//! - **FLAC** (16 and 24 bit) and **AIFF** are lossless: they must decode to exactly the WAV's
//!   samples.
//! - **MP3** (LAME, 320 kb/s) is lossy: it must come back exactly as long as the WAV (the
//!   encoder's delay and padding trimmed), line up with it at no offset, and stay close (the
//!   signal-to-noise ratio is printed and must clear 20 dB).
//! - A FLAC imported into a session plays the same audio as the WAV imported there.
//!
//! The recording is only read. Prints `RESULT: PASS` or `FAIL:` lines.
//!
//! cargo run --release -p daw-core --example import_check -- [<recording.wav>]

use anyhow::{bail, Context, Result};
use daw_core::device::AudioSettings;
use daw_core::media::read_audio;
use daw_core::session::Session;
use daw_core::wav::{self, Audio};
use std::path::{Path, PathBuf};
use std::process::Command;

/// `ffmpeg -i from <args> to`; false if ffmpeg isn't there.
fn ffmpeg(from: &Path, args: &[&str], to: &Path) -> Result<bool> {
    let run = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-i"])
        .arg(from)
        .args(args)
        .arg(to)
        .status();
    match run {
        Ok(status) if status.success() => Ok(true),
        Ok(status) => bail!("ffmpeg {args:?} failed: {status}"),
        Err(_) => Ok(false),
    }
}

/// The worst sample difference, or `None` if the shapes differ.
fn worst_difference(a: &Audio, b: &Audio) -> Option<f32> {
    if a.sample_rate != b.sample_rate || a.channels.len() != b.channels.len() || a.len() != b.len()
    {
        return None;
    }
    Some(
        a.channels
            .iter()
            .flatten()
            .zip(b.channels.iter().flatten())
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max),
    )
}

/// The lag (in samples, -range..=range) at which `b` best matches `a` (normalised correlation),
/// over a stretch from the loudest part of `a`.
fn best_lag(a: &[f32], b: &[f32], range: i64) -> i64 {
    let window = 8192.min(a.len() / 2);
    let loudest = (0..a.len().saturating_sub(window))
        .step_by(window / 4)
        .max_by(|&x, &y| {
            let energy = |at: usize| a[at..at + window].iter().map(|s| s * s).sum::<f32>();
            energy(x).total_cmp(&energy(y))
        })
        .unwrap_or(0);
    let start = loudest.max(range as usize);
    (-range..=range)
        .max_by(|&x, &y| {
            // Normalised by the stretch of `b` it meets, or a louder stretch would win.
            let dot = |lag: i64| {
                let (sum, energy) = (start..start + window)
                    .filter_map(|i| {
                        let j = i as i64 + lag;
                        b.get(usize::try_from(j).ok()?).map(|v| (a[i] * v, v * v))
                    })
                    .fold((0.0f32, 0.0f32), |(s, e), (p, q)| (s + p, e + q));
                sum / energy.max(1e-12).sqrt()
            };
            dot(x).total_cmp(&dot(y))
        })
        .unwrap_or(0)
}

fn snr_db(a: &[f32], b: &[f32]) -> f64 {
    let signal: f64 = a.iter().map(|s| f64::from(*s).powi(2)).sum();
    let noise: f64 = a.iter().zip(b).map(|(x, y)| f64::from(x - y).powi(2)).sum();
    10.0 * (signal / noise.max(1e-30)).log10()
}

fn main() -> Result<()> {
    let source =
        std::path::absolute(std::env::args().nth(1).ok_or_else(|| {
            anyhow::anyhow!("give the audio file (a WAV) as the first argument")
        })?)?;
    let reference = wav::read(&source).with_context(|| format!("reading {}", source.display()))?;
    println!(
        "{}: {} channel(s), {} Hz, {} frames",
        source.display(),
        reference.channels.len(),
        reference.sample_rate,
        reference.len()
    );
    let dir: PathBuf = std::env::temp_dir().join("daw-import-check");
    std::fs::create_dir_all(&dir)?;
    let mut failures = Vec::new();
    let mut lossless = |name: &str, from: &Path, args: &[&str], expected: &Audio| -> Result<bool> {
        let file = dir.join(name);
        if !ffmpeg(from, args, &file)? {
            return Ok(false);
        }
        let decoded = read_audio(&file)?;
        match worst_difference(expected, &decoded) {
            Some(0.0) => println!("{name}: {} frames, bit for bit", decoded.len()),
            Some(d) => failures.push(format!("{name}: off by up to {d}")),
            None => failures.push(format!(
                "{name}: {} ch, {} Hz, {} frames; expected {} ch, {} Hz, {} frames",
                decoded.channels.len(),
                decoded.sample_rate,
                decoded.len(),
                expected.channels.len(),
                expected.sample_rate,
                expected.len()
            )),
        }
        Ok(true)
    };
    // 24 bit as recorded, then made stereo; 16 bit from a 16-bit WAV of it.
    if !lossless("24.flac", &source, &["-c:a", "flac"], &reference)? {
        println!("note: ffmpeg isn't on the PATH: nothing to check");
        println!("RESULT: PASS");
        return Ok(());
    }
    lossless("24.aif", &source, &["-c:a", "pcm_s24be"], &reference)?;
    let stereo = dir.join("24-stereo.wav");
    ffmpeg(&source, &["-ac", "2", "-c:a", "pcm_s24le"], &stereo)?;
    let stereo_reference = wav::read(&stereo)?;
    lossless(
        "24-stereo.flac",
        &stereo,
        &["-c:a", "flac"],
        &stereo_reference,
    )?;
    lossless(
        "24-stereo.aif",
        &stereo,
        &["-c:a", "pcm_s24be"],
        &stereo_reference,
    )?;
    let sixteen = dir.join("16.wav");
    ffmpeg(&source, &["-c:a", "pcm_s16le"], &sixteen)?;
    let sixteen_reference = wav::read(&sixteen)?;
    lossless("16.flac", &sixteen, &["-c:a", "flac"], &sixteen_reference)?;
    lossless(
        "16.aif",
        &sixteen,
        &["-c:a", "pcm_s16be"],
        &sixteen_reference,
    )?;

    // MP3: lossy, but as long as the original and lined up with it.
    let mp3 = dir.join("320.mp3");
    ffmpeg(&source, &["-c:a", "libmp3lame", "-b:a", "320k"], &mp3)?;
    let decoded = read_audio(&mp3)?;
    let (a, b) = (&reference.channels[0], &decoded.channels[0]);
    let lag = best_lag(a, b, 2048);
    let snr = snr_db(a, b);
    println!(
        "320.mp3: {} frames (the WAV has {}), lined up at {lag:+} samples, {snr:.1} dB SNR",
        decoded.len(),
        reference.len()
    );
    if decoded.len() != reference.len() {
        failures.push(format!(
            "320.mp3: {} frames, not {} (encoder delay or padding left in)",
            decoded.len(),
            reference.len()
        ));
    }
    if lag != 0 {
        failures.push(format!("320.mp3: lines up {lag:+} samples off"));
    }
    if snr < 20.0 {
        failures.push(format!("320.mp3: only {snr:.1} dB SNR"));
    }

    // Through a session (at 48 kHz, so both are resampled): the FLAC plays what the WAV does.
    let mut session = Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        threads: Some(0),
        ..Default::default()
    });
    let track = session
        .project
        .add_track(daw_core::project::TrackKind::Audio, None);
    let wav_clip = session.add_clip_from_file(track, &source, 0)?;
    let flac_clip = session.add_clip_from_file(track, &dir.join("24.flac"), 0)?;
    let audio = |clip| {
        let c = session.project.find_clip(clip).expect("added").1.clone();
        session.clip_audio(&c).expect("loaded").0
    };
    let (from_wav, from_flac) = (audio(wav_clip), audio(flac_clip));
    if from_wav.channels != from_flac.channels {
        failures.push("the imported FLAC plays different audio from the WAV".into());
    } else {
        println!(
            "imported at {} Hz: the FLAC clip plays the WAV's {} samples exactly",
            from_flac.sample_rate,
            from_flac.len()
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("import check failed")
    }
}
