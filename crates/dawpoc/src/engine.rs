//! The processing core.
//!
//! [`Chain::process_block`] is the single processing path. Offline render (via
//! [`render_aligned`]) calls it today, and the live audio callback will call the very same
//! function, so preview and bounce can only differ through the [`ProcessMode`] the plugins
//! are told about, which is explicit and measurable.

use anyhow::{bail, Context, Result};
use daw_core::plugins::adapt_channels;
use vst3_host::{
    AudioBusLayout, BusAudioBuffers, BusDirection, MediaType, Plugin, ProcessMode, RestartFlags,
};

/// One plugin in a chain, with its preallocated bus buffers.
pub struct Stage {
    name: String,
    plugin: Plugin,
    buffers: BusAudioBuffers,
    layout: AudioBusLayout,
    latency: u32,
    tail: u32,
    restart_flags: RestartFlags,
}

impl Stage {
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn layout(&self) -> &AudioBusLayout {
        &self.layout
    }

    /// Reported latency in samples, read after activation.
    pub fn latency(&self) -> u32 {
        self.latency
    }

    /// Reported tail in samples (`u32::MAX` means infinite).
    pub fn tail(&self) -> u32 {
        self.tail
    }

    /// Restart requests the plugin raised while being activated.
    pub fn restart_flags(&self) -> RestartFlags {
        self.restart_flags
    }

    /// Start processing and capture everything that is only valid once the plugin is active.
    fn start(&mut self, block_size: usize) -> Result<()> {
        let name = &self.name;
        self.plugin
            .start_processing()
            .with_context(|| format!("{name}: start_processing"))?;
        // vst3-host caches latency in setupProcessing. A plugin that changes it on activation
        // says so through restartComponent, which only takes effect once serviced.
        let flags = self
            .plugin
            .service_host_requests()
            .with_context(|| format!("{name}: servicing restart requests"))?;
        if flags.reload_component() {
            bail!("{name}: plugin asked to be reloaded during activation");
        }
        self.restart_flags = flags;
        // Keep transport state constant so it never confounds a comparison.
        self.plugin
            .set_playing(true)
            .with_context(|| format!("{name}: set_playing"))?;
        self.layout = self
            .plugin
            .audio_bus_layout()
            .with_context(|| format!("{name}: querying bus layout"))?;
        match self.layout.outputs.first() {
            Some(bus) if bus.active && bus.channel_count > 0 => {}
            _ => bail!("{name}: main output bus is missing or inactive"),
        }
        self.buffers = self
            .plugin
            .create_bus_audio_buffers(block_size)
            .with_context(|| format!("{name}: allocating bus buffers"))?;
        self.latency = self.plugin.latency_samples();
        self.tail = self.plugin.tail_samples();
        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        self.plugin
            .stop_processing()
            .with_context(|| format!("{}: stop_processing", self.name))
    }
}

/// An ordered list of plugins processed in series at a fixed block size.
pub struct Chain {
    stages: Vec<Stage>,
    channels: usize,
    block_size: usize,
    mode: ProcessMode,
}

