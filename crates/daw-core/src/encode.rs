//! Export formats (dev plan 13.6): the file a mixdown, a render or the Waveform view's export
//! is written as. WAV (16 or 24-bit integer, or 32-bit float), AIFF, FLAC, MP3 and Ogg Vorbis,
//! encoded in-process (LAME and libvorbis are built in; FLAC is written in Rust), at the
//! audio's own rate or converted.
//!
//! The default, a 32-bit float WAV at the audio's rate, is written exactly as before this
//! module (`wav::write_f32`): every export and render null test runs through it. Integer depths
//! get TPDF dither of one least significant bit from a seeded generator, so an export repeats
//! exactly. Every file is written beside its target and renamed into place, so a failure never
//! leaves half a file.

use crate::wav;
use anyhow::{anyhow, bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A file format and its settings.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Format {
    /// `bits` 16 or 24 (integer) or 32 (float).
    Wav { bits: u16 },
    /// `bits` 16 or 24.
    Aiff { bits: u16 },
    /// `bits` 16 or 24; `level` 0 (fastest, largest) to 8 (slowest, smallest).
    Flac { bits: u16, level: u8 },
    /// Constant `bitrate` in kbps (128 to 320), or `vbr` quality 0 (best, V0) to 9: one of them.
    Mp3 {
        #[serde(default)]
        bitrate: Option<u32>,
        #[serde(default)]
        vbr: Option<u8>,
    },
    /// Vorbis `quality` 0 (smallest) to 10 (best).
    Ogg { quality: u8 },
}

impl Default for Format {
    fn default() -> Self {
        Format::Wav { bits: 32 }
    }
}

/// How an export is written: its format, at the audio's rate or `sample_rate`, with dither
/// on integer depths unless `dither` is off.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Encoding {
    #[serde(default)]
    pub format: Format,
    /// Convert to this rate (Hz) first; none keeps the audio's.
    #[serde(default)]
    pub sample_rate: Option<u32>,
    /// TPDF dither of one least significant bit when the format holds integers (default on).
    #[serde(default = "dither_by_default")]
    pub dither: bool,
}

fn dither_by_default() -> bool {
    true
}

impl Default for Encoding {
    fn default() -> Self {
        Self {
            format: Format::default(),
            sample_rate: None,
            dither: true,
        }
    }
}

/// The constant MP3 bitrates offered, kbps.
pub const MP3_BITRATES: [u32; 6] = [128, 160, 192, 224, 256, 320];
/// The rates MPEG-1 Layer III carries (LAME refuses others).
const MP3_RATES: [u32; 3] = [32_000, 44_100, 48_000];

impl Format {
    /// The file extension it is written with.
    pub fn extension(&self) -> &'static str {
        match self {
            Format::Wav { .. } => "wav",
            Format::Aiff { .. } => "aiff",
            Format::Flac { .. } => "flac",
            Format::Mp3 { .. } => "mp3",
            Format::Ogg { .. } => "ogg",
        }
    }

    /// A short description, as the export report and dialogs show it.
    pub fn describe(&self) -> String {
        match *self {
            Format::Wav { bits: 32 } => "WAV, 32-bit float".into(),
            Format::Wav { bits } => format!("WAV, {bits}-bit"),
            Format::Aiff { bits } => format!("AIFF, {bits}-bit"),
            Format::Flac { bits, level } => format!("FLAC, {bits}-bit, level {level}"),
            Format::Mp3 { vbr: Some(q), .. } => format!("MP3, VBR V{q}"),
            Format::Mp3 { bitrate, .. } => format!("MP3, {} kbps", bitrate.unwrap_or(320)),
            Format::Ogg { quality } => format!("Ogg Vorbis, quality {quality}"),
        }
    }

    /// The integer depth it holds samples at, if it does.
    pub fn int_bits(&self) -> Option<u16> {
        match *self {
            Format::Wav { bits: 32 } => None,
            Format::Wav { bits } | Format::Aiff { bits } | Format::Flac { bits, .. } => Some(bits),
            Format::Mp3 { .. } | Format::Ogg { .. } => None,
        }
    }
}

