//! Audio devices, MIDI inputs, engine statistics and CPU use.

use super::{error, NoArgs};
use crate::api::{ActionOutput, ApiError, ApiResult, Host, Registry, Safety};
use crate::device::{self, AudioSettings};
use crate::session::Session;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "get_audio_settings",
        "The audio configuration: chosen driver type (host), devices, sample rate, buffer size, \
         processing block and audio threads, plus what the running device actually opened with \
         (or why it failed). `processing_block` (samples, null: off) gathers device buffers \
         into larger blocks before processing; `stage_latency` is what that adds between input \
         and output now (samples). Recording latency: `round_trip` is the output-to-input round \
         trip measured from the device's timestamps (samples; null until known, or without an \
         input), `record_offset` the manual correction on top, and `record_shift` the total \
         that recordings are moved earlier by (including delay compensation and the stage \
         latency).",
        Safety::Read,
        |host, _: NoArgs| Ok(audio_view(host.session())),
    );
    r.add(
        "list_audio_devices",
        "Driver types (hosts: WASAPI, ASIO) and, for `host` (default: the current one), its \
         output and input devices plus the sample rates and buffer sizes the chosen output \
         device supports. ASIO drivers only appear when they can start.",
        Safety::Read,
        list_devices,
    );
    r.add_deferred(
        "set_audio_settings",
        "Change the audio configuration and restart the device; only the fields given change. \
         `host` is a driver type from list_audio_devices; `output_device`/`input_device` are \
         device names (\"\" = system default; ASIO uses the output driver for both); \
         `sample_rate` in Hz and `buffer_size` in samples (0 = device default); `threads` is \
         the number of worker threads or \"auto\"; `record_offset` in samples moves \
         recordings earlier (negative: later) on top of the measured round trip, for what the \
         driver does not report; `processing_block` in samples (0 = off, up to 2048) gathers \
         the device's buffers into blocks of at least that many before processing (rounded up \
         to whole buffers), so plug-ins that cost mostly per call (some amp-sim plug-ins at \
         16-sample buffers) run far cheaper, for (block - buffer) samples more latency; it \
         helps drivers that call back in bursts (FlexASIO on shared WASAPI), and exports use \
         the same block. Audio stops briefly (not for `record_offset` or `processing_block` \
         alone). A sample \
         rate change stops playback and reloads every plug-in and file at the new rate, a \
         plug-in a frame behind the app's busy bar (get_busy's `kind` is `sample_rate`); the \
         call answers once they are all in.",
        Safety::Destructive,
        |host, input: SetSettings, reply| match set_settings(host, input) {
            // A new rate reloads the plug-ins staged, as an open loads them: answered once in.
            Ok(()) => host.when_opened(
                reply,
                Box::new(|s| {
                    serde_json::to_value(audio_view(s))
                        .map(ActionOutput::Json)
                        .map_err(|e| ApiError(format!("encoding the result: {e}")))
                }),
            ),
            Err(e) => reply(Err(e)),
        },
    );
    r.returns::<AudioView>();
    r.add(
        "get_midi_inputs",
        "Open MIDI input ports (keyboards, controllers) and ports that failed to open (on \
         Windows a port can be open in one application at a time).",
        Safety::Read,
        |host, _: NoArgs| Ok(midi_view(host.session())),
    );
    r.add(
        "reconnect_midi",
        "Close and reopen all MIDI input ports, e.g. after plugging in a keyboard.",
        Safety::Edit,
        |host, _: NoArgs| {
            host.session().restart_midi();
            Ok(midi_view(host.session()))
        },
    );
    r.add(
        "get_stats",
        "Engine health: DSP load (time spent processing over the time the audio driver allows: \
         a buffer period, or a whole burst for a driver that calls back in bursts; over 1 is a \
         dropout), delay-compensation latency in samples, the block size the engine processes \
         and exports in (`block_size`), the device's buffer (`device_block`; smaller when \
         buffers are gathered into blocks, set_audio_settings' `processing_block`), the \
         driver's cadence in samples (the audio it asks for between waits: the device buffer \
         for an even driver, more for a bursty one such as FlexASIO on shared WASAPI), worker \
         threads, and counters of plug-in skips, engine skips and input gaps.",
        Safety::Read,
        |host, _: NoArgs| {
            let s = host.session().stats();
            Ok(Stats {
                dsp_load: s.dsp_load,
                latency: s.latency,
                block_size: s.block_size,
                device_block: s.device_block,
                cadence: s.cadence,
                workers: s.workers,
                plugin_skips: s.plugin_misses,
                engine_skips: s.skipped_blocks,
                input_gaps: s.input_underruns,
            })
        },
    );
    r.add(
        "get_cpu_usage",
        "CPU use of each track (and the master, `track` null) and of each insert on it, \
         averaged over the last half second of processing (`window`, seconds): \
         `per_buffer_us` (microseconds per audio buffer), `share` (that time over the \
         buffer's duration, its deadline: 1 is all of it) and `buffers` measured. A track's \
         figure is all its work (clips, inserts, fader, sends); its inserts are listed most \
         costly first, and the tracks too. Tracks run in parallel on the audio threads, so \
         shares can add up past 1 without a dropout; get_stats has the overall DSP load. \
         Every track and insert keeps running while stopped, so effects' tails and \
         monitoring work: the numbers are not zero then. A bypassed insert (`bypass`) isn't \
         called, so it costs (close to) nothing here. `process` is the whole app's CPU time \
         over the window in cores (1 = one core all the time): every thread, including what \
         plug-ins do on threads of their own, which the per-track figures can't see. Each \
         insert's `misses` counts the blocks it has missed since it loaded (its track silent \
         for that block) because its plug-in was held elsewhere when the audio thread came \
         for it. The same numbers as the app's CPU panel.",
        Safety::Read,
        |host, _: NoArgs| Ok(host.session().cpu_usage().clone()),
    );
}

