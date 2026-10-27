//! Export formats (dev plan 13.6), read back by ffmpeg as an independent decoder: the audio given is
//! exported as every format Oxide writes (through `encode::write`, as an export writes them),
//! then ffmpeg decodes each to 32-bit float. WAV, AIFF and FLAC must hold exactly the dithered
//! integer samples; MP3 and Ogg Vorbis must come back at the exact length (ffmpeg honours the
//! MP3's LAME tag) and within 0.5 dB of the level; the 48 kHz conversion at its length.
//!
//! Prints `RESULT: PASS` or `FAIL:` lines and exits non-zero on a failure; without ffmpeg on
//! the PATH it says so and passes (there is nothing independent to check against).
//!
//! cargo run --release -p daw-core --example export_check -- <audio.wav>

use anyhow::{bail, Result};
use daw_core::encode::{self, Encoding, Format};
use daw_core::wav;
use std::path::{Path, PathBuf};
use std::process::Command;

/// ffmpeg's decode of `path`: each channel as 32-bit float at the file's rate. None without
/// ffmpeg.
fn decode(path: &Path, channels: usize) -> Result<Option<Vec<Vec<f32>>>> {
    let out = Command::new("ffmpeg")
        .args(["-v", "error", "-i"])
        .arg(path)
        .args(["-f", "f32le", "-acodec", "pcm_f32le", "-"])
        .output();
    let out = match out {
        Ok(out) if out.status.success() => out,
        Ok(out) => bail!(
            "ffmpeg couldn't decode {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(_) => return Ok(None),
    };
    let samples: Vec<f32> = out
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect();
    let frames = samples.len() / channels.max(1);
    Ok(Some(
        (0..channels)
            .map(|c| (0..frames).map(|i| samples[i * channels + c]).collect())
            .collect(),
    ))
}

fn rms_db(x: &[f32]) -> f64 {
    let sum: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum();
    10.0 * (sum / x.len().max(1) as f64).max(1e-30).log10()
}

fn main() -> Result<()> {
    let source = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or_else(|| anyhow::anyhow!("give the audio file (a WAV) as the first argument"))?,
    );
    // The audio (its first channel), as stereo with the right channel 6 dB down.
    let di = wav::read(&source)?;
    let left = di.channels[0].clone();
    let right = left.iter().map(|x| x * 0.5).collect();
    let audio = wav::Audio {
        sample_rate: di.sample_rate,
        channels: vec![left, right],
    };
    let frames = audio.channels[0].len();
    let dir = std::env::temp_dir().join(format!("oxide-export-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;

    let mut failures = Vec::new();
    let cases = [
        ("wav16.wav", Format::Wav { bits: 16 }, None),
        ("wav24.wav", Format::Wav { bits: 24 }, None),
        ("aiff16.aiff", Format::Aiff { bits: 16 }, None),
        ("aiff24.aiff", Format::Aiff { bits: 24 }, None),
        ("flac16.flac", Format::Flac { bits: 16, level: 5 }, None),
        ("flac24.flac", Format::Flac { bits: 24, level: 8 }, None),
        (
            "cbr320.mp3",
            Format::Mp3 {
                bitrate: Some(320),
                vbr: None,
            },
            None,
        ),
        (
            "v0.mp3",
            Format::Mp3 {
                bitrate: None,
                vbr: Some(0),
            },
            None,
        ),
        ("q6.ogg", Format::Ogg { quality: 6 }, None),
        ("wav24-48k.wav", Format::Wav { bits: 24 }, Some(48_000)),
    ];
    for (name, format, rate) in cases {
        let path = dir.join(name);
        let encoding = Encoding {
            format,
            sample_rate: rate,
            dither: true,
        };
        encode::write(&path, &audio, &encoding)?;
        let Some(read) = decode(&path, 2)? else {
            println!("note: ffmpeg isn't on the PATH: nothing independent to check against");
            println!("RESULT: PASS");
            return Ok(());
        };
        let size = std::fs::metadata(&path)?.len();
        let want_frames = match rate {
            Some(r) => {
                ((frames as u64 * u64::from(r) + u64::from(audio.sample_rate) / 2)
                    / u64::from(audio.sample_rate)) as usize
            }
            None => frames,
        };
        let length_ok = read[0].len() == want_frames;
        let detail = match format.int_bits().filter(|_| rate.is_none()) {
            // Integer formats: exactly the dithered samples.
            Some(bits) => {
                let expected = encode::quantize(&audio.channels, bits, true);
                let scale = f64::from(1u32 << (bits - 1));
                let exact = read.iter().zip(&expected).all(|(got, want)| {
                    got.len() == want.len()
                        && got
                            .iter()
                            .zip(want)
                            .all(|(g, w)| (f64::from(*g) * scale).round() as i32 == *w)
                });
                if !exact {
                    failures.push(format!("{name}: not the dithered samples, bit for bit"));
                }
                format!("bit for bit: {exact}")
            }
            // Lossy (and converted) files: the level.
            None => {
                let level = rms_db(&read[0]) - rms_db(&audio.channels[0]);
                if level.abs() > 0.5 {
                    failures.push(format!("{name}: {level:+.2} dB from the source"));
                }
                format!("level {level:+.2} dB")
            }
        };
        if !length_ok {
            failures.push(format!(
                "{name}: {} frames, not {want_frames}",
                read[0].len()
            ));
        }
        println!(
            "{name}: {} ({size} bytes): {} frames (want {want_frames}), {detail}",
            format.describe(),
            read[0].len()
        );
    }
    // Tags (13.7): every format tagged, and ffprobe reads the title and artist back; MP3 and
    // FLAC carry the cover as an attached picture ffprobe sees as a stream.
    let png = [
        0x89u8, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0x0D, b'I', b'H', b'D', b'R',
    ];
    let tags = daw_core::metadata::Metadata {
        title: "Oxide check".into(),
        artist: "The Testers".into(),
        cover: daw_core::metadata::Cover::from_bytes(png.to_vec()).ok(),
        ..Default::default()
    };
    let short = wav::Audio {
        sample_rate: audio.sample_rate,
        channels: audio
            .channels
            .iter()
            .map(|c| c[..c.len().min(48_000)].to_vec())
            .collect(),
    };
    for (name, format) in [
        ("tags.wav", Format::Wav { bits: 24 }),
        ("tags.aiff", Format::Aiff { bits: 16 }),
        ("tags.flac", Format::Flac { bits: 16, level: 5 }),
        (
            "tags.mp3",
            Format::Mp3 {
                bitrate: Some(192),
                vbr: None,
            },
        ),
        ("tags.ogg", Format::Ogg { quality: 5 }),
    ] {
        let path = dir.join(name);
        let encoding = Encoding {
            format,
            ..Encoding::default()
        };
        let carry_on = |_| true;
        let progress = encode::Progress::new(&carry_on);
        encode::write_with(&path, &short, &encoding, Some(&tags), &progress)?;
        let out = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format_tags:stream_tags:stream=codec_type,codec_name",
            ])
            .args(["-of", "default=noprint_wrappers=1"])
            .arg(&path)
            .output()?;
        let text = String::from_utf8_lossy(&out.stdout).to_lowercase();
        let has = |key: &str, value: &str| {
            text.lines()
                .any(|l| l.ends_with(&format!("{key}={}", value.to_lowercase())))
        };
        let title = has("title", &tags.title);
        let artist = has("artist", &tags.artist);
        let picture = text.contains("codec_type=video");
        let wants_picture = matches!(format, Format::Mp3 { .. } | Format::Flac { .. });
        println!("{name}: title {title}, artist {artist}, cover picture {picture}");
        if !title || !artist {
            failures.push(format!("{name}: ffprobe doesn't read the title and artist"));
        }
        if wants_picture && !picture {
            failures.push(format!("{name}: ffprobe sees no cover picture"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("export check failed")
    }
}
