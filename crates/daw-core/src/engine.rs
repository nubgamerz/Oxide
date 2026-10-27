//! The engine core: transport, graph swapping, MIDI intake and recording taps.
//!
//! [`EngineCore::process`] is the one function that turns a block of hardware input into a
//! block of hardware output. The live device callback calls it, and so does mixdown export
//! ([`crate::export`]), with the same block size, so a bounce goes through exactly the code
//! that produced what you heard.
//!
//! The UI talks to the core through a lock-free command ring. The core lives behind a mutex
//! only so export can borrow it while the device is paused; the audio callback just
//! `try_lock`s it and outputs silence for the rare block where export holds it.

use crate::deadline::DeadlineMeter;
use crate::graph::{BlockContext, Graph, LiveMidi, Transport};
use crate::loudness::{Loudness, LoudnessMeter};
use crate::metronome::{ClickSettings, Metronome};
use crate::midi::TimedMidi;
use crate::pool::WorkerPool;
use crate::project::TrackId;
use crate::tempo::TempoMap;
use crate::trace;
use crate::util::AtomicF32;
use rtrb::{Consumer, Producer, RingBuffer};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use vst3_host::MidiEvent;

pub const MAX_IO_CHANNELS: usize = 64;

pub enum Command {
    SetGraph(Box<Graph>),
    Play,
    Stop,
    Locate(u64),
    /// Start this many samples before the located position (send after `Locate`, which
    /// clears it): a count-in that begins before the timeline does.
    PreRoll(u64),
    /// Where beats and bars fall: for the transport plugins see, the metronome and block splits.
    SetTempoMap(Box<TempoMap>),
    /// Wrap playback from the end of this range (samples) back to its start; `None` plays on.
    SetLoop(Option<(u64, u64)>),
    SetMetronome(ClickSettings),
    SetMidiPorts(Vec<Consumer<TimedMidi>>),
    /// Worker threads for parallel processing; `None` processes on the audio thread alone.
    SetPool(Option<Box<WorkerPool>>),
    StartRecording(Box<RecordJob>),
    StopRecording,
    /// Clear the loudness meter's integrated reading, LRA, true peak and maxima.
    ResetLoudness,
    /// Play a MIDI event on a track's instrument now (previewing a note from the editor).
    SendMidi {
        track: TrackId,
        event: MidiEvent,
    },
    /// Play an audio file on the master's outputs, outside the project (the Waveform view's
    /// playback), or stop it with `None`. Live only: [`EngineCore::render`] never plays it.
    Preview(Option<Box<Preview>>),
}

/// The Waveform view's playback: a file, at the engine's rate, from `position` to `end`.
pub struct Preview {
    pub audio: Arc<crate::media::AudioData>,
    pub position: u64,
    pub end: u64,
    /// The first of the output pair it plays on (the master's).
    pub channel: usize,
    /// The Waveform view's effects rack, settled, which the file plays through; `None` plays
    /// it as it is. Apply renders the same chain offline ([`crate::rack`]).
    pub rack: Option<Box<crate::rack::RackChain>>,
}

/// Things the audio thread must not free; sent back to the UI thread to be dropped there.
pub enum Garbage {
    Graph(Box<Graph>),
    MidiPorts(Vec<Consumer<TimedMidi>>),
    /// Dropping a pool joins its threads, which must not happen on the audio thread.
    Pool(Box<WorkerPool>),
    Recording(Box<RecordJob>),
    TempoMap(Box<TempoMap>),
    /// A preview that finished or was replaced: dropping it may free its audio.
    Preview(Box<Preview>),
}

/// One armed track's tap on the hardware input.
pub struct RecordTake {
    pub track: TrackId,
    pub channel: usize,
    pub stereo: bool,
    /// Interleaved samples (1 or 2 channels) to the disk writer.
    pub producer: Producer<f32>,
    /// The take's waveform as it records, for the arrangement to draw (21.1); `None` draws
    /// nothing until the take is a clip.
    pub peaks: Option<crate::record::PeakTap>,
}

pub struct RecordJob {
    pub takes: Vec<RecordTake>,
    /// The tap engages once the playhead reaches this (0 = at once), to the sample, so a
    /// count-in plays before it without being recorded.
    pub from: u64,
    /// The tap lets go here (punch-out), to the sample; playback carries on.
    pub to: u64,
    /// Set once the tap has opened. From then on `from` no longer applies, so a loop wrap
    /// (which takes the playhead back before `from`) does not close it: with delay
    /// compensation, the end of each pass is still arriving after the engine has wrapped.
    pub open: bool,
    /// Open only at `from` itself, never part-way past it: loop recording started while the
    /// playhead is already past the loop start waits for the next pass.
    pub exact_from: bool,
    /// While counting in, the click sounds until the heard position reaches this, even with
    /// the metronome off.
    pub count_in_to: Option<i64>,
    /// Timeline position of the first recorded sample, set by the engine.
    pub start: Option<u64>,
    /// Samples that could not be queued because the disk writer fell behind.
    pub dropped: u64,
    /// Where incoming MIDI goes while recording, stamped with what was being heard.
    pub midi: Option<Producer<RecordedMidi>>,
    /// MIDI events that didn't fit in `midi`.
    pub midi_dropped: u64,
    /// Loop wraps since recording started. Stamped on each MIDI event, so the session can
    /// tell the start of the first pass (before the loop start, heard before it) from just
    /// after a wrap (still hearing the loop's end).
    pub wraps: u32,
}

/// A MIDI event that arrived while recording.
#[derive(Debug, Clone, Copy)]
pub struct RecordedMidi {
    /// The heard position (the playhead minus delay compensation, through loop wraps;
    /// [`Metronome::heard`]) when the event was taken in, at the start of its block. The
    /// device's own output latency is not in it.
    pub heard: i64,
    pub wraps: u32,
    /// Index of the MIDI input port.
    pub port: usize,
    pub event: MidiEvent,
}

/// When an input buffer's first frame was captured, and its index among all frames written to
/// the input ring. The input callback sends one per buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputAnchor {
    pub frame: u64,
    pub capture_ns: u64,
}

/// Round-trip measurements averaged before the result is published.
const ROUND_TRIP_WINDOW: usize = 64;
/// Measurements needed before the average counts.
const ROUND_TRIP_SETTLED: usize = 16;

/// The round trip from output to input, measured from the device's timestamps: how many
/// samples after an output sample is played the input sample captured at that moment reaches
/// the record tap. It is the driver's output latency, its input latency, and any time the input
/// waited in the ring between the two callbacks.
struct RoundTrip {
    playback_ns: Option<u64>,
    anchor: Option<InputAnchor>,
    window: [f64; ROUND_TRIP_WINDOW],
    count: usize,
}

impl RoundTrip {
    fn new() -> Self {
        Self {
            playback_ns: None,
            anchor: None,
            window: [0.0; ROUND_TRIP_WINDOW],
            count: 0,
        }
    }

    /// Add one measurement: the next input frame the engine reads is ring frame `read`, and it
    /// is paired with the output frame that plays at `playback_ns`. Returns the average once
    /// settled.
    fn measure(&mut self, read: u64, rate: f64) -> Option<f64> {
        let (playback, anchor) = (self.playback_ns?, self.anchor?);
        // Captured when the anchor frame was, plus the frames after it.
        let since_capture = playback as f64 - anchor.capture_ns as f64;
        let frames = since_capture * rate / 1e9 - (read as f64 - anchor.frame as f64);
        self.window[self.count % ROUND_TRIP_WINDOW] = frames;
        self.count += 1;
        let n = self.count.min(ROUND_TRIP_WINDOW);
        (self.count >= ROUND_TRIP_SETTLED).then(|| self.window[..n].iter().sum::<f64>() / n as f64)
    }
}

/// A running average over the last [`ROUND_TRIP_WINDOW`] measurements, reported once
/// [`ROUND_TRIP_SETTLED`] have come in.
struct Average {
    window: [f64; ROUND_TRIP_WINDOW],
    count: usize,
}

impl Average {
    fn new() -> Self {
        Self {
            window: [0.0; ROUND_TRIP_WINDOW],
            count: 0,
        }
    }

    fn add(&mut self, value: f64) -> Option<f64> {
        self.window[self.count % ROUND_TRIP_WINDOW] = value;
        self.count += 1;
        let n = self.count.min(ROUND_TRIP_WINDOW);
        (self.count >= ROUND_TRIP_SETTLED).then(|| self.window[..n].iter().sum::<f64>() / n as f64)
    }
}

/// State the UI reads without locking.
pub struct EngineShared {
    /// Measured output-to-input round trip in samples (see `RoundTrip`); `i64::MIN` until the
    /// device has reported enough, or when there is no input.
    pub round_trip: AtomicI64,
    /// The device's output latency in samples: from when a buffer is handed over to when it
    /// plays, from the device's timestamps. `i64::MIN` until measured.
    pub output_latency: AtomicI64,
    pub position: AtomicU64,
    /// Timeline position of what the output is playing now (see `publish_position`).
    pub heard: AtomicU64,
    /// `heard` before it is clamped to the timeline: negative during a count-in that starts
    /// before 0 (a pre-roll), so the count-in display can count down there too (22.3).
    pub heard_signed: AtomicI64,
    pub playing: AtomicBool,
    pub recording: AtomicBool,
    /// Engine position of the recording's first sample ([`RecordJob::start`]) once its tap has
    /// opened, `u64::MAX` before: where the takes being recorded are drawn from.
    pub record_start: AtomicU64,
    /// Callback time over the time the driver allows it, 0..1+ (see [`DeadlineMeter`]).
    pub dsp_load: AtomicF32,
    /// Worst `dsp_load` since the UI last read it; above 1.0 is a dropout.
    pub dsp_peak: AtomicF32,
    /// Graph latency (delay compensation), in samples.
    pub latency: AtomicU64,
    /// Device blocks the engine could not process because export held the core.
    pub skipped_blocks: AtomicU64,
    /// Plugin skip counter and the export flag, shared with every plugin insert.
    pub inserts: crate::plugins::InsertFlags,
    /// Device input arrived late or not at all.
    pub input_underruns: AtomicU64,
    /// The block the engine processes in: the device callback's, or the larger one it gathers
    /// device buffers into ([`Self::processing_block`]). Export uses it, so a render runs the
    /// same blocks as playback.
    pub block_size: AtomicU64,
    /// Frames of the last device callback.
    pub device_block: AtomicU64,
    /// Gather device buffers into blocks of at least this many frames (0: process each device
    /// buffer as it comes). Set by the session from the audio settings; the engine picks it up
    /// at its next callback (23.1).
    pub processing_block: AtomicU64,
    /// What gathering adds between input and output, in frames (0 without it): the block
    /// gathered less one device buffer. Recording moves takes earlier by it too.
    pub stage_latency: AtomicU64,
    /// Frames the driver asks for between waits (0: it calls back evenly), [`DeadlineMeter`].
    pub cadence: AtomicU64,
    pub sample_rate: AtomicU64,
    /// Worker threads helping the audio thread.
    pub workers: AtomicU64,
    pub input_peaks: [AtomicF32; MAX_IO_CHANNELS],
    pub output_peaks: [AtomicF32; MAX_IO_CHANNELS],
    /// The master's loudness, as the live meter last read it.
    pub loudness: LoudnessShared,
    /// Send the device silence, after everything (meters included) has run: for checks that
    /// need audio flowing through the master but must not be heard.
    pub silent: AtomicBool,
    /// The preview's place in its file (engine-rate samples), and whether it is playing.
    pub preview_position: AtomicU64,
    pub preview_playing: AtomicBool,
    /// The master is muted: the preview, which plays outside the graph, keeps quiet too.
    pub preview_muted: AtomicBool,
}

