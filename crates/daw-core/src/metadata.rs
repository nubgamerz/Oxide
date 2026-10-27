//! A file's tags (dev plan 13.7): title, artist, album and the rest, and cover art, read when a
//! file opens in the Waveform view and written by Save and Export in each format's own way:
//! ID3v2.3 in MP3s (2.3 rather than 2.4, which Windows Explorer and many players don't read),
//! Vorbis comments in FLAC and Ogg (the cover as a PICTURE block), a LIST INFO chunk and an
//! `id3 ` chunk in WAV, an ID3 chunk in AIFF.

use anyhow::{bail, Context, Result};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::Arc;

/// A file's tags. An empty field is no tag.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Metadata {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub year: String,
    /// The track number ("3", or "3/12").
    pub track: String,
    pub genre: String,
    pub composer: String,
    pub comment: String,
    /// Cover art (the front cover).
    #[serde(skip)]
    #[schemars(skip)]
    pub cover: Option<Cover>,
}

/// An embedded image: its MIME type and bytes.
#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    pub mime: String,
    pub data: Arc<Vec<u8>>,
}

impl Cover {
    /// An image file's bytes as a cover: JPEG or PNG, told by its first bytes.
    pub fn from_bytes(data: Vec<u8>) -> Result<Cover> {
        let mime = if data.starts_with(&[0xFF, 0xD8]) {
            "image/jpeg"
        } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
            "image/png"
        } else {
            bail!("cover art must be a JPEG or PNG image");
        };
        Ok(Cover {
            mime: mime.into(),
            data: Arc::new(data),
        })
    }

    pub fn from_path(path: &Path) -> Result<Cover> {
        let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        Self::from_bytes(data).with_context(|| path.display().to_string())
    }
}

impl Metadata {
    pub fn is_empty(&self) -> bool {
        self.fields().iter().all(|(_, v)| v.is_empty()) && self.cover.is_none()
    }

