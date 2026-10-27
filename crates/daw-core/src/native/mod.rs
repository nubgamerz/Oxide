//! Built-in effects (dev plan 11): effects written in Rust, as inserts beside VST3 plug-ins.
//!
//! A native effect is a [`Kind`] (its name, parameters and a constructor) whose instances
//! implement [`NativeEffect`]. Everything else treats it as a plug-in: an insert slot holds it
//! by the path `native:<id>` ([`path_of`]), the graph runs it through [`NativeInsert`] (the
//! same real-time rules as `PluginInsert`: `try_lock` live, wait during export, a delay line
//! while bypassed), parameters are normalized 0..1 and described with the VST3 host's
//! [`Parameter`], automation reaches it sample-accurately, and its state is JSON of plain values
//! by parameter name, so saved projects survive a parameter being added.
//!
//! Each effect is measured against reference renders (`effect_check`, `out/effect-ref/<effect>/`)
//! and written as original code.

pub mod adaptive_noise;
pub mod amplify;
pub mod analog_delay;
pub mod biquad;
pub mod channel_mixer;
pub mod chorus;
pub mod click_pop;
pub mod click_remover;
pub mod compressor;
pub mod convolution_reverb;
pub mod convolver;
pub mod declipper;
pub mod deesser;
pub mod dehummer;
pub mod delay;
pub mod delay_line;
pub mod describe;
pub mod dynamics;
pub mod echo;
pub mod flanger;
pub mod full_reverb;
pub mod graphic_eq;
pub mod hard_limiter;
pub mod hiss_reduction;
pub mod multiband;
pub mod noise_reduction;
pub mod parametric_eq;
pub mod phaser;
/// The comparison against reference renders (`effect_check`): local only, not in the
/// repository; built with the `reference` feature where its files are present.
#[cfg(feature = "reference")]
pub mod reference;
pub mod spectral;
pub mod studio_reverb;

use crate::graph::{DelayLine, Processor, Transport, AUTOMATION_GRID, CHANNELS};
use crate::plugins::InsertFlags;
use anyhow::{anyhow, bail, Result};
use std::any::Any;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use vst3_host::parameters::Parameter;

/// What an insert's path starts with when it holds a native effect.
pub const PATH_PREFIX: &str = "native:";

/// Recursive state (a filter's memory, an envelope, a feedback line) under this is flushed to
/// zero where it decays: about −500 dB, far below any audio and far above the denormals (under
/// 1.2e-38 in f32, 2.2e-308 in f64) that a dying tail would otherwise run through, each
/// operation on them costing tens of times a normal one, for seconds or, stuck in a limit
/// cycle, for ever after the input stops (TD128, dev plan 18.1). `denormal_check` times every
/// effect's silent tail against its noise; a flush changes only values under the floor. Per
/// sample, never per block, so the output stays the same at any block size.
pub const TINY: f32 = 1e-25;

/// `x`, or zero under [`TINY`].
#[inline(always)]
pub fn flush(x: f32) -> f32 {
    if x.abs() < TINY {
        0.0
    } else {
        x
    }
}

/// The floor for double-precision filter state ([`flush_all`], about −600 dB): lower than
/// [`TINY`], since a cascade of f64 sections at high Q (the Parametric EQ's, f32 between
/// sections) carries a change at its floor about 1e12 times on into a band still ringing loud
/// (at 1e-25 the last bit of a −96 dB tail moved, −253 dB). Still far above f64's denormals,
/// and an f32 made from a value over it is never denormal.
pub const TINY64: f64 = 1e-30;

/// A recursive filter's whole state (`N` values), zeroed once all of it is under [`TINY64`]:
/// the filter has died away. Flushing one value of a live recursion instead (a direct form's
/// output while its older outputs are still over the floor) would cut its momentum, and a
/// resonant low section, kicked so again and again, rings on in a limit cycle just over the
/// floor. Until then the recursion runs untouched.
#[inline(always)]
pub fn flush_all<const N: usize>(state: [f64; N]) -> [f64; N] {
    if state.iter().all(|v| v.abs() < TINY64) {
        [0.0; N]
    } else {
        state
    }
}

/// One of an effect's parameters, in its plain units.
#[derive(Debug, Clone, Copy)]
pub struct ParamSpec {
    pub name: &'static str,
    pub unit: &'static str,
    pub min: f64,
    pub max: f64,
    pub default: f64,
    /// 0 for a continuous parameter, 1 for a switch, n for n + 1 choices (VST3's step count).
    pub steps: u32,
    /// Decimals in the displayed value.
    pub decimals: usize,
    /// Whether a lane can play it (switches such as Link Sliders can't).
    pub automatable: bool,
    /// A switch (by name) that makes the editor move this parameter with the others that
    /// name it, as Amplify's Link Sliders does. The DSP never sees the link. `Switch:group`
    /// links only the parameters naming the same group, so one switch can lock several pairs
    /// (the Echo's Lock: delays, feedbacks, levels), and turning such a switch on copies each
    /// group's first parameter onto the others.
    pub link: Option<&'static str>,
    /// The editor's slider is logarithmic (frequencies, Q); the normalized value stays linear.
    pub log_slider: bool,
    /// Shows the value its own way ("24 dB/oct" for a filter order); `None` for the number and
    /// unit.
    pub format: Option<fn(f64) -> String>,
}

impl ParamSpec {
    /// The plain value at normalized `n`: linear across the range; a stepped parameter takes
    /// the step `n` falls in, as VST3's `Parameter::normalized_to_plain` does.
    pub fn plain(&self, n: f64) -> f64 {
        let n = n.clamp(0.0, 1.0);
        if self.steps > 0 {
            let step = ((n * (f64::from(self.steps) + 1.0)) as u32).min(self.steps);
            self.min + f64::from(step) / f64::from(self.steps) * (self.max - self.min)
        } else {
            self.min + n * (self.max - self.min)
        }
    }

    /// The normalized value of `plain` (clamped to the range).
    pub fn normalized(&self, plain: f64) -> f64 {
        let n = ((plain - self.min) / (self.max - self.min)).clamp(0.0, 1.0);
        if self.steps > 0 {
            // The middle of the step, so it reads back as the same step.
            let step = (n * f64::from(self.steps)).round();
            (step + 0.5) / (f64::from(self.steps) + 1.0)
        } else {
            n
        }
    }

    /// How the value shows: "+6.0 dB", "On".
    pub fn display(&self, plain: f64) -> String {
        if let Some(format) = self.format {
            return format(plain);
        }
        if self.steps == 1 && self.min == 0.0 && self.max == 1.0 {
            return if plain >= 0.5 { "On" } else { "Off" }.into();
        }
        let number = if self.unit == "dB" && plain > 0.0 {
            format!("+{plain:.*}", self.decimals)
        } else {
            format!("{plain:.*}", self.decimals)
        };
        if self.unit.is_empty() {
            number
        } else {
            format!("{number} {}", self.unit)
        }
    }
}

/// The DSP of one instance. Parameters arrive as plain values, before the samples they apply
/// to. Must not allocate, lock or free: it runs on the audio thread.
pub trait NativeEffect: Send {
    /// Parameter `index` (of the kind's `params`) was set to `value` (plain units) by hand:
    /// the effect may glide there its own way, so a slider moved while playing doesn't click.
    fn set(&mut self, index: usize, value: f64);
    /// Automation: be at `value` exactly `samples` from now, moving in a straight line in the
    /// parameter's plain units (0: at once). The default jumps, for effects that don't glide.
    fn ramp(&mut self, index: usize, value: f64, _samples: usize) {
        self.set(index, value);
        self.snap();
    }
    /// Process samples `from..to` of each of the [`CHANNELS`] buffers in place.
    fn process(&mut self, io: &mut [Vec<f32>], from: usize, to: usize);
    /// Samples of delay the effect adds (look-ahead), for delay compensation.
    fn latency(&self) -> usize {
        0
    }
    /// Be at the values set so far at once, without gliding to them: a new or restored
    /// instance starts where its settings are.
    fn snap(&mut self) {}
    /// The transport has started playing (export starts it too, after its pre-roll). An effect
    /// whose output depends on where its own blocks fall (an FFT's) starts them here, so
    /// playback and its export cut the stream in the same places.
    fn start(&mut self) {}
    /// The transport of the block about to be processed (before [`NativeEffect::start`] and
    /// any parameter of the block): its tempo segment, for an effect that follows the song's
    /// tempo (the Vintage Delay's sync). The engine splits blocks at tempo changes, so one
    /// block has one tempo. The default ignores it.
    fn transport(&mut self, _transport: &Transport) {}
    /// What the effect learned from audio (made by the kind's [`Kind::capture`]: Noise
    /// Reduction's noise print, Clip Repair's clip levels) to use from now on: swap it with
    /// the one in use, so the old one goes back to be freed off the audio thread. The default
    /// leaves `print` as it is, for effects that learn nothing.
    fn take_print(&mut self, _print: &mut Vec<f32>) {}
    /// Counters the effect keeps for its editor (Click & Pop Repair's clicks corrected
    /// and rejected): atomics the DSP writes and the main thread reads, without a lock. Asked
    /// for once, when the instance is made. `None` for effects that count nothing.
    fn readouts(&self) -> Option<Arc<[AtomicU64]>> {
        None
    }
    /// The file the effect plays, prepared off the audio thread ([`Kind::reads`]: the
    /// Convolution Reverb's impulse): take `fresh` if one waits (leaving `None`) and the effect
    /// can, and leave in `spent`, when it is empty, one it has finished with, to be freed on the
    /// main thread. Called every block, so a finished one goes back as soon as it is done. The
    /// default takes nothing, for effects that play no file.
    fn take_file(&mut self, _fresh: &mut Option<FileBox>, _spent: &mut Option<FileBox>) {}
    /// What [`Kind::prepares`] made from the settings: take `fresh` if one waits (leaving
    /// `None`) and `spent` is empty, and leave the box in `spent` once done with it, to be freed
    /// on the main thread. Called every block, before the transport's start. The default takes
    /// nothing, for effects that prepare nothing.
    fn take_prepared(&mut self, _fresh: &mut Option<FileBox>, _spent: &mut Option<FileBox>) {}
}