/// [`Loudness`] readings in atomics, NaN for "none yet" (silence reads a finite floor).
pub struct LoudnessShared {
    momentary: AtomicF32,
    short_term: AtomicF32,
    integrated: AtomicF32,
    range: AtomicF32,
    true_peak: AtomicF32,
    max_momentary: AtomicF32,
    max_short_term: AtomicF32,
    seconds: AtomicF32,
}

impl Default for LoudnessShared {
    /// Nothing measured yet.
    fn default() -> Self {
        let none = || AtomicF32::new(f32::NAN);
        Self {
            momentary: none(),
            short_term: none(),
            integrated: none(),
            range: none(),
            true_peak: none(),
            max_momentary: none(),
            max_short_term: none(),
            seconds: AtomicF32::new(0.0),
        }
    }
}

impl LoudnessShared {
    fn fields(&self) -> [&AtomicF32; 7] {
        [
            &self.momentary,
            &self.short_term,
            &self.integrated,
            &self.range,
            &self.true_peak,
            &self.max_momentary,
            &self.max_short_term,
        ]
    }

    /// Audio thread: publish the meter's readings.
    fn publish(&self, l: &Loudness) {
        let values = [
            l.momentary,
            l.short_term,
            l.integrated,
            l.range,
            l.true_peak,
            l.max_momentary,
            l.max_short_term,
        ];
        for (field, value) in self.fields().into_iter().zip(values) {
            field.store(value.map_or(f32::NAN, |v| v as f32));
        }
        self.seconds.store(l.seconds as f32);
    }

    pub fn read(&self) -> Loudness {
        let [m, s, i, lra, tp, max_m, max_s] = self.fields().map(|f| {
            let v = f.load();
            (!v.is_nan()).then_some(f64::from(v))
        });
        Loudness {
            momentary: m,
            short_term: s,
            integrated: i,
            range: lra,
            true_peak: tp,
            max_momentary: max_m,
            max_short_term: max_s,
            seconds: f64::from(self.seconds.load()),
        }
    }
}

pub struct EngineCore {
    graph: Box<Graph>,
    commands: Consumer<Command>,
    garbage: Producer<Garbage>,
    shared: Arc<EngineShared>,
    playing: bool,
    position: u64,
    /// Samples still to play before `position` (a count-in that starts before the timeline
    /// does): the playhead is `position − pre_roll`, and nothing plays from the timeline.
    pre_roll: u64,
    tempo: Box<TempoMap>,
    loop_range: Option<(u64, u64)>,
    /// Live only: [`Self::render`] never runs it, so exports cannot contain a click.
    metronome: Metronome,
    /// The master's loudness, metered live: a measurement tap after the graph and before the
    /// click, never in [`Self::render`]. Built here (it allocates), only reset after.
    loudness: LoudnessMeter,
    record: Option<Box<RecordJob>>,
    midi_ports: Vec<Consumer<TimedMidi>>,
    /// Live MIDI for the device buffer being processed, placed in it (see `take_midi`).
    pending_midi: Vec<LiveMidi>,
    /// The part of it that falls in the block being processed.
    midi_block: Vec<LiveMidi>,
    /// When the current device callback started (see `set_callback_time`).
    callback_at: Option<Instant>,
    /// Frames that have left the input ring, read or trimmed.
    input_read: u64,
    round_trip: RoundTrip,
    output_latency: Average,
    /// Each callback's load against the driver's real deadline, timed from `clock`.
    deadline: DeadlineMeter,
    clock: Instant,
    input: Vec<Vec<f32>>,
    output: Vec<Vec<f32>>,
    /// Device buffers gathered into larger blocks, when [`EngineShared::processing_block`]
    /// asks for it (23.1).
    stage: Stage,
    max_block: usize,
    pool: Option<Box<WorkerPool>>,
    /// The Waveform view's playback, live only (see [`Command::Preview`]).
    preview: Option<Box<Preview>>,
}

/// Device buffers gathered into one larger block (23.1).
///
/// Some plug-ins cost mostly per call, whatever its length: a high-gain amp sim about 300 µs, so
/// at FlexASIO's 16-sample buffers it needs more than a whole core, and gathered into blocks of
/// 64 about a third of one (`crates/dawpoc/README.md`). Input waits until a
/// block is in, the block runs as one (graph, meters, click, preview, recording), and its output
/// plays out a device buffer at a time. None of a block's input is there before its last buffer
/// comes, so the output is `block − device` frames later than without: the price, shown where
/// the setting is chosen. The block is a whole number of device buffers, so every block is run
/// by the buffer that completes it, and that buffer plays the block's first frames at once.
///
/// It helps a driver that calls back in bursts (FlexASIO on shared WASAPI: thirty 16-sample
/// buffers back to back every 10 ms), where the burst as a whole has the time: an even driver
/// gives the callback that runs a block only its own period.
#[derive(Default)]
struct Stage {
    /// The block gathered, in frames (0: not gathering), and the device buffer it was set up
    /// for; a change of either starts over.
    block: usize,
    device: usize,
    /// Input gathered for the next block: its first `staged` frames.
    input: Vec<Vec<f32>>,
    staged: usize,
    /// The last block's output, played out from `played`.
    output: Vec<Vec<f32>>,
    played: usize,
}

/// The UI side of the engine.
pub struct EngineHandle {
    pub commands: Producer<Command>,
    pub garbage: Consumer<Garbage>,
    pub shared: Arc<EngineShared>,
}

const MAX_MIDI_PER_BLOCK: usize = 1024;

pub fn new_engine(max_block: usize) -> (EngineCore, EngineHandle) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(256);
    let (garbage_tx, garbage_rx) = RingBuffer::new(256);
    let shared = Arc::new(EngineShared::default());
    let core = EngineCore {
        graph: Box::new(Graph::empty(max_block)),
        commands: cmd_rx,
        garbage: garbage_tx,
        shared: shared.clone(),
        playing: false,
        position: 0,
        pre_roll: 0,
        tempo: Box::new(TempoMap::constant(48_000, 120.0, 4, 4)),
        loop_range: None,
        metronome: Metronome::new(),
        loudness: LoudnessMeter::new(48_000),
        record: None,
        midi_ports: Vec::new(),
        pending_midi: Vec::with_capacity(MAX_MIDI_PER_BLOCK),
        midi_block: Vec::with_capacity(MAX_MIDI_PER_BLOCK),
        callback_at: None,
        input_read: 0,
        round_trip: RoundTrip::new(),
        output_latency: Average::new(),
        deadline: DeadlineMeter::default(),
        clock: Instant::now(),
        input: Vec::new(),
        output: Vec::new(),
        stage: Stage::default(),
        max_block,
        pool: None,
        preview: None,
    };
    let handle = EngineHandle {
        commands: cmd_tx,
        garbage: garbage_rx,
        shared,
    };
    (core, handle)
}

impl EngineCore {
    pub fn max_block(&self) -> usize {
        self.max_block
    }

    pub fn shared(&self) -> &Arc<EngineShared> {
        &self.shared
    }

    /// Size the I/O scratch buffers for a device. Call with the device stopped.
    pub fn configure_io(&mut self, inputs: usize, outputs: usize) {
        let inputs = inputs.min(MAX_IO_CHANNELS);
        let outputs = outputs.min(MAX_IO_CHANNELS);
        self.input = vec![vec![0.0; self.max_block]; inputs];
        self.output = vec![vec![0.0; self.max_block]; outputs];
        self.stage = Stage {
            input: vec![vec![0.0; self.max_block]; inputs],
            output: vec![vec![0.0; self.max_block]; outputs],
            ..Stage::default()
        };
        // A new device brings a new input ring: measure the round trip afresh.
        self.input_read = 0;
        self.round_trip = RoundTrip::new();
        self.shared.round_trip.store(i64::MIN, Ordering::Relaxed);
        self.output_latency = Average::new();
        self.shared
            .output_latency
            .store(i64::MIN, Ordering::Relaxed);
    }

    pub fn input_channels(&self) -> usize {
        self.input.len()
    }

    pub fn output_channels(&self) -> usize {
        self.output.len()
    }