    /// The text fields, with their Vorbis comment names.
    fn fields(&self) -> [(&'static str, &str); 9] {
        [
            ("TITLE", &self.title),
            ("ARTIST", &self.artist),
            ("ALBUM", &self.album),
            ("ALBUMARTIST", &self.album_artist),
            ("DATE", &self.year),
            ("TRACKNUMBER", &self.track),
            ("GENRE", &self.genre),
            ("COMPOSER", &self.composer),
            ("COMMENT", &self.comment),
        ]
    }

    fn set_field(&mut self, key: &str, value: &str) {
        let field = match key.to_ascii_uppercase().as_str() {
            "TITLE" => &mut self.title,
            "ARTIST" => &mut self.artist,
            "ALBUM" => &mut self.album,
            "ALBUMARTIST" | "ALBUM ARTIST" => &mut self.album_artist,
            "DATE" | "YEAR" => &mut self.year,
            "TRACKNUMBER" => &mut self.track,
            "GENRE" => &mut self.genre,
            "COMPOSER" => &mut self.composer,
            "COMMENT" | "DESCRIPTION" => &mut self.comment,
            _ => return,
        };
        if field.is_empty() {
            *field = value.to_string();
        }
    }
}

/// The tags of the file at `path`, by its extension. A file without tags, or with tags this
/// can't read, gives none: opening a file never fails over its tags.
pub fn read(path: &Path) -> Metadata {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let read = match ext.as_str() {
        "mp3" => id3::Tag::read_from_path(path).map(|t| from_id3(&t)).ok(),
        "wav" | "wave" => id3::Tag::read_from_path(path)
            .map(|t| from_id3(&t))
            .ok()
            .or_else(|| read_list_info(path).ok()),
        "aif" | "aiff" | "aifc" => id3::Tag::read_from_path(path).map(|t| from_id3(&t)).ok(),
        "flac" => read_flac(path).ok(),
        "ogg" => read_symphonia(path).ok(),
        _ => None,
    };
    read.unwrap_or_default()
}

// ---------------------------------------------------------------------------- ID3

fn from_id3(tag: &id3::Tag) -> Metadata {
    use id3::TagLike;
    let text = |s: Option<&str>| s.unwrap_or_default().to_string();
    let track = match (tag.track(), tag.total_tracks()) {
        (Some(n), Some(t)) => format!("{n}/{t}"),
        (Some(n), None) => n.to_string(),
        _ => String::new(),
    };
    let year = tag
        .year()
        .map(|y| y.to_string())
        .or_else(|| tag.date_recorded().map(|d| d.year.to_string()))
        .unwrap_or_default();
    let cover = tag
        .pictures()
        .find(|p| p.picture_type == id3::frame::PictureType::CoverFront)
        .or_else(|| tag.pictures().next())
        .map(|p| Cover {
            mime: p.mime_type.clone(),
            data: Arc::new(p.data.clone()),
        });
    Metadata {
        title: text(tag.title()),
        artist: text(tag.artist()),
        album: text(tag.album()),
        album_artist: text(tag.album_artist()),
        year,
        track,
        genre: text(tag.genre_parsed().as_deref()),
        composer: tag
            .get("TCOM")
            .and_then(|f| f.content().text())
            .unwrap_or_default()
            .to_string(),
        comment: tag
            .comments()
            .next()
            .map(|c| c.text.clone())
            .unwrap_or_default(),
        cover,
    }
}

fn to_id3(m: &Metadata) -> id3::Tag {
    use id3::TagLike;
    let mut tag = id3::Tag::new();
    let set = |tag: &mut id3::Tag, id: &str, value: &str| {
        if !value.is_empty() {
            tag.set_text(id, value);
        }
    };
    set(&mut tag, "TIT2", &m.title);
    set(&mut tag, "TPE1", &m.artist);
    set(&mut tag, "TALB", &m.album);
    set(&mut tag, "TPE2", &m.album_artist);
    set(&mut tag, "TYER", &m.year);
    set(&mut tag, "TRCK", &m.track);
    set(&mut tag, "TCON", &m.genre);
    set(&mut tag, "TCOM", &m.composer);
    if !m.comment.is_empty() {
        tag.add_frame(id3::frame::Comment {
            lang: "eng".into(),
            description: String::new(),
            text: m.comment.clone(),
        });
    }
    if let Some(cover) = &m.cover {
        tag.add_frame(id3::frame::Picture {
            mime_type: cover.mime.clone(),
            picture_type: id3::frame::PictureType::CoverFront,
            description: String::new(),
            data: cover.data.to_vec(),
        });
    }
    tag
}

/// `m` as an ID3v2.3 tag, to put in front of an MP3's frames.
pub fn id3_bytes(m: &Metadata) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    to_id3(m)
        .write_to(&mut out, id3::Version::Id3v23)
        .context("writing the ID3 tag")?;
    Ok(out)
}

// ---------------------------------------------------------------------------- WAV and AIFF

/// Tag a finished WAV file: a LIST INFO chunk (what Windows Explorer shows) and an `id3 `
/// chunk (everything, the cover too).
pub fn tag_wav(path: &Path, m: &Metadata) -> Result<()> {
    let mut bytes = std::fs::read(path)?;
    if bytes.len() < 12 || &bytes[..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        bail!("{} isn't a WAV file", path.display());
    }
    let riff = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]) as usize;
    if bytes.len() != 8 + riff {
        bail!("{}: its RIFF size doesn't match the file", path.display());
    }
    // A chunk starts on an even byte: pad the last one if it ends odd.
    if bytes.len() % 2 == 1 {
        bytes.push(0);
    }
    let mut info = b"INFO".to_vec();
    for (id, value) in [
        (b"INAM", &m.title),
        (b"IART", &m.artist),
        (b"IPRD", &m.album),
        (b"ICRD", &m.year),
        (b"ITRK", &m.track),
        (b"IGNR", &m.genre),
        (b"ICMT", &m.comment),
    ] {
        if value.is_empty() {
            continue;
        }
        let mut text = value.as_bytes().to_vec();
        text.push(0);
        info.extend_from_slice(id);
        info.extend_from_slice(&(text.len() as u32).to_le_bytes());
        info.extend_from_slice(&text);
        if text.len() % 2 == 1 {
            info.push(0);
        }
    }
    if info.len() > 4 {
        bytes.extend_from_slice(b"LIST");
        bytes.extend_from_slice(&(info.len() as u32).to_le_bytes());
        bytes.extend_from_slice(&info);
    }
    let riff = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&riff.to_le_bytes());
    std::fs::write(path, bytes)?;
    to_id3(m)
        .write_to_path(path, id3::Version::Id3v23)
        .context("writing the WAV's ID3 chunk")?;
    Ok(())
}