/// A prepared file on its way to or from the DSP (the effect downcasts it to its own type).
pub type FileBox = Box<dyn Any + Send>;

/// A native effect as the browser and `add_insert` know it.
pub struct Kind {
    /// Stable, in saved projects: `native:<id>`.
    pub id: &'static str,
    /// Its name, as the browser and editors show it.
    pub name: &'static str,
    /// Its group, for the browser and the manual.
    pub category: &'static str,
    pub params: &'static [ParamSpec],
    /// A new instance at a sample rate, with every parameter at its default.
    pub make: fn(rate: f64) -> Box<dyn NativeEffect>,
    /// For the editor's curve: the gain in dB at a frequency (Hz), for plain parameter values
    /// at a sample rate, computed from the same design the DSP runs. `None` for effects
    /// without a frequency response to show.
    pub response: Option<Response>,
    /// For a dynamics effect's editor: the output level (dBFS) for a steady input level (dBFS,
    /// the `hz` argument), its static gain curve. `None` for other effects.
    pub transfer: Option<Response>,
    /// For an effect whose delay depends on its settings (a look-ahead): the delay for plain
    /// values at a rate, so the session sees a change at once and recompensates. `None`: the
    /// delay the instance reports when made.
    pub latency: Option<fn(rate: f64, values: &[f64]) -> usize>,
    /// For an effect that learns from audio (Noise Reduction's noise print, Hiss Reduction's
    /// noise floor, Clip Repair's clip levels): what it keeps of `audio` (one buffer per
    /// channel at `rate`) for plain `values`, on the main thread. It is saved with the settings
    /// and reaches the DSP through [`NativeEffect::take_print`]. `None` for effects that learn
    /// nothing.
    pub capture: Option<Capture>,
    /// For an effect whose scans of audio set its parameters (Click & Pop Repair's
    /// "Scan For Threshold Levels" and "Scan For All Levels"). Such a kind's `capture` runs its
    /// first scan, so everything that captures a print can scan it. `None` for the others.
    pub learns: Option<&'static Learns>,
    /// For an effect that plays a file the user chooses (the Convolution Reverb's impulse):
    /// how the file becomes what the DSP plays. `None` for the others.
    pub reads: Option<&'static Reads>,
    /// For an effect whose transport start needs work from its settings too slow for the audio
    /// thread (the Chorus's random modulation, seeded from them and warmed up): made on the main
    /// thread whenever one of those settings changes. `None` for the others.
    pub prepares: Option<&'static Prepares>,
}

/// What an effect starts from, made from its settings on the main thread ([`Kind::prepares`]):
/// [`Native::set`] makes it again whenever a parameter in `by` changes (before the DSP sees the
/// new value), and it reaches the DSP through a mailbox the insert only `try_lock`s
/// ([`NativeEffect::take_prepared`]); the one the DSP gives back is freed on the main thread.
/// A render makes it before it runs, so playback and export start alike.
pub struct Prepares {
    /// The parameters it is made from. None is automatable: a lane's value can't wait for it.
    pub by: &'static [usize],
    /// It, for plain `values` at `rate`.
    pub make: fn(rate: f64, values: &[f64]) -> FileBox,
}

/// How an effect's impulse is made ready, off the audio thread: on a worker of the instance's
/// own, on the main thread's request ([`Native::service_file`]), and handed to the DSP through a
/// mailbox the insert only `try_lock`s ([`NativeEffect::take_file`]); what the DSP gives back is
/// freed on the main thread. The impulse comes from one of two sources:
///
/// - **a file** the user chooses (the Convolution Reverb): read and brought to the engine's rate
///   once ([`Reads::load`]); a change of the parameters in [`Reads::shaped_by`] prepares it
///   again from that;
/// - **the settings** (the Space Reverb, `load` `None`): there is no file, and [`Reads::prepare`]
///   makes the impulse from the parameters alone (given an empty [`Loaded`]), again whenever one
///   in `shaped_by` or the rate changes.
///
/// Either way export waits for it ([`Native::file_pending`]), so what renders is what plays.
pub struct Reads {
    /// What the file is to the effect, for the editor, actions and errors ("impulse").
    pub what: &'static str,
    /// The parameters (indices) its preparation depends on: changing one prepares it again.
    /// Not automatable: a lane can't wait for a worker, so it could not render as it plays.
    pub shaped_by: &'static [usize],
    /// Read the file at an absolute path and bring it to `rate` (channels mapped as the
    /// effect needs them). `None`: the effect makes its impulse from its settings and plays no
    /// file.
    pub load: Option<fn(path: &Path, rate: f64) -> Result<Loaded>>,
    /// What the DSP plays, from the loaded file (empty for an impulse made from the settings)
    /// and plain parameter `values`, at `rate`.
    pub prepare: fn(rate: f64, values: &[f64], loaded: &Loaded) -> Prepared,
    /// Parameters that follow others (the Space
    /// Reverb's room dimensions follow Room Size and Dimension; read-only ones keep their value):
    /// run on the main thread after a parameter is set by hand ([`Native::set`]: its index and
    /// old plain value) and when an instance is made or restored (`None`). `None`: no
    /// parameter follows another.
    pub derive: Option<Derive>,
    /// Parameters the effect sets itself (the Space Reverb's room dimensions): read-only to
    /// everyone else ([`Native::parameters`]); `derive` keeps them.
    pub outputs: &'static [usize],
}

impl Reads {
    /// Whether the impulse is made from the settings rather than read from a file.
    pub fn makes(&self) -> bool {
        self.load.is_none()
    }
}

/// Parameters following others ([`Reads::derive`]): at `rate`, after parameter `changed.0` was
/// set (its old plain value `changed.1`; `None` for a new or restored instance), adjust the
/// plain `values` in place.
pub type Derive = fn(rate: f64, changed: Option<(usize, f64)>, values: &mut [f64]);

/// A file read and brought to the engine's rate ([`Reads::load`]), kept by the worker while
/// only the settings change. Empty for an impulse made from the settings.
#[derive(Debug, Default)]
pub struct Loaded {
    /// The audio at the engine's rate, its channels as the effect maps them.
    pub channels: Vec<Vec<f32>>,
    /// The file as it is: its channels, rate and length in frames.
    pub file_channels: usize,
    pub file_rate: u32,
    pub file_frames: usize,
}

/// A prepared file: the DSP's part and what the editor shows of it.
pub struct Prepared {
    pub dsp: FileBox,
    pub overview: Overview,
}

/// What the editor shows of a prepared file (no lock, no access to the DSP's buffers).
#[derive(Debug, Clone, Default)]
pub struct Overview {
    /// Per channel, the lowest and highest sample of each of a few hundred equal stretches of
    /// what the DSP plays, scaled by `gain` as given to [`Overview::of`].
    pub peaks: Vec<Vec<(f32, f32)>>,
    /// Its length in samples at `rate`.
    pub samples: usize,
    pub rate: f64,
    pub file_channels: usize,
    pub file_rate: u32,
    pub file_frames: usize,
    /// A line the effect adds about what it prepared (the Space Reverb's loudness cap), if any.
    pub note: Option<String>,
}