impl Chain {
    /// `channels` is the width of the signal flowing between stages (2 for stereo).
    pub fn new(channels: usize, block_size: usize, mode: ProcessMode) -> Self {
        Self {
            stages: Vec::new(),
            channels,
            block_size,
            mode,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels
    }

    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn mode(&self) -> ProcessMode {
        self.mode
    }

    pub fn stages(&self) -> &[Stage] {
        &self.stages
    }

    /// Append a freshly loaded (not yet processing) plugin: activate its main buses, apply the
    /// chain's process mode and start it.
    pub fn push(&mut self, name: impl Into<String>, mut plugin: Plugin) -> Result<()> {
        let name = name.into();
        let layout = plugin.audio_bus_layout().ok();
        let has_input = layout.as_ref().is_none_or(|l| !l.inputs.is_empty());
        if has_input {
            plugin
                .set_bus_active(MediaType::Audio, BusDirection::Input, 0, true)
                .with_context(|| format!("{name}: activating main input bus"))?;
        }
        plugin
            .set_bus_active(MediaType::Audio, BusDirection::Output, 0, true)
            .with_context(|| format!("{name}: activating main output bus"))?;
        plugin
            .set_process_mode(self.mode)
            .with_context(|| format!("{name}: set_process_mode({:?})", self.mode))?;

        let mut stage = Stage {
            name,
            plugin,
            buffers: BusAudioBuffers {
                inputs: Vec::new(),
                outputs: Vec::new(),
                sample_rate: 0.0,
                block_size: 0,
            },
            layout: AudioBusLayout {
                inputs: Vec::new(),
                outputs: Vec::new(),
            },
            latency: 0,
            tail: 0,
            restart_flags: RestartFlags::default(),
        };
        stage.start(self.block_size)?;
        self.stages.push(stage);
        Ok(())
    }

    /// Switch every plugin between realtime and offline processing. VST3 only allows this
    /// while a plugin is stopped, so each one is stopped, switched and restarted.
    pub fn set_process_mode(&mut self, mode: ProcessMode) -> Result<()> {
        for stage in &mut self.stages {
            stage.stop()?;
            stage
                .plugin
                .set_process_mode(mode)
                .with_context(|| format!("{}: set_process_mode({mode:?})", stage.name))?;
            stage.start(self.block_size)?;
        }
        self.mode = mode;
        Ok(())
    }

    /// Total reported latency of the chain in samples.
    pub fn latency(&self) -> usize {
        self.stages.iter().map(|s| s.latency as usize).sum()
    }

    /// Total reported tail in samples, `None` if any stage reports an infinite tail.
    pub fn tail(&self) -> Option<usize> {
        self.stages.iter().try_fold(0usize, |acc, s| {
            (s.tail != u32::MAX).then(|| acc + s.tail as usize)
        })
    }

    /// Process one block in place. `io` holds `channels()` buffers of exactly
    /// `block_size()` samples. Allocation-free on success.
    pub fn process_block(&mut self, io: &mut [Vec<f32>]) -> Result<()> {
        debug_assert_eq!(io.len(), self.channels);
        debug_assert!(io.iter().all(|c| c.len() == self.block_size));
        let frames = self.block_size;
        for stage in &mut self.stages {
            let buffers = &mut stage.buffers;
            if let Some(main_in) = buffers.inputs.first_mut() {
                adapt_channels(io, &mut main_in.channels, frames);
            }
            if let Err(e) = stage.plugin.process_bus_audio(buffers) {
                bail!("{}: process failed: {e}", stage.name);
            }
            adapt_channels(&buffers.outputs[0].channels, io, frames);
        }
        Ok(())
    }
}

/// Run `input` through a block processor and return output aligned sample-for-sample with
/// the input.
///
/// The input is fed in fixed `block`-sized pieces, zero-padded past its end. The first
/// `latency` output samples are dropped, and enough extra silent blocks are fed to flush them
/// back out, so `out[n]` corresponds to `input[n]`. The result is `out_len` samples long:
/// pass the input length for a sample-aligned render, or more to keep a tail.
pub fn render_aligned<F>(
    input: &[Vec<f32>],
    block: usize,
    latency: usize,
    out_len: usize,
    mut process: F,
) -> Result<Vec<Vec<f32>>>
where
    F: FnMut(&mut [Vec<f32>]) -> Result<()>,
{
    assert!(block > 0, "block size must be positive");
    let channels = input.len();
    let mut out = vec![vec![0.0f32; out_len]; channels];
    let mut io = vec![vec![0.0f32; block]; channels];
    let total = out_len + latency;
    let mut pos = 0;
    while pos < total {
        for (buf, src) in io.iter_mut().zip(input) {
            let start = pos.min(src.len());
            let available = (src.len() - start).min(block);
            buf[..available].copy_from_slice(&src[start..start + available]);
            buf[available..].fill(0.0);
        }
        process(&mut io)?;
        // Block sample i is absolute sample pos + i, which lands at output index pos + i - latency.
        let first = latency.saturating_sub(pos).min(block);
        for i in first..block {
            let t = pos + i - latency;
            if t >= out_len {
                break;
            }
            for (o, buf) in out.iter_mut().zip(&io) {
                o[t] = buf[i];
            }
        }
        pos += block;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fake plugin: a pure delay of `delay` samples, carried across blocks.
    fn delay_line(delay: usize, channels: usize) -> impl FnMut(&mut [Vec<f32>]) -> Result<()> {
        let mut history: Vec<std::collections::VecDeque<f32>> =
            (0..channels).map(|_| vec![0.0; delay].into()).collect();
        move |io| {
            for (buf, h) in io.iter_mut().zip(&mut history) {
                for s in buf.iter_mut() {
                    h.push_back(*s);
                    *s = h.pop_front().unwrap();
                }
            }
            Ok(())
        }
    }

    fn ramp(len: usize, channels: usize) -> Vec<Vec<f32>> {
        (0..channels)
            .map(|c| (0..len).map(|i| (i + 1) as f32 + c as f32 * 0.5).collect())
            .collect()
    }

    #[test]
    fn identity_with_no_latency() {
        let input = ramp(1000, 2);
        let out = render_aligned(&input, 64, 0, 1000, |_| Ok(())).unwrap();
        assert_eq!(out, input);
    }

    #[test]
    fn compensates_latency_for_awkward_sizes() {
        for (len, block, delay) in [(1000, 64, 37), (10, 64, 100), (513, 512, 512), (77, 1, 5)] {
            let input = ramp(len, 2);
            let out = render_aligned(&input, block, delay, len, delay_line(delay, 2)).unwrap();
            assert_eq!(out, input, "len {len} block {block} delay {delay}");
        }
    }

    #[test]
    fn uncompensated_latency_shows_as_shift() {
        let input = ramp(100, 1);
        let out = render_aligned(&input, 16, 0, 100, delay_line(3, 1)).unwrap();
        assert_eq!(&out[0][..3], &[0.0; 3]);
        assert_eq!(&out[0][3..], &input[0][..97]);
    }

    #[test]
    fn keeps_requested_tail() {
        let input = ramp(10, 1);
        let out = render_aligned(&input, 4, 2, 15, delay_line(2, 1)).unwrap();
        assert_eq!(&out[0][..10], &input[0][..]);
        assert_eq!(&out[0][10..], &[0.0; 5]);
    }
}