impl Encoding {
    /// Why these settings can't be written at `rate` (the audio's rate, before any
    /// conversion), naming the bad value.
    pub fn check(&self, rate: u32) -> Result<()> {
        let rate = self.sample_rate.unwrap_or(rate);
        if !(8_000..=192_000).contains(&rate) {
            bail!("sample rate {rate} Hz is outside 8000..192000");
        }
        match self.format {
            Format::Wav { bits } if ![16, 24, 32].contains(&bits) => {
                bail!("a WAV holds 16, 24 or 32-bit samples, not {bits}")
            }
            Format::Aiff { bits } | Format::Flac { bits, .. } if ![16, 24].contains(&bits) => {
                bail!(
                    "{} holds 16 or 24-bit samples, not {bits}",
                    self.format.extension()
                )
            }
            Format::Flac { level, .. } if level > 8 => {
                bail!("FLAC level {level} is outside 0..8")
            }
            Format::Mp3 { bitrate, vbr } => {
                if bitrate.is_some() && vbr.is_some() {
                    bail!("an MP3 is either a constant `bitrate` or `vbr`, not both");
                }
                if let Some(b) = bitrate.filter(|b| !MP3_BITRATES.contains(b)) {
                    bail!("MP3 bitrate {b} kbps isn't one of {MP3_BITRATES:?}");
                }
                if let Some(q) = vbr.filter(|q| *q > 9) {
                    bail!("MP3 VBR quality {q} is outside 0..9");
                }
                if !MP3_RATES.contains(&rate) {
                    bail!("an MP3 can't be {rate} Hz: convert to 44100 or 48000 (sample_rate)");
                }
            }
            Format::Ogg { quality } if quality > 10 => {
                bail!("Ogg Vorbis quality {quality} is outside 0..10")
            }
            _ => {}
        }
        Ok(())
    }
}

/// `path` with the format's extension: added when it has none; refused when it has another.
pub fn with_extension(path: &Path, format: &Format) -> Result<PathBuf> {
    let ext = format.extension();
    match path.extension().map(|e| e.to_string_lossy().to_lowercase()) {
        None => Ok(path.with_extension(ext)),
        Some(e) if e == ext || (ext == "aiff" && e == "aif") => Ok(path.to_path_buf()),
        Some(e) => bail!(
            "{} ends in .{e}, but the format is {}: use .{ext}",
            path.display(),
            format.describe()
        ),
    }
}

/// How far a write has got, for a progress bar: `report` takes the fraction done (0 to 1) of
/// the part `[start, start + span]` it stands for, and says whether to carry on (a cancel).
pub struct Progress<'a> {
    report: &'a dyn Fn(f32) -> bool,
    start: f32,
    span: f32,
}

impl<'a> Progress<'a> {
    pub fn new(report: &'a dyn Fn(f32) -> bool) -> Self {
        Self {
            report,
            start: 0.0,
            span: 1.0,
        }
    }

    /// The part `[from, to]` of this one, for a step of the work.
    fn part(&self, from: f32, to: f32) -> Progress<'a> {
        Progress {
            report: self.report,
            start: self.start + self.span * from,
            span: self.span * (to - from),
        }
    }

    /// `done` (0 to 1) of this part is done; an error if the user cancelled.
    fn at(&self, done: f32) -> Result<()> {
        if (self.report)(self.start + self.span * done.clamp(0.0, 1.0)) {
            Ok(())
        } else {
            bail!("export cancelled")
        }
    }
}

/// Write `audio` to `path` as `encoding` says.
pub fn write(path: &Path, audio: &wav::Audio, encoding: &Encoding) -> Result<()> {
    write_with(path, audio, encoding, None, &Progress::new(&|_| true))
}