impl Overview {
    /// `channels` (at `rate`) as a few hundred min/max pairs each, times `gain`.
    pub fn of(channels: &[Vec<f32>], rate: f64, gain: f32, loaded: &Loaded) -> Self {
        const COLUMNS: usize = 400;
        let samples = channels.iter().map(Vec::len).max().unwrap_or(0);
        let peaks = channels
            .iter()
            .map(|c| {
                (0..COLUMNS)
                    .map(|k| {
                        let from = (k * samples / COLUMNS).min(c.len());
                        let to = ((k + 1) * samples / COLUMNS).max(from + 1).min(c.len());
                        c[from..to.max(from)]
                            .iter()
                            .fold((0.0f32, 0.0f32), |(lo, hi), &x| {
                                (lo.min(x * gain), hi.max(x * gain))
                            })
                    })
                    .collect()
            })
            .collect();
        Self {
            peaks,
            samples,
            rate,
            file_channels: loaded.file_channels,
            file_rate: loaded.file_rate,
            file_frames: loaded.file_frames,
            note: None,
        }
    }
}

/// Scans that set an effect's parameters from audio, as Click & Pop Repair's scan buttons do. A scan's print (what it measured) is kept and saved like a noise print;
/// the values it sets are ordinary parameter values, which the user can change afterwards.
pub struct Learns {
    /// The scans' names, as the editor's buttons and `capture_noise_print`'s `scan` give them;
    /// the first is what a capture without a name runs.
    pub scans: &'static [&'static str],
    /// Scan number `scan` of `audio` (one buffer per channel at `rate`) for plain `values`:
    /// its print.
    pub run: Scan,
    /// The parameters a print sets: (index, plain value).
    pub sets: fn(print: &[f32]) -> Vec<(usize, f64)>,
}

/// A scan of audio by number (one buffer per channel at `rate`, plain `values`): its print.
pub type Scan = fn(rate: f64, values: &[f64], audio: &[Vec<f32>], scan: usize) -> Result<Vec<f32>>;

/// An effect's curve: the gain in dB at `hz` for plain parameter `values` at `rate`.
pub type Response = fn(rate: f64, values: &[f64], hz: f64) -> f64;

/// What an effect learns from a stretch of audio (its "print": a noise print, clip levels),
/// as numbers.
pub type Capture = fn(rate: f64, values: &[f64], audio: &[Vec<f32>]) -> Result<Vec<f32>>;

/// A print on its way to the DSP, without the audio thread locking the effect or freeing
/// anything: the main thread leaves a `fresh` one; the insert swaps it into the effect with
/// [`Mailbox::deliver`] and leaves the old one `spent`, which the main thread drops with the
/// next print or the instance.
#[derive(Default)]
struct Mailbox {
    fresh: Option<Vec<f32>>,
    spent: Option<Vec<f32>>,
}

impl Mailbox {
    /// On the audio thread: hand a fresh print to `effect`, if one waits and the last one's
    /// buffer has been collected. Only `try_lock`s, unless `wait` (offline, as the effect's
    /// own lock is taken then).
    fn deliver(mailbox: &Mutex<Mailbox>, effect: &mut dyn NativeEffect, wait: bool) {
        let mail = if wait {
            Some(mailbox.lock().unwrap_or_else(|poison| poison.into_inner()))
        } else {
            mailbox.try_lock().ok()
        };
        let Some(mut mail) = mail else {
            return;
        };
        if mail.spent.is_some() {
            return;
        }
        if let Some(mut print) = mail.fresh.take() {
            effect.take_print(&mut print);
            mail.spent = Some(print);
        }
    }
}

/// A prepared file on its way to the DSP and back, as [`Mailbox`] carries a print: the main
/// thread leaves a `fresh` one (replacing, and so freeing, one not taken yet); the insert offers
/// it to the effect every block ([`NativeEffect::take_file`]), which leaves one it has finished
/// with in `spent` for the main thread to drop.
#[derive(Default)]
struct FileMailbox {
    fresh: Option<FileBox>,
    spent: Option<FileBox>,
}

impl FileMailbox {
    /// On the audio thread: let `effect` take a fresh file or give back a spent one. Only
    /// `try_lock`s, unless `wait` (offline, as the effect's own lock is taken then).
    fn deliver(mailbox: &Mutex<FileMailbox>, effect: &mut dyn NativeEffect, wait: bool) {
        let mail = if wait {
            Some(mailbox.lock().unwrap_or_else(|poison| poison.into_inner()))
        } else {
            mailbox.try_lock().ok()
        };
        if let Some(mut mail) = mail {
            let mail = &mut *mail;
            effect.take_file(&mut mail.fresh, &mut mail.spent);
        }
    }
}

/// What [`Kind::prepares`] made on its way to the DSP and back, as [`FileMailbox`] carries a
/// file, with the settings (bits of the `by` parameters' plain values) it was made for.
#[derive(Default)]
struct PreparedMailbox {
    fresh: Option<FileBox>,
    spent: Option<FileBox>,
    made_for: Option<Vec<u64>>,
}

impl PreparedMailbox {
    /// On the audio thread: let `effect` take a fresh one or give back a spent one. Only
    /// `try_lock`s, unless `wait` (offline, as the effect's own lock is taken then).
    fn deliver(mailbox: &Mutex<PreparedMailbox>, effect: &mut dyn NativeEffect, wait: bool) {
        let mail = if wait {
            Some(mailbox.lock().unwrap_or_else(|poison| poison.into_inner()))
        } else {
            mailbox.try_lock().ok()
        };
        if let Some(mut mail) = mail {
            let mail = &mut *mail;
            effect.take_prepared(&mut mail.fresh, &mut mail.spent);
        }
    }
}

/// Which preparation an impulse needs: the file (resolved; `None` for one made from the
/// settings), the rate and the shaping parameters.
#[derive(Debug, Clone, PartialEq)]
struct FileKey {
    path: Option<PathBuf>,
    rate: u64,
    values: Vec<u64>,
}

impl FileKey {
    fn new(reads: &Reads, path: Option<PathBuf>, rate: f64, values: &[f64]) -> Self {
        Self {
            path,
            rate: rate.to_bits(),
            values: reads
                .shaped_by
                .iter()
                .map(|&i| values[i].to_bits())
                .collect(),
        }
    }
}

/// A job for the worker: what to prepare, at which rate, with which plain values.
type FileJob = (FileKey, f64, Vec<f64>);

/// A preparation's result, from the worker.
type FileDone = (FileKey, std::result::Result<Prepared, String>);

/// The instance's worker that reads and prepares its file. It ends when the instance drops its
/// side of the channel.
struct FileWorker {
    jobs: Sender<FileJob>,
    done: Receiver<FileDone>,
}

impl FileWorker {
    fn start(reads: &'static Reads) -> Result<Self> {
        let (jobs, rx) = channel::<FileJob>();
        let (tx, done) = channel::<FileDone>();
        std::thread::Builder::new()
            .name(format!("prepare {}", reads.what))
            .spawn(move || {
                // The file as loaded, while only the settings change.
                let mut cache: Option<(PathBuf, u64, Arc<Loaded>)> = None;
                for (key, rate, values) in rx {
                    let loaded = match (&key.path, reads.load) {
                        (Some(path), Some(load)) => match &cache {
                            Some((p, r, loaded)) if p == path && *r == key.rate => {
                                Ok(loaded.clone())
                            }
                            _ => load(path, rate).map(Arc::new),
                        },
                        // Made from the settings: nothing to read.
                        _ => Ok(Arc::new(Loaded::default())),
                    };
                    let result = match loaded {
                        Ok(loaded) => {
                            let prepared = (reads.prepare)(rate, &values, &loaded);
                            if let Some(path) = &key.path {
                                cache = Some((path.clone(), key.rate, loaded));
                            }
                            Ok(prepared)
                        }
                        Err(e) => Err(format!("{e:#}")),
                    };
                    if tx.send((key, result)).is_err() {
                        break;
                    }
                }
            })
            .map_err(|e| anyhow!("could not start the {} thread: {e}", reads.what))?;
        Ok(Self { jobs, done })
    }
}

/// An instance's file ([`Kind::reads`]) and its preparation.
#[derive(Default)]
struct FileState {
    /// The file as saved: relative to the project's folder when inside it.
    path: Option<PathBuf>,
    /// The preparation the file and settings ask for, as of the last service.
    wanted: Option<FileKey>,
    /// The last preparation finished (or failed).
    done: Option<FileKey>,
    /// The one the worker is on.
    running: Option<FileKey>,
    worker: Option<FileWorker>,
    overview: Option<Overview>,
    error: Option<String>,
}

