//! The manual's screenshots (dev plan 27.2): the list of shots the `manual` tier takes, and
//! the images the figures show, cut and compressed when the manual is written.
//!
//! - `shots.toml` (in the manual's folder) names each shot and the app arguments that make it
//!   (`--show` scenario, `--mixer`, a window size). `check.ps1 -Tier manual` runs the release
//!   app once per shot (`manual --list-shots` gives it the command lines), with `--screenshot`
//!   and `--marks`. Every figure's shot must be listed (the checker refuses one that isn't).
//! - [`image_file`] cuts a screenshot to a figure's [`crate::figure::Plan`], scales it down if
//!   it is wider than [`crate::figure::MAX_WIDTH`], and encodes it as a PNG: with a palette when
//!   the picture has 256 colours or fewer (exact), or when 256 chosen ones (NeuQuant,
//!   `color_quant`, MIT) stay within [`MIN_PSNR`] of it; else in full colour. The app's flat
//!   dark UI quantises cleanly: its text keeps its edges, at about half the bytes.
//! - The shots themselves are committed in `docs/manual/shots/` (28.1), packed by [`pack`]
//!   (`manual --pack-shots DIR`, which `scripts/manual_shots.ps1` runs after taking them):
//!   re-encoded without loss, so their marks still fit them pixel for pixel.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{bail, Context, Result};
use image::RgbImage;
use serde::Deserialize;

/// The shot list's file, in the manual's folder.
pub const LIST: &str = "shots.toml";

/// The closest a palette must keep a picture to its colours (peak signal to noise, dB) to be
/// used: above about 45 dB nothing is visible; the app's shots measure about 49-67.
pub const MIN_PSNR: f64 = 45.0;

/// `shots.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotList {
    pub shots: BTreeMap<String, Shot>,
}

/// One shot: what it shows and how the app is run to take it.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shot {
    /// What the picture shows (for whoever edits the list).
    pub about: String,
    /// The app's arguments after `--demo <wav> --mute`: a `--show` scenario, `--mixer`, ...
    #[serde(default)]
    pub args: Vec<String>,
    /// The window's size in points, `"1280x800"` (the app's own default otherwise).
    pub size: Option<String>,
}

impl ShotList {
    pub fn read(text: &str) -> Result<ShotList> {
        let list: ShotList = toml::from_str(text).context("reading shots.toml")?;
        for (name, shot) in &list.shots {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                bail!("shots.toml: \"{name}\" isn't a shot name (lower-case letters, digits, -)");
            }
            if let Some(size) = &shot.size {
                let ok = size
                    .split_once('x')
                    .is_some_and(|(w, h)| w.parse::<u32>().is_ok() && h.parse::<u32>().is_ok());
                if !ok {
                    bail!("shots.toml: {name}'s size \"{size}\" isn't WIDTHxHEIGHT");
                }
            }
        }
        Ok(list)
    }

    /// One line a shot: its name, then the app's arguments for it, separated by tabs.
    pub fn command_lines(&self) -> String {
        let mut out = String::new();
        for (name, shot) in &self.shots {
            let mut fields = vec![name.clone()];
            fields.extend(shot.args.iter().cloned());
            if let Some(size) = &shot.size {
                fields.extend(["--size".to_string(), size.clone()]);
            }
            out.push_str(&fields.join("\t"));
            out.push('\n');
        }
        out
    }
}

/// How an image was written.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Encoding {
    /// A palette of every colour the picture has (no loss).
    Exact,
    /// A palette of 256 chosen colours, this close (PSNR, dB).
    Quantised(f64),
    /// Full colour (no palette kept the picture close enough).
    FullColour,
}