#[derive(Serialize, JsonSchema)]
struct Stats {
    dsp_load: f32,
    latency: usize,
    block_size: usize,
    device_block: usize,
    cadence: usize,
    workers: usize,
    plugin_skips: u64,
    engine_skips: u64,
    input_gaps: u64,
}

#[derive(Serialize, JsonSchema)]
struct Running {
    host: String,
    output: String,
    input: Option<String>,
    sample_rate: u32,
    inputs: usize,
    outputs: usize,
    sample_format: String,
}

#[derive(Serialize, JsonSchema)]
struct AudioView {
    host: String,
    output_device: Option<String>,
    input_device: Option<String>,
    sample_rate: Option<u32>,
    buffer_size: Option<u32>,
    threads: Option<usize>,
    processing_block: Option<u32>,
    stage_latency: u64,
    record_offset: i32,
    round_trip: Option<u64>,
    record_shift: u64,
    running: Option<Running>,
    error: Option<String>,
}

fn audio_view(s: &Session) -> AudioView {
    let settings = &s.settings;
    AudioView {
        host: if settings.host.is_empty() {
            device::default_host_name()
        } else {
            settings.host.clone()
        },
        output_device: settings.output_device.clone(),
        input_device: settings.input_device.clone(),
        sample_rate: settings.sample_rate,
        buffer_size: settings.buffer_size,
        threads: settings.threads,
        processing_block: settings.processing_block,
        stage_latency: s.stage_latency(),
        record_offset: settings.record_offset,
        round_trip: s.round_trip(),
        record_shift: s.record_shift(),
        running: s.device.as_ref().map(|d| Running {
            host: d.info.host.clone(),
            output: d.info.output_name.clone(),
            input: d.info.input_name.clone(),
            sample_rate: d.info.sample_rate,
            inputs: d.info.inputs,
            outputs: d.info.outputs,
            sample_format: d.info.sample_format.clone(),
        }),
        error: s.device_error.clone(),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListDevices {
    /// Driver type to list; the current one when omitted.
    #[serde(default)]
    host: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct DevicesView {
    hosts: Vec<String>,
    host: String,
    outputs: Vec<String>,
    inputs: Vec<String>,
    sample_rates: Vec<u32>,
    /// Smallest and largest buffer size, when the device reports them.
    buffer_range: Option<(u32, u32)>,
    error: Option<String>,
}

fn list_devices(host: &mut dyn Host, input: ListDevices) -> ApiResult<DevicesView> {
    let s = host.session();
    let current = audio_view(s).host;
    let name = input.host.unwrap_or(current.clone());
    let hosts = device::host_names();
    if !hosts.iter().any(|h| h.eq_ignore_ascii_case(&name)) {
        return Err(error(format!(
            "unknown host '{name}'; available: {}",
            hosts.join(", ")
        )));
    }
    let mut view = DevicesView {
        hosts,
        host: name.clone(),
        outputs: Vec::new(),
        inputs: Vec::new(),
        sample_rates: Vec::new(),
        buffer_range: None,
        error: None,
    };
    match device::list_devices(&name) {
        Ok(list) => {
            view.outputs = list.outputs;
            view.inputs = list.inputs;
        }
        Err(e) => view.error = Some(format!("{e:#}")),
    }
    let output = if name.eq_ignore_ascii_case(&current) {
        s.settings.output_device.clone()
    } else {
        None
    };
    match device::device_caps(&name, output.as_deref()) {
        Ok(caps) => {
            view.sample_rates = caps.sample_rates;
            view.buffer_range = caps.buffer_range;
        }
        Err(e) => {
            view.error.get_or_insert(format!("{e:#}"));
        }
    }
    Ok(view)
}

#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
enum ThreadsArg {
    Count(usize),
    /// "auto": one per physical core minus one.
    Auto(String),
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetSettings {
    #[serde(default)]
    host: Option<String>,
    /// Device name; "" for the system default.
    #[serde(default)]
    output_device: Option<String>,
    /// Device name; "" for the system default. Ignored for ASIO.
    #[serde(default)]
    input_device: Option<String>,
    /// Hz; 0 for the device default.
    #[serde(default)]
    sample_rate: Option<u32>,
    /// Samples; 0 for the driver default.
    #[serde(default)]
    buffer_size: Option<u32>,
    /// Worker threads, or "auto".
    #[serde(default)]
    threads: Option<ThreadsArg>,
    /// Samples to move recordings earlier (negative: later), within ±1 second.
    #[serde(default)]
    record_offset: Option<i32>,
    /// Gather device buffers into blocks of at least this many samples; 0 for off.
    #[serde(default)]
    processing_block: Option<u32>,
}

fn set_settings(host: &mut dyn Host, input: SetSettings) -> ApiResult<()> {
    let s = host.session();
    let mut settings: AudioSettings = s.settings.clone();
    if let Some(name) = input.host {
        let hosts = device::host_names();
        let Some(found) = hosts.iter().find(|h| h.eq_ignore_ascii_case(&name)) else {
            return Err(error(format!(
                "unknown host '{name}'; available: {}",
                hosts.join(", ")
            )));
        };
        if !found.eq_ignore_ascii_case(&settings.host) {
            // Device names belong to a host; start from that host's defaults.
            settings.output_device = None;
            settings.input_device = None;
        }
        settings.host = found.clone();
    }
    let device_name = |name: String| (!name.is_empty()).then_some(name);
    if let Some(name) = input.output_device {
        settings.output_device = device_name(name);
    }
    if let Some(name) = input.input_device {
        settings.input_device = device_name(name);
    }
    if let Some(rate) = input.sample_rate {
        settings.sample_rate = (rate != 0).then_some(rate);
    }
    if let Some(size) = input.buffer_size {
        settings.buffer_size = (size != 0).then_some(size);
    }
    match input.threads {
        Some(ThreadsArg::Count(n)) => settings.threads = Some(n),
        Some(ThreadsArg::Auto(text)) if text.eq_ignore_ascii_case("auto") => {
            settings.threads = None
        }
        Some(ThreadsArg::Auto(text)) => {
            return Err(error(format!(
                "threads must be a number or \"auto\", not '{text}'"
            )));
        }
        None => {}
    }
    if let Some(offset) = input.record_offset {
        let second = s.sample_rate() as i32;
        if !(-second..=second).contains(&offset) {
            return Err(error(format!(
                "record_offset {offset} is more than a second ({second} samples) either way"
            )));
        }
        settings.record_offset = offset;
    }
    if let Some(block) = input.processing_block {
        let most = crate::session::MAX_BLOCK as u32;
        if block > most {
            return Err(error(format!(
                "processing_block {block} is over the engine's largest block, {most} samples"
            )));
        }
        settings.processing_block = (block != 0).then_some(block);
    }
    // An export holds the engine for its whole render: restarting the device (or reloading
    // every plug-in) under it would wait on the engine until it ends.
    let bouncing = matches!(
        s.export_status(),
        crate::session::ExportStatus::Running { kind, .. }
            if kind != crate::session::ExportKind::File
    );
    if bouncing && s.settings.needs_restart(&settings) {
        return Err(error(
            "an export or a track's render is running; change the audio settings once it is \
             done (or cancel_export it)",
        ));
    }
    s.apply_audio_settings(settings);
    Ok(())
}

#[derive(Serialize, JsonSchema)]
struct MidiView {
    ports: Vec<String>,
    errors: Vec<String>,
}

fn midi_view(s: &Session) -> MidiView {
    MidiView {
        ports: s.midi.ports.clone(),
        errors: s.midi.errors.clone(),
    }
}
