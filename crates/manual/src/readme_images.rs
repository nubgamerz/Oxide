//! The README's pictures (dev plan 28.3): cut from the manual's committed screenshots
//! (`docs/manual/shots`, neutral, never retaken for the README) and compressed with the
//! manual's own encoder ([`crate::shots::image_file`]), so they look like the manual's figures
//! and cost no extra app run.
//!
//! The list is `docs/images/images.toml`: one table an image, its file name the table's name.
//!
//! ```toml
//! [images.piano-roll]
//! about = "The piano roll under the Multitrack: Keys' chords"
//! shot = "piano-roll"            # docs/manual/shots/piano-roll.png and .json
//! marks = ["pane.piano_roll"]    # the rect round these marks (names from the shot's JSON)
//! pad = 4                        # grown by this many pixels each side
//! # rect = [0, 25, 1136, 545]    # or a rect in the shot's pixels (x, y, w, h)
//! max_width = 1280               # scaled down (Lanczos) past this width
//! ```
//!
//! `manual --readme-images docs/images/images.toml --out docs/images` (what
//! `scripts/readme_images.ps1` runs) writes `<name>.png` for each. The same shots and list
//! give the same bytes: the crop is whole pixels, the scaling and NeuQuant are deterministic
//! and the palette is numbered in the order colours first appear.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::figure::{crop_rect, scaled_size, Crop, Marks};
use crate::shots::{self, Encoding};

/// The list's file name, in the images' folder.
pub const LIST: &str = "images.toml";

/// The most the README's images may weigh together (about 1.5 MB, dev plan 28.3): GitHub's
/// front page loads them all.
pub const MAX_TOTAL: u64 = 1_536 * 1024;

/// `images.toml`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageList {
    pub images: BTreeMap<String, Image>,
}

/// One README image: which shot, which part of it, how wide at most.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Image {
    /// What the picture shows (for whoever edits the list, and the README's caption).
    pub about: String,
    /// The screenshot's name in the manual's `shots` folder (no extension).
    pub shot: String,
    /// Crop to the rect round these marks.
    #[serde(default)]
    pub marks: Vec<String>,
    /// Pixels added round `marks` each side.
    #[serde(default)]
    pub pad: f64,
    /// Or crop to this rect (x, y, w, h) in the shot's pixels.
    pub rect: Option<[f64; 4]>,
    /// Scaled down past this width (the crop's own width otherwise).
    pub max_width: Option<u32>,
}

impl ImageList {
    pub fn read(text: &str) -> Result<ImageList> {
        let list: ImageList = toml::from_str(text).context("reading images.toml")?;
        if list.images.is_empty() {
            bail!("images.toml lists no images");
        }
        for (name, image) in &list.images {
            if name.is_empty()
                || !name
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                bail!(
                    "images.toml: \"{name}\" isn't an image name (lower-case letters, digits, -)"
                );
            }
            if image.rect.is_some() && !image.marks.is_empty() {
                bail!("images.toml: {name} gives both marks and a rect; pick one");
            }
            // A pad only grows a crop round marks.
            if image.marks.is_empty() && image.pad != 0.0 {
                bail!("images.toml: {name} has a pad but no marks");
            }
            if image.pad < 0.0 || !image.pad.is_finite() {
                bail!("images.toml: {name}'s pad {} isn't 0 or more", image.pad);
            }
            if let Some(rect) = image.rect {
                if rect.iter().any(|v| !v.is_finite() || *v < 0.0) || rect[2] < 1.0 || rect[3] < 1.0
                {
                    bail!("images.toml: {name}'s rect {rect:?} isn't x, y, width, height");
                }
            }
            if image.max_width == Some(0) {
                bail!("images.toml: {name}'s max_width is 0");
            }
        }
        Ok(list)
    }
}