/// Every native effect.
pub static KINDS: &[&Kind] = &[
    &amplify::KIND,
    &channel_mixer::KIND,
    &parametric_eq::KIND,
    &graphic_eq::KIND_10,
    &graphic_eq::KIND_20,
    &graphic_eq::KIND_30,
    &hard_limiter::KIND,
    &compressor::KIND,
    &deesser::KIND,
    &dynamics::KIND,
    &multiband::KIND,
    &noise_reduction::KIND,
    &hiss_reduction::KIND,
    &adaptive_noise::KIND,
    &dehummer::KIND,
    &click_remover::KIND,
    &declipper::KIND,
    &click_pop::KIND,
    &studio_reverb::KIND,
    &convolution_reverb::KIND,
    &full_reverb::KIND,
    &delay::KIND,
    &echo::KIND,
    &analog_delay::KIND,
    &chorus::KIND,
    &flanger::KIND,
    &phaser::KIND,
];

/// An effect that was renamed (dev plan 28.5): what older versions called it.
pub struct Renamed {
    /// The id older projects, templates and racks hold (`native:<old_id>`); `None` when only
    /// the name changed.
    pub old_id: Option<&'static str>,
    /// The name older versions gave it, and so its insert slots' names in their files.
    pub old_name: &'static str,
    /// Its id today.
    pub id: &'static str,
}

/// Every renamed effect: the one table that keeps older files opening. Anything that reads an
/// insert's path (projects, templates, backups, rack presets, `rack.json`, `add_insert` and
/// the other actions) goes through [`kind_of`] or [`find`], which read the old ids as the new
/// ones, and saved slots are brought up to date as they are read ([`current_path`],
/// [`current_name`]), so a save writes only today's ids.
pub static RENAMED: &[Renamed] = &[
    Renamed {
        old_id: Some("single-band-compressor"),
        old_name: "Single-band Compressor",
        id: "compressor",
    },
    Renamed {
        old_id: Some("dynamics-processing"),
        old_name: "Dynamics Processing",
        id: "dynamics",
    },
    Renamed {
        old_id: Some("hard-limiter"),
        old_name: "Hard Limiter",
        id: "limiter",
    },
    Renamed {
        old_id: Some("deesser"),
        old_name: "DeEsser",
        id: "de-esser",
    },
    Renamed {
        old_id: Some("dehummer"),
        old_name: "DeHummer",
        id: "hum-remover",
    },
    Renamed {
        old_id: Some("declipper"),
        old_name: "DeClipper",
        id: "clip-repair",
    },
    Renamed {
        old_id: Some("auto-click-remover"),
        old_name: "Automatic Click Remover",
        id: "click-remover",
    },
    Renamed {
        old_id: Some("click-pop-eliminator"),
        old_name: "Click/Pop Eliminator",
        id: "click-pop-repair",
    },
    Renamed {
        old_id: Some("adaptive-noise-reduction"),
        old_name: "Adaptive Noise Reduction",
        id: "auto-noise-reduction",
    },
    Renamed {
        old_id: Some("studio-reverb"),
        old_name: "Studio Reverb",
        id: "room-reverb",
    },
    Renamed {
        old_id: Some("full-reverb"),
        old_name: "Full Reverb",
        id: "space-reverb",
    },
    Renamed {
        old_id: Some("analog-delay"),
        old_name: "Analog Delay",
        id: "vintage-delay",
    },
    Renamed {
        old_id: None,
        old_name: "Parametric Equalizer",
        id: "parametric-eq",
    },
    Renamed {
        old_id: None,
        old_name: "Graphic Equalizer (10 Bands)",
        id: "graphic-eq-10",
    },
    Renamed {
        old_id: None,
        old_name: "Graphic Equalizer (20 Bands)",
        id: "graphic-eq-20",
    },
    Renamed {
        old_id: None,
        old_name: "Graphic Equalizer (30 Bands)",
        id: "graphic-eq-30",
    },
];

/// A native effect by its id, by today's id for an old one.
fn by_id(id: &str) -> Option<&'static Kind> {
    let id = RENAMED
        .iter()
        .find(|r| r.old_id == Some(id))
        .map_or(id, |r| r.id);
    KINDS.iter().copied().find(|k| k.id == id)
}

/// A native effect by its id or name (case-insensitive); an old id or name
/// ([`RENAMED`]) finds the effect it became.
pub fn find(spec: &str) -> Option<&'static Kind> {
    let needle = spec.trim().to_lowercase();
    by_id(&needle)
        .or_else(|| {
            KINDS
                .iter()
                .copied()
                .find(|k| k.name.to_lowercase() == needle)
        })
        .or_else(|| {
            let r = RENAMED
                .iter()
                .find(|r| r.old_name.to_lowercase() == needle)?;
            by_id(r.id)
        })
}

/// The insert path of a native effect.
pub fn path_of(kind: &Kind) -> PathBuf {
    PathBuf::from(format!("{PATH_PREFIX}{}", kind.id))
}

/// The native effect an insert path names, if it names one (an old id names the effect it
/// became).
pub fn kind_of(path: &Path) -> Option<&'static Kind> {
    by_id(path.to_str()?.strip_prefix(PATH_PREFIX)?)
}

/// An insert path as it is written today: an old native id ([`RENAMED`]) becomes the new one;
/// any other path is kept as it is.
pub fn current_path(path: PathBuf) -> PathBuf {
    match kind_of(&path) {
        Some(kind) if path.to_str() != Some(&format!("{PATH_PREFIX}{}", kind.id)) => path_of(kind),
        _ => path,
    }
}

/// An insert slot's name as it is written today: the name an older version gave a native
/// effect (at `path`) becomes its name now. Any other name (a slot named otherwise) is kept.
pub fn current_name(path: &Path, name: String) -> String {
    match kind_of(path) {
        Some(kind)
            if RENAMED
                .iter()
                .any(|r| r.id == kind.id && r.old_name == name) =>
        {
            kind.name.to_string()
        }
        _ => name,
    }
}

/// Normalized parameter values shared by the main thread and the insert, without a lock: the
/// editor, actions and automation write them, the insert hands changes to the DSP each block.
struct Values(Box<[AtomicU64]>);

impl Values {
    fn get(&self, i: usize) -> f64 {
        f64::from_bits(self.0[i].load(Ordering::Relaxed))
    }

    fn set(&self, i: usize, value: f64) {
        self.0[i].store(value.to_bits(), Ordering::Relaxed);
    }
}

/// A loaded native effect: what the session keeps for its insert.
pub struct Native {
    pub kind: &'static Kind,
    values: Arc<Values>,
    effect: Arc<Mutex<Box<dyn NativeEffect>>>,
    latency: usize,
    /// The print the effect uses ([`Kind::capture`]), kept here for saving and the editor.
    print: Option<Vec<f32>>,
    mailbox: Arc<Mutex<Mailbox>>,
    /// The effect's counters ([`NativeEffect::readouts`]), if it keeps any.
    readouts: Option<Arc<[AtomicU64]>>,
    /// The file it plays ([`Kind::reads`]) and its preparation (boxed: most effects play none).
    file: Box<FileState>,
    files: Arc<Mutex<FileMailbox>>,
    /// What the effect starts from, made from the settings ([`Kind::prepares`]).
    prepared: Arc<Mutex<PreparedMailbox>>,
    /// The engine's rate it was made at, for parameters that follow others ([`Reads::derive`]).
    rate: f64,
}

impl Native {
    /// A new instance at `rate`, restored from `state` (from [`Native::save_state`]) if given.
    pub fn new(kind: &'static Kind, rate: f64, state: Option<&[u8]>) -> Result<Self> {
        let values: Box<[AtomicU64]> = kind
            .params
            .iter()
            .map(|p| AtomicU64::new(p.normalized(p.default).to_bits()))
            .collect();
        let effect = (kind.make)(rate);
        let latency = effect.latency();
        let readouts = effect.readouts();
        let mut native = Self {
            kind,
            values: Arc::new(Values(values)),
            effect: Arc::new(Mutex::new(effect)),
            latency,
            print: None,
            mailbox: Arc::default(),
            readouts,
            file: Box::default(),
            files: Arc::default(),
            prepared: Arc::default(),
            rate,
        };
        if let Some(state) = state {
            native.load_state(state)?;
        }
        native.derive(None);
        native.snap();
        Ok(native)
    }