/// [`write`] with the file's tags (`metadata`, in the format's own way), reporting how far it
/// has got (and stopping when `progress` says to).
pub fn write_with(
    path: &Path,
    audio: &wav::Audio,
    encoding: &Encoding,
    metadata: Option<&crate::metadata::Metadata>,
    progress: &Progress,
) -> Result<()> {
    let metadata = metadata.filter(|m| !m.is_empty());
    encoding.check(audio.sample_rate)?;
    if audio.channels.is_empty() {
        bail!("no audio to write");
    }
    let converting = encoding.sample_rate.filter(|r| *r != audio.sample_rate);
    // Converting the rate is about as long as encoding: half the bar each.
    let encoded = if converting.is_some() {
        progress.part(0.5, 1.0)
    } else {
        progress.part(0.0, 1.0)
    };
    let converted;
    let audio = match converting {
        Some(rate) => {
            converted = wav::Audio {
                sample_rate: rate,
                channels: resample_with(
                    &audio.channels,
                    audio.sample_rate,
                    rate,
                    &progress.part(0.0, 0.5),
                )?,
            };
            &converted
        }
        None => audio,
    };
    let mut temp = path.as_os_str().to_owned();
    temp.push(".writing");
    let temp = PathBuf::from(temp);
    let written = match encoding.format {
        Format::Wav { bits: 32 } => wav::write_f32(&temp, audio),
        Format::Wav { bits } => write_wav_int(&temp, audio, bits, encoding.dither, &encoded),
        format => encode_bytes(audio, &format, encoding.dither, metadata, &encoded)
            .and_then(|bytes| std::fs::write(&temp, bytes).map_err(Into::into)),
    };
    // WAV and AIFF are tagged once written, with chunks after their audio.
    let written = written.and_then(|()| match (encoding.format, metadata) {
        (Format::Wav { .. }, Some(m)) => crate::metadata::tag_wav(&temp, m),
        (Format::Aiff { .. }, Some(m)) => crate::metadata::tag_aiff(&temp, m),
        _ => Ok(()),
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&temp);
        return Err(e.context(format!("writing {}", path.display())));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::rename(&temp, path).with_context(|| format!("writing {}", path.display()))
}

/// The bytes of a format that is built in memory (all but WAV).
fn encode_bytes(
    audio: &wav::Audio,
    format: &Format,
    dither: bool,
    metadata: Option<&crate::metadata::Metadata>,
    progress: &Progress,
) -> Result<Vec<u8>> {
    match *format {
        Format::Aiff { bits } => {
            let bytes = aiff(audio, bits, dither);
            progress.at(1.0)?;
            Ok(bytes)
        }
        Format::Flac { bits, level } => {
            // flacenc encodes in one call: the bar moves before and after it.
            progress.at(0.1)?;
            let mut bytes = flac(audio, bits, level, dither)?;
            if let Some(m) = metadata {
                crate::metadata::tag_flac(&mut bytes, m)?;
            }
            progress.at(1.0)?;
            Ok(bytes)
        }
        Format::Mp3 { bitrate, vbr } => {
            let frames = mp3(audio, bitrate, vbr, progress)?;
            // The ID3 tag goes in front of the frames (the LAME tag stays the first frame).
            let mut bytes = match metadata {
                Some(m) => crate::metadata::id3_bytes(m)?,
                None => Vec::new(),
            };
            bytes.extend_from_slice(&frames);
            Ok(bytes)
        }
        Format::Ogg { quality } => ogg(audio, quality, metadata, progress),
        Format::Wav { .. } => unreachable!("WAV is written by hound"),
    }
}

/// A small, fast generator for the dither: the same every export, so files repeat.
struct Noise(u64);

impl Noise {
    fn new() -> Self {
        Self(0x9E37_79B9_7F4A_7C15)
    }
    /// Uniform in [-0.5, 0.5).
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 - 0.5
    }
}

/// `channels` as `bits`-bit integers: scaled, with TPDF dither of one least significant bit
/// (the sum of two uniform values) unless `dither` is off, rounded and clipped. The samples
/// an integer file holds; tests compare decoded files with it.
pub fn quantize(channels: &[Vec<f32>], bits: u16, dither: bool) -> Vec<Vec<i32>> {
    let full = f64::from(1u32 << (bits - 1));
    let (low, high) = (-full, full - 1.0);
    let mut noise = Noise::new();
    let frames = channels.iter().map(Vec::len).max().unwrap_or(0);
    let mut out = vec![Vec::with_capacity(frames); channels.len()];
    // Frame by frame, so the noise sequence doesn't depend on the channel count's layout.
    for i in 0..frames {
        for (c, channel) in channels.iter().enumerate() {
            let x = f64::from(channel.get(i).copied().unwrap_or(0.0)) * full;
            let d = if dither {
                noise.next() + noise.next()
            } else {
                0.0
            };
            out[c].push((x + d).round().clamp(low, high) as i32);
        }
    }
    out
}