/// A figure's image: `source` (a screenshot) cut to `crop` (x, y, w, h), scaled to `size` if
/// that differs, and encoded. Returns the PNG's bytes and how it was encoded.
pub fn image_file(
    source: &RgbImage,
    crop: [u32; 4],
    size: [u32; 2],
) -> Result<(Vec<u8>, Encoding)> {
    let [x, y, w, h] = crop;
    if x + w > source.width() || y + h > source.height() || w == 0 || h == 0 {
        bail!(
            "the crop {x} {y} {w} {h} isn't inside the {}x{} screenshot (its marks file \
             doesn't match the picture)",
            source.width(),
            source.height()
        );
    }
    let mut picture = image::imageops::crop_imm(source, x, y, w, h).to_image();
    if [w, h] != size {
        picture = image::imageops::resize(
            &picture,
            size[0],
            size[1],
            image::imageops::FilterType::Lanczos3,
        );
    }
    encode(&picture)
}

/// Reads a screenshot as RGB (the app writes RGBA with every pixel opaque).
pub fn read(path: &Path) -> Result<RgbImage> {
    Ok(image::open(path)
        .with_context(|| format!("reading {}", path.display()))?
        .to_rgb8())
}

/// Encodes `picture` as small as it stays clean: see the module's docs.
pub fn encode(picture: &RgbImage) -> Result<(Vec<u8>, Encoding)> {
    let pixels = picture.as_raw();
    // Every colour, while there are no more than a palette holds.
    let mut exact: HashMap<[u8; 3], u8> = HashMap::new();
    let mut fits = true;
    for p in pixels.chunks_exact(3) {
        let key = [p[0], p[1], p[2]];
        if !exact.contains_key(&key) {
            if exact.len() == 256 {
                fits = false;
                break;
            }
            let n = exact.len() as u8;
            exact.insert(key, n);
        }
    }
    if fits {
        let mut palette = vec![0u8; exact.len() * 3];
        for (colour, &n) in &exact {
            palette[usize::from(n) * 3..usize::from(n) * 3 + 3].copy_from_slice(colour);
        }
        let indices: Vec<u8> = pixels
            .chunks_exact(3)
            .map(|p| exact[&[p[0], p[1], p[2]]])
            .collect();
        return Ok((indexed(picture, &palette, &indices)?, Encoding::Exact));
    }

    // 256 colours chosen by NeuQuant, each pixel given the nearest (looked up once a colour).
    let rgba: Vec<u8> = pixels
        .chunks_exact(3)
        .flat_map(|p| [p[0], p[1], p[2], 255])
        .collect();
    let quant = color_quant::NeuQuant::new(1, 256, &rgba);
    let palette = quant.color_map_rgb();
    let mut nearest: HashMap<[u8; 3], u8> = HashMap::new();
    let mut error = 0.0f64;
    let mut indices = Vec::with_capacity(pixels.len() / 3);
    for p in pixels.chunks_exact(3) {
        let key = [p[0], p[1], p[2]];
        let n = *nearest
            .entry(key)
            .or_insert_with(|| quant.index_of(&[p[0], p[1], p[2], 255]) as u8);
        let q = &palette[usize::from(n) * 3..usize::from(n) * 3 + 3];
        for c in 0..3 {
            let d = f64::from(p[c]) - f64::from(q[c]);
            error += d * d;
        }
        indices.push(n);
    }
    let psnr = psnr(error, pixels.len());
    if psnr >= MIN_PSNR {
        return Ok((
            indexed(picture, &palette, &indices)?,
            Encoding::Quantised(psnr),
        ));
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, picture.width(), picture.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(pixels)?;
    writer.finish()?;
    Ok((bytes, Encoding::FullColour))
}

/// Encodes `picture` without losing a pixel, as small as this encoder gets it: a palette of
/// every colour when there are 256 or fewer, else full colour (RGB), adaptively filtered at
/// the strongest compression. The committed shots are kept this way (28.1): the manual cuts
/// and compresses its figures from them, so they must be the app's pixels exactly.
pub fn encode_lossless(picture: &RgbImage) -> Result<Vec<u8>> {
    if let (bytes, Encoding::Exact) = encode(picture)? {
        return Ok(bytes);
    }
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, picture.width(), picture.height());
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_deflate_compression(png::DeflateCompression::Level(9));
    encoder.set_filter(png::Filter::Adaptive);
    let mut writer = encoder.write_header()?;
    writer.write_image_data(picture.as_raw())?;
    writer.finish()?;
    Ok(bytes)
}