    /// Hand every value (and the print) to the DSP and have it start there (before any audio
    /// runs).
    fn snap(&self) {
        self.prepare(&self.plain_values());
        let mut effect = self
            .effect
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        // What it starts from first, so the settings find it there.
        PreparedMailbox::deliver(&self.prepared, &mut **effect, true);
        for (i, p) in self.kind.params.iter().enumerate() {
            effect.set(i, p.plain(self.values.get(i)));
        }
        Mailbox::deliver(&self.mailbox, &mut **effect, true);
        FileMailbox::deliver(&self.files, &mut **effect, true);
        effect.snap();
    }

    /// On the main thread: make what the effect starts from ([`Kind::prepares`]) for plain
    /// `values`, if they differ from what the last one was made for, and post it for the DSP.
    /// A fresh one not yet taken is replaced (freed here), and a spent one collected.
    fn prepare(&self, values: &[f64]) {
        let Some(prepares) = self.kind.prepares else {
            return;
        };
        let made_for: Vec<u64> = prepares.by.iter().map(|&i| values[i].to_bits()).collect();
        let lock = || {
            self.prepared
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
        };
        if lock().made_for.as_ref() == Some(&made_for) {
            return;
        }
        // Made outside the lock, so the insert's `try_lock` rarely misses it.
        let made = (prepares.make)(self.rate, values);
        let (old, spent) = {
            let mut mail = lock();
            mail.made_for = Some(made_for);
            (mail.fresh.replace(made), mail.spent.take())
        };
        drop((old, spent));
    }

    /// The print the effect uses, if it learns one and has one.
    pub fn print(&self) -> Option<&[f32]> {
        self.print.as_deref()
    }

    /// Learn a print from `audio` (one buffer per channel at `rate`, the engine's) with the
    /// settings now, and have the effect use it from its next block, even while it plays.
    pub fn capture(&mut self, rate: f64, audio: &[Vec<f32>]) -> Result<()> {
        if self.kind.learns.is_some() {
            return self.scan(rate, audio, 0);
        }
        let capture = self
            .kind
            .capture
            .ok_or_else(|| anyhow!("{} doesn't learn from audio", self.kind.name))?;
        let print = capture(rate, &self.plain_values(), audio)?;
        self.set_print(print);
        Ok(())
    }

    /// Run scan number `scan` ([`Kind::learns`]) on `audio` (one buffer per channel at `rate`,
    /// the engine's): the parameters it learns take their new values (the effect follows from
    /// its next block, as a slider moved by hand) and its print is kept for the editor and
    /// saved with the settings.
    pub fn scan(&mut self, rate: f64, audio: &[Vec<f32>], scan: usize) -> Result<()> {
        let learns = self
            .kind
            .learns
            .ok_or_else(|| anyhow!("{} has no scans", self.kind.name))?;
        if scan >= learns.scans.len() {
            bail!("{} has no scan {scan}", self.kind.name);
        }
        let print = (learns.run)(rate, &self.plain_values(), audio, scan)?;
        for (i, plain) in (learns.sets)(&print) {
            if let Some(p) = self.kind.params.get(i) {
                self.values.set(i, p.normalized(plain));
            }
        }
        self.set_print(print);
        Ok(())
    }

    /// The scan a name gives (case-insensitive, or a unique part of it: "all levels").
    pub fn scan_named(&self, name: &str) -> Result<usize> {
        let learns = self
            .kind
            .learns
            .ok_or_else(|| anyhow!("{} has no scans", self.kind.name))?;
        let needle = name.trim().to_lowercase();
        let names: Vec<String> = learns.scans.iter().map(|s| s.to_lowercase()).collect();
        if let Some(i) = names.iter().position(|n| *n == needle) {
            return Ok(i);
        }
        let partial: Vec<usize> = (0..names.len())
            .filter(|&i| !needle.is_empty() && names[i].contains(&needle))
            .collect();
        match partial[..] {
            [i] => Ok(i),
            _ => bail!(
                "{} has no scan '{name}' (its scans: {})",
                self.kind.name,
                learns.scans.join(", ")
            ),
        }
    }

    /// The effect's counters now ([`NativeEffect::readouts`]); empty if it keeps none.
    pub fn readouts(&self) -> Vec<u64> {
        self.readouts
            .as_ref()
            .map(|r| r.iter().map(|v| v.load(Ordering::Relaxed)).collect())
            .unwrap_or_default()
    }

    /// Use `print` from the effect's next block. The previous one handed over is dropped here,
    /// on the main thread.
    fn set_print(&mut self, print: Vec<f32>) {
        let mut mail = self
            .mailbox
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        mail.spent = None;
        mail.fresh = Some(print.clone());
        self.print = Some(print);
    }

    /// The file the effect plays, as saved (relative to the project's folder when inside it).
    pub fn file(&self) -> Option<&Path> {
        self.file.path.as_deref()
    }

    /// Play `path` (as it is to be saved: relative to the project's folder when inside it;
    /// [`Native::service_file`] resolves it). It is prepared on the next service; until it is
    /// in, the file before it plays on.
    pub fn set_file(&mut self, path: PathBuf) -> Result<()> {
        match self.kind.reads {
            None => bail!("{} plays no file", self.kind.name),
            Some(reads) if reads.makes() => bail!(
                "{} makes its {} from its settings: it plays no file",
                self.kind.name,
                reads.what
            ),
            Some(_) => {}
        }
        self.file.path = Some(path);
        self.file.wanted = None;
        self.file.error = None;
        Ok(())
    }

    /// What the editor shows of the prepared file, once there is one.
    pub fn file_overview(&self) -> Option<&Overview> {
        self.file.overview.as_ref()
    }

    /// Why the file could not be read or prepared, if it couldn't.
    pub fn file_error(&self) -> Option<&str> {
        self.file.error.as_deref()
    }

    /// Whether the file (or a change of its settings) is still being prepared, as of the last
    /// [`Native::service_file`]: an export waits for it, so it renders what plays.
    pub fn file_pending(&self) -> bool {
        let Some(reads) = self.kind.reads else {
            return false;
        };
        // A file-playing effect with no file has nothing to prepare; one that makes its
        // impulse always has.
        if self.file.path.is_none() && !reads.makes() {
            return false;
        }
        // Chosen or restored and not serviced since.
        let Some(wanted) = &self.file.wanted else {
            return true;
        };
        // A shaping setting changed since the last service.
        let values = self.plain_values();
        let changed = reads
            .shaped_by
            .iter()
            .zip(&wanted.values)
            .any(|(&i, &v)| values[i].to_bits() != v);
        changed || self.file.running.is_some() || self.file.done.as_ref() != Some(wanted)
    }

    /// The file's preparation, on the main thread each frame: free what the DSP gave back,
    /// take a finished preparation (into the mailbox, for the effect's next block), and start
    /// the next if the file, `rate` or the shaping settings changed (one at a time: the latest
    /// settings win). `resolve` makes the saved path absolute. Returns an error to report, once.
    pub fn service_file(
        &mut self,
        rate: f64,
        resolve: &dyn Fn(&Path) -> PathBuf,
    ) -> Option<String> {
        let reads = self.kind.reads?;
        // Dropped here, off the audio thread.
        let spent = self
            .files
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .spent
            .take();
        drop(spent);
        let mut report = None;
        let mut finished = Vec::new();
        if let Some(worker) = &self.file.worker {
            while let Ok(done) = worker.done.try_recv() {
                finished.push(done);
            }
        }
        for (key, result) in finished {
            if self.file.running.as_ref() == Some(&key) {
                self.file.running = None;
            }
            // A preparation for settings since changed is dropped; the next starts below.
            if self.file.wanted.as_ref() == Some(&key) {
                match result {
                    Ok(prepared) => self.deliver_file(prepared),
                    Err(e) => {
                        report = Some(format!("{} {}: {e}", self.kind.name, reads.what));
                        self.file.error = Some(e);
                    }
                }
            }
            self.file.done = Some(key);
        }
        let values = self.plain_values();
        self.file.wanted = if reads.makes() {
            Some(FileKey::new(reads, None, rate, &values))
        } else {
            self.file
                .path
                .as_ref()
                .map(|path| FileKey::new(reads, Some(resolve(path)), rate, &values))
        };
        let Some(key) = self.file.wanted.clone() else {
            return report;
        };
        if self.file.running.is_some() || self.file.done.as_ref() == Some(&key) {
            return report;
        }
        if self.file.worker.is_none() {
            match FileWorker::start(reads) {
                Ok(worker) => self.file.worker = Some(worker),
                Err(e) => {
                    self.file.done = Some(key);
                    self.file.error = Some(format!("{e:#}"));
                    return self.file.error.clone();
                }
            }
        }
        let sent = self
            .file
            .worker
            .as_ref()
            .is_some_and(|w| w.jobs.send((key.clone(), rate, values)).is_ok());
        if sent {
            self.file.running = Some(key);
        } else {
            self.file.done = Some(key);
            self.file.error = Some(format!("the {} thread has stopped", reads.what));
            report = self.file.error.clone();
        }
        report
    }