fn write_wav_int(
    path: &Path,
    audio: &wav::Audio,
    bits: u16,
    dither: bool,
    progress: &Progress,
) -> Result<()> {
    let spec = hound::WavSpec {
        channels: audio.channels.len() as u16,
        sample_rate: audio.sample_rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    let samples = quantize(&audio.channels, bits, dither);
    let mut w = hound::WavWriter::create(path, spec)?;
    let frames = samples[0].len();
    for i in 0..frames {
        if i % 65_536 == 0 {
            progress.at(i as f32 / frames as f32)?;
        }
        for channel in &samples {
            w.write_sample(channel[i])?;
        }
    }
    w.finalize()?;
    Ok(())
}

/// A sample rate as the 80-bit IEEE 754 extended float AIFF's COMM chunk holds.
fn extended(rate: u32) -> [u8; 10] {
    let mut out = [0u8; 10];
    if rate == 0 {
        return out;
    }
    let exponent = 31 - rate.leading_zeros();
    let mantissa = u64::from(rate) << (63 - exponent);
    out[..2].copy_from_slice(&(16_383 + exponent as u16).to_be_bytes());
    out[2..].copy_from_slice(&mantissa.to_be_bytes());
    out
}

/// An AIFF file: FORM/AIFF, COMM, SSND, big-endian integers.
fn aiff(audio: &wav::Audio, bits: u16, dither: bool) -> Vec<u8> {
    let samples = quantize(&audio.channels, bits, dither);
    let channels = samples.len();
    let frames = samples[0].len();
    let bytes = usize::from(bits / 8);
    let data = frames * channels * bytes;
    let mut out = Vec::with_capacity(data + 54);
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&((4 + 26 + 16 + data) as u32).to_be_bytes());
    out.extend_from_slice(b"AIFF");
    out.extend_from_slice(b"COMM");
    out.extend_from_slice(&18u32.to_be_bytes());
    out.extend_from_slice(&(channels as u16).to_be_bytes());
    out.extend_from_slice(&(frames as u32).to_be_bytes());
    out.extend_from_slice(&bits.to_be_bytes());
    out.extend_from_slice(&extended(audio.sample_rate));
    out.extend_from_slice(b"SSND");
    out.extend_from_slice(&((8 + data) as u32).to_be_bytes());
    out.extend_from_slice(&[0; 8]);
    for i in 0..frames {
        for channel in &samples {
            out.extend_from_slice(&channel[i].to_be_bytes()[4 - bytes..]);
        }
    }
    out
}

/// A FLAC file. flacenc has no libFLAC 0..8 presets; `level` is mapped onto the largest LPC
/// order it tries (0: fixed predictors only; then 2, 4, 6, 8, 8, 10, 12, 12), which is what
/// trades encoding time for size.
fn flac(audio: &wav::Audio, bits: u16, level: u8, dither: bool) -> Result<Vec<u8>> {
    use flacenc::component::BitRepr;
    use flacenc::error::Verify;
    let samples = quantize(&audio.channels, bits, dither);
    let interleaved: Vec<i32> = (0..samples[0].len())
        .flat_map(|i| samples.iter().map(move |c| c[i]))
        .collect();
    let mut config = flacenc::config::Encoder::default();
    const ORDERS: [usize; 9] = [0, 2, 4, 6, 8, 8, 10, 12, 12];
    let order = ORDERS[usize::from(level.min(8))];
    config.subframe_coding.use_lpc = order > 0;
    if order > 0 {
        config.subframe_coding.qlpc.lpc_order = order;
    }
    let config = config
        .into_verified()
        .map_err(|e| anyhow!("FLAC settings: {e:?}"))?;
    let source = flacenc::source::MemSource::from_samples(
        &interleaved,
        samples.len(),
        usize::from(bits),
        audio.sample_rate as usize,
    );
    let stream = flacenc::encode_with_fixed_block_size(&config, source, config.block_size)
        .map_err(|e| anyhow!("FLAC encoding: {e:?}"))?;
    let mut sink = flacenc::bitsink::ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|e| anyhow!("FLAC writing: {e:?}"))?;
    let mut bytes = sink.into_inner();
    // STREAMINFO's minimum block size leaves out the last block (FLAC's format spec), but
    // flacenc counts the short last one, which makes readers take the fixed-size blocks for
    // variable ones (Symphonia then can't read the file). Every block but the last is
    // `block_size`, so that is the minimum.
    if bytes.len() > 12 && &bytes[..4] == b"fLaC" {
        let max: [u8; 2] = [bytes[10], bytes[11]];
        bytes[8..10].copy_from_slice(&max);
    }
    Ok(bytes)
}