    pub fn position(&self) -> u64 {
        self.position
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// The loop the playhead is inside and will wrap at. Playback that starts at or past the
    /// loop end plays on, as in most DAWs. Recording loops too: the session cuts the one
    /// continuous take into a take per pass (loop recording).
    fn active_loop(&self) -> Option<(u64, u64)> {
        let (start, end) = self.loop_range?;
        (self.playing && self.position < end).then_some((start, end))
    }

    /// How many of `frames` to process next: no further than the loop end or the end of a
    /// pre-roll, so the wrap, or the timeline start, falls exactly between two blocks.
    fn next_chunk(&self, frames: usize) -> usize {
        let frames = self.until_tempo_change(frames);
        if self.playing && self.pre_roll > 0 {
            return frames.min(self.pre_roll as usize);
        }
        match self.active_loop() {
            Some((_, end)) => frames.min((end - self.position) as usize),
            None => frames,
        }
    }

    /// No further than the next tempo or meter change, so plug-ins get it on its sample.
    fn until_tempo_change(&self, frames: usize) -> usize {
        if !self.playing {
            return frames;
        }
        let at = self.playhead();
        match self.tempo.next_change_after(at) {
            Some(change) => frames.min((change - at) as usize),
            None => frames,
        }
    }

    /// The playhead, negative during a pre-roll that starts before the timeline.
    fn playhead(&self) -> i64 {
        self.position as i64 - self.pre_roll as i64
    }

    /// The playhead for the next block.
    fn transport(&self) -> Transport {
        Transport {
            playing: self.playing,
            position: self.playhead(),
            meter: self.tempo.meter_at(self.playhead()),
        }
    }

    fn dispose(&mut self, garbage: Garbage) {
        // If the UI stops draining, leak rather than free on the audio thread.
        if let Err(rtrb::PushError::Full(g)) = self.garbage.push(garbage) {
            std::mem::forget(g);
        }
    }

    /// Apply queued commands. Called at the start of every block.
    pub fn handle_commands(&mut self) {
        let budget = self.commands.buffer().capacity();
        for _ in 0..budget {
            let Ok(command) = self.commands.pop() else {
                break;
            };
            match command {
                Command::SetGraph(mut graph) => {
                    graph.take_state_from(&mut self.graph);
                    let old = std::mem::replace(&mut self.graph, graph);
                    let latency = self.graph.latency();
                    if old.latency() != latency {
                        self.metronome.sync(self.playhead(), latency);
                    }
                    self.shared.latency.store(latency as u64, Ordering::Relaxed);
                    self.dispose(Garbage::Graph(old));
                }
                Command::Play => {
                    if !self.playing {
                        self.metronome.sync(self.playhead(), self.graph.latency());
                    }
                    self.playing = true;
                }
                Command::Stop => {
                    self.playing = false;
                    self.pre_roll = 0;
                    self.close_recording();
                }
                Command::Locate(position) => {
                    self.position = position;
                    self.pre_roll = 0;
                    self.graph.reset();
                    self.metronome.sync(self.playhead(), self.graph.latency());
                }
                Command::PreRoll(samples) => {
                    self.pre_roll = samples;
                    self.metronome.sync(self.playhead(), self.graph.latency());
                }
                Command::SetTempoMap(map) => {
                    let old = std::mem::replace(&mut self.tempo, map);
                    self.dispose(Garbage::TempoMap(old));
                    self.metronome.tempo_changed();
                }
                Command::SetLoop(range) => self.loop_range = range.filter(|(s, e)| e > s),
                Command::SetMetronome(settings) => self.metronome.settings = settings,
                Command::ResetLoudness => self.loudness.reset(),
                Command::Preview(preview) => {
                    if let Some(p) = &preview {
                        self.shared
                            .preview_position
                            .store(p.position, Ordering::Relaxed);
                    }
                    self.shared
                        .preview_playing
                        .store(preview.is_some(), Ordering::Relaxed);
                    if let Some(old) = std::mem::replace(&mut self.preview, preview) {
                        self.dispose(Garbage::Preview(old));
                    }
                }
                Command::SetPool(pool) => {
                    let workers = pool.as_ref().map_or(0, |p| p.workers());
                    self.shared.workers.store(workers as u64, Ordering::Relaxed);
                    if let Some(old) = std::mem::replace(&mut self.pool, pool) {
                        self.dispose(Garbage::Pool(old));
                    }
                }
                Command::SetMidiPorts(ports) => {
                    let old = std::mem::replace(&mut self.midi_ports, ports);
                    self.dispose(Garbage::MidiPorts(old));
                }
                Command::StartRecording(job) => {
                    self.close_recording();
                    self.shared.record_start.store(u64::MAX, Ordering::Relaxed);
                    self.record = Some(job);
                }
                Command::StopRecording => self.close_recording(),
                Command::SendMidi { track, event } => {
                    self.graph.send_midi_to(u64::from(track), event);
                }
            }
        }
        self.shared.playing.store(self.playing, Ordering::Relaxed);
        self.shared
            .recording
            .store(self.record.is_some(), Ordering::Relaxed);
        // Publish a locate now, not only after the next processed block.
        self.publish_position();
    }

    /// Take in the live MIDI that arrived since the last device buffer and place each event
    /// in this one (`total` frames).
    ///
    /// An event that came in Δ before this callback plays Δ before the end of this buffer:
    /// one buffer after it arrived, every time, instead of at whichever block start came
    /// next. Steady rather than jittery, the way DAWs place timestamped live MIDI. While
    /// recording it also goes to the session, stamped with what was heard when it came in
    /// (the session keeps what falls in the recorded window and takes off the device's
    /// output latency). With no callback time (tests, no device) everything plays at the
    /// buffer's start and is stamped with its heard position.
    ///
    /// `base` is where the device buffer starts in the block being gathered ([`Stage`]; 0 when
    /// not gathering): its events go after those already gathered, placed from there.
    fn take_midi(&mut self, total: usize, base: usize) {
        if base == 0 {
            self.pending_midi.clear();
        }
        let now = self.callback_at.take();
        let rate = self.sample_rate_hint().unwrap_or(0.0);
        // While gathering, the heard position moves only when a block runs, but the device
        // plays the last block out a buffer at a time: `base` frames of it have gone by. With
        // that added, a stamp less the stage latency (which the session takes off with the
        // output latency) is what was playing when the key came in, as without gathering.
        let heard = self.metronome.heard() + base as i64;
        let recording = self.playing;
        for (port, rx) in self.midi_ports.iter_mut().enumerate() {
            while self.pending_midi.len() < MAX_MIDI_PER_BLOCK {
                let Ok(timed) = rx.pop() else {
                    break;
                };
                // Arrived during this callback: as late as possible in it.
                let ago = now.map_or(0.0, |now| {
                    now.saturating_duration_since(timed.at).as_secs_f64() * rate
                });
                let offset = if now.is_some() {
                    (total as f64 - ago)
                        .round()
                        .clamp(0.0, total.saturating_sub(1) as f64) as usize
                } else {
                    0
                };
                self.pending_midi.push(LiveMidi {
                    port,
                    event: timed.event,
                    offset: base + offset,
                });
                let Some(job) = self.record.as_deref_mut().filter(|_| recording) else {
                    continue;
                };
                let recorded = RecordedMidi {
                    heard: heard - ago.round() as i64,
                    wraps: job.wraps,
                    port,
                    event: timed.event,
                };
                if let Some(midi) = &mut job.midi {
                    if midi.push(recorded).is_err() {
                        job.midi_dropped += 1;
                    }
                }
            }
        }
        // In time order, for slicing into chunks and for the plug-ins.
        self.pending_midi.sort_by_key(|m| m.offset);
    }

    /// Hand the recording back to the main thread (which finishes its files), after sending
    /// each take's last, partial peak bin. Allocation-free.
    fn close_recording(&mut self) {
        if let Some(mut job) = self.record.take() {
            for take in &mut job.takes {
                if let Some(peaks) = &mut take.peaks {
                    peaks.flush();
                }
            }
            self.dispose(Garbage::Recording(job));
        }
    }

    /// The device callback is starting now: live MIDI is placed relative to this. Call
    /// before [`Self::process`] (the device does); without it, live MIDI plays at the
    /// buffer's start.
    pub fn set_callback_time(&mut self, at: Instant) {
        self.callback_at = Some(at);
    }

    /// Process one block of at most `max_block` frames from `self.input` into `self.output`.
    /// `at` is where it starts in the device buffer, for slicing that buffer's live MIDI.
    fn process_block(&mut self, frames: usize, at: usize) {
        for out in &mut self.output {
            out[..frames].fill(0.0);
        }
        self.midi_block.clear();
        for live in &self.pending_midi {
            if (at..at + frames).contains(&live.offset) {
                self.midi_block.push(LiveMidi {
                    offset: live.offset - at,
                    ..*live
                });
            }
        }

        // The record tap: between the record point (or punch-in) and the punch-out, to the
        // sample, never during a pre-roll.
        let tap = match self.record.as_deref() {
            Some(job) if job.exact_from && !job.open && self.position > job.from => None,
            Some(job) if self.playing && self.pre_roll == 0 => {
                let skip = if job.open {
                    0
                } else {
                    job.from.saturating_sub(self.position)
                };
                let end = job.to.saturating_sub(self.position).min(frames as u64);
                (skip < end).then_some((skip as usize, end as usize))
            }
            _ => None,
        };
        if let Some((skip, end)) = tap {
            if let Some(job) = self.record.as_deref_mut() {
                job.open = true;
                if job.start.is_none() {
                    let start = self.position + skip as u64;
                    job.start = Some(start);
                    self.shared.record_start.store(start, Ordering::Relaxed);
                }
                let frames = end - skip;
                for take in &mut job.takes {
                    let right = if take.stereo {
                        take.channel + 1
                    } else {
                        take.channel
                    };
                    let (Some(l), Some(r)) = (self.input.get(take.channel), self.input.get(right))
                    else {
                        continue;
                    };
                    let width = if take.stereo { 2 } else { 1 };
                    if take.producer.slots() < frames * width {
                        job.dropped += frames as u64;
                        continue;
                    }
                    for i in skip..skip + frames {
                        let _ = take.producer.push(l[i]);
                        if take.stereo {
                            let _ = take.producer.push(r[i]);
                        }
                    }
                    // The same frames the file gets (none when the writer fell behind), so the
                    // peaks stay aligned with it.
                    if let Some(peaks) = &mut take.peaks {
                        for i in skip..skip + frames {
                            peaks.add(l[i], r[i]);
                        }
                    }
                }
            }
        }

        let transport = self.transport();
        let looping = self.active_loop();
        let tracing = trace::on();
        if tracing {
            trace::graph_start();
        }
        self.graph.process_with(
            &mut BlockContext {
                frames,
                transport,
                input: &self.input,
                output: &mut self.output,
                midi: &self.midi_block,
            },
            self.pool.as_deref(),
        );
        if tracing {
            trace::graph_end();
        }
        self.meter_loudness(frames);
        // After the master and its loudness tap, like the click: the Waveform view's playback.
        self.play_preview(frames);
        // After the master: no plug-in or fader touches the click.
        self.metronome.process(
            &mut self.output,
            frames,
            &self.tempo,
            self.playing,
            self.record.is_some(),
            self.record.as_ref().and_then(|j| j.count_in_to),
        );
        if self.playing {
            if self.pre_roll > 0 {
                self.pre_roll -= frames as u64;
            } else {
                self.position += frames as u64;
            }
        }
        // Wrap without resetting anything: delay lines and plugins hold audio from just before
        // the loop end that is still on its way out, and it must play on seamlessly.
        if let Some((start, end)) = looping {
            if self.position >= end {
                self.position = start;
                self.metronome.wrapped(start, self.graph.latency());
                if let Some(job) = self.record.as_deref_mut() {
                    job.wraps += 1;
                }
            }
        }
        for (peak, buf) in self.shared.input_peaks.iter().zip(&self.input) {
            peak.fetch_max_positive(buf[..frames].iter().fold(0.0, |m, s| m.max(s.abs())));
        }
        for (peak, buf) in self.shared.output_peaks.iter().zip(&self.output) {
            peak.fetch_max_positive(buf[..frames].iter().fold(0.0, |m, s| m.max(s.abs())));
        }
        self.publish_position();
    }

    /// The loudness meter's tap: the master's output pair (where the click also plays), after
    /// the graph and before the click. It measures what leaves that pair, tracks sent straight
    /// to it included. Integrated loudness and the rest accumulate while the timeline plays.
    /// Allocation-free.
    /// Add the preview's next `frames` to the master's output pair (a mono file to both), and
    /// hand it back once it reaches its end. Silent while the master is muted. Live only:
    /// `render` never calls this, so no export contains a preview.
    fn play_preview(&mut self, frames: usize) {
        let Some(p) = self.preview.as_mut() else {
            return;
        };
        let n = (frames as u64).min(p.end.saturating_sub(p.position)) as usize;
        let from = p.position as usize;
        let muted = self.shared.preview_muted.load(Ordering::Relaxed);
        if let Some(rack) = p.rack.as_mut() {
            // Through the rack, every block, muted or not: Apply's render runs the same blocks.
            rack.process(&p.audio, p.position, n);
            if !muted {
                for (c, src) in rack.output().iter().enumerate() {
                    if let Some(out) = self.output.get_mut(p.channel + c) {
                        for (o, s) in out[..n].iter_mut().zip(&src[..n]) {
                            *o += *s;
                        }
                    }
                }
            }
        } else if !muted {
            let channels = &p.audio.channels;
            for c in 0..2 {
                let (Some(src), Some(out)) = (
                    channels.get(c.min(channels.len().saturating_sub(1))),
                    self.output.get_mut(p.channel + c),
                ) else {
                    continue;
                };
                let src = src.get(from..).unwrap_or(&[]);
                for (o, s) in out[..n].iter_mut().zip(src) {
                    *o += *s;
                }
            }
        }
        p.position += n as u64;
        self.shared
            .preview_position
            .store(p.position, Ordering::Relaxed);
        if p.position >= p.end {
            self.shared.preview_playing.store(false, Ordering::Relaxed);
            if let Some(done) = self.preview.take() {
                self.dispose(Garbage::Preview(done));
            }
        }
    }

    fn meter_loudness(&mut self, frames: usize) {
        if self.loudness.rate() != self.tempo.sample_rate {
            self.loudness.set_rate(self.tempo.sample_rate);
        }
        self.loudness
            .set_running(self.playing && self.pre_roll == 0);
        let left = self.metronome.settings.channel;
        if let (Some(l), Some(r)) = (self.output.get(left), self.output.get(left + 1)) {
            self.loudness.process(&l[..frames], Some(&r[..frames]));
        }
        self.shared.loudness.publish(&self.loudness.readings());
    }

    /// Where the playhead is, and what the output is playing now (behind it by the delay
    /// compensation, and still before a loop wrap for that long after one).
    fn publish_position(&self) {
        self.shared.position.store(self.position, Ordering::Relaxed);
        let heard = if self.playing {
            self.metronome.heard()
        } else {
            self.position as i64
        };
        self.shared.heard_signed.store(heard, Ordering::Relaxed);
        self.shared
            .heard
            .store(heard.max(0) as u64, Ordering::Relaxed);
    }

    /// Process a device buffer: deinterleave input from the ring, run the graph in chunks of
    /// at most `max_block`, and interleave into `out`.
    pub fn process<T: cpal::Sample + cpal::FromSample<f32>>(
        &mut self,
        out: &mut [T],
        out_channels: usize,
        mut input: Option<&mut Consumer<f32>>,
        in_channels: usize,
    ) {
        let started = Instant::now();
        let total = out.len() / out_channels.max(1);
        let rate = self.sample_rate_hint();
        let budget = rate.map(|rate| {
            let start = started.duration_since(self.clock).as_nanos() as u64;
            self.deadline.begin(start, total, rate)
        });
        let tracing = trace::on();
        if tracing {
            let period = rate.map_or(0, |rate| (total as f64 / rate * 1e9) as u64);
            trace::block_begin(budget.unwrap_or(0), period);
        }
        self.handle_commands();
        let gather = self.gathering(total);
        self.shared
            .block_size
            .store(gather.unwrap_or(total) as u64, Ordering::Relaxed);
        self.shared
            .device_block
            .store(total as u64, Ordering::Relaxed);
        if let Some(rx) = input.as_deref_mut() {
            self.input_read += trim_input_backlog(rx, in_channels, total);
            // The first input frame read now goes with the first output frame played.
            if let Some(rate) = self.sample_rate_hint() {
                if let Some(frames) = self.round_trip.measure(self.input_read, rate) {
                    let frames = frames.round() as i64;
                    self.shared.round_trip.store(frames, Ordering::Relaxed);
                }
            }
        }
        // Silenced (checks that must make no sound): everything runs and every meter reads it,
        // but the device gets zeros.
        let silent = self.shared.silent.load(Ordering::Relaxed);
        match gather {
            None => {
                self.take_midi(total, 0);
                let mut done = 0;
                while done < total {
                    let frames = self.next_chunk((total - done).min(self.max_block));
                    self.read_input(input.as_deref_mut(), in_channels, frames);
                    self.process_block(frames, done);
                    interleave(out, out_channels, &self.output, 0, done, frames, silent);
                    done += frames;
                }
            }
            Some(block) => {
                let at = self.stage.staged;
                self.take_midi(total, at);
                self.read_input(input, in_channels, total);
                for (dst, src) in self.stage.input.iter_mut().zip(&self.input) {
                    dst[at..at + total].copy_from_slice(&src[..total]);
                }
                self.stage.staged += total;
                if self.stage.staged >= block {
                    self.run_gathered(block);
                }
                let from = self.stage.played;
                interleave(
                    out,
                    out_channels,
                    &self.stage.output,
                    from,
                    0,
                    total,
                    silent,
                );
                self.stage.played += total;
            }
        }
        if tracing {
            trace::block_end();
        }
        if budget.is_some() {
            let end = self.clock.elapsed().as_nanos() as u64;
            let load = self.deadline.end(end);
            self.shared.dsp_load.store(self.deadline.shown());
            self.shared.dsp_peak.fetch_max_positive(load);
            self.shared
                .cadence
                .store(self.deadline.cadence(), Ordering::Relaxed);
        }
    }

    /// The block to gather device buffers of `frames` into ([`Stage`]), or `None` to process
    /// each buffer as it comes: the fewest whole buffers that reach
    /// [`EngineShared::processing_block`], within `max_block`. A new size, of either, starts
    /// over: the output plays silence until the first block is in.
    fn gathering(&mut self, frames: usize) -> Option<usize> {
        let wanted = self.shared.processing_block.load(Ordering::Relaxed) as usize;
        let block = match frames {
            0 => 0,
            _ if wanted <= frames => 0,
            _ => (wanted.div_ceil(frames) * frames).min(self.max_block / frames * frames),
        };
        let block = if block > frames { block } else { 0 };
        if block != self.stage.block || frames != self.stage.device {
            self.stage.block = block;
            self.stage.device = frames;
            self.stage.staged = 0;
            for c in &mut self.stage.output {
                c.fill(0.0);
            }
            // The block's other buffers play silence until the first block is in.
            self.stage.played = frames;
            self.shared
                .stage_latency
                .store(block.saturating_sub(frames) as u64, Ordering::Relaxed);
        }
        (block > 0).then_some(block)
    }

    /// Run a gathered block through the engine into the stage's output, in chunks as a device
    /// buffer is (split at loop ends and tempo changes). Allocation-free.
    fn run_gathered(&mut self, block: usize) {
        let mut done = 0;
        while done < block {
            let frames = self.next_chunk((block - done).min(self.max_block));
            for (dst, src) in self.input.iter_mut().zip(&self.stage.input) {
                dst[..frames].copy_from_slice(&src[done..done + frames]);
            }
            self.process_block(frames, done);
            for (dst, src) in self.stage.output.iter_mut().zip(&self.output) {
                dst[done..done + frames].copy_from_slice(&src[..frames]);
            }
            done += frames;
        }
        self.stage.staged = 0;
        self.stage.played = 0;
    }

    fn sample_rate_hint(&self) -> Option<f64> {
        let rate = self.shared.sample_rate.load(Ordering::Relaxed);
        (rate > 0).then_some(rate as f64)
    }

    fn read_input(&mut self, input: Option<&mut Consumer<f32>>, in_channels: usize, frames: usize) {
        for buf in &mut self.input {
            buf[..frames].fill(0.0);
        }
        let Some(rx) = input else {
            return;
        };
        let in_channels = in_channels.max(1);
        let needed = frames * in_channels;
        let available = rx.slots().min(needed);
        let frames_available = available / in_channels;
        if frames_available < frames {
            self.shared.input_underruns.fetch_add(1, Ordering::Relaxed);
        }
        let Ok(chunk) = rx.read_chunk(frames_available * in_channels) else {
            return;
        };
        let (a, b) = chunk.as_slices();
        for (i, s) in a.iter().chain(b).enumerate() {
            let (f, c) = (i / in_channels, i % in_channels);
            if let Some(buf) = self.input.get_mut(c) {
                buf[f] = *s;
            }
        }
        chunk.commit_all();
        self.input_read += frames_available as u64;
    }

    /// The device's timing for the buffer about to be processed: when the callback ran and
    /// when its first output frame will play (their difference is the output latency), and
    /// the newest input capture anchor (see `RoundTrip`). Call before [`Self::process`];
    /// without it neither latency is measured.
    pub fn set_clock(&mut self, callback_ns: u64, playback_ns: u64, anchor: Option<InputAnchor>) {
        self.round_trip.playback_ns = Some(playback_ns);
        self.round_trip.anchor = anchor;
        if let Some(rate) = self.sample_rate_hint() {
            let frames = playback_ns.saturating_sub(callback_ns) as f64 * rate / 1e9;
            if let Some(average) = self.output_latency.add(frames) {
                self.shared
                    .output_latency
                    .store(average.round() as i64, Ordering::Relaxed);
            }
        }
    }

    /// Run the transport offline for export: `frames` samples from the current position, with
    /// silent input, appending the hardware outputs to `sink`. An export renders its range
    /// straight through, so the loop does not apply here.
    pub fn render(&mut self, frames: usize, block: usize, sink: &mut [Vec<f32>]) {
        let block = block.clamp(1, self.max_block);
        let mut done = 0;
        while done < frames {
            let n = self.until_tempo_change((frames - done).min(block));
            for buf in &mut self.input {
                buf[..n].fill(0.0);
            }
            self.midi_block.clear();
            let transport = self.transport();
            for out in &mut self.output {
                out[..n].fill(0.0);
            }
            self.graph.process_with(
                &mut BlockContext {
                    frames: n,
                    transport,
                    input: &self.input,
                    output: &mut self.output,
                    midi: &[],
                },
                self.pool.as_deref(),
            );
            if self.playing {
                self.position += n as u64;
            }
            for (dst, src) in sink.iter_mut().zip(&self.output) {
                dst.extend_from_slice(&src[..n]);
            }
            done += n;
        }
    }

    pub fn graph_latency(&self) -> usize {
        self.graph.latency()
    }

    /// Set transport state directly (export only; the device must not be running it).
    pub fn set_transport(&mut self, playing: bool, position: u64) {
        self.playing = playing;
        self.position = position;
        self.pre_roll = 0;
        self.graph.reset();
        // As a locate does, so playback resumed after an export clicks in time.
        self.metronome.sync(self.playhead(), self.graph.latency());
    }
}

/// Interleave `frames` of the planar `from`, from its frame `src`, into the device buffer `out`
/// from its frame `dst`. Channels the device lacks are dropped; channels the engine lacks, frames
/// past `from`'s end, and everything when `silent`, play silence.
fn interleave<T: cpal::Sample + cpal::FromSample<f32>>(
    out: &mut [T],
    out_channels: usize,
    from: &[Vec<f32>],
    src: usize,
    dst: usize,
    frames: usize,
    silent: bool,
) {
    for f in 0..frames {
        let frame = &mut out[(dst + f) * out_channels..(dst + f + 1) * out_channels];
        for (c, sample) in frame.iter_mut().enumerate() {
            let v = match from.get(c).and_then(|b| b.get(src + f)) {
                Some(&v) if !silent => v,
                _ => 0.0,
            };
            *sample = T::from_sample(v);
        }
    }
}

/// Keep the input ring from drifting into extra latency: never let more than a couple of
/// device buffers queue up. Measured against the whole buffer, once per callback, because a
/// loop wrap can cut a buffer into a tiny chunk whose own size says nothing about the backlog.
/// Returns the frames dropped.
fn trim_input_backlog(rx: &mut Consumer<f32>, in_channels: usize, frames: usize) -> u64 {
    let in_channels = in_channels.max(1);
    let excess = rx.slots().saturating_sub(frames * in_channels * 3);
    if excess > 0 {
        let drop = excess - excess % in_channels;
        if let Ok(chunk) = rx.read_chunk(drop) {
            chunk.commit_all();
            return (drop / in_channels) as u64;
        }
    }
    0
}

impl Default for EngineShared {
    fn default() -> Self {
        Self {
            round_trip: AtomicI64::new(i64::MIN),
            output_latency: AtomicI64::new(i64::MIN),
            position: AtomicU64::new(0),
            heard: AtomicU64::new(0),
            heard_signed: AtomicI64::new(0),
            playing: AtomicBool::new(false),
            recording: AtomicBool::new(false),
            record_start: AtomicU64::new(u64::MAX),
            dsp_load: AtomicF32::new(0.0),
            dsp_peak: AtomicF32::new(0.0),
            latency: AtomicU64::new(0),
            skipped_blocks: AtomicU64::new(0),
            inserts: Default::default(),
            input_underruns: AtomicU64::new(0),
            block_size: AtomicU64::new(0),
            device_block: AtomicU64::new(0),
            processing_block: AtomicU64::new(0),
            stage_latency: AtomicU64::new(0),
            cadence: AtomicU64::new(0),
            sample_rate: AtomicU64::new(0),
            workers: AtomicU64::new(0),
            input_peaks: std::array::from_fn(|_| AtomicF32::new(0.0)),
            output_peaks: std::array::from_fn(|_| AtomicF32::new(0.0)),
            loudness: LoudnessShared::default(),
            preview_position: AtomicU64::new(0),
            preview_playing: AtomicBool::new(false),
            preview_muted: AtomicBool::new(false),
            silent: AtomicBool::new(false),
        }
    }
}

impl EngineShared {
    pub fn set_sample_rate(&self, rate: u32) {
        self.sample_rate.store(u64::from(rate), Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphBuilder, NodeInput, NodeParams, NodeSpec, SpecTarget};

    #[test]
    fn interleaves_input_through_monitoring_to_output() {
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(2, 2);
        let params = Arc::new(NodeParams::default());
        params.monitoring.store(true, Ordering::Relaxed);
        let mut b = GraphBuilder::new();
        b.add(NodeSpec {
            key: 1,
            label: "t".into(),
            params: params.clone(),
            clips: vec![],
            input: NodeInput::Audio {
                channel: 1,
                stereo: false,
            },
            inserts: vec![],
            target: SpecTarget::Hardware(0),
        });
        assert!(handle
            .commands
            .push(Command::SetGraph(Box::new(b.build(64))))
            .is_ok());

        let (mut tx, mut rx) = RingBuffer::<f32>::new(4096);
        let mut device_block = || {
            // Input as a device delivers it: left channel silent, right channel 0.5.
            for _ in 0..100 {
                tx.push(0.0).unwrap();
                tx.push(0.5).unwrap();
            }
            // 100 frames is more than max_block, so this also exercises chunking.
            let mut out = vec![0.0f32; 100 * 2];
            core.process(&mut out, 2, Some(&mut rx), 2);
            out
        };
        // The fader ramps up from silence over the first chunk, then holds.
        device_block();
        let out = device_block();
        assert!(
            out.iter().all(|s| (*s - 0.5).abs() < 1e-6),
            "{:?}",
            &out[..8]
        );
        assert_eq!(core.shared().input_underruns.load(Ordering::Relaxed), 0);
        assert!(matches!(handle.garbage.pop(), Ok(Garbage::Graph(_))));
    }

    #[test]
    fn loops_without_gap_or_overlap_at_any_block_size() {
        use crate::graph::{ClipSource, DelayLine, Processor};
        use crate::media::AudioData;

        struct Lag(DelayLine);
        impl Processor for Lag {
            fn latency(&self) -> usize {
                self.0.delay()
            }
            fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
                self.0.process(io, frames);
            }
        }

        // A clip whose every sample is distinct, on a track that gets 300 samples of delay
        // compensation from a silent "slow" track, so audio from before each wrap is still in
        // a delay line when the playhead jumps back.
        let ramp: Vec<f32> = (0..20_000).map(|i| i as f32 * 1e-4).collect();
        const LAG: usize = 300;
        let (start, end, from) = (1000u64, 1500u64, 1200u64);
        let graph = || {
            let mut b = GraphBuilder::new();
            let spec = |key, inserts: Vec<Box<dyn Processor>>, clips| NodeSpec {
                key,
                label: String::new(),
                params: Arc::new(NodeParams::default()),
                clips,
                input: NodeInput::None,
                inserts,
                target: SpecTarget::Hardware(0),
            };
            b.add(spec(1, vec![Box::new(Lag(DelayLine::new(LAG, 2)))], vec![]));
            let clip = ClipSource {
                data: Arc::new(AudioData::new(48_000, vec![ramp.clone()])),
                start: 0,
                offset: 0,
                length: ramp.len() as u64,
                gain: 1.0,
                fades: Default::default(),
            };
            b.add(spec(2, vec![], vec![clip]));
            b.build(256)
        };

        // What the output must be: the playhead's path through the timeline, 300 late.
        let total = 4000;
        let mut path = Vec::new();
        let mut at = from;
        while path.len() < total {
            path.push(at);
            at += 1;
            if at == end {
                at = start;
            }
        }
        let expected: Vec<f32> = (0..total)
            .map(|n| n.checked_sub(LAG).map_or(0.0, |k| ramp[path[k] as usize]))
            .collect();

        for device_block in [1usize, 7, 64, 100, 256, 333, 512] {
            let (mut core, mut handle) = new_engine(256);
            core.configure_io(0, 2);
            for c in [
                Command::SetGraph(Box::new(graph())),
                Command::SetLoop(Some((start, end))),
                Command::Locate(from),
                Command::Play,
            ] {
                assert!(handle.commands.push(c).is_ok());
            }
            let mut left = Vec::new();
            while left.len() < total {
                let mut out = vec![0.0f32; device_block * 2];
                core.process(&mut out, 2, None, 0);
                left.extend(out.chunks(2).map(|f| f[0]));
            }
            // The fader ramps up over the first block; after that every sample must match.
            let skip = LAG + 256;
            for n in skip..total {
                assert_eq!(
                    left[n].to_bits(),
                    expected[n].to_bits(),
                    "block {device_block}: sample {n} is {}, expected {}",
                    left[n],
                    expected[n]
                );
            }
            assert!(core.position() >= start && core.position() < end);
        }
    }

