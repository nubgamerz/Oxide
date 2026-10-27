//! Project templates: a project's setup without its song, to start new projects from.
//!
//! A template is the project file's JSON (`<name>.oxide-template`) with every track, folder,
//! bus, route, send, mixer setting and insert (with the plug-in's saved state: an amp
//! sim's settings come back as they were), the tempo map, time signature, loudness target
//! and markers, but no clips, recordings or automation points: only what a new song starts
//! from. They live in one folder for the whole app (the app's `Templates`, beside the
//! program), never in a project.

use crate::project::Project;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub const EXTENSION: &str = "oxide-template";

/// A template on disk.
#[derive(Debug, Clone, PartialEq)]
pub struct Template {
    pub name: String,
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
}

/// `project` as a template called `name`: the setup without the song. Its plug-in states must
/// have been read already (`Session::save_template` does it).
pub fn of(project: &Project, name: &str) -> Project {
    let mut t = project.clone();
    t.name = name.trim().to_string();
    for track in &mut t.tracks {
        track.clips.clear();
        track.midi_clips.clear();
        // The lanes stay, ready to draw on; their moves were this song's.
        for lane in &mut track.automation {
            lane.points.clear();
        }
    }
    for lane in &mut t.master.automation {
        lane.points.clear();
    }
    t.loop_region = Default::default();
    t.punch = Default::default();
    t
}

/// The file of the template called `name` in `dir`.
pub fn path_of(dir: &Path, name: &str) -> Result<PathBuf> {
    if name.trim().is_empty() {
        bail!("a template needs a name");
    }
    Ok(dir.join(format!("{}.{EXTENSION}", crate::folder::file_name(name))))
}

/// The templates in `dir`, by name (none when it doesn't exist yet).
pub fn list(dir: &Path) -> Vec<Template> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut list: Vec<Template> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension()
                .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
        })
        .filter_map(|path| {
            let meta = std::fs::metadata(&path).ok()?;
            Some(Template {
                name: path.file_stem()?.to_string_lossy().into_owned(),
                modified: meta.modified().ok()?,
                size: meta.len(),
                path,
            })
        })
        .collect();
    list.sort_by_key(|t| t.name.to_lowercase());
    list
}

/// Read the template called `name` in `dir`: its JSON, checked to be a project.
pub fn read(dir: &Path, name: &str) -> Result<(String, PathBuf)> {
    let path = path_of(dir, name)?;
    if !path.is_file() {
        bail!(
            "no template called \"{}\" in {}",
            name.trim(),
            dir.display()
        );
    }
    let json =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str::<Project>(&json)
        .with_context(|| format!("{} is not a project template", path.display()))?;
    Ok((json, path))
}

/// Rename the template `from` to `to` in `dir`; refused if `to` exists.
pub fn rename(dir: &Path, from: &str, to: &str) -> Result<PathBuf> {
    let (json, old) = read(dir, from)?;
    let new = path_of(dir, to)?;
    if new == old {
        return Ok(new);
    }
    if new.exists() {
        bail!("a template called \"{}\" exists already", to.trim());
    }
    // The name inside follows the file's.
    let mut project: Project = serde_json::from_str(&json)?;
    project.name = to.trim().to_string();
    crate::session::write_whole(&new, &serde_json::to_string_pretty(&project)?)?;
    std::fs::remove_file(&old).with_context(|| format!("removing {}", old.display()))?;
    Ok(new)
}

/// Delete the template called `name` in `dir`.
pub fn delete(dir: &Path, name: &str) -> Result<()> {
    let path = path_of(dir, name)?;
    if !path.is_file() {
        bail!(
            "no template called \"{}\" in {}",
            name.trim(),
            dir.display()
        );
    }
    std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))
}
