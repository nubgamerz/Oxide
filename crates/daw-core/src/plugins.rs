//! Finding and loading VST3 plugins.

use crate::graph::{DelayLine, Meter, Processor, Transport, CHANNELS};
use anyhow::{anyhow, bail, Context, Result};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use vst3_host::{
    AudioBusLayout, BusAudioBuffers, BusDirection, MediaType, MidiChannel, MidiEvent, Plugin,
    TransportPosition, Vst3Host,
};

/// Build an in-process host. Process isolation is off: the helper binary is not built by
/// cargo for a dependency, and isolation would add IPC to every block we measure.
pub fn host(sample_rate: f64, block_size: usize) -> Result<Vst3Host> {
    Vst3Host::builder()
        .sample_rate(sample_rate)
        .block_size(block_size)
        .scan_default_paths()
        .with_process_isolation(false)
        .build()
        .context("building VST3 host")
}

/// Installed `.vst3` bundles, found by walking the standard folders without loading anything.
pub fn installed(host: &Vst3Host) -> Vec<PathBuf> {
    let mut paths = host.scan_plugin_paths();
    paths.sort();
    paths.dedup();
    paths
}

/// The vendor folder a bundle sits in, e.g. "Acme Audio" ("Other" at top level).
pub fn vendor(path: &Path) -> String {
    if crate::native::kind_of(path).is_some() {
        return BUILT_IN.into();
    }
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy().into_owned());
    match parent {
        Some(p) if !p.eq_ignore_ascii_case("VST3") => p,
        _ => "Other".into(),
    }
}

/// What the browser and `list_plugins` call the native effects' "vendor".
pub const BUILT_IN: &str = "Built in";

