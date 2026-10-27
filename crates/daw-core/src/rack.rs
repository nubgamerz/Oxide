//! The Waveform view's effects rack as it runs (dev plan 10.4): one chain of inserts that plays
//! a file live (the engine's preview) and renders it offline (Apply). Both go through
//! [`RackChain::process`], from the same settled start, in the same blocks, with the same
//! transport, so what Apply writes is what the preview played: render = preview for the rack.
//!
//! The chain works in [`CHANNELS`] channels like a track: a mono file plays on both (and Apply
//! averages them back), a stereo file on its two. Its inserts carry their own
//! [`InsertFlags`]: Apply waits for plug-in locks (offline) without touching the flags the
//! live graph's inserts read. An Apply's chain also carries a snapshot of each slot's bypass
//! (taken when it starts), so switching a slot while it renders changes only the preview.
//!
//! The rack is also kept (dev plan 18.2): [`RackFile`] is what is written beside the program
//! (`rack.json`, between runs) and as a named preset (`RackPresets/<name>.json`); the session
//! does the writing (`Session::restore_rack`, `save_rack_preset`, …).

use crate::graph::{Processor, Transport, CHANNELS};
use crate::media::AudioData;
use crate::plugins::InsertFlags;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::SystemTime;

/// The rack as kept on disk: its slots in order, each with its settings. The file the app keeps
/// between runs and a preset are the same thing.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RackFile {
    #[serde(default = "version")]
    pub version: u32,
    pub slots: Vec<RackSlot>,
}

fn version() -> u32 {
    1
}

impl RackFile {
    pub fn new(slots: Vec<RackSlot>) -> Self {
        Self {
            version: version(),
            slots,
        }
    }
}

/// One slot: its plug-in (a VST3 path or `native:<id>`), bypass, and its settings (VST3 state
/// bytes; a built-in effect's parameter values and what it learned, as JSON), base64 in the file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(from = "RackSlotFile")]
pub struct RackSlot {
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub bypass: bool,
    #[serde(default, with = "crate::project::base64_bytes")]
    pub state: Option<Arc<Vec<u8>>>,
}

/// A slot as a file holds it: a built-in effect under an older id or name
/// (`native::RENAMED`) is read as the effect it became.
#[derive(Deserialize)]
struct RackSlotFile {
    name: String,
    path: PathBuf,
    #[serde(default)]
    bypass: bool,
    #[serde(default, with = "crate::project::base64_bytes")]
    state: Option<Arc<Vec<u8>>>,
}

impl From<RackSlotFile> for RackSlot {
    fn from(f: RackSlotFile) -> Self {
        let name = crate::native::current_name(&f.path, f.name);
        Self {
            name,
            path: crate::native::current_path(f.path),
            bypass: f.bypass,
            state: f.state,
        }
    }
}

/// Read a rack file.
pub fn read(path: &Path) -> Result<RackFile> {
    let json =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&json)
        .with_context(|| format!("{} is not an effects rack", path.display()))
}

/// Rack presets: named racks in one folder for the whole app (`RackPresets`, beside the
/// program). A preset's name is its file's name, so a name
/// Windows can't take as one is refused rather than changed.
pub mod presets {
    use super::*;

    pub const EXTENSION: &str = "json";

    /// A preset on disk.
    #[derive(Debug, Clone, PartialEq)]
    pub struct Preset {
        pub name: String,
        pub path: PathBuf,
        pub modified: SystemTime,
    }