/// An MP3 through LAME, with the LAME tag (its encoder delay and padding) in the first frame,
/// so players and decoders that honour it get back exactly the audio's length.
fn mp3(
    audio: &wav::Audio,
    bitrate: Option<u32>,
    vbr: Option<u8>,
    progress: &Progress,
) -> Result<Vec<u8>> {
    use mp3lame_encoder::{Bitrate, Builder, DualPcm, FlushGap, MonoPcm, Quality, VbrMode};
    let bad = |e: mp3lame_encoder::BuildError| anyhow!("MP3 settings: {e:?}");
    let quality = |q: u8| match q {
        0 => Quality::Best,
        1 => Quality::SecondBest,
        2 => Quality::NearBest,
        3 => Quality::VeryNice,
        4 => Quality::Nice,
        5 => Quality::Good,
        6 => Quality::Decent,
        7 => Quality::Ok,
        8 => Quality::SecondWorst,
        _ => Quality::Worst,
    };
    let mut b = Builder::new().ok_or_else(|| anyhow!("LAME didn't start"))?;
    let channels = audio.channels.len().min(2);
    b.set_num_channels(channels as u8).map_err(bad)?;
    b.set_sample_rate(audio.sample_rate).map_err(bad)?;
    b.set_quality(Quality::NearBest).map_err(bad)?;
    b.set_to_write_vbr_tag(true).map_err(bad)?;
    match vbr {
        Some(q) => {
            b.set_vbr_mode(VbrMode::Mtrh).map_err(bad)?;
            b.set_vbr_quality(quality(q)).map_err(bad)?;
        }
        None => {
            let kbps = match bitrate.unwrap_or(320) {
                128 => Bitrate::Kbps128,
                160 => Bitrate::Kbps160,
                192 => Bitrate::Kbps192,
                224 => Bitrate::Kbps224,
                256 => Bitrate::Kbps256,
                _ => Bitrate::Kbps320,
            };
            b.set_vbr_mode(VbrMode::Off).map_err(bad)?;
            b.set_brate(kbps).map_err(bad)?;
        }
    }
    let mut encoder = b.build().map_err(bad)?;
    let mut out = Vec::new();
    let encode_err = |e: mp3lame_encoder::EncodeError| anyhow!("MP3 encoding: {e:?}");
    let frames = audio.channels[0].len();
    for start in (0..frames).step_by(8192) {
        progress.at(start as f32 / frames as f32)?;
        let end = (start + 8192).min(frames);
        let left = &audio.channels[0][start..end];
        out.reserve(mp3lame_encoder::max_required_buffer_size(left.len()));
        if channels == 2 {
            let right = &audio.channels[1][start..end];
            encoder
                .encode_to_vec(DualPcm { left, right }, &mut out)
                .map_err(encode_err)?;
        } else {
            encoder
                .encode_to_vec(MonoPcm(left), &mut out)
                .map_err(encode_err)?;
        }
    }
    out.reserve(mp3lame_encoder::max_required_buffer_size(0).max(7200));
    encoder
        .flush_to_vec::<FlushGap>(&mut out)
        .map_err(encode_err)?;
    // The first frame LAME wrote is a placeholder for the tag: write the tag over it.
    // It writes into the vector's spare room, so make that room first.
    let mut tag = Vec::with_capacity(encoder.lame_tag_size());
    if encoder.lame_tag_size() > 0 && encoder.lame_tag_encode_to_vec(&mut tag).is_some() {
        let at = encoder.id3v2_tag_size();
        if at + tag.len() <= out.len() {
            out[at..at + tag.len()].copy_from_slice(&tag);
        }
    }
    Ok(out)
}