/// The bundle name without the `.vst3` extension; a native effect's own name.
pub fn display_name(path: &Path) -> String {
    if let Some(kind) = crate::native::kind_of(path) {
        return kind.name.into();
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Resolve a `--plugin` argument: an existing path is used as is, otherwise it is matched
/// case-insensitively against the built-in effects' names and installed bundle names (exact
/// name first, then substring). A native effect's `native:<id>` path resolves to itself (an old
/// id, `native::RENAMED`, to today's).
pub fn resolve(spec: &str, installed: &[PathBuf]) -> Result<PathBuf> {
    let as_path = Path::new(spec);
    if let Some(kind) = crate::native::kind_of(as_path) {
        return Ok(crate::native::path_of(kind));
    }
    if as_path.exists() {
        return Ok(as_path.to_path_buf());
    }
    if let Some(kind) = crate::native::find(spec) {
        return Ok(crate::native::path_of(kind));
    }
    let needle = spec.to_lowercase();
    if let Some(exact) = installed
        .iter()
        .find(|p| display_name(p).to_lowercase() == needle)
    {
        return Ok(exact.clone());
    }
    let candidates: Vec<PathBuf> = crate::native::KINDS
        .iter()
        .map(|k| crate::native::path_of(k))
        .chain(installed.iter().cloned())
        .collect();
    let matches: Vec<&PathBuf> = candidates
        .iter()
        .filter(|p| display_name(p).to_lowercase().contains(&needle))
        .collect();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => bail!("no installed plugin matches '{spec}' (try `dawpoc list`)"),
        many => bail!(
            "'{spec}' is ambiguous: {}",
            many.iter()
                .map(|p| display_name(p))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// Load a plugin and optionally restore a preset.
///
/// Rust panics during load are turned into errors. A plugin that aborts or throws a C++
/// exception through the host frames still takes the whole process down; nothing in-process
/// can catch that.
pub fn load(host: &mut Vst3Host, path: &Path, preset: Option<&Path>) -> Result<Plugin> {
    let name = display_name(path);
    let mut plugin = catch_unwind(AssertUnwindSafe(|| host.load_plugin(path)))
        .map_err(|_| anyhow!("{name}: panicked while loading"))?
        .with_context(|| format!("{name}: failed to load"))?;

    if let Some(preset) = preset {
        let is_vstpreset = preset
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("vstpreset"));
        let result = if is_vstpreset {
            plugin.load_vstpreset(preset)
        } else {
            plugin.load_preset(preset)
        };
        result.with_context(|| format!("{name}: loading preset {}", preset.display()))?;
    }
    Ok(plugin)
}

/// What activation learned about a plugin.
#[derive(Debug, Clone)]
pub struct Activation {
    pub latency: usize,
    pub has_audio_input: bool,
    /// Bus layout once active, so insert buffers can be built without locking the plugin.
    pub layout: AudioBusLayout,
    /// Why `state` could not be restored, if it couldn't (the plug-in runs with its defaults).
    pub state_error: Option<String>,
}

/// Activate a freshly loaded (not yet processing) plugin for live use: enable its main buses,
/// start processing, apply any latency change it announces on activation, restore `state`
/// (its saved settings), then run `preroll` samples of silence so plugins that settle after
/// activation (an amp sim may take about a second) sound the same from the first real sample.
///
/// The state goes in once the plug-in has run (started, and one block processed), not before:
/// a JUCE sampler restored before it has run was seen to keep its settings but never load
/// its instrument's samples, and play silence. The pre-roll comes after, so a plug-in settles
/// with its own settings, not its defaults.
pub fn activate(
    plugin: &mut Plugin,
    block: usize,
    preroll: usize,
    state: Option<&[u8]>,
) -> Result<Activation> {
    let name = plugin.info().name.clone();
    let layout = plugin.audio_bus_layout().ok();
    let has_audio_input = layout.as_ref().is_some_and(|l| !l.inputs.is_empty());
    if has_audio_input {
        plugin
            .set_bus_active(MediaType::Audio, BusDirection::Input, 0, true)
            .with_context(|| format!("{name}: activating main input bus"))?;
    }
    plugin
        .set_bus_active(MediaType::Audio, BusDirection::Output, 0, true)
        .with_context(|| format!("{name}: activating main output bus"))?;
    // Extra outputs start off, whatever the plug-in's defaults (a sampler may turn all 32 on):
    // they cost processing, and the session turns on the ones a track listens to
    // (`Session::sync_plugin_outputs`). Now, before processing starts, that costs nothing.
    let extra = layout.as_ref().map_or(0, |l| l.outputs.len());
    for k in 1..extra {
        if layout.as_ref().is_some_and(|l| l.outputs[k].active) {
            plugin
                .set_bus_active(MediaType::Audio, BusDirection::Output, k as i32, false)
                .with_context(|| format!("{name}: turning off output bus {k}"))?;
        }
    }
    plugin
        .start_processing()
        .with_context(|| format!("{name}: start_processing"))?;
    // vst3-host caches latency in setupProcessing; some plugins change it on activation and
    // say so through restartComponent, which only takes effect once serviced.
    let flags = plugin
        .service_host_requests()
        .with_context(|| format!("{name}: servicing restart requests"))?;
    if flags.reload_component() {
        bail!("{name}: plugin asked to be reloaded during activation");
    }
    let mut buffers = plugin
        .create_bus_audio_buffers(block)
        .with_context(|| format!("{name}: allocating bus buffers"))?;
    let mut silence = |plugin: &mut Plugin, samples: usize| -> Result<()> {
        let mut done = 0;
        while done < samples {
            buffers.clear();
            plugin
                .process_bus_audio(&mut buffers)
                .with_context(|| format!("{name}: pre-roll"))?;
            done += block;
        }
        Ok(())
    };
    // `load_state` stops processing around `setState` and starts it again; a plug-in may
    // change its latency or buses for the restored settings, which servicing picks up.
    let state_error = match state {
        Some(state) => {
            silence(plugin, block)?;
            let error = plugin.load_state(state).err().map(|e| e.to_string());
            let _ = plugin.service_host_requests();
            error
        }
        None => None,
    };
    silence(plugin, preroll)?;
    Ok(Activation {
        latency: plugin.latency_samples() as usize,
        has_audio_input,
        layout: plugin
            .audio_bus_layout()
            .with_context(|| format!("{name}: querying bus layout"))?,
        state_error,
    })
}

/// Hand queued parameter changes to the plugin's processor without processing audio: VST3's
/// parameter flush, a process call with zero samples. Parameter changes reach the processor
/// only through `process`, and a plugin saves its processor's values, so without this a
/// change made just before saving (or while no audio runs) would be lost. Call with the
/// plugin locked, which keeps the audio thread out.
pub fn flush_parameters(plugin: &mut Plugin) {
    if !plugin.is_processing() {
        return;
    }
    if let Ok(mut buffers) = plugin.create_bus_audio_buffers(1) {
        for bus in buffers.inputs.iter_mut().chain(&mut buffers.outputs) {
            bus.channels.iter_mut().for_each(Vec::clear);
        }
        let _ = plugin.process_bus_audio(&mut buffers);
    }
}

/// Whether a bundle's `moduleinfo.json` declares an instrument, without loading it: `None`
/// when it has no moduleinfo (most bundles; only loading them tells).
pub fn declares_instrument(path: &Path) -> Option<bool> {
    let info = vst3_host::discovery::read_module_info(path).ok()??;
    Some(info.classes.iter().any(|c| {
        c.category == "Audio Module Class" && c.sub_categories.iter().any(|s| s == "Instrument")
    }))
}

/// Whether a plugin describes itself as an instrument.
pub fn is_instrument(plugin: &Plugin) -> bool {
    let info = plugin.info();
    info.category.contains("Instrument") || (info.has_midi_input && info.audio_inputs == 0)
}

/// Flags shared between the session and every insert in the graph.
#[derive(Clone, Default)]
pub struct InsertFlags {
    /// Blocks where a plugin could not be processed.
    pub misses: Arc<AtomicU64>,
    /// Set during export: wait for the plugin lock instead of skipping the plugin, because a
    /// bounce must never contain a dry block.
    pub offline: Arc<AtomicBool>,
    /// The same count for this one insert ([`Self::counting`]), kept by the session across
    /// graph rebuilds: which track a held lock silenced.
    pub insert_misses: Arc<AtomicU64>,
}

impl InsertFlags {
    /// These flags, counting this insert's misses into `misses` as well as the total.
    pub fn counting(&self, misses: &Arc<AtomicU64>) -> Self {
        Self {
            insert_misses: misses.clone(),
            ..self.clone()
        }
    }
}

/// A plugin as an insert in the routing graph.
///
/// The plugin itself sits behind a mutex because its editor runs on the UI thread. Live, the
/// audio thread only ever `try_lock`s it: if the UI holds it (opening an editor, saving state)
/// that block of the track goes silent instead of waiting, and the miss is counted. During
/// export it waits instead.
pub struct PluginInsert {
    plugin: Arc<Mutex<Plugin>>,
    buffers: BusAudioBuffers,
    bypass: Arc<AtomicBool>,
    flags: InsertFlags,
    latency: usize,
    /// Keeps delay compensation intact while bypassed or skipped.
    passthrough: DelayLine,
    pending_midi: MidiQueue,
    /// Automation points for the next block: parameter id, normalized value, sample offset.
    /// Dropped when the block is skipped or bypassed: lanes resend their value every block.
    pending_params: Vec<(u32, f64, usize)>,
    /// The plugin refused a parameter change (it has no controller, say): stop asking, since
    /// every refusal builds an error message, which allocates.
    params_refused: bool,
    /// What this insert last told the plugin, so unchanged settings are not resent.
    sent: Option<(Meter, bool)>,
    /// The plug-in processed this block, so its extra outputs hold this block's audio.
    processed: bool,
    frames: usize,
}

/// Automation points one insert takes per block. A lane sends one per grid line
/// ([`crate::graph::AUTOMATION_GRID`]), so this covers many lanes at the largest block.
const MAX_PENDING_PARAMS: usize = 2048;

/// Tell the plugin where the transport is for the block it is about to process: playing or
/// not, tempo and meter when they change, and the position every block (a locate or loop has
/// to land at once; the plugin's own count would drift past it). Plain field writes in
/// vst3-host, so this is safe on the audio thread.
pub fn send_transport(
    plugin: &mut Plugin,
    transport: &Transport,
    sent: &mut Option<(Meter, bool)>,
) {
    let meter = transport.meter;
    if sent.is_none_or(|(m, _)| m != meter) {
        // vst3-host formats an error message for values it refuses, which would allocate on
        // the audio thread, so only valid ones are passed (a hand-edited project could hold
        // anything).
        let tempo = meter.tempo;
        if tempo.is_finite() && tempo > 0.0 {
            let _ = plugin.set_tempo(tempo);
        }
        if (1..=64).contains(&meter.numerator) && matches!(meter.denominator, 1 | 2 | 4 | 8 | 16) {
            let _ = plugin.set_time_signature(meter.numerator as i32, meter.denominator as i32);
        }
    }
    if sent.is_none_or(|(_, playing)| playing != transport.playing) {
        let _ = plugin.set_playing(transport.playing);
    }
    *sent = Some((meter, transport.playing));
    let _ = plugin.set_transport_position(TransportPosition {
        samples: transport.position,
        quarter_notes: transport.quarter_notes(),
    });
}

const MAX_PENDING_MIDI: usize = 512;

/// MIDI waiting for the plugin's next processed block, each event with its sample offset in
/// that block. It must never lose a note-off, or the note hangs: events survive blocks where
/// the plugin is skipped (and then play late, at the start of the next one), a bypassed plugin
/// keeps only its note-offs, and if the queue overflows the plugin gets all-notes-off.
pub struct MidiQueue {
    events: Vec<(MidiEvent, usize)>,
    overflowed: bool,
}

impl MidiQueue {
    pub fn new() -> Self {
        Self {
            events: Vec::with_capacity(MAX_PENDING_MIDI),
            overflowed: false,
        }
    }

    /// At the start of the next block.
    pub fn push(&mut self, event: MidiEvent) {
        self.push_at(event, 0);
    }

    /// `offset` samples into the next block. Push in time order.
    pub fn push_at(&mut self, event: MidiEvent, offset: usize) {
        if self.events.len() < MAX_PENDING_MIDI {
            self.events.push((event, offset));
        } else {
            self.overflowed = true;
        }
    }

    /// A block went by without the plugin: what is queued is late now, and plays at the start
    /// of the next block it does process. Its offsets were for the block that was missed.
    pub fn late(&mut self) {
        for (_, offset) in &mut self.events {
            *offset = 0;
        }
    }

    /// While bypassed, new notes are pointless but releases still matter.
    pub fn keep_note_offs(&mut self) {
        self.events.retain(|(e, _)| match e {
            MidiEvent::NoteOff { .. } => true,
            MidiEvent::NoteOn { velocity, .. } => *velocity == 0,
            _ => false,
        });
        self.late();
    }

    /// Hand everything to `send` with its offset, starting with all-notes-off on every
    /// channel if events were lost.
    pub fn flush(&mut self, mut send: impl FnMut(MidiEvent, usize)) {
        if std::mem::take(&mut self.overflowed) {
            for index in 0..16 {
                let channel = MidiChannel::from_index(index).expect("16 MIDI channels");
                for controller in [64, 123] {
                    // Sustain off, then all notes off.
                    send(
                        MidiEvent::ControlChange {
                            channel,
                            controller,
                            value: 0,
                        },
                        0,
                    );
                }
            }
        }
        for (event, offset) in self.events.drain(..) {
            send(event, offset);
        }
    }
}

impl Default for MidiQueue {
    fn default() -> Self {
        Self::new()
    }
}

impl PluginInsert {
    /// Build on the UI thread; allocates everything the audio thread will need. Does not touch
    /// the plugin, so rebuilding the graph never makes the audio thread miss a block.
    pub fn new(
        plugin: Arc<Mutex<Plugin>>,
        layout: &AudioBusLayout,
        sample_rate: f64,
        bypass: Arc<AtomicBool>,
        flags: InsertFlags,
        latency: usize,
        max_block: usize,
    ) -> Self {
        Self {
            plugin,
            buffers: BusAudioBuffers::new(layout, max_block, sample_rate),
            bypass,
            flags,
            latency,
            passthrough: DelayLine::new(latency, CHANNELS),
            pending_midi: MidiQueue::new(),
            pending_params: Vec::with_capacity(MAX_PENDING_PARAMS),
            params_refused: false,
            sent: None,
            processed: false,
            frames: 0,
        }
    }
}

/// A block the plugin could not process goes silent rather than dry: dry audio past a limiter
/// or straight from a DI would be louder than what the chain normally produces. Queued MIDI is
/// kept for the next block the plugin does process.
pub(crate) fn skip(flags: &InsertFlags, io: &mut [Vec<f32>], frames: usize) {
    flags.misses.fetch_add(1, Ordering::Relaxed);
    flags.insert_misses.fetch_add(1, Ordering::Relaxed);
    for channel in io.iter_mut() {
        channel[..frames].fill(0.0);
    }
}

/// Resize every bus channel to `frames` without reallocating (capacity is `max_block`).
fn set_frames(buffers: &mut BusAudioBuffers, frames: usize) {
    for bus in buffers.inputs.iter_mut().chain(&mut buffers.outputs) {
        for channel in &mut bus.channels {
            channel.resize(frames, 0.0);
        }
    }
}

impl Processor for PluginInsert {
    fn latency(&self) -> usize {
        self.latency
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize, transport: &Transport) {
        self.processed = false;
        self.frames = frames;
        if self.bypass.load(Ordering::Relaxed) {
            self.pending_midi.keep_note_offs();
            self.pending_params.clear();
            self.passthrough.process(io, frames);
            return;
        }
        let guard = if self.flags.offline.load(Ordering::Relaxed) {
            Some(
                self.plugin
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner()),
            )
        } else {
            self.plugin.try_lock().ok()
        };
        let Some(mut plugin) = guard else {
            skip(&self.flags, io, frames);
            self.pending_midi.late();
            self.pending_params.clear();
            return;
        };
        send_transport(&mut plugin, transport, &mut self.sent);
        // Values are in 0..1 already (the lane clamps them), so the only refusal left is a
        // plugin that can't take changes at all.
        for &(id, value, offset) in &self.pending_params {
            if self.params_refused {
                break;
            }
            if plugin.set_parameter_at(id, value, offset as i32).is_err() {
                self.params_refused = true;
            }
        }
        self.pending_params.clear();
        self.pending_midi.flush(|event, offset| {
            let _ = plugin.send_midi_event_at(event, offset as i32);
        });
        set_frames(&mut self.buffers, frames);
        if let Some(main_in) = self.buffers.inputs.first_mut() {
            if main_in.active {
                adapt_channels(io, &mut main_in.channels, frames);
            }
        }
        for bus in self.buffers.inputs.iter_mut().skip(1) {
            bus.channels.iter_mut().for_each(|c| c.fill(0.0));
        }
        if plugin.process_bus_audio(&mut self.buffers).is_err() {
            skip(&self.flags, io, frames);
            return;
        }
        if let Some(main_out) = self.buffers.outputs.first() {
            adapt_channels(&main_out.channels, io, frames);
        }
        self.processed = true;
    }

    fn output_bus(&self, bus: usize) -> Option<&[Vec<f32>]> {
        let out = self.buffers.outputs.get(bus).filter(|b| b.active)?;
        (bus > 0 && self.processed && out.channels.iter().all(|c| c.len() >= self.frames))
            .then_some(out.channels.as_slice())
    }

    fn send_midi(&mut self, event: MidiEvent) {
        self.pending_midi.push(event);
    }

    fn send_midi_at(&mut self, event: MidiEvent, offset: usize) {
        self.pending_midi.push_at(event, offset);
    }

    fn set_parameter_at(&mut self, id: u32, value: f64, offset: usize) {
        if self.pending_params.len() < MAX_PENDING_PARAMS {
            self.pending_params.push((id, value, offset));
        }
    }

    fn as_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }

    fn take_state_from(&mut self, old: &mut dyn Processor) -> bool {
        let Some(old) = old
            .as_any_mut()
            .and_then(|a| a.downcast_mut::<PluginInsert>())
        else {
            return false;
        };
        if !Arc::ptr_eq(&self.plugin, &old.plugin) {
            return false;
        }
        std::mem::swap(&mut self.pending_midi, &mut old.pending_midi);
        if self.passthrough.delay() == old.passthrough.delay() {
            std::mem::swap(&mut self.passthrough, &mut old.passthrough);
        }
        true
    }
}

/// Copy `frames` samples between buffers of possibly different channel counts.
/// Many-to-mono averages the channels; otherwise channels are mapped by index, wrapping
/// around (mono to stereo duplicates, stereo to quad repeats L, R).
pub fn adapt_channels(src: &[Vec<f32>], dst: &mut [Vec<f32>], frames: usize) {
    if src.is_empty() {
        dst.iter_mut().for_each(|d| d[..frames].fill(0.0));
        return;
    }
    if dst.len() == 1 && src.len() > 1 {
        let scale = 1.0 / src.len() as f32;
        for (i, d) in dst[0][..frames].iter_mut().enumerate() {
            *d = src.iter().map(|c| c[i]).sum::<f32>() * scale;
        }
        return;
    }
    for (c, d) in dst.iter_mut().enumerate() {
        d[..frames].copy_from_slice(&src[c % src.len()][..frames]);
    }
}

/// Parse `NAME=VALUE` with VALUE a normalized 0..1 parameter value.
pub fn parse_param(s: &str) -> Result<(String, f64), String> {
    let (name, value) = s
        .rsplit_once('=')
        .ok_or_else(|| format!("expected NAME=VALUE, got '{s}'"))?;
    let value: f64 = value
        .trim()
        .parse()
        .map_err(|e| format!("bad value in '{s}': {e}"))?;
    if !(0.0..=1.0).contains(&value) {
        return Err(format!("value in '{s}' must be normalized 0..1"));
    }
    Ok((name.trim().to_string(), value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed() -> Vec<PathBuf> {
        [
            r"C:\VST3\Acme Audio\Mixer 5 Compressor.vst3",
            r"C:\VST3\Acme Audio\Mixer 5 Equalizer.vst3",
            r"C:\VST3\Acme Audio\Mixer 5.vst3",
            r"C:\VST3\Example Amps\Amp Sim Lead.vst3",
        ]
        .iter()
        .map(PathBuf::from)
        .collect()
    }

    #[test]
    fn resolves_exact_then_unique_substring() {
        let all = installed();
        assert_eq!(resolve("mixer 5", &all).unwrap(), all[2]);
        assert_eq!(resolve("lead", &all).unwrap(), all[3]);
        assert!(resolve("mixer 5 e", &all).is_ok());
        assert!(resolve("mixer", &all).is_err(), "ambiguous");
        assert!(resolve("mastering", &all).is_err(), "missing");
    }

    fn note(on: bool, n: u8) -> MidiEvent {
        let channel = MidiChannel::Ch1;
        if on {
            MidiEvent::NoteOn {
                channel,
                note: n,
                velocity: 100,
            }
        } else {
            MidiEvent::NoteOff {
                channel,
                note: n,
                velocity: 0,
            }
        }
    }

    fn drained(queue: &mut MidiQueue) -> Vec<MidiEvent> {
        let mut out = Vec::new();
        queue.flush(|e, _| out.push(e));
        out
    }

    #[test]
    fn midi_queue_keeps_releases_and_recovers_from_overflow() {
        let mut q = MidiQueue::new();
        q.push(note(true, 60));
        q.push(note(false, 60));
        q.push(note(true, 62));
        // Bypassed: only the release survives.
        q.keep_note_offs();
        assert_eq!(drained(&mut q), vec![note(false, 60)]);

        // Overflow: everything that fits is kept, and all-notes-off goes out first.
        for k in 0..MAX_PENDING_MIDI + 10 {
            q.push(note(true, (k % 128) as u8));
        }
        let out = drained(&mut q);
        assert_eq!(out.len(), 32 + MAX_PENDING_MIDI);
        assert!(matches!(
            out[1],
            MidiEvent::ControlChange {
                channel: MidiChannel::Ch1,
                controller: 123,
                value: 0
            }
        ));
        assert!(
            drained(&mut q).is_empty(),
            "flag and queue reset after flushing"
        );
    }

    #[test]
    fn a_skip_counts_for_the_insert_and_the_total() {
        let flags = InsertFlags::default();
        let (a, b) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)));
        let (fa, fb) = (flags.counting(&a), flags.counting(&b));
        let mut io = vec![vec![1.0f32; 4]; 2];
        skip(&fa, &mut io, 3);
        skip(&fa, &mut io, 3);
        skip(&fb, &mut io, 3);
        assert_eq!(io[0], [0.0, 0.0, 0.0, 1.0], "silent for the block");
        assert_eq!(a.load(Ordering::Relaxed), 2);
        assert_eq!(b.load(Ordering::Relaxed), 1);
        assert_eq!(
            flags.misses.load(Ordering::Relaxed),
            3,
            "and all of them in the total"
        );
        assert!(Arc::ptr_eq(&fa.offline, &flags.offline), "the rest shared");
    }

    #[test]
    fn adapts_channel_counts() {
        let stereo = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let mut mono = vec![vec![0.0; 2]];
        adapt_channels(&stereo, &mut mono, 2);
        assert_eq!(mono, vec![vec![2.0, 3.0]]);

        let mut back = vec![vec![0.0; 2]; 2];
        adapt_channels(&mono, &mut back, 2);
        assert_eq!(back, vec![vec![2.0, 3.0], vec![2.0, 3.0]]);

        let mut same = vec![vec![9.0; 2]; 2];
        adapt_channels(&stereo, &mut same, 2);
        assert_eq!(same, stereo);

        adapt_channels(&[], &mut same, 2);
        assert_eq!(same, vec![vec![0.0; 2]; 2]);
    }

    #[test]
    fn parses_params() {
        assert_eq!(
            parse_param("Threshold=0.25").unwrap(),
            ("Threshold".into(), 0.25)
        );
        assert!(parse_param("Threshold").is_err());
        assert!(parse_param("Gain=2").is_err());
    }
}