/// The part of a shot an image takes (x, y, w, h, in the shot's pixels) and the size it is
/// written at. Unlike a manual figure, a mark the shot lacks or a rect past its edge is an
/// error: the README would silently show the wrong picture.
pub fn plan(name: &str, image: &Image, marks: &Marks) -> Result<([u32; 4], [u32; 2])> {
    let crop = if let Some(rect) = image.rect {
        let [w, h] = marks.image;
        if rect[0] + rect[2] > w || rect[1] + rect[3] > h {
            bail!(
                "{name}: the rect {rect:?} isn't inside {}'s {w}x{h} pixels",
                image.shot
            );
        }
        Some(Crop::Rect(rect))
    } else if image.marks.is_empty() {
        None
    } else {
        let missing: Vec<&str> = image
            .marks
            .iter()
            .filter(|m| !marks.marks.contains_key(*m))
            .map(String::as_str)
            .collect();
        if !missing.is_empty() {
            let known: Vec<&str> = marks.marks.keys().map(String::as_str).collect();
            bail!(
                "{name}: the shot {} has no mark {} (it has: {})",
                image.shot,
                missing.join(", "),
                known.join(", ")
            );
        }
        Some(Crop::Marks {
            names: image.marks.clone(),
            pad: image.pad,
        })
    };
    let crop = crop_rect(crop.as_ref(), marks);
    let size = scaled_size(crop, image.max_width.unwrap_or(u32::MAX));
    Ok((crop, size))
}

/// A shot's marks file (`<shots>/<shot>.json`).
pub fn read_marks(shots: &Path, shot: &str) -> Result<Marks> {
    let path = shots.join(format!("{shot}.json"));
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("reading {}", path.display()))
}

/// Every image's crop and size, checked against its shot's marks (no picture read).
pub fn plan_all(list: &ImageList, shots: &Path) -> Result<Vec<([u32; 4], [u32; 2])>> {
    list.images
        .iter()
        .map(|(name, image)| plan(name, image, &read_marks(shots, &image.shot)?))
        .collect()
}

/// An image written by [`make`].
#[derive(Debug)]
pub struct Made {
    pub name: String,
    pub path: PathBuf,
    pub size: [u32; 2],
    pub bytes: u64,
    pub encoding: Encoding,
    /// False when the file already held these bytes (left untouched).
    pub changed: bool,
}