    /// Why `name` can't be a preset's name, if it can't.
    pub fn check_name(name: &str) -> Result<()> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            bail!("a rack preset needs a name");
        }
        if let Some(c) = trimmed
            .chars()
            .find(|c| c.is_control() || r#"<>:"/\|?*"#.contains(*c))
        {
            bail!("a rack preset's name can't hold '{c}' (\"{trimmed}\"): it names a file");
        }
        if trimmed.ends_with('.') {
            bail!("a rack preset's name can't end with a dot (\"{trimmed}\")");
        }
        if trimmed.chars().count() > 100 {
            bail!("a rack preset's name is at most 100 characters");
        }
        let stem = trimmed.split('.').next().unwrap_or("").to_ascii_uppercase();
        let reserved = ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
            || ((stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.len() == 4
                && stem.as_bytes()[3].is_ascii_digit());
        if reserved {
            bail!("\"{trimmed}\" is a name Windows keeps for a device: pick another");
        }
        Ok(())
    }

    /// The file of the preset called `name` in `dir`.
    pub fn path_of(dir: &Path, name: &str) -> Result<PathBuf> {
        check_name(name)?;
        Ok(dir.join(format!("{}.{EXTENSION}", name.trim())))
    }

    /// The presets in `dir`, by name (none when it doesn't exist yet).
    pub fn list(dir: &Path) -> Vec<Preset> {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return Vec::new();
        };
        let mut list: Vec<Preset> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| {
                p.extension()
                    .is_some_and(|e| e.eq_ignore_ascii_case(EXTENSION))
            })
            .filter_map(|path| {
                let meta = std::fs::metadata(&path).ok()?;
                Some(Preset {
                    name: path.file_stem()?.to_string_lossy().into_owned(),
                    modified: meta.modified().ok()?,
                    path,
                })
            })
            .collect();
        list.sort_by_key(|p| p.name.to_lowercase());
        list
    }

    /// Read the preset called `name` in `dir`.
    pub fn load(dir: &Path, name: &str) -> Result<RackFile> {
        let path = path_of(dir, name)?;
        if !path.is_file() {
            bail!(
                "no rack preset called \"{}\" in {}",
                name.trim(),
                dir.display()
            );
        }
        read(&path)
    }

    /// Write `rack` as the preset `name` in `dir` (made if needed); refused if one of that name
    /// exists, unless `replace`. Returns its path.
    pub fn save(dir: &Path, name: &str, rack: &RackFile, replace: bool) -> Result<PathBuf> {
        let path = path_of(dir, name)?;
        if path.exists() && !replace {
            bail!(
                "a rack preset called \"{}\" exists already: replace it or pick another name",
                name.trim()
            );
        }
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        crate::session::write_whole(&path, &serde_json::to_string_pretty(rack)?)?;
        Ok(path)
    }

    /// Delete the preset called `name` in `dir`.
    pub fn delete(dir: &Path, name: &str) -> Result<()> {
        let path = path_of(dir, name)?;
        if !path.is_file() {
            bail!(
                "no rack preset called \"{}\" in {}",
                name.trim(),
                dir.display()
            );
        }
        std::fs::remove_file(&path).with_context(|| format!("deleting {}", path.display()))
    }
}

pub struct RackChain {
    inserts: Vec<Box<dyn Processor>>,
    flags: InsertFlags,
    io: Vec<Vec<f32>>,
    max_block: usize,
}

impl RackChain {
    /// A chain of `inserts` (built with `flags`), for blocks of at most `max_block`.
    pub fn new(inserts: Vec<Box<dyn Processor>>, flags: InsertFlags, max_block: usize) -> Self {
        Self {
            inserts,
            flags,
            io: vec![vec![0.0; max_block]; CHANNELS],
            max_block,
        }
    }

    /// The chain's latency: its inserts' summed.
    pub fn latency(&self) -> usize {
        self.inserts.iter().map(|i| i.latency()).sum()
    }

    pub fn flags(&self) -> &InsertFlags {
        &self.flags
    }

    /// Wait for plug-in locks (offline: settling and Apply, on a thread of their own) or only
    /// try them (live, on the audio thread).
    pub fn set_offline(&self, offline: bool) {
        self.flags.offline.store(offline, Ordering::Relaxed);
    }

    /// Run `samples` of silence through the stopped chain, in blocks of `block`, as export
    /// settles a project's plug-ins: whatever they did before cannot reach what follows.
    pub fn settle(&mut self, samples: usize, block: usize) {
        let block = block.clamp(1, self.max_block);
        let mut done = 0;
        while done < samples {
            let n = (samples - done).min(block);
            for c in &mut self.io {
                c[..n].fill(0.0);
            }
            self.run(n, 0, false);
            done += n;
        }
    }

