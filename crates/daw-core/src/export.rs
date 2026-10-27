//! Mixdown export through the live engine.
//!
//! Export borrows the very [`EngineCore`] the device callback runs, with the same graph,
//! plugin instances, process mode and block size, so the file contains what playback
//! produces. While it runs the device outputs silence.
//!
//! Every export is measured (EBU R 128, the same meter as the master's live tap) on exactly the
//! samples written. Optionally it is normalised to a loudness target: one constant gain on the
//! rendered samples after `render()`, so it is no second processing path. Off, the file is
//! bit-identical to playback; on, it differs from it by exactly that gain.

use crate::engine::EngineCore;
use crate::loudness::{measure, normalise_gain, Loudness};
use crate::wav;
use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

pub struct ExportSpec {
    pub path: PathBuf,
    pub start: u64,
    pub end: u64,
    /// Extra time after `end` for reverb and delay tails, in samples.
    pub tail: u64,
    /// Hardware output pair to capture (where the master goes).
    pub channel: usize,
    pub block: usize,
    pub sample_rate: u32,
    /// Silence rendered and discarded before the export, in samples.
    pub preroll: usize,
    /// Normalise the file to a loudness target, or leave its level alone.
    pub normalize: Option<Normalize>,
    /// The file's format (a 32-bit float WAV by default, written as always).
    pub encoding: crate::encode::Encoding,
    /// Read the written file back on the export's thread ([`Done::audio`]): a track's render,
    /// whose file goes on a new track, so the UI thread never reads or decodes it.
    pub read_back: bool,
    /// While set, the render waits halfway ([`crate::session::Session::hold_export`]: check
    /// runs catch its progress bar on screen with it).
    pub hold: Arc<AtomicBool>,
}

/// What to normalise an export to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Normalize {
    /// Integrated loudness, LUFS.
    pub target: f64,
    /// The true peak the gain may not push past, dBTP.
    pub ceiling: f64,
}

/// What a finished export wrote.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ExportReport {
    pub path: PathBuf,
    /// The written file's loudness.
    pub loudness: Loudness,
    /// How it was normalised, if it was asked to be.
    pub normalized: Option<Normalized>,
}

/// How an export was normalised.
#[derive(Debug, Clone, PartialEq, serde::Serialize, schemars::JsonSchema)]
pub struct Normalized {
    pub target: f64,
    pub ceiling: f64,
    /// The gain applied, dB; `None` when there was nothing to normalise (all below the
    /// -70 LUFS gate), and the file was left as rendered.
    pub gain_db: Option<f64>,
    /// The ceiling held the gain below what the target needed.
    pub held_by_peak: bool,
}

/// A finished export: its report, and the file read back when asked ([`ExportSpec::read_back`]).
pub struct Done {
    pub report: ExportReport,
    /// The written file, decoded at the session's rate as the media pool would load it.
    pub audio: Option<Arc<crate::media::AudioData>>,
}

pub struct ExportJob {
    pub progress: Arc<crate::util::AtomicF32>,
    pub cancel: Arc<AtomicBool>,
    handle: Option<JoinHandle<Result<Done>>>,
}

impl ExportJob {
    pub fn is_finished(&self) -> bool {
        self.handle.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Collect the result once finished.
    pub fn take_result(&mut self) -> Option<Result<Done>> {
        if !self.is_finished() {
            return None;
        }
        let handle = self.handle.take()?;
        Some(
            handle
                .join()
                .unwrap_or_else(|_| Err(anyhow!("export thread panicked"))),
        )
    }
}

/// Holds the export flag up for as long as it lives, including on early returns.
struct OfflineGuard<'a>(&'a AtomicBool);

impl<'a> OfflineGuard<'a> {
    fn set(flag: &'a AtomicBool) -> Self {
        flag.store(true, Ordering::Relaxed);
        Self(flag)
    }
}

impl Drop for OfflineGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Relaxed);
    }
}

pub fn start(core: Arc<Mutex<EngineCore>>, spec: ExportSpec) -> ExportJob {
    let progress = Arc::new(crate::util::AtomicF32::new(0.0));
    let cancel = Arc::new(AtomicBool::new(false));
    let (p, c) = (progress.clone(), cancel.clone());
    let handle = std::thread::Builder::new()
        .name("export".into())
        .spawn(move || {
            let report = run(&core, &spec, &p, &c)?;
            // Read back here, off the UI thread: a long render takes a while to decode.
            let audio = if spec.read_back {
                Some(Arc::new(crate::media::load(
                    &report.path,
                    spec.sample_rate,
                )?))
            } else {
                None
            };
            Ok(Done { report, audio })
        })
        .ok();
    ExportJob {
        progress,
        cancel,
        handle,
    }
}

/// Write audio that is already made (a Waveform view document) as `encoding` says, in the
/// background like an export: the same job, progress and report (its loudness is the audio's).
pub fn start_encode(
    audio: wav::Audio,
    path: PathBuf,
    encoding: crate::encode::Encoding,
    metadata: crate::metadata::Metadata,
) -> ExportJob {
    let progress = Arc::new(crate::util::AtomicF32::new(0.0));
    let cancel = Arc::new(AtomicBool::new(false));
    let (p, c) = (progress.clone(), cancel.clone());
    let handle = std::thread::Builder::new()
        .name("encode".into())
        .spawn(move || {
            // Measuring the loudness is the first tenth of the bar, writing the rest.
            let loudness = measure(
                &audio.channels[0],
                audio.channels.get(1).map(Vec::as_slice),
                audio.sample_rate,
            );
            let writing = |done: f32| {
                p.store(0.1 + 0.9 * done);
                !c.load(Ordering::Relaxed)
            };
            p.store(0.1);
            crate::encode::write_with(
                &path,
                &audio,
                &encoding,
                Some(&metadata),
                &crate::encode::Progress::new(&writing),
            )?;
            p.store(1.0);
            Ok(Done {
                report: ExportReport {
                    path,
                    loudness,
                    normalized: None,
                },
                audio: None,
            })
        })
        .ok();
    ExportJob {
        progress,
        cancel,
        handle,
    }
}