    /// Read and prepare `path` (absolute) at `rate` now, on this thread, and play it from the
    /// effect's next block: for renders, checks and tests, which don't run a worker.
    pub fn prepare_file_now(&mut self, rate: f64, path: &Path) -> Result<()> {
        self.prepare_now(rate, Some(path))
    }

    /// Prepare the impulse at `rate` now, on this thread, for the settings now: read from
    /// `path` (absolute) for an effect that plays a file, made from the settings (`path`
    /// `None`) for one that makes it. It plays from the effect's next block.
    pub fn prepare_now(&mut self, rate: f64, path: Option<&Path>) -> Result<()> {
        let reads = self
            .kind
            .reads
            .ok_or_else(|| anyhow!("{} plays no file", self.kind.name))?;
        let loaded = match (path, reads.load) {
            (Some(path), Some(load)) => load(path, rate)?,
            (None, None) => Loaded::default(),
            (Some(_), None) => bail!(
                "{} makes its {} from its settings: it plays no file",
                self.kind.name,
                reads.what
            ),
            (None, Some(_)) => bail!("{} needs a file to prepare", self.kind.name),
        };
        let values = self.plain_values();
        let prepared = (reads.prepare)(rate, &values, &loaded);
        self.file.path = path.map(Path::to_path_buf);
        let key = FileKey::new(reads, path.map(Path::to_path_buf), rate, &values);
        self.file.wanted = Some(key.clone());
        self.file.done = Some(key);
        self.deliver_file(prepared);
        Ok(())
    }

    fn deliver_file(&mut self, prepared: Prepared) {
        // A fresh one not taken yet is replaced, and freed here.
        let old = self
            .files
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .fresh
            .replace(prepared.dsp);
        drop(old);
        self.file.overview = Some(prepared.overview);
        self.file.error = None;
    }

    pub fn latency(&self) -> usize {
        self.latency
    }

    /// The delay the settings now ask for, if it differs from [`Native::latency`]: the
    /// session then takes it and rebuilds the graph around it. No lock.
    pub fn latency_changed(&mut self, rate: f64) -> Option<usize> {
        let of = self.kind.latency?;
        let now = of(rate, &self.plain_values());
        (now != self.latency).then(|| {
            self.latency = now;
            now
        })
    }

    /// The parameters' plain values now.
    pub fn plain_values(&self) -> Vec<f64> {
        self.kind
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| p.plain(self.values.get(i)))
            .collect()
    }

    /// The parameters, as VST3 plug-ins describe theirs: ids are the indices.
    pub fn parameters(&self) -> Vec<Parameter> {
        self.kind
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| Parameter {
                id: i as u32,
                name: p.name.into(),
                value: self.values.get(i),
                min: 0.0,
                max: 1.0,
                default: p.normalized(p.default),
                unit: p.unit.into(),
                step_count: p.steps as i32,
                can_automate: p.automatable,
                is_read_only: self.kind.reads.is_some_and(|r| r.outputs.contains(&i)),
                is_bypass: false,
                flags: 0,
            })
            .collect()
    }

    fn spec(&self, id: u32) -> Result<&'static ParamSpec> {
        self.kind
            .params
            .get(id as usize)
            .ok_or_else(|| anyhow!("{} has no parameter {id}", self.kind.name))
    }

    /// How normalized `value` of parameter `id` shows.
    pub fn format(&self, id: u32, value: f64) -> Result<String> {
        let spec = self.spec(id)?;
        Ok(spec.display(spec.plain(value)))
    }

    /// Set parameter `id` to normalized `value`; the insert passes it on with its next block.
    /// Parameters that follow it move with it ([`Reads::derive`]).
    pub fn set(&self, id: u32, value: f64) -> Result<()> {
        let spec = self.spec(id)?;
        if !(0.0..=1.0).contains(&value) {
            bail!("value {value} is outside 0..1");
        }
        let old = spec.plain(self.values.get(id as usize));
        // What the effect starts from is posted before the DSP can see the value it is for.
        if self.kind.prepares.is_some() {
            let mut plain = self.plain_values();
            plain[id as usize] = spec.plain(value);
            self.prepare(&plain);
        }
        self.values.set(id as usize, value);
        self.derive(Some((id as usize, old)));
        Ok(())
    }

    /// Parameter `id`'s normalized value now (0 for an id it doesn't have).
    pub fn value(&self, id: u32) -> f64 {
        if (id as usize) < self.kind.params.len() {
            self.values.get(id as usize)
        } else {
            0.0
        }
    }

    /// Move the parameters that follow others ([`Reads::derive`]) after parameter `changed.0`
    /// was set (from plain `changed.1`), or for a new or restored instance (`None`). Only the
    /// values it changes are stored again, so the rest keep their exact normalized values.
    fn derive(&self, changed: Option<(usize, f64)>) {
        let Some(derive) = self.kind.reads.and_then(|r| r.derive) else {
            return;
        };
        let before = self.plain_values();
        let mut after = before.clone();
        derive(self.rate, changed, &mut after);
        for (i, (b, a)) in before.iter().zip(&after).enumerate() {
            if a.to_bits() != b.to_bits() {
                self.values.set(i, self.kind.params[i].normalized(*a));
            }
        }
    }

    /// The parameters' plain values by name, as JSON.
    pub fn save_state(&self) -> Vec<u8> {
        let plain: BTreeMap<&str, f64> = self
            .kind
            .params
            .iter()
            .enumerate()
            .map(|(i, p)| (p.name, p.plain(self.values.get(i))))
            .collect();
        let mut json = serde_json::json!({ "version": 1, "params": plain });
        if let Some(print) = &self.print {
            json["print"] = serde_json::json!(print);
        }
        if let Some(file) = &self.file.path {
            // Forward slashes, so a project moved to another system still finds it.
            json["file"] = serde_json::json!(file.to_string_lossy().replace('\\', "/"));
        }
        serde_json::to_vec(&json).unwrap_or_default()
    }

    /// Take another instance's [`Native::save_state`] while this one may be running (a synced
    /// rack copying a whole state across, dev plan 21.2): its values reach the DSP with the next
    /// block as a set's do, a print through the mailbox, a file through the next service.
    pub fn restore(&mut self, state: &[u8]) -> Result<()> {
        self.load_state(state)?;
        self.derive(None);
        if self.kind.prepares.is_some() {
            self.prepare(&self.plain_values());
        }
        Ok(())
    }

    /// Restore [`Native::save_state`]'s JSON, before the instance runs. Names it doesn't know
    /// are ignored; parameters it lacks keep their defaults.
    fn load_state(&mut self, state: &[u8]) -> Result<()> {
        let json: serde_json::Value = serde_json::from_slice(state)
            .map_err(|e| anyhow!("{}: unreadable settings: {e}", self.kind.name))?;
        let params = json
            .get("params")
            .and_then(|p| p.as_object())
            .ok_or_else(|| anyhow!("{}: settings have no params", self.kind.name))?;
        for (i, p) in self.kind.params.iter().enumerate() {
            if let Some(v) = params.get(p.name).and_then(|v| v.as_f64()) {
                self.values.set(i, p.normalized(v));
            }
        }
        // A print only for an effect that learns one: `snap` hands it over.
        if self.kind.capture.is_some() {
            if let Some(print) = json.get("print") {
                let print: Vec<f32> = serde_json::from_value(print.clone())
                    .map_err(|e| anyhow!("{}: unreadable print: {e}", self.kind.name))?;
                self.set_print(print);
            }
        }
        // A file only for an effect that plays one: prepared on the first service.
        if self.kind.reads.is_some_and(|r| !r.makes()) {
            if let Some(file) = json.get("file").and_then(|f| f.as_str()) {
                self.file.path = Some(PathBuf::from(file));
            }
        }
        Ok(())
    }

    /// The insert the graph runs. Every graph's insert shares this instance's DSP, so a graph
    /// swap carries its state over with no handover.
    pub fn insert(&self, bypass: Arc<AtomicBool>, flags: InsertFlags) -> NativeInsert {
        NativeInsert {
            kind: self.kind,
            effect: self.effect.clone(),
            values: self.values.clone(),
            // NaN: every parameter goes to the DSP on the first block.
            applied: vec![f64::NAN; self.kind.params.len()],
            bypass,
            flags,
            latency: self.latency,
            passthrough: DelayLine::new(self.latency, CHANNELS),
            pending: Vec::with_capacity(MAX_PENDING),
            next: vec![usize::MAX; MAX_PENDING],
            last: vec![usize::MAX; self.kind.params.len()],
            ahead: vec![None; self.kind.params.len()],
            expected: None,
            was_playing: false,
            mailbox: self.mailbox.clone(),
            files: self.kind.reads.map(|_| self.files.clone()),
            prepared: self.kind.prepares.map(|_| self.prepared.clone()),
        }
    }
}