    /// The Waveform view's preview plays its file on the output pair, sample for sample at any
    /// block size, stops exactly at its end, stays quiet while the master is muted, and is
    /// never in a render (an export).
    #[test]
    fn a_preview_plays_its_file_live_and_never_in_a_render() {
        use crate::media::AudioData;
        let ramp: Vec<f32> = (0..6_000).map(|i| (i as f32 * 1e-4) - 0.3).collect();
        let audio = Arc::new(AudioData::new(48_000, vec![ramp.clone()]));
        let (from, end) = (1_000u64, 4_321u64);
        let preview = || {
            Box::new(Preview {
                audio: audio.clone(),
                position: from,
                end,
                channel: 0,
                rack: None,
            })
        };
        for device_block in [1usize, 64, 333, 512] {
            let (mut core, mut handle) = new_engine(256);
            core.configure_io(0, 2);
            assert!(handle
                .commands
                .push(Command::Preview(Some(preview())))
                .is_ok());
            let total = (end - from) as usize + 700;
            let (mut left, mut right) = (Vec::new(), Vec::new());
            while left.len() < total {
                let mut out = vec![0.0f32; device_block * 2];
                core.process(&mut out, 2, None, 0);
                left.extend(out.chunks(2).map(|f| f[0]));
                right.extend(out.chunks(2).map(|f| f[1]));
            }
            for n in 0..total {
                let want = ramp
                    .get(from as usize + n)
                    .copied()
                    .filter(|_| n < (end - from) as usize);
                let want = want.unwrap_or(0.0);
                assert_eq!(
                    left[n].to_bits(),
                    want.to_bits(),
                    "block {device_block}: {n}"
                );
                assert_eq!(right[n].to_bits(), want.to_bits(), "mono to both: {n}");
            }
            assert!(!handle.shared.preview_playing.load(Ordering::Relaxed));
            assert_eq!(handle.shared.preview_position.load(Ordering::Relaxed), end);
        }

        // Muted master: nothing.
        let (mut core, mut handle) = new_engine(256);
        core.configure_io(0, 2);
        handle.shared.preview_muted.store(true, Ordering::Relaxed);
        assert!(handle
            .commands
            .push(Command::Preview(Some(preview())))
            .is_ok());
        let mut out = vec![0.0f32; 1024];
        core.process(&mut out, 2, None, 0);
        assert!(
            out.iter().all(|s| *s == 0.0),
            "a muted master silences the preview"
        );

        // A render with a preview playing: silence (an export never contains one).
        let (mut core, mut handle) = new_engine(256);
        core.configure_io(0, 2);
        assert!(handle
            .commands
            .push(Command::Preview(Some(preview())))
            .is_ok());
        core.handle_commands();
        let mut sink = vec![vec![0.0f32; 2_000]; 2];
        core.render(2_000, 256, &mut sink);
        assert!(
            sink.iter().flatten().all(|s| *s == 0.0),
            "no preview in a render"
        );
    }