fn run(
    core: &Mutex<EngineCore>,
    spec: &ExportSpec,
    progress: &crate::util::AtomicF32,
    cancel: &AtomicBool,
) -> Result<ExportReport> {
    let mut core = core.lock().map_err(|_| anyhow!("engine lock poisoned"))?;
    core.handle_commands();
    // Plugins wait for their lock while this is set, so the UI touching a plugin can delay
    // the export but never put a dry block in it.
    let flags = core.shared().inserts.clone();
    let _offline = OfflineGuard::set(&flags.offline);
    let misses_before = flags.misses.load(Ordering::Relaxed);
    let (was_playing, was_at) = (core.is_playing(), core.position());
    let latency = core.graph_latency();
    let length = (spec.end.saturating_sub(spec.start) + spec.tail) as usize;
    // Render the delay-compensation latency extra and drop it from the front, so the file
    // starts exactly at `start`.
    let total = length + latency;
    let outputs = core.output_channels();
    // Settle the plugins on silence first, so envelopes, lookahead buffers and amp-model
    // state left over from playback cannot leak into the start of the file. A fresh plugin
    // gets the same treatment on load, so every export starts from the same place.
    let mut discard: Vec<Vec<f32>> = vec![Vec::new(); outputs];
    core.set_transport(false, spec.start);
    core.render(spec.preroll, spec.block, &mut discard);
    drop(discard);
    let mut rendered: Vec<Vec<f32>> = vec![Vec::with_capacity(total); outputs];
    core.set_transport(true, spec.start);
    let chunk = spec.block.max(1) * 64;
    // The bar: rendering, then writing the file (barely any time for the plain WAV; an MP3 or
    // a converted rate takes a while, so it gets a quarter).
    let render_share = if spec.encoding == crate::encode::Encoding::default() {
        0.97
    } else {
        0.75
    };
    let mut done = 0;
    while done < total {
        if cancel.load(Ordering::Relaxed) {
            core.set_transport(was_playing, was_at);
            return Err(anyhow!("export cancelled"));
        }
        // Held halfway for a check run's screenshot of the bar (the UI keeps painting).
        while spec.hold.load(Ordering::Relaxed)
            && done * 2 >= total
            && !cancel.load(Ordering::Relaxed)
        {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let n = (total - done).min(chunk);
        core.render(n, spec.block, &mut rendered);
        done += n;
        progress.store(render_share * done as f32 / total as f32);
    }
    core.set_transport(was_playing, was_at);
    drop(core);
    let missed = flags.misses.load(Ordering::Relaxed) - misses_before;
    if missed > 0 {
        return Err(anyhow!(
            "{missed} plug-in blocks failed to process during export"
        ));
    }

    let pick = |c: usize| -> Vec<f32> {
        rendered
            .get(c)
            .map(|ch| ch[latency.min(ch.len())..].to_vec())
            .unwrap_or_else(|| vec![0.0; length])
    };
    let mut audio = wav::Audio {
        sample_rate: spec.sample_rate,
        channels: vec![pick(spec.channel), pick(spec.channel + 1)],
    };
    let normalized = spec.normalize.map(|n| normalise(&mut audio, n));
    // The report is of the file as written: after a gain the -70 LUFS gate may let other blocks
    // through, so it is measured again rather than shifted.
    let loudness = measure(
        &audio.channels[0],
        Some(&audio.channels[1]),
        spec.sample_rate,
    );
    let writing = |p: f32| {
        progress.store(render_share + (1.0 - render_share) * p);
        !cancel.load(Ordering::Relaxed)
    };
    crate::encode::write_with(
        &spec.path,
        &audio,
        &spec.encoding,
        None,
        &crate::encode::Progress::new(&writing),
    )?;
    progress.store(1.0);
    Ok(ExportReport {
        path: spec.path.clone(),
        loudness,
        normalized,
    })
}

/// Bring `audio` to the target with one constant gain (see [`normalise_gain`]).
fn normalise(audio: &mut wav::Audio, n: Normalize) -> Normalized {
    let before = measure(
        &audio.channels[0],
        Some(&audio.channels[1]),
        audio.sample_rate,
    );
    let gain = normalise_gain(before.integrated, before.true_peak, n.target, n.ceiling);
    if let Some(gain) = gain {
        let factor = gain_factor(gain.db);
        for channel in &mut audio.channels {
            channel.iter_mut().for_each(|s| *s *= factor);
        }
    }
    Normalized {
        target: n.target,
        ceiling: n.ceiling,
        gain_db: gain.map(|g| g.db),
        held_by_peak: gain.is_some_and(|g| g.held_by_peak),
    }
}

/// The sample multiplier for a gain in dB, as normalising applies it.
pub fn gain_factor(db: f64) -> f32 {
    10f64.powf(db / 20.0) as f32
}
