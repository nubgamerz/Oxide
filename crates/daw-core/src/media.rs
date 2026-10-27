//! Audio files held in memory for clip playback, with waveform peaks for drawing.

use crate::wav;
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Samples per peak entry at the finest level.
pub const PEAK_BLOCK: usize = 256;

/// Decoded audio: planar, at the project sample rate, plus min/max peaks per channel.
#[derive(Debug)]
pub struct AudioData {
    pub sample_rate: u32,
    pub channels: Vec<Vec<f32>>,
    /// `peaks[channel][i]` covers samples `i * PEAK_BLOCK .. (i + 1) * PEAK_BLOCK`.
    pub peaks: Vec<Vec<(f32, f32)>>,
    /// Per channel, the other levels of [`crate::peaks::BLOCKS`] for drawing at any zoom (the
    /// 256-sample one, `peaks`, left empty here), built with it. Read them through
    /// [`AudioData::wave`].
    levels: Vec<[Vec<(f32, f32)>; crate::peaks::LEVELS]>,
    /// Where its attacks are (the whole file, all channels mixed), found the first time a
    /// stretch asks to keep them.
    onsets: std::sync::OnceLock<Vec<usize>>,
}

impl AudioData {
    /// The decoded audio with its peaks at every level ([`crate::peaks::build`]: one pass over
    /// the samples, as the 256-sample peaks alone always took). Called where the audio was
    /// decoded or made (the loading worker, a stretch's worker, an edit), never per frame.
    pub fn new(sample_rate: u32, channels: Vec<Vec<f32>>) -> Self {
        let (mut peaks, mut levels) = (Vec::new(), Vec::new());
        for channel in &channels {
            let mut built = crate::peaks::build(channel);
            peaks.push(std::mem::take(&mut built[1]));
            levels.push(built);
        }
        Self {
            sample_rate,
            channels,
            peaks,
            levels,
            onsets: std::sync::OnceLock::new(),
        }
    }

    /// One channel as a view draws it at any zoom (23.2): its samples and every peak level.
    pub fn wave(&self, channel: usize) -> crate::peaks::Wave<'_> {
        let samples = self.channels.get(channel).map_or(&[][..], Vec::as_slice);
        let (Some(levels), Some(peaks)) = (self.levels.get(channel), self.peaks.get(channel))
        else {
            return crate::peaks::Wave::default();
        };
        crate::peaks::Wave {
            samples,
            levels: std::array::from_fn(|l| {
                if l == 1 {
                    peaks.as_slice()
                } else {
                    levels[l].as_slice()
                }
            }),
        }
    }

    /// The attacks in the whole file (sample positions), as a stretch that keeps them sees
    /// them: `detect_onsets` at its default sensitivity on the channels mixed to mono.
    pub fn onsets(&self) -> &[usize] {
        self.onsets.get_or_init(|| {
            let mono = crate::analysis::mono(&self.channels, 0, self.len());
            let settings = crate::analysis::OnsetSettings::new(0.5, 50.0, self.sample_rate);
            crate::analysis::detect_onsets(&mono, settings)
                .into_iter()
                .map(|o| o.position)
                .collect()
        })
    }

    pub fn len(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Min/max over `start..end` samples of one channel, using the peak cache.
    pub fn peak_range(&self, channel: usize, start: usize, end: usize) -> (f32, f32) {
        match self.peaks.get(channel) {
            Some(peaks) => peak_range(peaks, start, end),
            None => (0.0, 0.0),
        }
    }
}

/// Min/max over `start..end` samples of one channel's peaks (one `(min, max)` per
/// [`PEAK_BLOCK`] samples): a file's, or a take's while it records, drawn alike.
pub fn peak_range(peaks: &[(f32, f32)], start: usize, end: usize) -> (f32, f32) {
    let first = start / PEAK_BLOCK;
    let last = end.div_ceil(PEAK_BLOCK).min(peaks.len());
    if first >= last {
        let i = first.min(peaks.len().saturating_sub(1));
        return peaks.get(i).copied().unwrap_or((0.0, 0.0));
    }
    peaks[first..last]
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), &(a, b)| {
            (lo.min(a), hi.max(b))
        })
}

/// File extensions the importer reads (it goes by the content, so these are for file
/// pickers).
pub const AUDIO_EXTENSIONS: [&str; 8] =
    ["wav", "wave", "flac", "mp3", "aif", "aiff", "aifc", "ogg"];

/// Read an audio file: WAV through hound, exactly as recorded and exported; FLAC, MP3, AIFF
/// (and WAV variants hound can't read) through symphonia, decoded to `f32` the same way every
/// time. MP3 encoder delay and padding are trimmed, so a file is as long as what was encoded.
pub fn read_audio(path: &Path) -> Result<wav::Audio> {
    let mut head = [0u8; 12];
    let is_wav = std::fs::File::open(path)
        .and_then(|mut f| std::io::Read::read_exact(&mut f, &mut head))
        .is_ok()
        && (&head[0..4] == b"RIFF" || &head[0..4] == b"RF64")
        && &head[8..12] == b"WAVE";
    if is_wav {
        if let Ok(audio) = wav::read(path) {
            return Ok(audio);
        }
    }
    decode(path)
}