/// Rewrites the screenshot at `path` with [`encode_lossless`] when that is smaller, keeping
/// its pixels (and so its marks) exactly; a picture with any transparency is left alone.
/// Returns its size before and after.
pub fn pack(path: &Path) -> Result<(u64, u64)> {
    let before = std::fs::metadata(path)
        .with_context(|| format!("reading {}", path.display()))?
        .len();
    let image = image::open(path).with_context(|| format!("reading {}", path.display()))?;
    if image.color().has_alpha() && image.to_rgba8().pixels().any(|p| p[3] != 255) {
        return Ok((before, before));
    }
    let bytes = encode_lossless(&image.to_rgb8())?;
    if (bytes.len() as u64) < before {
        std::fs::write(path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        return Ok((before, bytes.len() as u64));
    }
    Ok((before, before))
}

/// Peak signal to noise (dB) of a squared error summed over `samples` values of 0-255.
fn psnr(squared_error: f64, samples: usize) -> f64 {
    if squared_error == 0.0 {
        return f64::INFINITY;
    }
    let mean = squared_error / samples as f64;
    10.0 * (255.0 * 255.0 / mean).log10()
}

/// An 8-bit palette PNG, the smaller of unfiltered and adaptively filtered (palette indices
/// usually compress best unfiltered; a picture with gradients sometimes doesn't).
fn indexed(picture: &RgbImage, palette: &[u8], indices: &[u8]) -> Result<Vec<u8>> {
    let mut best: Option<Vec<u8>> = None;
    for filter in [png::Filter::NoFilter, png::Filter::Adaptive] {
        let mut bytes = Vec::new();
        let mut encoder = png::Encoder::new(&mut bytes, picture.width(), picture.height());
        encoder.set_color(png::ColorType::Indexed);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_palette(palette.to_vec());
        encoder.set_deflate_compression(png::DeflateCompression::Level(9));
        encoder.set_filter(filter);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(indices)?;
        writer.finish()?;
        if best.as_ref().is_none_or(|b| bytes.len() < b.len()) {
            best = Some(bytes);
        }
    }
    Ok(best.unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(bytes: &[u8]) -> RgbImage {
        image::load_from_memory(bytes).expect("a PNG").to_rgb8()
    }

    #[test]
    fn packing_keeps_every_pixel_and_never_grows_a_file() {
        // Many colours (a gradient with noise), as an anti-aliased screenshot has.
        let picture = RgbImage::from_fn(120, 80, |x, y| {
            image::Rgb([(x * 2) as u8, (y * 3) as u8, ((x * 7 + y * 13) % 251) as u8])
        });
        assert_eq!(decode(&encode_lossless(&picture).unwrap()), picture);
        // Written as the app writes a shot (RGBA, opaque, light compression), then packed.
        let dir = std::env::temp_dir().join(format!("oxide-pack-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("shot.png");
        image::DynamicImage::ImageRgb8(picture.clone())
            .to_rgba8()
            .save(&path)
            .unwrap();
        let (before, after) = pack(&path).unwrap();
        assert!(after <= before, "{after} > {before}");
        assert_eq!(read(&path).unwrap(), picture);
        // Packing again changes nothing.
        let (again, same) = pack(&path).unwrap();
        assert_eq!((again, same), (after, after));
        // Few colours: an exact palette.
        let flat = RgbImage::from_fn(64, 64, |x, _| {
            image::Rgb([if x < 32 { 20 } else { 200 }, 30, 40])
        });
        assert_eq!(decode(&encode_lossless(&flat).unwrap()), flat);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn few_colours_are_kept_exactly_in_a_palette() {
        // 8 x 4 x 3 = 96 colours.
        let picture = RgbImage::from_fn(40, 20, |x, y| {
            image::Rgb([
                (x / 5 * 30) as u8,
                (y / 5 * 60) as u8,
                ((x + y) % 3) as u8 * 80,
            ])
        });
        let (bytes, encoding) = encode(&picture).expect("encoded");
        assert_eq!(encoding, Encoding::Exact);
        assert_eq!(decode(&bytes), picture);
    }

    #[test]
    fn many_colours_are_quantised_when_close_or_kept_whole() {
        // Like a UI's anti-aliased text: flat colours and the blends between them, 600 in all
        // (more than a palette holds), 256 of them keeping it close.
        let lerp =
            |a: [u8; 3], b: [u8; 3], t: f64| {
                image::Rgb([0, 1, 2].map(|c| {
                    (f64::from(a[c]) + (f64::from(b[c]) - f64::from(a[c])) * t).round() as u8
                }))
            };
        let smooth = RgbImage::from_fn(300, 40, |x, y| {
            let t = f64::from(x) / 299.0;
            if y < 20 {
                lerp([0x1b, 0x1d, 0x22], [0xe0, 0xe4, 0xea], t)
            } else {
                lerp([0x1b, 0x1d, 0x22], [0x4a, 0xa8, 0xff], t)
            }
        });
        let (bytes, encoding) = encode(&smooth).expect("encoded");
        let Encoding::Quantised(psnr) = encoding else {
            panic!("expected a palette, got {encoding:?}");
        };
        assert!(psnr >= MIN_PSNR, "{psnr}");
        assert_eq!(decode(&bytes).dimensions(), smooth.dimensions());
        // Noise: no palette keeps it, so it stays in full colour, exactly.
        let mut seed = 1u32;
        let noise = RgbImage::from_fn(64, 64, |_, _| {
            let mut next = || {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (seed >> 24) as u8
            };
            image::Rgb([next(), next(), next()])
        });
        let (bytes, encoding) = encode(&noise).expect("encoded");
        assert_eq!(encoding, Encoding::FullColour);
        assert_eq!(decode(&bytes), noise);
    }

    #[test]
    fn a_figure_image_is_its_crop_scaled() {
        let source = RgbImage::from_fn(100, 50, |x, y| {
            image::Rgb([
                if x >= 20 { 200 } else { 0 },
                if y >= 10 { 100 } else { 0 },
                0,
            ])
        });
        let (bytes, _) = image_file(&source, [20, 10, 40, 20], [40, 20]).expect("made");
        let made = decode(&bytes);
        assert_eq!(made.dimensions(), (40, 20));
        assert!(made.pixels().all(|p| p.0 == [200, 100, 0]));
        let (bytes, _) = image_file(&source, [0, 0, 100, 50], [50, 25]).expect("made");
        assert_eq!(decode(&bytes).dimensions(), (50, 25));
        assert!(image_file(&source, [90, 0, 20, 10], [20, 10]).is_err());
    }

    #[test]
    fn the_list_gives_each_shot_its_command_line() {
        let list = ShotList::read(
            "[shots.main-window]\nabout = \"The window\"\n\n[shots.settings-audio]\nabout = \"Audio\"\nargs = [\"--show\", \"settings-audio\"]\nsize = \"1280x800\"\n",
        )
        .expect("read");
        assert_eq!(
            list.command_lines(),
            "main-window\nsettings-audio\t--show\tsettings-audio\t--size\t1280x800\n"
        );
        assert!(ShotList::read("[shots.Bad_Name]\nabout = \"x\"\n").is_err());
        assert!(ShotList::read("[shots.a]\nabout = \"x\"\nsize = \"big\"\n").is_err());
        assert!(ShotList::read("[shots.a]\nabout = \"x\"\nflags = []\n").is_err());
    }
}