/// Automation points one native insert takes per block (as `PluginInsert`'s).
const MAX_PENDING: usize = 2048;

/// A native effect as a node's insert.
pub struct NativeInsert {
    kind: &'static Kind,
    effect: Arc<Mutex<Box<dyn NativeEffect>>>,
    values: Arc<Values>,
    /// The normalized values the DSP has, so only changes are passed on.
    applied: Vec<f64>,
    bypass: Arc<AtomicBool>,
    flags: InsertFlags,
    latency: usize,
    /// Keeps delay compensation intact while bypassed.
    passthrough: DelayLine,
    /// Automation points for the next block: parameter, normalized value, sample offset.
    pending: Vec<(u32, f64, usize)>,
    /// For each pending point (in time order), the index of its parameter's next point.
    next: Vec<usize>,
    /// Scratch for building `next`: each parameter's latest point seen.
    last: Vec<usize>,
    /// Each parameter's ramp under way: the timeline position it is heading for.
    ahead: Vec<Option<i64>>,
    /// Where the next block starts if playback carries straight on.
    expected: Option<i64>,
    /// Whether the transport was playing in the last block, to tell the effect it started.
    was_playing: bool,
    /// A new print on its way to the effect.
    mailbox: Arc<Mutex<Mailbox>>,
    /// Prepared files on their way to and from the effect, for an effect that plays one.
    files: Option<Arc<Mutex<FileMailbox>>>,
    /// What the effect starts from on its way to it, for an effect that prepares one.
    prepared: Option<Arc<Mutex<PreparedMailbox>>>,
}

impl Processor for NativeInsert {
    fn latency(&self) -> usize {
        self.latency
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize, transport: &Transport) {
        let start = transport.position;
        let continuous = self.expected == Some(start);
        self.expected = Some(start + frames as i64);
        if self.bypass.load(Ordering::Relaxed) {
            self.pending.clear();
            self.ahead.fill(None);
            self.passthrough.process(io, frames);
            return;
        }
        let effect = self.effect.clone();
        let guard = if self.flags.offline.load(Ordering::Relaxed) {
            Some(effect.lock().unwrap_or_else(|poison| poison.into_inner()))
        } else {
            effect.try_lock().ok()
        };
        let Some(mut effect) = guard else {
            crate::plugins::skip(&self.flags, io, frames);
            self.pending.clear();
            self.ahead.fill(None);
            return;
        };
        if self.kind.capture.is_some() {
            let wait = self.flags.offline.load(Ordering::Relaxed);
            Mailbox::deliver(&self.mailbox, &mut **effect, wait);
        }
        if let Some(prepared) = &self.prepared {
            let wait = self.flags.offline.load(Ordering::Relaxed);
            PreparedMailbox::deliver(prepared, &mut **effect, wait);
        }
        if let Some(files) = &self.files {
            let wait = self.flags.offline.load(Ordering::Relaxed);
            FileMailbox::deliver(files, &mut **effect, wait);
        }
        effect.transport(transport);
        if transport.playing && !self.was_playing {
            effect.start();
        }
        self.was_playing = transport.playing;
        let params = self.kind.params;
        // Automation: lanes send a point at the block's start, on every grid line, and the next
        // grid line past the block (`wants_lookahead`), one lane after another. In time order
        // (no allocation), each point links to its parameter's next one, so the effect can
        // draw the line between them.
        self.pending.retain(|p| (p.0 as usize) < params.len());
        self.pending.sort_unstable_by_key(|p| p.2);
        self.last.fill(usize::MAX);
        for k in (0..self.pending.len()).rev() {
            let i = self.pending[k].0 as usize;
            self.next[k] = self.last[i];
            self.last[i] = k;
        }
        // Settings made by hand since the last block, for parameters no lane is playing (a
        // lane overrules the knob).
        for (i, spec) in params.iter().enumerate() {
            let value = self.values.get(i);
            if self.last[i] == usize::MAX && self.applied[i] != value {
                self.applied[i] = value;
                effect.set(i, spec.plain(value));
            }
        }
        // The grid line after the block's start: a ramp still heading there carries on.
        let next_line = start.div_euclid(AUTOMATION_GRID) * AUTOMATION_GRID + AUTOMATION_GRID;
        let mut at = 0;
        for k in 0..self.pending.len() {
            let (id, value, offset) = self.pending[k];
            // Points past the block only say where the last line goes.
            if offset >= frames {
                break;
            }
            if offset > at {
                effect.process(io, at, offset);
                at = offset;
            }
            let i = id as usize;
            let spec = &params[i];
            self.values.set(i, value);
            // A block start between grid lines, mid-way along a line already being drawn: the
            // line carries on untouched, so a split block plays as one would.
            if offset == 0 && continuous && self.ahead[i] == Some(next_line) {
                continue;
            }
            // There now; then in a line to the parameter's next point, or held when there is
            // none (a lane that holds sends only its start).
            effect.ramp(i, spec.plain(value), 0);
            self.ahead[i] = None;
            self.applied[i] = value;
            if let Some(&(_, to, when)) = self.pending.get(self.next[k]) {
                if spec.steps == 0 {
                    effect.ramp(i, spec.plain(to), when - offset);
                    self.ahead[i] = Some(start + when as i64);
                    self.applied[i] = to;
                }
            }
        }
        if frames > at {
            effect.process(io, at, frames);
        }
        self.pending.clear();
    }

    fn set_parameter_at(&mut self, id: u32, value: f64, offset: usize) {
        if self.pending.len() < MAX_PENDING {
            self.pending.push((id, value, offset));
        }
    }

    fn wants_lookahead(&self) -> bool {
        true
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    /// The same effect in the previous graph: carry on from it, so a rebuild while playing is
    /// no new start (the Graphic EQ would clear) and lines being drawn carry on. No allocation.
    fn take_state_from(&mut self, old: &mut dyn Processor) -> bool {
        let Some(old) = old
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<NativeInsert>())
        else {
            return false;
        };
        if !Arc::ptr_eq(&self.effect, &old.effect) {
            return false;
        }
        self.was_playing = old.was_playing;
        self.expected = old.expected;
        self.applied.copy_from_slice(&old.applied);
        self.ahead.copy_from_slice(&old.ahead);
        if self.passthrough.delay() == old.passthrough.delay() {
            std::mem::swap(&mut self.passthrough, &mut old.passthrough);
        }
        true
    }
}

/// Render `audio` (one buffer per channel, mono played on both) through a fresh instance of
/// `kind` with `params` (plain values by name), in blocks of `block`, as the engine runs it
/// offline. For `effect_check` and tests; the session's inserts run the same [`NativeInsert`].
pub fn render(
    kind: &'static Kind,
    rate: f64,
    params: &BTreeMap<String, f64>,
    audio: &[Vec<f32>],
    block: usize,
) -> Result<Vec<Vec<f32>>> {
    render_with_print(kind, rate, params, None, audio, block)
}

/// [`render`], with the effect's print first captured from `print` (one buffer per channel),
/// as the Waveform view's "Capture noise print / clip levels" does before the effect runs.
pub fn render_with_print(
    kind: &'static Kind,
    rate: f64,
    params: &BTreeMap<String, f64>,
    print: Option<&[Vec<f32>]>,
    audio: &[Vec<f32>],
    block: usize,
) -> Result<Vec<Vec<f32>>> {
    render_with_file(kind, rate, params, print, None, audio, block)
}