/// A WAV's LIST INFO chunk, for files tagged by other programs (no `id3 ` chunk).
fn read_list_info(path: &Path) -> Result<Metadata> {
    let bytes = std::fs::read(path)?;
    let mut m = Metadata::default();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let id = &bytes[at..at + 4];
        let size = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let body = &bytes[at + 8..(at + 8 + size).min(bytes.len())];
        if id == b"LIST" && body.starts_with(b"INFO") {
            let mut i = 4;
            while i + 8 <= body.len() {
                let sub = &body[i..i + 4];
                let n = u32::from_le_bytes([body[i + 4], body[i + 5], body[i + 6], body[i + 7]])
                    as usize;
                let text = &body[i + 8..(i + 8 + n).min(body.len())];
                let text = String::from_utf8_lossy(text)
                    .trim_end_matches('\0')
                    .to_string();
                let field = match sub {
                    b"INAM" => Some(&mut m.title),
                    b"IART" => Some(&mut m.artist),
                    b"IPRD" => Some(&mut m.album),
                    b"ICRD" => Some(&mut m.year),
                    b"ITRK" => Some(&mut m.track),
                    b"IGNR" => Some(&mut m.genre),
                    b"ICMT" => Some(&mut m.comment),
                    _ => None,
                };
                if let Some(field) = field {
                    *field = text;
                }
                i += 8 + n + n % 2;
            }
        }
        at += 8 + size + size % 2;
    }
    Ok(m)
}

/// Tag a finished AIFF file: an ID3 chunk.
pub fn tag_aiff(path: &Path, m: &Metadata) -> Result<()> {
    to_id3(m)
        .write_to_path(path, id3::Version::Id3v23)
        .context("writing the AIFF's ID3 chunk")?;
    Ok(())
}

// ---------------------------------------------------------------------------- Vorbis comments

/// The comments as `KEY=value` pairs, as Vorbis comments and FLAC hold them. A track "3/12"
/// is TRACKNUMBER=3 and TRACKTOTAL=12, as readers expect.
pub fn vorbis_comments(m: &Metadata) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for (k, v) in m.fields().iter().filter(|(_, v)| !v.is_empty()) {
        match v.split_once('/').filter(|_| *k == "TRACKNUMBER") {
            Some((n, total)) => {
                out.push((k.to_string(), n.trim().to_string()));
                out.push(("TRACKTOTAL".to_string(), total.trim().to_string()));
            }
            None => out.push((k.to_string(), v.to_string())),
        }
    }
    out
}

/// The track number with the total read beside it ("3" and "12": "3/12").
fn with_total(track: &mut String, total: Option<String>) {
    if let Some(total) = total.filter(|t| !t.is_empty() && !track.contains('/')) {
        if !track.is_empty() {
            *track = format!("{track}/{total}");
        }
    }
}

/// A FLAC PICTURE block's body (all big-endian): the front cover. Ogg Vorbis carries the same
/// bytes, base64'd, as a METADATA_BLOCK_PICTURE comment.
pub fn picture_block(cover: &Cover) -> Vec<u8> {
    let mut out = Vec::new();
    let be = |out: &mut Vec<u8>, n: u32| out.extend_from_slice(&n.to_be_bytes());
    be(&mut out, 3); // the front cover
    be(&mut out, cover.mime.len() as u32);
    out.extend_from_slice(cover.mime.as_bytes());
    be(&mut out, 0); // no description
    for _ in 0..4 {
        be(&mut out, 0); // width, height, depth, colours: unknown
    }
    be(&mut out, cover.data.len() as u32);
    out.extend_from_slice(&cover.data);
    out
}