/// Decode with symphonia: the first audio track, every packet, interleaved `f32` split into
/// channels.
fn decode(path: &Path) -> Result<wav::Audio> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::errors::Error;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(
            &hint,
            source,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(|e| anyhow!("{}: not an audio file this can read ({e})", path.display()))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| anyhow!("{} has no audio track", path.display()))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or_else(|| anyhow!("{}: no audio codec parameters", path.display()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())
        .map_err(|e| anyhow!("{}: {e}", path.display()))?;
    let mut interleaved: Vec<f32> = Vec::new();
    let mut spec = None;
    let mut chunk: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(packet)) => packet,
            // The end, or a new stream chained after it (only in Ogg): stop at the first.
            Ok(None) | Err(Error::ResetRequired) => break,
            Err(e) => bail!("{}: {e}", path.display()),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buffer) => {
                let this = (buffer.spec().rate(), buffer.spec().channels().count());
                if *spec.get_or_insert(this) != this {
                    bail!("{}: the format changes partway through", path.display());
                }
                chunk.resize(buffer.samples_interleaved(), 0.0);
                buffer.copy_to_slice_interleaved(&mut chunk);
                interleaved.extend_from_slice(&chunk);
            }
            // A damaged packet: skipped, as players do.
            Err(Error::DecodeError(e)) => log::warn!("{}: {e}", path.display()),
            Err(e) => bail!("{}: {e}", path.display()),
        }
    }
    let Some((sample_rate, count)) = spec.filter(|s| s.1 > 0) else {
        bail!("{} holds no audio", path.display());
    };
    let frames = interleaved.len() / count;
    let mut channels = vec![Vec::with_capacity(frames); count];
    for frame in interleaved.chunks_exact(count) {
        for (channel, &s) in channels.iter_mut().zip(frame) {
            channel.push(s);
        }
    }
    Ok(wav::Audio {
        sample_rate,
        channels,
    })
}

/// Every file [`load`] has read in this test process, and on which thread: the tests' way to
/// see where a project's files are decoded (tests run side by side, so each looks only at its
/// own files).
#[cfg(test)]
pub(crate) static LOADS: std::sync::Mutex<Vec<(std::thread::ThreadId, PathBuf)>> =
    std::sync::Mutex::new(Vec::new());

/// Load an audio file (see [`read_audio`]), converting it to `sample_rate` if it was recorded
/// at another rate.
pub fn load(path: &Path, sample_rate: u32) -> Result<AudioData> {
    #[cfg(test)]
    if let Ok(mut loads) = LOADS.lock() {
        loads.push((std::thread::current().id(), path.to_path_buf()));
    }
    let audio = read_audio(path)?;
    let channels = if audio.sample_rate == sample_rate {
        audio.channels
    } else {
        audio
            .channels
            .iter()
            .map(|c| resample(c, audio.sample_rate, sample_rate))
            .collect()
    };
    Ok(AudioData::new(sample_rate, channels))
}

/// Windowed-sinc resampler. Good enough for importing material at a different rate; not a
/// mastering-grade converter.
pub fn resample(input: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || input.is_empty() {
        return input.to_vec();
    }
    const TAPS: isize = 32;
    let ratio = f64::from(to) / f64::from(from);
    // Lowpass at the lower Nyquist when downsampling.
    let cutoff = ratio.min(1.0);
    let out_len = (input.len() as f64 * ratio).round() as usize;
    (0..out_len)
        .map(|n| {
            let pos = n as f64 / ratio;
            let center = pos.floor() as isize;
            let mut acc = 0.0f64;
            let mut norm = 0.0f64;
            for k in (center - TAPS + 1)..=(center + TAPS) {
                let x = pos - k as f64;
                let sinc = if x.abs() < 1e-9 {
                    1.0
                } else {
                    let a = std::f64::consts::PI * x * cutoff;
                    a.sin() / a
                };
                let w = 0.5 + 0.5 * (std::f64::consts::PI * x / TAPS as f64).cos();
                let tap = sinc * w;
                norm += tap;
                if k >= 0 && (k as usize) < input.len() {
                    acc += tap * f64::from(input[k as usize]);
                }
            }
            if norm.abs() > 1e-12 {
                (acc / norm) as f32
            } else {
                0.0
            }
        })
        .collect()
}

/// Decoded files shared by clips, keyed by resolved path.
#[derive(Default)]
pub struct MediaPool {
    files: HashMap<PathBuf, Arc<AudioData>>,
}

impl MediaPool {
    pub fn get_or_load(&mut self, path: &Path, sample_rate: u32) -> Result<Arc<AudioData>> {
        if let Some(data) = self.files.get(path) {
            if data.sample_rate == sample_rate {
                return Ok(data.clone());
            }
        }
        let data = Arc::new(load(path, sample_rate)?);
        self.files.insert(path.to_path_buf(), data.clone());
        Ok(data)
    }

