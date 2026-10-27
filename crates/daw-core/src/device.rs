//! Audio devices through cpal: WASAPI always, ASIO (FlexASIO, ASIO4ALL, interface drivers)
//! with the `asio` feature.

use crate::engine::{EngineCore, InputAnchor};
use anyhow::{anyhow, bail, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, FromSample, Sample, SampleFormat, SizedSample, StreamConfig, SupportedBufferSize,
};
use rtrb::{Consumer, Producer, RingBuffer};
use serde::{Deserialize, Serialize};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    /// cpal host name, e.g. "WASAPI" or "ASIO". Empty means the default host.
    pub host: String,
    pub output_device: Option<String>,
    /// Ignored for ASIO, where input and output are the same driver.
    pub input_device: Option<String>,
    pub sample_rate: Option<u32>,
    pub buffer_size: Option<u32>,
    /// Worker threads that help the audio thread; `None` picks one per physical core minus
    /// one, and `Some(0)` processes everything on the audio thread.
    pub threads: Option<usize>,
    /// Extra samples to move recordings earlier (negative: later), on top of the round trip
    /// measured from the device. For what the driver does not report.
    pub record_offset: i32,
    /// Gather device buffers into blocks of at least this many samples before processing
    /// (`None`: process each buffer as it comes). Plug-ins that cost mostly per call (some
    /// heavy amp sims) then run far cheaper at tiny buffers, for `block − buffer` samples more
    /// latency (23.1). Applies without restarting the device.
    pub processing_block: Option<u32>,
}

impl AudioSettings {
    /// Whether switching to `other` needs the device reopened (not for the record offset or
    /// the processing block).
    pub fn needs_restart(&self, other: &AudioSettings) -> bool {
        AudioSettings {
            record_offset: 0,
            processing_block: None,
            ..self.clone()
        } != AudioSettings {
            record_offset: 0,
            processing_block: None,
            ..other.clone()
        }
    }
}

/// What a running device actually opened with.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub host: String,
    pub output_name: String,
    pub input_name: Option<String>,
    pub sample_rate: u32,
    pub buffer_size: Option<u32>,
    pub inputs: usize,
    pub outputs: usize,
    pub sample_format: String,
}

pub struct RunningDevice {
    output: cpal::Stream,
    input: Option<cpal::Stream>,
    pub info: DeviceInfo,
    pub errors: Arc<Mutex<Vec<String>>>,
}

impl RunningDevice {
    pub fn pause(&self) {
        let _ = self.output.pause();
        if let Some(input) = &self.input {
            let _ = input.pause();
        }
    }
}

/// A clock in place of a device, for checks that need the engine running beside the main
/// thread as it does live (a parameter read or an editor meeting the audio thread), without
/// opening a device: a thread of audio priority runs the engine every `block` samples at
/// the real rate, as a device's callback does (`try_lock`, a silent block counted in
/// `skipped_blocks` while the engine is held), and the audio goes nowhere. With an
/// [`InputSignal`] its inputs play that signal, so a check can record without a device (21.1).
pub struct VirtualDevice {
    running: Arc<std::sync::atomic::AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Input channels the signal fills (0 without one).
    pub inputs: usize,
}

/// What a [`VirtualDevice`]'s inputs play: called once a block, on its thread, to fill the
/// block's frames interleaved (`inputs` samples a frame). It runs where a device's input
/// callback would, so it must not allocate or block.
pub type InputSignal = Box<dyn FnMut(&mut [f32]) + Send>;

impl VirtualDevice {
    pub fn start(core: Arc<Mutex<EngineCore>>, block: usize, sample_rate: u32) -> Result<Self> {
        Self::start_with_input(core, block, sample_rate, 0, None)
    }