/// [`render_with_print`], with the effect's file (an impulse response) first prepared from
/// `file` (an absolute path) with the settings, as the session prepares it before an export;
/// an effect that makes its impulse from its settings (the Space Reverb) makes it first.
pub fn render_with_file(
    kind: &'static Kind,
    rate: f64,
    params: &BTreeMap<String, f64>,
    print: Option<&[Vec<f32>]>,
    file: Option<&Path>,
    audio: &[Vec<f32>],
    block: usize,
) -> Result<Vec<Vec<f32>>> {
    let mut insert = offline_insert(kind, rate, params, print, file)?;
    let len = audio.first().map_or(0, Vec::len);
    let total = len + insert.latency();
    let mut out = vec![Vec::with_capacity(len); audio.len().max(1)];
    let mut io = vec![vec![0.0f32; block]; CHANNELS];
    let transport = Transport::default();
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        for (c, buf) in io.iter_mut().enumerate() {
            let src = &audio[c.min(audio.len().saturating_sub(1))];
            for (k, s) in buf[..n].iter_mut().enumerate() {
                *s = src.get(pos + k).copied().unwrap_or(0.0);
            }
        }
        insert.process(&mut io, n, &transport);
        for (c, o) in out.iter_mut().enumerate() {
            o.extend_from_slice(&io[c.min(CHANNELS - 1)][..n]);
        }
        pos += n;
    }
    let latency = insert.latency();
    for o in &mut out {
        o.drain(..latency);
    }
    Ok(out)
}

/// A fresh instance of `kind` as an offline insert (it waits for its lock rather than
/// skipping), set up as [`render_with_file`] sets it up: `params` (plain values by name), the
/// print captured from `print`, the impulse prepared from `file` or made from the settings.
/// For [`render_with_file`] and checks that time the insert block by block (`denormal_check`).
pub fn offline_insert(
    kind: &'static Kind,
    rate: f64,
    params: &BTreeMap<String, f64>,
    print: Option<&[Vec<f32>]>,
    file: Option<&Path>,
) -> Result<NativeInsert> {
    let mut native = Native::new(kind, rate, None)?;
    for (name, &value) in params {
        let i = kind
            .params
            .iter()
            .position(|p| p.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| anyhow!("{} has no parameter '{name}'", kind.name))?;
        native.values.set(i, kind.params[i].normalized(value));
    }
    native.derive(None);
    if let Some(print) = print {
        native.capture(rate, print)?;
    }
    // An impulse: the file's, or one the effect makes from its settings.
    if let Some(reads) = kind.reads {
        if file.is_some() || reads.makes() {
            native.prepare_now(rate, file)?;
        }
    }
    native.snap();
    // A delay that follows the settings (a look-ahead, a crossover): theirs, not the defaults'.
    native.latency_changed(rate);
    let flags = InsertFlags::default();
    flags.offline.store(true, Ordering::Relaxed);
    Ok(native.insert(Arc::new(AtomicBool::new(false)), flags))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stepped_and_continuous_values_round_trip() {
        let gain = ParamSpec {
            name: "Gain",
            unit: "dB",
            min: -96.0,
            max: 48.0,
            default: 0.0,
            steps: 0,
            decimals: 1,
            automatable: true,
            link: None,
            log_slider: false,
            format: None,
        };
        assert!((gain.plain(gain.normalized(6.0)) - 6.0).abs() < 1e-12);
        assert_eq!(gain.display(6.0), "+6.0 dB");
        assert_eq!(gain.display(-3.25), "-3.2 dB");
        let switch = ParamSpec {
            name: "Link",
            unit: "",
            min: 0.0,
            max: 1.0,
            default: 1.0,
            steps: 1,
            decimals: 0,
            automatable: false,
            link: None,
            log_slider: false,
            format: None,
        };
        assert_eq!(switch.plain(switch.normalized(1.0)), 1.0);
        assert_eq!(switch.plain(switch.normalized(0.0)), 0.0);
        assert_eq!(switch.display(1.0), "On");
    }

    #[test]
    fn renamed_effects_are_found_by_their_old_ids_and_names() {
        for r in RENAMED {
            let kind = find(r.id).expect("a renamed effect exists");
            assert_eq!(
                find(r.old_name).map(|k| k.id),
                Some(kind.id),
                "{}",
                r.old_name
            );
            assert_ne!(r.old_name, kind.name, "{} kept its name", r.id);
            let name = current_name(&path_of(kind), r.old_name.to_string());
            assert_eq!(name, kind.name);
            if let Some(old) = r.old_id {
                assert!(KINDS.iter().all(|k| k.id != old), "{old} is still an id");
                let path = PathBuf::from(format!("{PATH_PREFIX}{old}"));
                assert_eq!(kind_of(&path).map(|k| k.id), Some(kind.id));
                assert_eq!(find(old).map(|k| k.id), Some(kind.id));
                assert_eq!(current_path(path.clone()), path_of(kind));
                assert_eq!(current_name(&path, r.old_name.to_string()), kind.name);
            }
        }
        // Other paths and names are kept.
        let vst = PathBuf::from("C:/VST3/DeHummer.vst3");
        assert_eq!(current_path(vst.clone()), vst);
        assert_eq!(current_name(&vst, "DeHummer".into()), "DeHummer");
        let amp = path_of(find("amplify").unwrap());
        assert_eq!(current_path(amp.clone()), amp);
        let hum = path_of(find("hum-remover").unwrap());
        assert_eq!(current_name(&hum, "My hum".into()), "My hum");
    }

    #[test]
    fn a_native_effect_is_found_by_name_and_path() {
        let kind = find("AMPLIFY").expect("amplify is built in");
        assert_eq!(kind_of(&path_of(kind)).map(|k| k.id), Some(kind.id));
        assert!(kind_of(Path::new("C:/VST3/Amplify.vst3")).is_none());
    }

    #[test]
    fn a_graph_swap_while_playing_carries_an_effect_on() {
        // A Graphic EQ (it restarts its FFT blocks when the transport starts) with a band up,
        // run straight through, and again with its insert replaced half way, as a graph
        // rebuild replaces it: the same samples.
        let kind = find("graphic-eq-10").unwrap();
        let run = |swap_at: Option<usize>| -> Vec<f32> {
            let native = Native::new(kind, 48_000.0, None).unwrap();
            native.set(7, 0.6).unwrap();
            let flags = InsertFlags::default();
            let bypass = Arc::new(AtomicBool::new(false));
            let mut insert = native.insert(bypass.clone(), flags.clone());
            let mut out = Vec::new();
            let block = 480;
            for b in 0..60 {
                if Some(b) == swap_at {
                    let mut next = native.insert(bypass.clone(), flags.clone());
                    assert!(next.take_state_from(&mut insert));
                    insert = next;
                }
                let mut io: Vec<Vec<f32>> = (0..CHANNELS)
                    .map(|c| {
                        (0..block)
                            .map(|i| (((b * block + i) as f32) * (0.05 + c as f32 * 0.03)).sin())
                            .collect()
                    })
                    .collect();
                let transport = Transport {
                    playing: true,
                    position: (b * block) as i64,
                    ..Transport::default()
                };
                insert.process(&mut io, block, &transport);
                out.extend_from_slice(&io[0]);
            }
            out
        };
        assert_eq!(run(None), run(Some(31)));
    }

    #[test]
    fn a_print_captured_while_playing_reaches_the_effect_without_a_gap() {
        let kind = find("Noise Reduction").unwrap();
        let mut native = Native::new(kind, 48_000.0, None).unwrap();
        let flags = InsertFlags::default();
        let mut insert = native.insert(Arc::new(AtomicBool::new(false)), flags.clone());
        let hiss = |b: usize, i: usize| {
            (((b * 512 + i) as f32 * 12.9898).sin() * 43_758.547).fract() * 0.03
        };
        let level = |insert: &mut NativeInsert, from: usize| -> f64 {
            let mut power = 0.0;
            for b in from..from + 40 {
                let mut io: Vec<Vec<f32>> = (0..CHANNELS)
                    .map(|_| (0..512).map(|i| hiss(b, i)).collect())
                    .collect();
                let transport = Transport {
                    playing: true,
                    position: (b * 512) as i64,
                    ..Transport::default()
                };
                insert.process(&mut io, 512, &transport);
                power += io[0].iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
            power
        };
        let before = level(&mut insert, 0);
        let noise: Vec<f32> = (0..40_000).map(|i| hiss(i / 512, i % 512)).collect();
        native.capture(48_000.0, &[noise]).unwrap();
        level(&mut insert, 40);
        let after = level(&mut insert, 80);
        assert!(after < before / 100.0, "{before} then {after}");
        assert_eq!(flags.misses.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn state_survives_a_round_trip_and_unknown_names() {
        let kind = find("amplify").unwrap();
        let a = Native::new(kind, 48_000.0, None).unwrap();
        a.set(0, 0.8).unwrap();
        let state = a.save_state();
        let b = Native::new(kind, 48_000.0, Some(&state)).unwrap();
        assert_eq!(a.parameters()[0].value, b.parameters()[0].value);
        let c = Native::new(
            kind,
            48_000.0,
            Some(br#"{"version":1,"params":{"Nope":3}}"#),
        );
        assert!(c.is_ok());
    }
}