/// Cuts, scales and encodes every image in `list` from the shots in `shots`, writing
/// `<out>/<name>.png`. A file already holding the same bytes is left alone.
pub fn make(list: &ImageList, shots: &Path, out: &Path) -> Result<Vec<Made>> {
    std::fs::create_dir_all(out).with_context(|| format!("making {}", out.display()))?;
    let mut made = Vec::new();
    for (name, image) in &list.images {
        let marks = read_marks(shots, &image.shot)?;
        let (crop, size) = plan(name, image, &marks)?;
        let source_path = shots.join(format!("{}.png", image.shot));
        let source = shots::read(&source_path)?;
        if [source.width(), source.height()] != marks.image.map(|v| v as u32) {
            bail!(
                "{name}: {} is {}x{} but its marks say {}x{}",
                source_path.display(),
                source.width(),
                source.height(),
                marks.image[0],
                marks.image[1]
            );
        }
        let (bytes, encoding) = shots::image_file(&source, crop, size)
            .with_context(|| format!("making {name} from {}", source_path.display()))?;
        let path = out.join(format!("{name}.png"));
        let changed = std::fs::read(&path).map_or(true, |old| old != bytes);
        if changed {
            std::fs::write(&path, &bytes).with_context(|| format!("writing {}", path.display()))?;
        }
        made.push(Made {
            name: name.clone(),
            path,
            size,
            bytes: bytes.len() as u64,
            encoding,
            changed,
        });
    }
    Ok(made)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks() -> Marks {
        serde_json::from_str(
            r#"{"image": [1440, 900], "marks": {"pane.mixer": [2, 476, 1136, 395], "export": [457.5, 296, 526, 309]}}"#,
        )
        .unwrap()
    }

    fn image(toml_text: &str) -> Image {
        let list = ImageList::read(&format!("[images.x]\n{toml_text}")).expect("read");
        list.images.into_values().next().unwrap()
    }

    #[test]
    fn an_image_is_its_marks_padded_its_rect_or_the_whole_shot_scaled() {
        let m = marks();
        // Clipped at the shot's left edge (2 - 4), grown on the right (2 + 1136 + 4).
        let padded = image("about = \"a\"\nshot = \"s\"\nmarks = [\"pane.mixer\"]\npad = 4\n");
        assert_eq!(
            plan("x", &padded, &m).unwrap(),
            ([0, 472, 1142, 403], [1142, 403])
        );
        // Grown outwards to whole pixels.
        let half = image("about = \"a\"\nshot = \"s\"\nmarks = [\"export\"]\n");
        assert_eq!(plan("x", &half, &m).unwrap().0, [457, 296, 527, 309]);
        let rect = image("about = \"a\"\nshot = \"s\"\nrect = [0, 25, 1136, 545]\n");
        assert_eq!(
            plan("x", &rect, &m).unwrap(),
            ([0, 25, 1136, 545], [1136, 545])
        );
        let whole = image("about = \"a\"\nshot = \"s\"\nmax_width = 1280\n");
        assert_eq!(
            plan("x", &whole, &m).unwrap(),
            ([0, 0, 1440, 900], [1280, 800])
        );
    }

    #[test]
    fn a_missing_mark_or_a_rect_off_the_shot_is_an_error() {
        let m = marks();
        let missing = image("about = \"a\"\nshot = \"s\"\nmarks = [\"pane.mixer\", \"nope\"]\n");
        let error = plan("x", &missing, &m).unwrap_err().to_string();
        assert!(error.contains("no mark nope"), "{error}");
        let off = image("about = \"a\"\nshot = \"s\"\nrect = [1000, 0, 500, 10]\n");
        assert!(plan("x", &off, &m).is_err());
    }

    #[test]
    fn the_list_refuses_what_it_cannot_mean() {
        let bad = [
            "[images.Hero]\nabout = \"a\"\nshot = \"s\"\n",
            "[images.a]\nabout = \"a\"\nshot = \"s\"\nmarks = [\"m\"]\nrect = [0, 0, 1, 1]\n",
            "[images.a]\nabout = \"a\"\nshot = \"s\"\npad = 4\n",
            "[images.a]\nabout = \"a\"\nshot = \"s\"\nrect = [0, 0, 0, 1]\n",
            "[images.a]\nabout = \"a\"\nshot = \"s\"\nmax_width = 0\n",
            "[images.a]\nabout = \"a\"\nshot = \"s\"\nwidth = 10\n",
            "[images.a]\nshot = \"s\"\n",
            "",
        ];
        for text in bad {
            assert!(ImageList::read(text).is_err(), "accepted {text:?}");
        }
    }

    /// The README's own list names shots and marks that exist (no picture is made here: the
    /// script makes them, `scripts/readme_images.ps1`), and its images stay under budget in
    /// pixels: none wider than its shot.
    #[test]
    fn the_readmes_list_matches_the_committed_shots() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let text = std::fs::read_to_string(root.join("docs/images").join(LIST))
            .expect("docs/images/images.toml reads");
        let list = ImageList::read(&text).expect("docs/images/images.toml parses");
        let planned = plan_all(&list, &root.join("docs/manual/shots")).expect("plans");
        for ((name, _), (_, size)) in list.images.iter().zip(&planned) {
            assert!(
                root.join("docs/images")
                    .join(format!("{name}.png"))
                    .exists(),
                "docs/images/{name}.png is missing: run scripts/readme_images.ps1"
            );
            assert!(size[0] >= 200 && size[1] >= 100, "{name} is tiny: {size:?}");
        }
    }
}