    /// Render = preview for the Waveform view's rack: a file previewed through a settled chain
    /// at any device block size is, after the chain's latency, sample for sample what Apply's
    /// offline render of the same chain gives; and a render of the project still never
    /// contains the preview.
    #[test]
    fn a_rack_preview_is_what_apply_renders() {
        use crate::media::AudioData;
        use crate::rack::RackChain;
        let signal: Vec<f32> = (0..9_000)
            .map(|i| ((i * 53) % 97) as f32 / 97.0 - 0.5)
            .collect();
        let audio = Arc::new(AudioData::new(48_000, vec![signal]));
        let (from, to) = (1_000u64, 6_000u64);
        let chain = || {
            let inserts: Vec<Box<dyn crate::graph::Processor>> = vec![
                Box::new(crate::rack::tests::Smooth::new(37)),
                Box::new(crate::rack::tests::Smooth::new(5)),
            ];
            let mut rack = RackChain::new(inserts, Default::default(), 256);
            rack.settle(2_000, 256);
            rack
        };
        // Apply: the offline render.
        let mut offline = chain();
        let latency = offline.latency() as u64;
        let applied = offline
            .render(&audio, from, to, 64, |_| {}, || false)
            .unwrap();
        for device_block in [1usize, 64, 333, 512] {
            let (mut core, mut handle) = new_engine(256);
            core.configure_io(0, 2);
            let preview = Preview {
                audio: audio.clone(),
                position: from,
                end: to + latency,
                channel: 0,
                rack: Some(Box::new(chain())),
            };
            assert!(handle
                .commands
                .push(Command::Preview(Some(Box::new(preview))))
                .is_ok());
            let total = (to - from + latency) as usize;
            let mut left = Vec::new();
            while left.len() < total {
                let mut out = vec![0.0f32; device_block * 2];
                core.process(&mut out, 2, None, 0);
                left.extend(out.chunks(2).map(|f| f[0]));
            }
            let heard = &left[latency as usize..total];
            assert_eq!(heard.len(), applied[0].len());
            for (n, (h, a)) in heard.iter().zip(&applied[0]).enumerate() {
                assert_eq!(h.to_bits(), a.to_bits(), "block {device_block}: sample {n}");
            }
        }
        let (mut core, mut handle) = new_engine(256);
        core.configure_io(0, 2);
        let preview = Preview {
            audio: audio.clone(),
            position: from,
            end: to,
            channel: 0,
            rack: Some(Box::new(chain())),
        };
        assert!(handle
            .commands
            .push(Command::Preview(Some(Box::new(preview))))
            .is_ok());
        core.handle_commands();
        let mut sink = vec![vec![0.0f32; 2_000]; 2];
        core.render(2_000, 256, &mut sink);
        assert!(
            sink.iter().flatten().all(|s| *s == 0.0),
            "no preview in a render"
        );
    }