    pub fn insert(&mut self, path: PathBuf, data: Arc<AudioData>) {
        self.files.insert(path, data);
    }

    pub fn get(&self, path: &Path) -> Option<&Arc<AudioData>> {
        self.files.get(path)
    }

    pub fn clear(&mut self) {
        self.files.clear();
    }

    /// Forget every file `same` says is the one just saved, so the next load reads it again.
    pub fn forget(&mut self, same: impl Fn(&Path) -> bool) {
        self.files.retain(|path, _| !same(path));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peaks_cover_ranges() {
        let samples: Vec<f32> = (0..1000).map(|i| (i as f32 / 999.0) * 2.0 - 1.0).collect();
        let data = AudioData::new(48_000, vec![samples]);
        let (lo, hi) = data.peak_range(0, 0, 1000);
        assert_eq!((lo, hi), (-1.0, 1.0));
        let (lo, hi) = data.peak_range(0, 0, 256);
        assert!(lo == -1.0 && hi < 0.0);
    }

    /// A 16-bit AIFF file written by hand: `FORM`/`AIFF`, a `COMM` chunk (with the rate as an
    /// 80-bit float) and an `SSND` chunk of big-endian samples.
    fn write_aiff(path: &Path, rate: u32, channels: &[Vec<i16>]) {
        let frames = channels[0].len();
        let mut comm = Vec::new();
        comm.extend((channels.len() as i16).to_be_bytes());
        comm.extend((frames as u32).to_be_bytes());
        comm.extend(16i16.to_be_bytes());
        // 80-bit extended: sign and exponent, then the mantissa with its integer bit.
        let exponent = 31 - rate.leading_zeros();
        comm.extend(((16383 + exponent) as u16).to_be_bytes());
        comm.extend((u64::from(rate) << (63 - exponent)).to_be_bytes());
        let mut ssnd = vec![0u8; 8];
        for i in 0..frames {
            for channel in channels {
                ssnd.extend(channel[i].to_be_bytes());
            }
        }
        let mut body = b"AIFF".to_vec();
        for (id, chunk) in [(b"COMM", &comm), (b"SSND", &ssnd)] {
            body.extend(id);
            body.extend((chunk.len() as u32).to_be_bytes());
            body.extend(chunk);
        }
        let mut file = b"FORM".to_vec();
        file.extend((body.len() as u32).to_be_bytes());
        file.extend(body);
        std::fs::write(path, file).unwrap();
    }

    #[test]
    fn aiff_reads_bit_for_bit_and_wav_stays_on_hound() {
        let dir = std::env::temp_dir().join(format!("daw-media-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let left: Vec<i16> = (0..3000)
            .map(|i| ((i * 37) % 65_536 - 32_768) as i16)
            .collect();
        let right: Vec<i16> = left.iter().map(|s| s.wrapping_neg()).collect();
        let aiff = dir.join("two.aif");
        write_aiff(&aiff, 48_000, &[left.clone(), right.clone()]);
        let audio = read_audio(&aiff).unwrap();
        assert_eq!(
            (audio.sample_rate, audio.channels.len(), audio.len()),
            (48_000, 2, 3000)
        );
        let scaled = |v: &[i16]| {
            v.iter()
                .map(|s| f32::from(*s) / 32_768.0)
                .collect::<Vec<_>>()
        };
        assert!(audio.channels[0] == scaled(&left));
        assert!(audio.channels[1] == scaled(&right));
        // The same samples as WAV read through hound, unchanged.
        let wav = dir.join("two.wav");
        crate::wav::write_f32(&wav, &audio).unwrap();
        assert_eq!(read_audio(&wav).unwrap(), crate::wav::read(&wav).unwrap());
        // Not audio: refused, by name.
        let text = dir.join("notes.mp3");
        std::fs::write(&text, "not audio at all").unwrap();
        let refused = format!("{:#}", read_audio(&text).unwrap_err());
        assert!(refused.contains("notes.mp3"), "{refused}");
        let _ = std::fs::remove_dir_all(dir);
    }
    #[test]
    fn resampling_keeps_a_sine_in_tune() {
        let from = 44_100;
        let to = 48_000;
        let freq = 1000.0;
        let input: Vec<f32> = (0..from)
            .map(|i| (2.0 * std::f32::consts::PI * freq * i as f32 / from as f32).sin())
            .collect();
        let out = resample(&input, from, to);
        assert_eq!(out.len(), to as usize);
        // Compare against an ideal sine at the new rate, away from the edges.
        let err = (1000..47_000)
            .map(|i| {
                let ideal = (2.0 * std::f32::consts::PI * freq * i as f32 / to as f32).sin();
                (out[i] - ideal).abs()
            })
            .fold(0.0f32, f32::max);
        assert!(err < 1e-2, "max error {err}");
    }
}
