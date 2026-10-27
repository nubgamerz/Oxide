//! WAV file I/O. Audio is held planar, `[channel][sample]`, as `f32`.

use anyhow::{bail, Context, Result};
use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use std::path::Path;

/// Planar audio with its sample rate.
#[derive(Debug, Clone, PartialEq)]
pub struct Audio {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
}

impl Audio {
    /// Frames per channel.
    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn num_channels(&self) -> usize {
        self.channels.len()
    }

    pub fn seconds(&self) -> f64 {
        self.len() as f64 / f64::from(self.sample_rate)
    }
}

/// Read a WAV file of any integer or float format into normalized `f32`.
pub fn read(path: &Path) -> Result<Audio> {
    let mut reader =
        WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let spec = reader.spec();
    let interleaved: Vec<f32> = match spec.sample_format {
        SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>()?,
        SampleFormat::Int => {
            let scale = 1.0 / (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()?
        }
    };
    let n_ch = usize::from(spec.channels);
    if n_ch == 0 {
        bail!("{} has no channels", path.display());
    }
    let frames = interleaved.len() / n_ch;
    let mut channels = vec![Vec::with_capacity(frames); n_ch];
    for frame in interleaved.chunks_exact(n_ch) {
        for (ch, &s) in channels.iter_mut().zip(frame) {
            ch.push(s);
        }
    }
    Ok(Audio {
        sample_rate: spec.sample_rate,
        channels,
    })
}

/// Write 32-bit float WAV, so a render is stored exactly as the engine produced it.
pub fn write_f32(path: &Path, audio: &Audio) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let spec = WavSpec {
        channels: u16::try_from(audio.num_channels()).context("too many channels")?,
        sample_rate: audio.sample_rate,
        bits_per_sample: 32,
        sample_format: SampleFormat::Float,
    };
    let mut writer =
        WavWriter::create(path, spec).with_context(|| format!("creating {}", path.display()))?;
    for i in 0..audio.len() {
        for ch in &audio.channels {
            writer.write_sample(ch[i])?;
        }
    }
    writer.finalize()?;
    Ok(())
}

/// A WAV file's sample format, so an edited file can be written back the way it was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Format {
    pub bits: u16,
    pub float: bool,
}

impl Format {
    /// 32-bit float: stores the engine's samples exactly.
    pub const FLOAT: Format = Format {
        bits: 32,
        float: true,
    };
}

/// A WAV file's format, or `None` when the file isn't a WAV (FLAC, MP3, AIFF).
pub fn read_format(path: &Path) -> Option<Format> {
    let spec = WavReader::open(path).ok()?.spec();
    Some(Format {
        bits: spec.bits_per_sample,
        float: spec.sample_format == SampleFormat::Float,
    })
}

/// Write `audio` as a WAV in `format` (16-, 24- or 32-bit integer, or 32-bit float), clamping
/// integer samples to full scale. Written beside `path` first and renamed over it, so a failed
/// write never leaves a half-written file where the old one was. The folder must exist.
pub fn write_with_format(path: &Path, audio: &Audio, format: Format) -> Result<()> {
    let spec = WavSpec {
        channels: u16::try_from(audio.num_channels()).context("too many channels")?,
        sample_rate: audio.sample_rate,
        bits_per_sample: format.bits,
        sample_format: if format.float {
            SampleFormat::Float
        } else {
            SampleFormat::Int
        },
    };
    if format.float && format.bits != 32 {
        bail!("float WAV must be 32-bit, not {}", format.bits);
    }
    if !format.float && !matches!(format.bits, 8 | 16 | 24 | 32) {
        bail!("{}-bit integer WAV isn't supported", format.bits);
    }
    let name = path
        .file_name()
        .context("the path has no file name")?
        .to_string_lossy();
    let temp = path.with_file_name(format!(".{name}.saving"));
    let written = (|| -> Result<()> {
        let mut writer = WavWriter::create(&temp, spec)
            .with_context(|| format!("creating {}", temp.display()))?;
        let full = (1i64 << (format.bits - 1)) as f64;
        for i in 0..audio.len() {
            for ch in &audio.channels {
                if format.float {
                    writer.write_sample(ch[i])?;
                } else {
                    let v = (f64::from(ch[i]) * full).round().clamp(-full, full - 1.0);
                    writer.write_sample(v as i32)?;
                }
            }
        }
        writer.finalize()?;
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(e);
    }
    std::fs::rename(&temp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_formats_round_trip_and_keep_their_format() {
        let dir = std::env::temp_dir().join(format!("daw-wav-format-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let samples: Vec<f32> = (0..1000).map(|i| ((i as f32) * 0.01).sin() * 0.9).collect();
        let audio = Audio {
            sample_rate: 44_100,
            channels: vec![samples.clone(), samples.iter().map(|v| -v).collect()],
        };
        for (bits, tolerance) in [(16u16, 1.0 / 32_768.0), (24, 1.0 / 8_388_608.0)] {
            let path = dir.join(format!("{bits}.wav"));
            let format = Format { bits, float: false };
            write_with_format(&path, &audio, format).unwrap();
            assert_eq!(
                read_format(&path),
                Some(format),
                "{bits}-bit stays {bits}-bit"
            );
            let back = read(&path).unwrap();
            assert_eq!(back.sample_rate, 44_100);
            for (a, b) in back.channels[1].iter().zip(&audio.channels[1]) {
                assert!((a - b).abs() <= tolerance, "{bits}: {a} vs {b}");
            }
        }
        // Into a folder that doesn't exist: an error, and nothing left behind.
        let missing = dir.join("no such folder").join("x.wav");
        assert!(write_with_format(&missing, &audio, Format::FLOAT).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn float_roundtrip_is_exact() {
        let dir = std::env::temp_dir().join("dawpoc-wav-test");
        let path = dir.join("rt.wav");
        let audio = Audio {
            sample_rate: 48_000,
            channels: vec![vec![0.0, 0.5, -0.25, 1e-9], vec![1.0, -1.0, 0.125, -3e-7]],
        };
        write_f32(&path, &audio).unwrap();
        assert_eq!(read(&path).unwrap(), audio);
        let _ = std::fs::remove_dir_all(dir);
    }
}