    /// Render = preview for MIDI: playback from X through the device callback, and an export
    /// from X the way `export.rs` runs one (a stopped pre-roll, then playing), hand the
    /// instrument the same events on the same samples. Both start at X, so both chase the
    /// note X falls inside.
    #[test]
    fn midi_clips_play_the_same_live_and_in_an_export() {
        use crate::graph::Processor;
        use crate::midi_clip::{MidiClip, Note};
        use crate::sequencer::Sequence;
        use std::sync::Mutex;
        use vst3_host::MidiEvent;

        type Log = Arc<Mutex<Vec<(i64, MidiEvent)>>>;
        /// Logs each event at its absolute sample: the block's position plus its offset.
        struct Spy {
            pending: Vec<(MidiEvent, usize)>,
            log: Log,
        }
        impl Processor for Spy {
            fn latency(&self) -> usize {
                0
            }
            fn process(&mut self, _: &mut [Vec<f32>], frames: usize, transport: &Transport) {
                for (event, offset) in self.pending.drain(..) {
                    assert!(offset < frames);
                    if transport.playing {
                        self.log
                            .lock()
                            .unwrap()
                            .push((transport.position + offset as i64, event));
                    }
                }
            }
            fn send_midi_at(&mut self, event: MidiEvent, offset: usize) {
                self.pending.push((event, offset));
            }
        }
        // At 120 and 48 kHz a tick is 25 samples. A note spans X = 30000; the rest follow.
        let notes: Vec<Note> = [
            (0, 1920, 48),
            (1300, 100, 60),
            (1301, 7, 64),
            (2000, 960, 67),
        ]
        .iter()
        .enumerate()
        .map(|(i, &(tick, length, pitch))| Note {
            id: i as u32,
            tick,
            length,
            pitch,
            velocity: 90,
            channel: 0,
        })
        .collect();
        let clip = MidiClip {
            id: 99,
            name: String::new(),
            start: 0,
            length: 3840,
            notes,
            controllers: Vec::new(),
        };
        let graph = |log: Log| {
            let mut b = GraphBuilder::new();
            let i = b.add(NodeSpec {
                key: 1,
                label: String::new(),
                params: Arc::new(NodeParams::default()),
                clips: Vec::new(),
                input: NodeInput::None,
                inserts: vec![Box::new(Spy {
                    pending: Vec::new(),
                    log,
                })],
                target: SpecTarget::Hardware(0),
            });
            b.set_sequence(i, Sequence::new(std::slice::from_ref(&clip), true));
            b.build(512)
        };
        let (from, total) = (30_000u64, 80_000usize);

        let live: Log = Arc::default();
        let (mut core, mut handle) = new_engine(512);
        core.configure_io(0, 2);
        for c in [
            Command::SetGraph(Box::new(graph(live.clone()))),
            Command::Locate(from),
            Command::Play,
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        let mut done = 0;
        while done < total {
            let mut out = vec![0.0f32; 333 * 2];
            core.process(&mut out, 2, None, 0);
            done += 333;
        }

        let export: Log = Arc::default();
        let (mut core, mut handle) = new_engine(512);
        core.configure_io(0, 2);
        assert!(handle
            .commands
            .push(Command::SetGraph(Box::new(graph(export.clone()))))
            .is_ok());
        core.handle_commands();
        let mut sink = vec![Vec::new(); 2];
        core.set_transport(false, from);
        core.render(9_600, 512, &mut sink);
        core.set_transport(true, from);
        core.render(total, 512, &mut sink);

        let within = |log: &Log| -> Vec<(i64, MidiEvent)> {
            let end = (from as usize + total - 333) as i64;
            log.lock()
                .unwrap()
                .iter()
                .copied()
                .filter(|(at, _)| *at < end)
                .collect()
        };
        let (live, export) = (within(&live), within(&export));
        // The chased note at X, then 60, 64 and 67 on their ticks (x25 samples).
        let starts: Vec<i64> = live
            .iter()
            .filter(|(_, e)| matches!(e, MidiEvent::NoteOn { .. }))
            .map(|(at, _)| *at)
            .collect();
        assert_eq!(starts, [30_000, 32_500, 32_525, 50_000]);
        assert_eq!(live, export, "the export hears exactly what playback does");
    }

    /// The loudness meter (6.1) reads the master's pair after the graph and before the click,
    /// accumulates while playing, and resets on command.
    #[test]
    fn the_loudness_tap_meters_the_master_pair_without_the_click() {
        use crate::graph::ClipSource;
        use crate::media::AudioData;
        // A -23 dBFS 1 kHz tone for 5 s on outputs 3/4, the master's pair, where a loud click
        // also plays.
        let tone: Vec<f32> = (0..240_000)
            .map(|i| {
                let amplitude = 10f64.powf(-23.0 / 20.0);
                (amplitude * (2.0 * std::f64::consts::PI * 1000.0 * i as f64 / 48_000.0).sin())
                    as f32
            })
            .collect();
        let mut b = GraphBuilder::new();
        b.add(NodeSpec {
            key: 1,
            label: String::new(),
            params: Arc::new(NodeParams::default()),
            clips: vec![ClipSource {
                data: Arc::new(AudioData::new(48_000, vec![tone])),
                start: 0,
                offset: 0,
                length: 240_000,
                gain: 1.0,
                fades: Default::default(),
            }],
            input: NodeInput::None,
            inserts: vec![],
            target: SpecTarget::Hardware(2),
        });
        let (mut core, mut handle) = new_engine(256);
        core.configure_io(0, 4);
        let click = ClickSettings {
            enabled: true,
            gain: 1.0,
            channel: 2,
            ..ClickSettings::default()
        };
        for c in [
            Command::SetGraph(Box::new(b.build(256))),
            Command::SetMetronome(click),
            Command::Play,
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        assert_eq!(
            handle.shared.loudness.read(),
            Loudness::default(),
            "nothing yet"
        );
        // Silenced: the device hears nothing, the meter still reads the tone.
        handle.shared.silent.store(true, Ordering::Relaxed);
        let mut out = vec![0.0f32; 480 * 4];
        for _ in 0..450 {
            core.process(&mut out, 4, None, 0);
        }
        assert!(
            out.iter().all(|s| *s == 0.0),
            "silent: the device gets zeros"
        );
        let l = handle.shared.loudness.read();
        let i = l.integrated.unwrap();
        assert!((i + 23.0).abs() < 0.1, "the tone without the click: {i}");
        assert!((l.momentary.unwrap() + 23.0).abs() < 0.1);
        assert!((l.seconds - 4.5).abs() < 0.02, "{}", l.seconds);

        assert!(handle.commands.push(Command::ResetLoudness).is_ok());
        core.process(&mut out, 4, None, 0);
        let l = handle.shared.loudness.read();
        assert_eq!(l.integrated, None, "reset");
        assert!(l.seconds < 0.02);
    }

    #[test]
    fn metronome_lines_up_with_compensated_audio_and_stays_out_of_exports() {
        use crate::graph::{ClipSource, DelayLine, Processor};
        use crate::media::AudioData;

        struct Lag(DelayLine);
        impl Processor for Lag {
            fn latency(&self) -> usize {
                self.0.delay()
            }
            fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
                self.0.process(io, frames);
            }
        }
        // An impulse on beat 2 (24000 at 120 BPM), on a track that gets 300 samples of delay
        // compensation from a silent slow track. The click plays on outputs 3/4.
        let mut impulse = vec![0.0f32; 48_000];
        impulse[24_000] = 1.0;
        let mut b = GraphBuilder::new();
        let spec = |key, inserts: Vec<Box<dyn Processor>>, clips| NodeSpec {
            key,
            label: String::new(),
            params: Arc::new(NodeParams::default()),
            clips,
            input: NodeInput::None,
            inserts,
            target: SpecTarget::Hardware(0),
        };
        b.add(spec(1, vec![Box::new(Lag(DelayLine::new(300, 2)))], vec![]));
        let clip = ClipSource {
            data: Arc::new(AudioData::new(48_000, vec![impulse])),
            start: 0,
            offset: 0,
            length: 48_000,
            gain: 1.0,
            fades: Default::default(),
        };
        b.add(spec(2, vec![], vec![clip]));

        let (mut core, mut handle) = new_engine(256);
        core.configure_io(0, 4);
        let click = ClickSettings {
            enabled: true,
            gain: 1.0,
            channel: 2,
            ..ClickSettings::default()
        };
        for c in [
            Command::SetGraph(Box::new(b.build(256))),
            Command::SetMetronome(click),
            Command::Play,
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        let mut out = vec![Vec::new(); 4];
        while out[0].len() < 30_000 {
            let mut buf = vec![0.0f32; 480 * 4];
            core.process(&mut buf, 4, None, 0);
            for frame in buf.chunks(4) {
                for (c, v) in frame.iter().enumerate() {
                    out[c].push(*v);
                }
            }
        }
        let hit = out[0].iter().position(|v| *v != 0.0).unwrap();
        assert_eq!(hit, 24_300, "the impulse arrives behind the compensation");
        // A click starts at zero; its onset is the sample before the first non-zero one.
        // Beat 1's click sounds at 300 and lasts 30 ms; look past it.
        let second_click = out[2][5000..].iter().position(|v| *v != 0.0).unwrap() + 5000 - 1;
        assert_eq!(
            second_click, hit,
            "the click on beat 2 sounds with the audio on beat 2"
        );
        // The mono impulse on left and right, and no click there.
        assert_eq!(out[0..2].iter().flatten().filter(|v| **v != 0.0).count(), 2);

        // Export renders through the same graph, without the click.
        let mut sink = vec![Vec::new(); 4];
        core.set_transport(true, 0);
        core.render(30_000, 256, &mut sink);
        assert!(sink[2].iter().chain(&sink[3]).all(|v| *v == 0.0));
        assert_eq!(sink[0].iter().filter(|v| **v != 0.0).count(), 1);
    }

    #[test]
    fn monitored_input_survives_a_loop_wrap_inside_a_buffer() {
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(1, 2);
        let params = Arc::new(NodeParams::default());
        params.monitoring.store(true, Ordering::Relaxed);
        let mut b = GraphBuilder::new();
        b.add(NodeSpec {
            key: 1,
            label: "guitar".into(),
            params,
            clips: vec![],
            input: NodeInput::Audio {
                channel: 0,
                stereo: false,
            },
            inserts: vec![],
            target: SpecTarget::Hardware(0),
        });
        // The loop end falls a few frames into each 100-frame callback, so its first chunk
        // is tiny; the input for the rest of the callback must not be thrown away as backlog.
        for c in [
            Command::SetGraph(Box::new(b.build(64))),
            Command::SetLoop(Some((0, 70))),
            Command::Locate(60),
            Command::Play,
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        let (mut tx, mut rx) = RingBuffer::<f32>::new(4096);
        let mut outputs = Vec::new();
        for _ in 0..12 {
            for _ in 0..100 {
                tx.push(0.5).unwrap();
            }
            let mut out = vec![0.0f32; 100 * 2];
            core.process(&mut out, 2, Some(&mut rx), 1);
            outputs.push(out);
        }
        // The fader ramps up during the first callback.
        for out in &outputs[1..] {
            assert!(out.chunks(2).all(|f| f[0] == 0.5), "{:?}", &out[..16]);
        }
        assert_eq!(core.shared().input_underruns.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn loop_applies_inside_it_and_while_recording_but_not_past_its_end() {
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(0, 2);
        let mut run = |commands: Vec<Command>| {
            for c in commands {
                assert!(handle.commands.push(c).is_ok());
            }
            let mut out = vec![0.0f32; 64 * 2 * 4];
            core.process(&mut out, 2, None, 0);
            core.position()
        };
        // Starting past the loop end plays on.
        let past = run(vec![
            Command::SetLoop(Some((0, 100))),
            Command::Locate(200),
            Command::Play,
        ]);
        assert_eq!(past, 456);
        // Inside it, the playhead wraps, here more than once a buffer: 50 to the end, two
        // whole passes, and 6 more.
        assert_eq!(run(vec![Command::Locate(50)]), 6);
        // An empty range is no loop.
        assert_eq!(
            run(vec![Command::SetLoop(Some((80, 80))), Command::Locate(50)]),
            306
        );
        // Recording loops as well (loop recording, 1.6): the same 6 as playback.
        let job = RecordJob {
            takes: Vec::new(),
            from: 0,
            to: u64::MAX,
            count_in_to: None,
            open: false,
            exact_from: false,
            start: None,
            dropped: 0,
            midi: None,
            midi_dropped: 0,
            wraps: 0,
        };
        assert_eq!(
            run(vec![
                Command::SetLoop(Some((0, 100))),
                Command::Locate(50),
                Command::StartRecording(Box::new(job)),
            ]),
            6
        );
    }

    /// While recording, incoming MIDI goes to the session stamped with the heard position:
    /// the playhead less the delay compensation, and through a loop wrap the loop's end for
    /// that long, with the wrap counted.
    #[test]
    fn recorded_midi_is_stamped_with_what_was_heard() {
        use crate::graph::{DelayLine, Processor};
        use vst3_host::MidiChannel;

        struct Lag(DelayLine);
        impl Processor for Lag {
            fn latency(&self) -> usize {
                self.0.delay()
            }
            fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
                self.0.process(io, frames);
            }
        }
        let mut b = GraphBuilder::new();
        b.add(NodeSpec {
            key: 1,
            label: String::new(),
            params: Arc::new(NodeParams::default()),
            clips: Vec::new(),
            input: NodeInput::None,
            inserts: vec![Box::new(Lag(DelayLine::new(300, 2)))],
            target: SpecTarget::Hardware(0),
        });
        let (mut core, mut handle) = new_engine(512);
        core.configure_io(0, 2);
        let (mut keyboard, port) = RingBuffer::new(64);
        let (recorded, mut midi) = RingBuffer::new(64);
        let job = RecordJob {
            takes: Vec::new(),
            from: 0,
            to: u64::MAX,
            count_in_to: None,
            open: false,
            exact_from: false,
            start: None,
            dropped: 0,
            midi: Some(recorded),
            midi_dropped: 0,
            wraps: 0,
        };
        for c in [
            Command::SetGraph(Box::new(b.build(512))),
            Command::SetMidiPorts(vec![port]),
            Command::SetLoop(Some((0, 4800))),
            Command::Locate(0),
            Command::StartRecording(Box::new(job)),
            Command::Play,
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        let key = MidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note: 60,
            velocity: 100,
        };
        // 480-sample callbacks: the loop is 10 of them. A key before blocks 0, 2, 10 (the
        // first after the wrap) and 11.
        for block in 0..12 {
            if [0, 2, 10, 11].contains(&block) {
                // No callback time: stamped at the block's heard position, as before 4.5.
                let timed = TimedMidi {
                    event: key,
                    at: Instant::now(),
                };
                assert!(keyboard.push(timed).is_ok());
            }
            let mut out = vec![0.0f32; 480 * 2];
            core.process(&mut out, 2, None, 0);
        }
        let mut stamps = Vec::new();
        while let Ok(m) = midi.pop() {
            assert_eq!(m.event, key);
            stamps.push((m.heard, m.wraps));
        }
        assert_eq!(stamps, [(-300, 0), (660, 0), (4500, 1), (180, 1)]);
    }

    /// Live MIDI plays one buffer after it came in, on its sample, and is recorded where it
    /// was heard: an event 5 ms before a 480-sample callback plays 240 into it; one older
    /// than a buffer plays at its start; one that came in during the callback, at its end.
    /// Across a mid-buffer loop wrap, an event plays in the chunk after the wrap.
    #[test]
    fn live_midi_plays_one_buffer_after_it_came_in() {
        use crate::graph::Processor;
        use std::sync::Mutex;
        use std::time::Duration;
        use vst3_host::MidiChannel;

        type Log = Arc<Mutex<Vec<(i64, MidiEvent)>>>;
        /// Logs each event at its absolute position: the block's plus its offset.
        struct Spy {
            pending: Vec<(MidiEvent, usize)>,
            log: Log,
        }
        impl Processor for Spy {
            fn latency(&self) -> usize {
                0
            }
            fn process(&mut self, _: &mut [Vec<f32>], frames: usize, transport: &Transport) {
                for (event, offset) in self.pending.drain(..) {
                    assert!(offset < frames);
                    self.log
                        .lock()
                        .unwrap()
                        .push((transport.position + offset as i64, event));
                }
            }
            fn send_midi_at(&mut self, event: MidiEvent, offset: usize) {
                self.pending.push((event, offset));
            }
        }
        let key = |note| MidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note,
            velocity: 100,
        };
        // One run: from `from`, with a 4800-sample loop, recording. `keys` are (callback,
        // how long before it the key came in, in µs (negative: after it started), pitch).
        let run = |from: u64, keys: &[(usize, i64, u8)]| -> (Vec<(i64, MidiEvent)>, Vec<i64>) {
            let log: Log = Arc::default();
            let params = Arc::new(NodeParams::default());
            params.monitoring.store(true, Ordering::Relaxed);
            let mut b = GraphBuilder::new();
            b.add(NodeSpec {
                key: 1,
                label: String::new(),
                params,
                clips: Vec::new(),
                input: NodeInput::Midi {
                    port: None,
                    channel: None,
                },
                inserts: vec![Box::new(Spy {
                    pending: Vec::new(),
                    log: log.clone(),
                })],
                target: SpecTarget::Hardware(0),
            });
            let (mut core, mut handle) = new_engine(512);
            core.configure_io(0, 2);
            core.shared().set_sample_rate(48_000);
            let (mut keyboard, port) = RingBuffer::new(64);
            let (recorded, mut midi) = RingBuffer::new(64);
            let job = RecordJob {
                takes: Vec::new(),
                from: 0,
                to: u64::MAX,
                count_in_to: None,
                open: false,
                exact_from: false,
                start: None,
                dropped: 0,
                midi: Some(recorded),
                midi_dropped: 0,
                wraps: 0,
            };
            for c in [
                Command::SetGraph(Box::new(b.build(512))),
                Command::SetMidiPorts(vec![port]),
                Command::SetLoop(Some((0, 4800))),
                Command::Locate(from),
                Command::StartRecording(Box::new(job)),
                Command::Play,
            ] {
                assert!(handle.commands.push(c).is_ok());
            }
            for callback in 0..4 {
                let now = Instant::now();
                for &(_, micros, pitch) in keys.iter().filter(|k| k.0 == callback) {
                    let at = if micros >= 0 {
                        now.checked_sub(Duration::from_micros(micros as u64))
                            .unwrap()
                    } else {
                        now + Duration::from_micros((-micros) as u64)
                    };
                    assert!(keyboard
                        .push(TimedMidi {
                            event: key(pitch),
                            at
                        })
                        .is_ok());
                }
                core.set_callback_time(now);
                let mut out = vec![0.0f32; 480 * 2];
                core.process(&mut out, 2, None, 0);
            }
            let played = log.lock().unwrap().clone();
            let mut stamps = Vec::new();
            while let Ok(m) = midi.pop() {
                stamps.push(m.heard);
            }
            (played, stamps)
        };

        // From 0, no delay compensation: callback 2 starts at 960.
        let (played, stamps) = run(0, &[(2, 5_000, 60), (2, 20_000, 62), (2, -1_000, 64)]);
        assert_eq!(
            played,
            [(960, key(62)), (960 + 240, key(60)), (960 + 479, key(64))],
            "older than a buffer: its start; 5 ms: 240 in; during the callback: its end"
        );
        assert_eq!(stamps, [960 - 240, 960 - 960, 960], "where each was heard");

        // From 4500, the loop wraps 300 into callback 0. A key 130 samples before it plays
        // 350 in: 50 after the wrap. It was heard before the wrap, at 4370.
        let (played, stamps) = run(4500, &[(0, 2_708, 72)]);
        assert_eq!(played, [(50, key(72))]);
        assert_eq!(stamps, [4500 - 130]);
    }

    /// A note preview reaches the named track's instrument, armed and monitoring or not, and no
    /// other track's.
    #[test]
    fn send_midi_reaches_only_its_tracks_instrument() {
        use crate::graph::Processor;
        use std::sync::Mutex;
        use vst3_host::MidiChannel;

        type Log = Arc<Mutex<Vec<MidiEvent>>>;
        struct Spy(Log);
        impl Processor for Spy {
            fn latency(&self) -> usize {
                0
            }
            fn process(&mut self, _: &mut [Vec<f32>], _: usize, _: &Transport) {}
            fn send_midi(&mut self, event: MidiEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let (a, b): (Log, Log) = Default::default();
        let mut builder = GraphBuilder::new();
        for (key, log) in [(1u64, &a), (2, &b)] {
            builder.add(NodeSpec {
                key,
                label: String::new(),
                params: Arc::new(NodeParams::default()),
                clips: Vec::new(),
                // Not listening to any MIDI input, and not monitoring.
                input: NodeInput::None,
                inserts: vec![Box::new(Spy(log.clone()))],
                target: SpecTarget::Hardware(0),
            });
        }
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(0, 2);
        let note = MidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note: 64,
            velocity: 90,
        };
        for c in [
            Command::SetGraph(Box::new(builder.build(64))),
            Command::SendMidi {
                track: 1,
                event: note,
            },
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        let mut out = vec![0.0f32; 64 * 2];
        core.process(&mut out, 2, None, 0);
        assert_eq!(*a.lock().unwrap(), [note]);
        assert!(b.lock().unwrap().is_empty());
    }

    /// A note dragged in the piano roll (20.3) as `Session::preview_note` and `preview_note_off` send
    /// it: struck, then (moved to another pitch) released and the new one struck in one block,
    /// then released; each reaches the track's instrument in that order, through the CPU
    /// panel's wrapper every insert has (`usage::Timed`).
    #[test]
    fn a_dragged_notes_on_and_off_reach_the_instrument_in_order() {
        use crate::graph::Processor;
        use std::sync::Mutex;
        use vst3_host::MidiChannel;

        type Log = Arc<Mutex<Vec<MidiEvent>>>;
        struct Spy(Log);
        impl Processor for Spy {
            fn latency(&self) -> usize {
                0
            }
            fn process(&mut self, _: &mut [Vec<f32>], _: usize, _: &Transport) {}
            fn send_midi(&mut self, event: MidiEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let log: Log = Default::default();
        let mut builder = GraphBuilder::new();
        builder.add(NodeSpec {
            key: 7,
            label: String::new(),
            params: Arc::new(NodeParams::default()),
            clips: Vec::new(),
            input: NodeInput::None,
            inserts: vec![Box::new(crate::usage::Timed::new(
                Box::new(Spy(log.clone())),
                Arc::new(crate::usage::Usage::default()),
            ))],
            target: SpecTarget::Hardware(0),
        });
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(0, 2);
        let on = |note| MidiEvent::NoteOn {
            channel: MidiChannel::Ch1,
            note,
            velocity: 90,
        };
        let off = |note| MidiEvent::NoteOff {
            channel: MidiChannel::Ch1,
            note,
            velocity: 0,
        };
        let mut out = vec![0.0f32; 64 * 2];
        assert!(handle
            .commands
            .push(Command::SetGraph(Box::new(builder.build(64))))
            .is_ok());
        for events in [vec![on(60)], vec![], vec![off(60), on(62)], vec![off(62)]] {
            for event in events {
                assert!(handle
                    .commands
                    .push(Command::SendMidi { track: 7, event })
                    .is_ok());
            }
            core.process(&mut out, 2, None, 0);
        }
        assert_eq!(*log.lock().unwrap(), [on(60), off(60), on(62), off(62)]);
    }

    #[test]
    fn plugins_follow_play_stop_locate_and_tempo() {
        let (processor, seen) = crate::graph::tests::spy(0);
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(0, 2);
        let mut b = GraphBuilder::new();
        b.add(NodeSpec {
            key: 1,
            label: "t".into(),
            params: Arc::new(NodeParams::default()),
            clips: vec![],
            input: NodeInput::None,
            inserts: vec![processor],
            target: SpecTarget::Hardware(0),
        });
        let waltz = TempoMap::constant(44_100, 90.0, 3, 4);
        let mut block = |commands: Vec<Command>| {
            for c in commands {
                assert!(handle.commands.push(c).is_ok());
            }
            let mut out = vec![0.0f32; 64 * 2];
            core.process(&mut out, 2, None, 0);
            *seen.lock().unwrap().last().unwrap()
        };
        let graph = Command::SetGraph(Box::new(b.build(64)));
        let stopped = block(vec![graph]);
        assert!(!stopped.playing);
        assert_eq!(stopped.position, 0);

        let started = block(vec![
            Command::SetTempoMap(Box::new(waltz.clone())),
            Command::Locate(1000),
            Command::Play,
        ]);
        assert!(started.playing);
        assert_eq!(
            started.position, 1000,
            "the first block starts at the locate point"
        );
        assert_eq!(started.meter, waltz.meter_at(1000));
        assert_eq!(block(vec![]).position, 1064);

        let halted = block(vec![Command::Stop]);
        assert!(!halted.playing);
        assert_eq!(halted.position, 1128);
        assert_eq!(block(vec![]).position, 1128, "a stopped playhead stays put");
    }

    #[test]
    fn blocks_split_at_tempo_changes() {
        let (processor, seen) = crate::graph::tests::spy(0);
        let (mut core, mut handle) = new_engine(64);
        core.configure_io(0, 2);
        let mut b = GraphBuilder::new();
        b.add(NodeSpec {
            key: 1,
            label: "t".into(),
            params: Arc::new(NodeParams::default()),
            clips: vec![],
            input: NodeInput::None,
            inserts: vec![processor],
            target: SpecTarget::Hardware(0),
        });
        // 120 for one 4/4 bar (96000 samples), then 60.
        let mut project = crate::project::Project::default();
        project.tempo_changes.push(crate::project::TempoChange {
            bar: 1,
            beat: 0,
            tempo: Some(60.0),
            time_signature: None,
        });
        let map = TempoMap::of(&project, 48_000);
        for c in [
            Command::SetGraph(Box::new(b.build(64))),
            Command::SetTempoMap(Box::new(map)),
            Command::Locate(95_900),
            Command::Play,
        ] {
            assert!(handle.commands.push(c).is_ok());
        }
        let mut out = vec![0.0f32; 100 * 2];
        for _ in 0..3 {
            core.process(&mut out, 2, None, 0);
        }
        let seen = seen.lock().unwrap();
        let change = seen.iter().position(|t| t.meter.tempo == 60.0).unwrap();
        assert_eq!(
            seen[change].position, 96_000,
            "a block starts on the change"
        );
        assert_eq!(seen[change - 1].meter.tempo, 120.0);
        assert!(
            (seen[change].quarter_notes() - 4.0).abs() < 1e-9,
            "one 4/4 bar in"
        );
    }

    #[test]
    fn graph_swaps_through_commands_keep_playing_seamlessly() {
        use crate::graph::DelayLine;
        use crate::media::AudioData;

        /// A track playing a ramp, behind a 300-sample "plugin" delay into a master.
        fn graph() -> Graph {
            struct Lag(DelayLine);
            impl crate::graph::Processor for Lag {
                fn latency(&self) -> usize {
                    self.0.delay()
                }
                fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
                    self.0.process(io, frames);
                }
            }
            let ramp: Vec<f32> = (0..20_000).map(|i| (i % 1000) as f32 / 1000.0).collect();
            let mut b = GraphBuilder::new();
            let spec = |key, inserts: Vec<Box<dyn crate::graph::Processor>>, clips| NodeSpec {
                key,
                label: String::new(),
                params: Arc::new(NodeParams::default()),
                clips,
                input: NodeInput::None,
                inserts,
                target: SpecTarget::Hardware(0),
            };
            let master = b.add(spec(9, vec![], vec![]));
            let clip = crate::graph::ClipSource {
                data: Arc::new(AudioData::new(48_000, vec![ramp])),
                start: 0,
                offset: 0,
                length: 20_000,
                gain: 1.0,
                fades: Default::default(),
            };
            // The silent "slow" track forces 300 samples of compensation onto the clip track,
            // so that track's delay line holds audio a swap must not lose.
            let slow = b.add(spec(1, vec![Box::new(Lag(DelayLine::new(300, 2)))], vec![]));
            let fast = b.add(spec(2, vec![], vec![clip]));
            b.set_target(slow, SpecTarget::Spec(master));
            b.set_target(fast, SpecTarget::Spec(master));
            b.build(256)
        }

        let run = |swaps: bool| {
            let (mut core, mut handle) = new_engine(256);
            core.configure_io(0, 2);
            let mut keys = Vec::new();
            let send = |handle: &mut EngineHandle, keys: &mut Vec<u64>| {
                let mut g = graph();
                g.link_previous(keys);
                *keys = g.keys();
                assert!(handle.commands.push(Command::SetGraph(Box::new(g))).is_ok());
            };
            send(&mut handle, &mut keys);
            assert!(handle.commands.push(Command::Play).is_ok());
            let mut out = Vec::new();
            for k in 0..60 {
                if swaps && k % 5 == 2 {
                    send(&mut handle, &mut keys);
                }
                let mut buf = vec![0.0f32; 256 * 2];
                core.process(&mut buf, 2, None, 0);
                out.extend(buf);
                while handle.garbage.pop().is_ok() {}
            }
            out
        };
        let steady = run(false);
        assert!(steady.iter().any(|s| *s > 0.5));
        let swapped = run(true);
        assert!(
            steady
                .iter()
                .zip(&swapped)
                .all(|(a, b)| a.to_bits() == b.to_bits()),
            "a graph swap changed the output"
        );
    }
}

#[cfg(test)]
mod gathering_tests;