/// An Ogg Vorbis file; quality 0..10 is Vorbis's -0.2..1 scale's 0..1. A fixed stream serial,
/// so an export repeats.
fn ogg(
    audio: &wav::Audio,
    quality: u8,
    metadata: Option<&crate::metadata::Metadata>,
    progress: &Progress,
) -> Result<Vec<u8>> {
    use std::num::{NonZeroU32, NonZeroU8};
    use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};
    let rate = NonZeroU32::new(audio.sample_rate).ok_or_else(|| anyhow!("no sample rate"))?;
    let channels = NonZeroU8::new(audio.channels.len().min(255) as u8)
        .ok_or_else(|| anyhow!("no channels"))?;
    let mut builder =
        VorbisEncoderBuilder::new_with_serial(rate, channels, Vec::new(), 0x4F58_4944);
    builder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr {
        target_quality: f32::from(quality.min(10)) / 10.0,
    });
    if let Some(m) = metadata {
        let tag_err = |e: vorbis_rs::VorbisError| anyhow!("Ogg Vorbis tags: {e}");
        builder
            .comment_tags(crate::metadata::vorbis_comments(m))
            .map_err(tag_err)?;
        if let Some(cover) = &m.cover {
            use base64::Engine;
            let block = crate::metadata::picture_block(cover);
            let encoded = base64::engine::general_purpose::STANDARD.encode(block);
            builder
                .comment_tag("METADATA_BLOCK_PICTURE", encoded)
                .map_err(tag_err)?;
        }
    }
    let mut encoder = builder
        .build()
        .map_err(|e| anyhow!("Ogg Vorbis settings: {e}"))?;
    let frames = audio.channels[0].len();
    for start in (0..frames).step_by(4096) {
        if start % (4096 * 16) == 0 {
            progress.at(start as f32 / frames as f32)?;
        }
        let end = (start + 4096).min(frames);
        let block: Vec<&[f32]> = audio.channels.iter().map(|c| &c[start..end]).collect();
        encoder
            .encode_audio_block(&block)
            .map_err(|e| anyhow!("Ogg Vorbis encoding: {e}"))?;
    }
    encoder
        .finish()
        .map_err(|e| anyhow!("Ogg Vorbis encoding: {e}"))
}

/// `channels` from `from` Hz to `to` Hz with a high-quality FFT resampler, its delay taken
/// off the front, at exactly `round(len × to / from)` frames, so a converted export starts
/// where the audio does.
pub fn resample(channels: &[Vec<f32>], from: u32, to: u32) -> Result<Vec<Vec<f32>>> {
    resample_with(channels, from, to, &Progress::new(&|_| true))
}