    /// [`Self::start`], with `inputs` input channels playing `signal`. The engine must already
    /// be configured with those inputs (`EngineCore::configure_io`).
    pub fn start_with_input(
        core: Arc<Mutex<EngineCore>>,
        block: usize,
        sample_rate: u32,
        inputs: usize,
        mut signal: Option<InputSignal>,
    ) -> Result<Self> {
        let channels = core
            .lock()
            .map_err(|_| anyhow!("engine lock poisoned"))?
            .output_channels();
        let inputs = if signal.is_some() { inputs.max(1) } else { 0 };
        let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let period = std::time::Duration::from_secs_f64(block as f64 / f64::from(sample_rate));
        let thread = {
            let running = running.clone();
            std::thread::Builder::new()
                .name("virtual device".into())
                .spawn(move || {
                    crate::cpu::make_realtime();
                    let shared = match core.lock() {
                        Ok(core) => core.shared().clone(),
                        Err(_) => return,
                    };
                    let mut out = vec![0.0f32; block * channels];
                    // As a device's input stream: a block of the signal into a ring each
                    // period, which the engine reads (and trims if it backs up).
                    let mut captured = vec![0.0f32; block * inputs];
                    let (mut tx, mut rx) = RingBuffer::<f32>::new((block * inputs * 8).max(1));
                    let mut next = std::time::Instant::now();
                    while running.load(Ordering::Relaxed) {
                        if let Some(signal) = signal.as_mut() {
                            signal(&mut captured);
                            // A full ring (the engine held up) drops the block, as a device's
                            // input does; the engine counts the gap as an underrun.
                            if let Ok(chunk) = tx.write_chunk_uninit(captured.len()) {
                                chunk.fill_from_iter(captured.iter().copied());
                            }
                        }
                        match core.try_lock() {
                            Ok(mut core) => {
                                core.set_callback_time(std::time::Instant::now());
                                let input = (inputs > 0).then_some(&mut rx);
                                core.process(&mut out, channels, input, inputs);
                            }
                            Err(_) => {
                                shared.skipped_blocks.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                        next += period;
                        let now = std::time::Instant::now();
                        if next > now {
                            std::thread::sleep(next - now);
                        } else {
                            // Late (the machine stalled): carry on from now, as a device does.
                            next = now;
                        }
                    }
                })?
        };
        Ok(Self {
            running,
            thread: Some(thread),
            inputs,
        })
    }
}

impl Drop for VirtualDevice {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// The host used when the settings name none (WASAPI on Windows).
pub fn default_host_name() -> String {
    cpal::default_host().id().name().to_string()
}

pub fn host_names() -> Vec<String> {
    cpal::available_hosts()
        .into_iter()
        .map(|id| id.name().to_string())
        .collect()
}

fn find_host(name: &str) -> Result<cpal::Host> {
    if name.is_empty() {
        return Ok(cpal::default_host());
    }
    let id = cpal::available_hosts()
        .into_iter()
        .find(|id| id.name().eq_ignore_ascii_case(name))
        .ok_or_else(|| anyhow!("audio host '{name}' is not available in this build"))?;
    cpal::host_from_id(id).with_context(|| format!("opening audio host {name}"))
}

pub fn is_asio(host: &str) -> bool {
    host.eq_ignore_ascii_case("ASIO")
}

#[derive(Debug, Clone, Default)]
pub struct DeviceList {
    pub outputs: Vec<String>,
    pub inputs: Vec<String>,
}

pub fn list_devices(host: &str) -> Result<DeviceList> {
    let host = find_host(host)?;
    let mut list = DeviceList::default();
    for device in host.devices()? {
        let name = device.to_string();
        if device.supports_output() {
            list.outputs.push(name.clone());
        }
        if device.supports_input() {
            list.inputs.push(name);
        }
    }
    Ok(list)
}

/// Sample rates and buffer sizes a device's output supports, for the settings dialog.
#[derive(Debug, Clone, Default)]
pub struct DeviceCaps {
    pub sample_rates: Vec<u32>,
    pub buffer_range: Option<(u32, u32)>,
    pub default_rate: u32,
}

pub fn device_caps(host: &str, device: Option<&str>) -> Result<DeviceCaps> {
    let host = find_host(host)?;
    let device = find_output(&host, device)?;
    let default = device.default_output_config()?;
    let mut rates = Vec::new();
    let mut buffer_range = None;
    for range in device.supported_output_configs()? {
        for rate in [44_100, 48_000, 88_200, 96_000, 176_400, 192_000] {
            if (range.min_sample_rate()..=range.max_sample_rate()).contains(&rate)
                && !rates.contains(&rate)
            {
                rates.push(rate);
            }
        }
        if let SupportedBufferSize::Range { min, max } = range.buffer_size() {
            buffer_range = Some((*min, *max));
        }
    }
    rates.sort_unstable();
    Ok(DeviceCaps {
        sample_rates: rates,
        buffer_range,
        default_rate: default.sample_rate(),
    })
}

fn find_output(host: &cpal::Host, name: Option<&str>) -> Result<cpal::Device> {
    match name {
        Some(name) => host
            .output_devices()?
            .find(|d| d.to_string() == name)
            .ok_or_else(|| anyhow!("output device '{name}' not found")),
        None => host
            .default_output_device()
            .ok_or_else(|| anyhow!("no default output device")),
    }
}

fn find_input(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    match name {
        Some(name) => host.input_devices().ok()?.find(|d| d.to_string() == name),
        None => host.default_input_device(),
    }
}

/// Open the device and start running `core` from its callback.
///
/// The output callback drives the engine; input arrives through a ring. With ASIO both streams
/// are serviced by the same driver callback, so the ring adds no latency.
pub fn start(settings: &AudioSettings, core: Arc<Mutex<EngineCore>>) -> Result<RunningDevice> {
    let host = find_host(&settings.host)?;
    let host_name = host.id().name().to_string();
    let asio = is_asio(&host_name);
    let output = find_output(&host, settings.output_device.as_deref())?;
    let output_name = output.to_string();
    let supported = output
        .default_output_config()
        .with_context(|| format!("{output_name}: no output configuration"))?;
    let sample_rate = settings.sample_rate.unwrap_or(supported.sample_rate());
    let sample_format = supported.sample_format();
    let config = StreamConfig {
        channels: supported.channels(),
        sample_rate,
        buffer_size: settings
            .buffer_size
            .map_or(BufferSize::Default, BufferSize::Fixed),
    };

    let input_device = if asio {
        Some(output.clone())
    } else {
        find_input(&host, settings.input_device.as_deref())
    };
    let input_config = input_device.as_ref().and_then(|d| {
        let supported = d.default_input_config().ok()?;
        Some((
            StreamConfig {
                channels: supported.channels(),
                sample_rate,
                buffer_size: config.buffer_size,
            },
            supported.sample_format(),
        ))
    });

    let in_channels = input_config
        .as_ref()
        .map_or(0, |(c, _)| c.channels as usize);
    let out_channels = config.channels as usize;
    let shared = {
        let mut core = core.lock().map_err(|_| anyhow!("engine lock poisoned"))?;
        core.configure_io(in_channels, out_channels);
        core.shared().set_sample_rate(sample_rate);
        core.shared().clone()
    };

    let errors: Arc<Mutex<Vec<String>>> = Arc::default();
    // A second of input headroom; the engine trims it to a few blocks.
    let (input_tx, input_rx) =
        RingBuffer::<f32>::new((sample_rate as usize * in_channels.max(1)).max(4096));
    let (anchor_tx, anchor_rx) = RingBuffer::<InputAnchor>::new(64);

    let mut input_stream = None;
    let mut input_name = None;
    if let (Some(device), Some((in_config, in_format))) = (&input_device, input_config) {
        match build_input(
            device,
            &in_config,
            in_format,
            input_tx,
            anchor_tx,
            errors.clone(),
        ) {
            Ok(stream) => {
                input_name = Some(device.to_string());
                input_stream = Some(stream);
            }
            Err(e) => errors
                .lock()
                .unwrap()
                .push(format!("input disabled: {e:#}")),
        }
    }
    let input_rx = input_stream.as_ref().map(|_| (input_rx, anchor_rx));

    let output_stream = build_output(
        &output,
        &config,
        sample_format,
        core,
        input_rx,
        in_channels,
        errors.clone(),
    )
    .with_context(|| format!("opening {output_name} at {sample_rate} Hz"))?;

    if let Some(stream) = &input_stream {
        stream.play().context("starting input stream")?;
    }
    output_stream.play().context("starting output stream")?;
    shared.skipped_blocks.store(0, Ordering::Relaxed);

    Ok(RunningDevice {
        output: output_stream,
        input: input_stream,
        info: DeviceInfo {
            host: host_name,
            output_name,
            input_name,
            sample_rate,
            buffer_size: settings.buffer_size,
            inputs: in_channels,
            outputs: out_channels,
            sample_format: format!("{sample_format:?}"),
        },
        errors,
    })
}

fn push_error(errors: &Arc<Mutex<Vec<String>>>, message: String) {
    if let Ok(mut e) = errors.lock() {
        if e.len() < 32 {
            e.push(message);
        }
    }
}

fn build_input(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    tx: Producer<f32>,
    anchors: Producer<InputAnchor>,
    errors: Arc<Mutex<Vec<String>>>,
) -> Result<cpal::Stream> {
    fn typed<T: SizedSample>(
        device: &cpal::Device,
        config: &StreamConfig,
        mut tx: Producer<f32>,
        mut anchors: Producer<InputAnchor>,
        errors: Arc<Mutex<Vec<String>>>,
    ) -> Result<cpal::Stream>
    where
        f32: FromSample<T>,
    {
        let channels = usize::from(config.channels).max(1);
        let mut written = 0u64;
        Ok(device.build_input_stream::<T, _, _>(
            *config,
            move |data: &[T], info: &cpal::InputCallbackInfo| {
                // When this buffer's first frame was captured, and where it lands in the ring:
                // the engine measures the round trip from it (see `EngineCore::measure`).
                let capture = info.timestamp().capture.as_nanos() as u64;
                let _ = anchors.push(InputAnchor {
                    frame: written,
                    capture_ns: capture,
                });
                // Drop what does not fit; the engine reports the gap as an underrun.
                let fits = data.len().min(tx.slots()) / channels * channels;
                if let Ok(chunk) = tx.write_chunk_uninit(fits) {
                    chunk.fill_from_iter(data.iter().map(|s| f32::from_sample(*s)));
                    written += (fits / channels) as u64;
                }
            },
            move |e| push_error(&errors, format!("input stream: {e}")),
            None,
        )?)
    }
    match format {
        SampleFormat::F32 => typed::<f32>(device, config, tx, anchors, errors),
        SampleFormat::I16 => typed::<i16>(device, config, tx, anchors, errors),
        SampleFormat::I32 => typed::<i32>(device, config, tx, anchors, errors),
        SampleFormat::U16 => typed::<u16>(device, config, tx, anchors, errors),
        SampleFormat::I24 => typed::<cpal::I24>(device, config, tx, anchors, errors),
        SampleFormat::F64 => typed::<f64>(device, config, tx, anchors, errors),
        other => bail!("unsupported input sample format {other:?}"),
    }
}

fn build_output(
    device: &cpal::Device,
    config: &StreamConfig,
    format: SampleFormat,
    core: Arc<Mutex<EngineCore>>,
    input: Option<(Consumer<f32>, Consumer<InputAnchor>)>,
    in_channels: usize,
    errors: Arc<Mutex<Vec<String>>>,
) -> Result<cpal::Stream> {
    fn typed<T: SizedSample + FromSample<f32>>(
        device: &cpal::Device,
        config: &StreamConfig,
        core: Arc<Mutex<EngineCore>>,
        input: Option<(Consumer<f32>, Consumer<InputAnchor>)>,
        in_channels: usize,
        errors: Arc<Mutex<Vec<String>>>,
    ) -> Result<cpal::Stream> {
        let channels = config.channels as usize;
        let shared = core
            .lock()
            .map_err(|_| anyhow!("engine lock poisoned"))?
            .shared()
            .clone();
        let (mut input, mut anchors) = match input {
            Some((samples, anchors)) => (Some(samples), Some(anchors)),
            None => (None, None),
        };
        let mut anchor = None;
        Ok(device.build_output_stream::<T, _, _>(
            *config,
            move |out: &mut [T], info: &cpal::OutputCallbackInfo| match core.try_lock() {
                Ok(mut core) => {
                    // The newest capture anchor: any one serves, the engine extrapolates.
                    while let Some(Ok(next)) = anchors.as_mut().map(Consumer::pop) {
                        anchor = Some(next);
                    }
                    let timestamp = info.timestamp();
                    let callback = timestamp.callback.as_nanos() as u64;
                    let playback = timestamp.playback.as_nanos() as u64;
                    core.set_clock(callback, playback, anchor);
                    // The clock live MIDI is stamped with on arrival (`midi::TimedMidi`).
                    core.set_callback_time(std::time::Instant::now());
                    core.process(out, channels, input.as_mut(), in_channels);
                }
                Err(_) => {
                    // Export holds the engine: output silence rather than wait.
                    out.fill(T::EQUILIBRIUM);
                    shared.skipped_blocks.fetch_add(1, Ordering::Relaxed);
                }
            },
            move |e| push_error(&errors, format!("output stream: {e}")),
            None,
        )?)
    }
    match format {
        SampleFormat::F32 => typed::<f32>(device, config, core, input, in_channels, errors),
        SampleFormat::I16 => typed::<i16>(device, config, core, input, in_channels, errors),
        SampleFormat::I32 => typed::<i32>(device, config, core, input, in_channels, errors),
        SampleFormat::U16 => typed::<u16>(device, config, core, input, in_channels, errors),
        SampleFormat::I24 => typed::<cpal::I24>(device, config, core, input, in_channels, errors),
        SampleFormat::F64 => typed::<f64>(device, config, core, input, in_channels, errors),
        other => bail!("unsupported output sample format {other:?}"),
    }
}