    /// Process `frames` (at most the chain's block) of `audio` from sample `at`, playing;
    /// samples past the file's end are silence. The result is in [`Self::output`].
    pub fn process(&mut self, audio: &AudioData, at: u64, frames: usize) {
        let frames = frames.min(self.max_block);
        let source = &audio.channels;
        for (c, io) in self.io.iter_mut().enumerate() {
            let Some(src) = source.get(c.min(source.len().saturating_sub(1))) else {
                io[..frames].fill(0.0);
                continue;
            };
            let from = (at as usize).min(src.len());
            let to = (from + frames).min(src.len());
            io[..to - from].copy_from_slice(&src[from..to]);
            io[to - from..frames].fill(0.0);
        }
        self.run(frames, at, true);
    }

    /// The last block's output, [`CHANNELS`] channels.
    pub fn output(&self) -> &[Vec<f32>] {
        &self.io
    }

    /// Each insert in turn, each told the position of the audio it gets: behind the input by
    /// the latency in front of it, as the graph tells a track's inserts.
    fn run(&mut self, frames: usize, at: u64, playing: bool) {
        let mut before = 0;
        for insert in &mut self.inserts {
            let transport = Transport {
                playing,
                ..Transport::playing_at(at)
            }
            .earlier(before);
            insert.process(&mut self.io, frames, &transport);
            before += insert.latency();
        }
    }

    /// Apply's render: `audio[from..to]` through the (settled) chain in blocks of `block`, fed on
    /// to `to + latency` so the delayed output covers the range, with the latency dropped:
    /// `to - from` samples per channel. `progress` hears the fraction done; `cancel` stops it.
    pub fn render(
        &mut self,
        audio: &AudioData,
        from: u64,
        to: u64,
        block: usize,
        mut progress: impl FnMut(f32),
        cancel: impl Fn() -> bool,
    ) -> Option<Vec<Vec<f32>>> {
        let block = block.clamp(1, self.max_block);
        let latency = self.latency() as u64;
        let total = (to - from + latency) as usize;
        let mut out: Vec<Vec<f32>> = (0..CHANNELS).map(|_| Vec::with_capacity(total)).collect();
        let mut at = from;
        while (at - from) < total as u64 {
            if cancel() {
                return None;
            }
            let n = (total - (at - from) as usize).min(block);
            self.process(audio, at, n);
            for (o, io) in out.iter_mut().zip(&self.io) {
                o.extend_from_slice(&io[..n]);
            }
            at += n as u64;
            progress((at - from) as f32 / total as f32);
        }
        for o in &mut out {
            o.drain(..latency as usize);
        }
        Some(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::graph::DelayLine;

    /// A stateful, latent stand-in for a plug-in: a one-pole low-pass, then a delay.
    pub(crate) struct Smooth {
        state: [f32; CHANNELS],
        delay: DelayLine,
    }

    impl Smooth {
        pub(crate) fn new(latency: usize) -> Self {
            Self {
                state: [0.0; CHANNELS],
                delay: DelayLine::new(latency, CHANNELS),
            }
        }
    }

    impl Processor for Smooth {
        fn latency(&self) -> usize {
            self.delay.delay()
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
            for (c, ch) in io.iter_mut().enumerate().take(CHANNELS) {
                for v in &mut ch[..frames] {
                    self.state[c] += 0.05 * (*v - self.state[c]);
                    *v = self.state[c];
                }
            }
            self.delay.process(io, frames);
        }
    }

    fn chain() -> RackChain {
        let inserts: Vec<Box<dyn Processor>> =
            vec![Box::new(Smooth::new(37)), Box::new(Smooth::new(5))];
        RackChain::new(inserts, InsertFlags::default(), 512)
    }

    #[test]
    fn a_render_is_the_same_at_any_block_size_with_the_latency_dropped() {
        let ramp: Vec<f32> = (0..5_000)
            .map(|i| ((i * 37) % 101) as f32 / 101.0 - 0.5)
            .collect();
        let audio = AudioData::new(48_000, vec![ramp]);
        let mut first = chain();
        assert_eq!(first.latency(), 42);
        first.settle(1000, 128);
        let a = first
            .render(&audio, 1000, 3000, 64, |_| {}, || false)
            .unwrap();
        assert_eq!(a[0].len(), 2000);
        assert_eq!(a[0], a[1], "a mono file on both channels");
        let mut second = chain();
        second.settle(1000, 128);
        let b = second
            .render(&audio, 1000, 3000, 333, |_| {}, || false)
            .unwrap();
        assert_eq!(
            a, b,
            "the block size doesn't change a sample-by-sample chain"
        );
    }
}