fn resample_with(
    channels: &[Vec<f32>],
    from: u32,
    to: u32,
    progress: &Progress,
) -> Result<Vec<Vec<f32>>> {
    use rubato::{FftFixedIn, Resampler};
    let frames = channels.iter().map(Vec::len).max().unwrap_or(0);
    let want = ((frames as u64 * u64::from(to) + u64::from(from) / 2) / u64::from(from)) as usize;
    if from == to || frames == 0 {
        return Ok(channels.to_vec());
    }
    let err = |e: &dyn std::fmt::Display| anyhow!("converting the sample rate: {e}");
    let mut r = FftFixedIn::<f32>::new(from as usize, to as usize, 4096, 2, channels.len())
        .map_err(|e| err(&e))?;
    let delay = r.output_delay();
    let mut out: Vec<Vec<f32>> = vec![Vec::with_capacity(want + delay + 8192); channels.len()];
    let append = |res: Vec<Vec<f32>>, out: &mut Vec<Vec<f32>>| {
        for (o, r) in out.iter_mut().zip(res) {
            o.extend(r);
        }
    };
    let padded: Vec<Vec<f32>> = channels
        .iter()
        .map(|c| {
            let mut c = c.clone();
            c.resize(frames, 0.0);
            c
        })
        .collect();
    let mut at = 0;
    while at + r.input_frames_next() <= frames {
        if at % (4096 * 16) == 0 {
            progress.at(at as f32 / frames as f32)?;
        }
        let need = r.input_frames_next();
        let block: Vec<&[f32]> = padded.iter().map(|c| &c[at..at + need]).collect();
        let res = r.process(&block, None).map_err(|e| err(&e))?;
        append(res, &mut out);
        at += need;
    }
    let rest: Vec<&[f32]> = padded.iter().map(|c| &c[at..]).collect();
    let res = r.process_partial(Some(&rest), None).map_err(|e| err(&e))?;
    append(res, &mut out);
    // Zeros through it until the delayed end of the audio is out.
    while out[0].len() < want + delay {
        let res = r
            .process_partial::<&[f32]>(None, None)
            .map_err(|e| err(&e))?;
        append(res, &mut out);
    }
    for channel in &mut out {
        channel.drain(..delay);
        channel.truncate(want);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One second of a 1 kHz tone at -6 dBFS, from silence at 0.1 s, in stereo (the right
    /// channel half as loud).
    fn tone(rate: u32) -> wav::Audio {
        let n = rate as usize;
        let left: Vec<f32> = (0..n)
            .map(|i| {
                if i < n / 10 {
                    0.0
                } else {
                    0.5 * (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / rate as f32).sin()
                }
            })
            .collect();
        let right = left.iter().map(|x| x * 0.5).collect();
        wav::Audio {
            sample_rate: rate,
            channels: vec![left, right],
        }
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide-encode-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    fn rms_db(x: &[f32]) -> f64 {
        let sum: f64 = x.iter().map(|v| f64::from(*v).powi(2)).sum();
        10.0 * (sum / x.len().max(1) as f64).log10()
    }

    fn encoding(format: Format) -> Encoding {
        Encoding {
            format,
            ..Encoding::default()
        }
    }

    #[test]
    fn the_default_writes_what_it_always_did() {
        let audio = tone(48_000);
        let (new, old) = (temp("default.wav"), temp("old.wav"));
        write(&new, &audio, &Encoding::default()).unwrap();
        wav::write_f32(&old, &audio).unwrap();
        assert_eq!(std::fs::read(&new).unwrap(), std::fs::read(&old).unwrap());
    }

    #[test]
    fn integer_files_hold_the_dithered_samples_exactly() {
        let audio = tone(44_100);
        for (format, name) in [
            (Format::Wav { bits: 16 }, "16.wav"),
            (Format::Wav { bits: 24 }, "24.wav"),
            (Format::Aiff { bits: 16 }, "16.aiff"),
            (Format::Aiff { bits: 24 }, "24.aiff"),
            (Format::Flac { bits: 16, level: 0 }, "16-0.flac"),
            (Format::Flac { bits: 24, level: 5 }, "24-5.flac"),
            (Format::Flac { bits: 16, level: 8 }, "16-8.flac"),
        ] {
            let path = temp(name);
            write(&path, &audio, &encoding(format)).unwrap();
            let read = crate::media::read_audio(&path).unwrap();
            assert_eq!(read.sample_rate, 44_100, "{name}");
            let bits = format.int_bits().unwrap();
            let expected = quantize(&audio.channels, bits, true);
            let scale = f64::from(1u32 << (bits - 1));
            for (c, channel) in read.channels.iter().enumerate() {
                assert_eq!(channel.len(), expected[c].len(), "{name}");
                let ints: Vec<i32> = channel
                    .iter()
                    .map(|x| (f64::from(*x) * scale).round() as i32)
                    .collect();
                assert!(ints == expected[c], "{name}: channel {c} differs");
            }
        }
        // Dither: silence gets ±1 LSB of noise, and the same every time.
        let silent = vec![vec![0.0f32; 1000]];
        let a = quantize(&silent, 16, true);
        assert!(a[0].iter().all(|x| x.abs() <= 1) && a[0].iter().any(|x| *x != 0));
        assert_eq!(a, quantize(&silent, 16, true));
        assert!(quantize(&silent, 16, false)[0].iter().all(|x| *x == 0));
    }

    #[test]
    fn aiff_rates_are_80_bit_floats() {
        assert_eq!(extended(44_100)[..4], [0x40, 0x0E, 0xAC, 0x44]);
        assert_eq!(extended(48_000)[..4], [0x40, 0x0E, 0xBB, 0x80]);
        assert!(extended(48_000)[4..].iter().all(|b| *b == 0));
    }

    #[test]
    fn mp3_and_ogg_come_back_at_their_length_and_level() {
        let audio = tone(48_000);
        let want = rms_db(&audio.channels[0][4800..]);
        for (format, name) in [
            (
                Format::Mp3 {
                    bitrate: Some(320),
                    vbr: None,
                },
                "320.mp3",
            ),
            (
                Format::Mp3 {
                    bitrate: None,
                    vbr: Some(2),
                },
                "v2.mp3",
            ),
            (Format::Ogg { quality: 6 }, "q6.ogg"),
        ] {
            let path = temp(name);
            write(&path, &audio, &encoding(format)).unwrap();
            let read = crate::media::read_audio(&path).unwrap();
            assert_eq!(read.channels.len(), 2, "{name}");
            assert_eq!(read.channels[0].len(), 48_000, "{name}: exact length");
            let level = rms_db(&read.channels[0][4800..]);
            assert!(
                (level - want).abs() < 0.5,
                "{name}: {level:.2} vs {want:.2} dB"
            );
            // It starts where the audio does: silent for its first tenth of a second.
            let onset = read.channels[0]
                .iter()
                .position(|x| x.abs() > 0.05)
                .unwrap();
            assert!(onset.abs_diff(4800) < 48, "{name}: starts at {onset}");
        }
    }

    #[test]
    fn a_converted_export_keeps_its_length_pitch_level_and_start() {
        let audio = tone(48_000);
        let out = resample(&audio.channels, 48_000, 44_100).unwrap();
        assert_eq!(out[0].len(), 44_100);
        let level = rms_db(&out[0][8000..40_000]) - rms_db(&audio.channels[0][8000..40_000]);
        assert!(level.abs() < 0.1, "level {level:.3} dB");
        // 1 kHz: 2000 zero crossings a second; over 0.5 s, 1000.
        let crossings = out[0][10_000..32_050]
            .windows(2)
            .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
            .count();
        assert!(crossings.abs_diff(1000) <= 2, "{crossings} crossings");
        let onset = out[0].iter().position(|x| x.abs() > 0.05).unwrap();
        assert!(onset.abs_diff(4410) <= 4, "starts at {onset}");
    }

    #[test]
    fn writing_reports_its_progress_and_stops_when_cancelled() {
        let audio = tone(48_000);
        let seen = std::cell::RefCell::new(Vec::new());
        let report = |p: f32| {
            seen.borrow_mut().push(p);
            true
        };
        let mp3 = Encoding {
            sample_rate: Some(44_100),
            ..encoding(Format::Mp3 {
                bitrate: Some(192),
                vbr: None,
            })
        };
        write_with(
            &temp("progress.mp3"),
            &audio,
            &mp3,
            None,
            &Progress::new(&report),
        )
        .unwrap();
        let seen = seen.into_inner();
        assert!(seen.len() > 4, "{seen:?}");
        assert!(
            seen.windows(2).all(|w| w[0] <= w[1]),
            "only forward: {seen:?}"
        );
        assert!(seen.iter().any(|p| *p > 0.5) && seen.iter().all(|p| (0.0..=1.0).contains(p)));
        // Cancelled partway: an error, and no file.
        let path = temp("cancelled.mp3");
        let _ = std::fs::remove_file(&path);
        let stop = |p: f32| p < 0.3;
        let err = write_with(&path, &audio, &mp3, None, &Progress::new(&stop)).unwrap_err();
        assert!(format!("{err:#}").contains("cancelled"), "{err:#}");
        assert!(!path.exists());
    }

    #[test]
    fn settings_are_checked_and_extensions_follow_the_format() {
        let mp3 = Format::Mp3 {
            bitrate: Some(320),
            vbr: None,
        };
        assert!(encoding(mp3)
            .check(96_000)
            .unwrap_err()
            .to_string()
            .contains("convert"));
        let ok = Encoding {
            sample_rate: Some(44_100),
            ..encoding(mp3)
        };
        assert!(ok.check(96_000).is_ok());
        assert!(encoding(Format::Wav { bits: 20 }).check(48_000).is_err());
        assert!(encoding(Format::Flac { bits: 16, level: 9 })
            .check(48_000)
            .is_err());
        let both = Format::Mp3 {
            bitrate: Some(128),
            vbr: Some(0),
        };
        assert!(encoding(both).check(48_000).is_err());
        assert_eq!(
            with_extension(Path::new("a"), &mp3).unwrap(),
            PathBuf::from("a.mp3")
        );
        assert!(with_extension(Path::new("a.wav"), &mp3).is_err());
        assert!(with_extension(Path::new("a.aif"), &Format::Aiff { bits: 16 }).is_ok());
    }
}