fn read_picture_block(body: &[u8]) -> Option<Cover> {
    let be = |at: usize| -> Option<usize> {
        Some(u32::from_be_bytes(body.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let mime_len = be(4)?;
    let mime = String::from_utf8_lossy(body.get(8..8 + mime_len)?).into_owned();
    let mut at = 8 + mime_len;
    let desc = be(at)?;
    at += 4 + desc + 16;
    let len = be(at)?;
    let data = body.get(at + 4..at + 4 + len)?.to_vec();
    Some(Cover {
        mime,
        data: Arc::new(data),
    })
}

/// FLAC bytes (`fLaC`, then STREAMINFO alone) with a VORBIS_COMMENT block and, with a cover,
/// a PICTURE block after STREAMINFO; the last one is flagged as the last.
pub fn tag_flac(bytes: &mut Vec<u8>, m: &Metadata) -> Result<()> {
    if bytes.len() < 42 || &bytes[..4] != b"fLaC" || bytes[4] & 0x7F != 0 {
        bail!("not a FLAC stream that starts with its STREAMINFO");
    }
    if bytes[4] & 0x80 == 0 {
        bail!("this FLAC stream already has metadata blocks after STREAMINFO");
    }
    let mut comment = Vec::new();
    let le = |out: &mut Vec<u8>, n: usize| out.extend_from_slice(&(n as u32).to_le_bytes());
    let vendor = b"Oxide";
    le(&mut comment, vendor.len());
    comment.extend_from_slice(vendor);
    let pairs = vorbis_comments(m);
    le(&mut comment, pairs.len());
    for (k, v) in &pairs {
        let line = format!("{k}={v}");
        le(&mut comment, line.len());
        comment.extend_from_slice(line.as_bytes());
    }
    let mut blocks: Vec<(u8, Vec<u8>)> = vec![(4, comment)];
    if let Some(cover) = &m.cover {
        blocks.push((6, picture_block(cover)));
    }
    let mut inserted = Vec::new();
    let count = blocks.len();
    for (i, (kind, body)) in blocks.into_iter().enumerate() {
        let last = if i + 1 == count { 0x80 } else { 0 };
        inserted.push(kind | last);
        inserted.extend_from_slice(&(body.len() as u32).to_be_bytes()[1..]);
        inserted.extend_from_slice(&body);
    }
    bytes[4] &= 0x7F; // STREAMINFO is no longer the last block
    let at = 8 + 34;
    bytes.splice(at..at, inserted);
    Ok(())
}

fn read_flac(path: &Path) -> Result<Metadata> {
    let bytes = std::fs::read(path)?;
    if &bytes[..4.min(bytes.len())] != b"fLaC" {
        bail!("not a FLAC file");
    }
    let mut m = Metadata::default();
    let mut at = 4;
    while at + 4 <= bytes.len() {
        let header = bytes[at];
        let len = u32::from_be_bytes([0, bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let body = bytes.get(at + 4..at + 4 + len).unwrap_or_default();
        match header & 0x7F {
            4 => read_vorbis_comment(body, &mut m),
            6 if m.cover.is_none() => m.cover = read_picture_block(body),
            _ => {}
        }
        if header & 0x80 != 0 {
            break;
        }
        at += 4 + len;
    }
    Ok(m)
}

/// A VORBIS_COMMENT body (little-endian lengths) into `m`.
fn read_vorbis_comment(body: &[u8], m: &mut Metadata) {
    let le = |at: usize| -> Option<usize> {
        Some(u32::from_le_bytes(body.get(at..at + 4)?.try_into().ok()?) as usize)
    };
    let Some(vendor) = le(0) else { return };
    let mut at = 4 + vendor;
    let Some(count) = le(at) else { return };
    at += 4;
    let mut total = None;
    for _ in 0..count {
        let Some(len) = le(at) else { break };
        let Some(line) = body.get(at + 4..at + 4 + len) else {
            break;
        };
        let line = String::from_utf8_lossy(line);
        if let Some((k, v)) = line.split_once('=') {
            if k.eq_ignore_ascii_case("TRACKTOTAL") || k.eq_ignore_ascii_case("TOTALTRACKS") {
                total = Some(v.to_string());
            } else {
                m.set_field(k, v);
            }
        }
        at += 4 + len;
    }
    with_total(&mut m.track, total);
}

/// Ogg Vorbis comments, through Symphonia.
fn read_symphonia(path: &Path) -> Result<Metadata> {
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::FormatOptions;
    use symphonia::core::io::MediaSourceStream;
    use symphonia::core::meta::{MetadataOptions, StandardTag};
    let file = std::fs::File::open(path)?;
    let source = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        source,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;
    let mut m = Metadata::default();
    let mut total = None;
    let mut metadata = format.metadata();
    let Some(revision) = metadata.skip_to_latest() else {
        return Ok(m);
    };
    for tag in &revision.media.tags {
        let (key, value) = match &tag.std {
            Some(StandardTag::TrackTitle(v)) => ("TITLE", v.to_string()),
            Some(StandardTag::Artist(v)) => ("ARTIST", v.to_string()),
            Some(StandardTag::Album(v)) => ("ALBUM", v.to_string()),
            Some(StandardTag::AlbumArtist(v)) => ("ALBUMARTIST", v.to_string()),
            Some(StandardTag::Genre(v)) => ("GENRE", v.to_string()),
            Some(StandardTag::Composer(v)) => ("COMPOSER", v.to_string()),
            Some(StandardTag::Comment(v)) => ("COMMENT", v.to_string()),
            Some(StandardTag::RecordingDate(v) | StandardTag::ReleaseDate(v)) => {
                ("DATE", v.to_string())
            }
            Some(StandardTag::ReleaseYear(y) | StandardTag::RecordingYear(y)) => {
                ("DATE", y.to_string())
            }
            Some(StandardTag::TrackNumber(n)) => ("TRACKNUMBER", n.to_string()),
            Some(StandardTag::TrackTotal(n)) => {
                total = Some(n.to_string());
                continue;
            }
            _ => continue,
        };
        m.set_field(key, &value);
    }
    with_total(&mut m.track, total);
    m.cover = revision.media.visuals.first().map(|v| Cover {
        mime: v.media_type.clone().unwrap_or_else(|| "image/png".into()),
        data: Arc::new(v.data.to_vec()),
    });
    Ok(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encode::{Encoding, Format, Progress};
    use crate::wav;

    fn tags() -> Metadata {
        // A tiny PNG: its signature is what makes it one.
        let png = [
            0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 1, 2, 3, 4, 5, 6, 7, 8,
        ];
        Metadata {
            title: "Long Road Home".into(),
            artist: "The Testers".into(),
            album: "Rust Never Sleeps".into(),
            album_artist: "Various".into(),
            year: "2026".into(),
            track: "3/12".into(),
            genre: "Rock".into(),
            composer: "J. Doe".into(),
            comment: "Mixed in Oxide; é ü ✓".into(),
            cover: Some(Cover::from_bytes(png.to_vec()).unwrap()),
        }
    }

    fn audio() -> wav::Audio {
        let n = 24_000;
        let left: Vec<f32> = (0..n).map(|i| 0.3 * (i as f32 * 0.05).sin()).collect();
        wav::Audio {
            sample_rate: 48_000,
            channels: vec![left.clone(), left],
        }
    }

    /// Every format, every field and the cover: written and read back the same, and the audio
    /// still reads the same with the tags in.
    #[test]
    fn tags_round_trip_in_every_format() {
        let dir = std::env::temp_dir().join(format!("oxide-tags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = tags();
        let none = |_| true;
        for (name, format) in [
            ("a.wav", Format::Wav { bits: 32 }),
            ("b.wav", Format::Wav { bits: 24 }),
            ("c.aiff", Format::Aiff { bits: 16 }),
            ("d.flac", Format::Flac { bits: 16, level: 5 }),
            (
                "e.mp3",
                Format::Mp3 {
                    bitrate: Some(192),
                    vbr: None,
                },
            ),
            ("f.ogg", Format::Ogg { quality: 5 }),
        ] {
            let (tagged, plain) = (dir.join(name), dir.join(format!("plain-{name}")));
            let encoding = Encoding {
                format,
                ..Encoding::default()
            };
            let progress = Progress::new(&none);
            crate::encode::write_with(&tagged, &audio(), &encoding, Some(&m), &progress).unwrap();
            crate::encode::write_with(&plain, &audio(), &encoding, None, &progress).unwrap();
            let read = read(&tagged);
            // ID3v2.3 holds the year as YYYY; ID3 doesn't know an album artist's frame by
            // that name in v1, but TPE2 is it.
            assert_eq!(read, m, "{name}");
            assert_eq!(super::read(&plain), Metadata::default(), "{name}: untagged");
            let (a, b) = (
                crate::media::read_audio(&tagged).unwrap(),
                crate::media::read_audio(&plain).unwrap(),
            );
            assert!(
                a.channels == b.channels,
                "{name}: the audio is the same with tags"
            );
        }
    }

    #[test]
    fn covers_are_jpeg_or_png() {
        assert_eq!(
            Cover::from_bytes(vec![0xFF, 0xD8, 0]).unwrap().mime,
            "image/jpeg"
        );
        assert!(Cover::from_bytes(b"GIF89a".to_vec()).is_err());
    }
}
