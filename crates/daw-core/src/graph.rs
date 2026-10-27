//! The compiled routing graph the audio thread runs.
//!
//! A [`Graph`] is built on the UI thread from the project (see [`GraphBuilder`]) and handed
//! to the engine whole. Each node has a main output and any number of sends (pre- or
//! post-fader), each an edge to another node or to a hardware output pair.
//!
//! Nodes are stored in processing order: every node comes after all the nodes that feed it.
//! They are grouped into levels by their longest path to an output; the nodes of a level are
//! independent, so they run in parallel on the [`WorkerPool`].
//!
//! Plugin delay compensation is solved at build time. Every edge gets its own delay line, so
//! that every signal arrives at a bus, and at the hardware outputs, at the same moment, even
//! when one node's main output and its sends go to places that hear their inputs at
//! different times.

use crate::automation::AutomationLane;
use crate::media::AudioData;
use crate::pool::WorkerPool;
use crate::project::Fade;
use crate::sequencer::{Sequence, SequencePlayer};
use crate::tempo::TICKS_PER_QUARTER;
use crate::trace;
use crate::util::{db_to_gain, AtomicF32};
use std::any::Any;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::time::Instant;
use vst3_host::MidiEvent;

/// Everything inside the mixer is stereo.
pub const CHANNELS: usize = 2;

/// The tempo and meter in force for a block: one segment of the tempo map
/// ([`crate::tempo::TempoMap::meter_at`]), and what turns its sample positions into musical
/// time.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Meter {
    pub sample_rate: u32,
    /// Quarter notes per minute, which is also what VST3 means by tempo.
    pub tempo: f64,
    pub numerator: u32,
    pub denominator: u32,
    /// Where the segment starts, in samples, and in quarter notes from the timeline start.
    pub origin_sample: f64,
    pub origin_quarter_notes: f64,
}

impl Default for Meter {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            tempo: 120.0,
            numerator: 4,
            denominator: 4,
            origin_sample: 0.0,
            origin_quarter_notes: 0.0,
        }
    }
}

impl Meter {
    /// Quarter notes from the timeline start to `position` (samples; negative before it).
    /// Extrapolated from this segment: a processor whose latency puts its audio a few samples
    /// before a tempo change uses the new slope for them (TD30).
    pub fn quarter_notes(&self, position: i64) -> f64 {
        self.origin_quarter_notes
            + (position as f64 - self.origin_sample) / f64::from(self.sample_rate.max(1))
                * self.tempo
                / 60.0
    }

    /// Where `quarter_notes` falls, in samples (fractional), by this segment's tempo: the
    /// inverse of [`Self::quarter_notes`]. The sequencer uses it on the audio thread, where the
    /// engine splits blocks at tempo changes so each chunk lies inside one segment.
    pub fn sample_at_quarter_notes(&self, quarter_notes: f64) -> f64 {
        self.origin_sample
            + (quarter_notes - self.origin_quarter_notes) * 60.0 / self.tempo
                * f64::from(self.sample_rate.max(1))
    }
}

/// The transport as one processor sees it for one block.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transport {
    pub playing: bool,
    /// Timeline position of the block's first sample, in samples. For a processor this is the
    /// position of the audio it is processing: behind the playhead by the delay-compensation
    /// and plugin latency in front of it, so it can be negative near the start.
    pub position: i64,
    pub meter: Meter,
}

impl Transport {
    /// Playing from `position`, at the default tempo and meter.
    pub fn playing_at(position: u64) -> Self {
        Self {
            playing: true,
            position: position as i64,
            meter: Meter::default(),
        }
    }

    /// The same transport for audio `samples` older.
    pub fn earlier(self, samples: usize) -> Self {
        Self {
            position: self.position - samples as i64,
            ..self
        }
    }

    pub fn quarter_notes(&self) -> f64 {
        self.meter.quarter_notes(self.position)
    }
}

/// A plugin (or test stand-in) in a node's insert chain.
pub trait Processor: Send {
    /// Reported processing latency in samples.
    fn latency(&self) -> usize;
    /// Process `frames` samples of `io` in place. `io` has [`CHANNELS`] buffers at least
    /// `frames` long. `transport` describes the audio in `io`.
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize, transport: &Transport);
    /// Deliver a MIDI event at the start of the next block.
    fn send_midi(&mut self, _event: MidiEvent) {}
    /// Deliver a MIDI event `offset` samples into the next block (for MIDI clips). Events
    /// arrive in time order.
    fn send_midi_at(&mut self, event: MidiEvent, _offset: usize) {
        self.send_midi(event);
    }
    /// Move parameter `id` to `value` (normalized, 0..1) `offset` samples into the next block
    /// (automation). Several points in one block are a line the plugin follows.
    fn set_parameter_at(&mut self, _id: u32, _value: f64, _offset: usize) {}
    /// Whether a moving lane should also send its next grid line past the block (an `offset`
    /// at or after `frames`), so the line toward it can be drawn whole: what makes a native
    /// effect's automation the same at any block size. VST3 plug-ins take offsets inside the
    /// block only.
    fn wants_lookahead(&self) -> bool {
        false
    }
    /// What the last [`Self::process`] wrote to the plug-in's extra output `bus` (1 and up:
    /// a multi-out instrument's kick, snare…), channel by channel; `None` when it has none or
    /// produced nothing this block (bypassed, skipped), which a listening track hears as
    /// silence.
    fn output_bus(&self, _bus: usize) -> Option<&[Vec<f32>]> {
        None
    }
    /// For handing state between graphs (see [`Graph::take_state_from`]).
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        None
    }
    /// Take over the running state of `old` if it is the same processor in the previous graph
    /// (for a plugin: the same instance). Returns whether it was. Must not allocate.
    fn take_state_from(&mut self, _old: &mut dyn Processor) -> bool {
        false
    }
}

/// Mixer state shared between a track's UI and its graph node, so fader moves never
/// require rebuilding the graph.
#[derive(Debug)]
pub struct NodeParams {
    pub gain_db: AtomicF32,
    pub pan: AtomicF32,
    /// Mute after solo is resolved; computed on the UI thread. Silences the sends too.
    pub muted: AtomicBool,
    /// Silence the main output only, keeping the sends: solo in place for a track heard only
    /// through a soloed bus it sends to.
    pub main_muted: AtomicBool,
    /// Feed the live input (audio or MIDI) through the track.
    pub monitoring: AtomicBool,
    pub peak: [AtomicF32; CHANNELS],
    /// Targets being written (automation write modes, [WRITE_VOLUME] and the rest): the
    /// values above are the control's, and win over the target's lane.
    pub writing: AtomicU8,
    /// The node's processing time, for the CPU panel (`crate::usage`).
    pub usage: crate::usage::Usage,
}

/// [NodeParams::writing] bits.
pub const WRITE_VOLUME: u8 = 1;
pub const WRITE_PAN: u8 = 2;
pub const WRITE_MUTE: u8 = 4;

impl Default for NodeParams {
    fn default() -> Self {
        Self {
            gain_db: AtomicF32::new(0.0),
            pan: AtomicF32::new(0.0),
            muted: AtomicBool::new(false),
            main_muted: AtomicBool::new(false),
            monitoring: AtomicBool::new(false),
            peak: Default::default(),
            writing: AtomicU8::new(0),
            usage: Default::default(),
        }
    }
}

impl NodeParams {
    /// Left/right gains for the current fader, pan (balance law) and mute.
    fn target_gains(&self) -> [f32; CHANNELS] {
        if self.muted.load(Ordering::Relaxed) {
            return [0.0; CHANNELS];
        }
        balance(self.gain_db.load(), self.pan.load())
    }
}

/// Left/right gains for a fader level and a pan, by the balance law.
fn balance(gain_db: f32, pan: f32) -> [f32; CHANNELS] {
    let gain = db_to_gain(gain_db);
    let pan = pan.clamp(-1.0, 1.0);
    [gain * (1.0 - pan).min(1.0), gain * (1.0 + pan).min(1.0)]
}

/// Automation is evaluated on this grid of the timeline, in samples, and moves in a straight
/// line from one grid point to the next. The grid is anchored to the timeline, not to blocks,
/// so what a sample gets depends only on its position: playback at any buffer size and export
/// give the same audio, and nothing has to be carried across blocks or graph swaps. A jump
/// (or a mute) takes up to one grid step, ending on the grid line at or after its point.
pub const AUTOMATION_GRID: i64 = 32;

/// A lane's value at timeline position `position` (samples), through `meter`.
fn lane_value(lane: &AutomationLane, meter: &Meter, position: i64) -> f64 {
    let tick = meter.quarter_notes(position) * f64::from(TICKS_PER_QUARTER);
    lane.value_at(tick).unwrap_or(0.0)
}

/// Call `apply(k, value)` for each of the `frames` samples from timeline position `start`,
/// with `value` interpolated between `value_at` on the [`AUTOMATION_GRID`] either side.
fn on_grid<const N: usize>(
    start: i64,
    frames: usize,
    mut value_at: impl FnMut(i64) -> [f32; N],
    mut apply: impl FnMut(usize, [f32; N]),
) {
    let mut cell = start.div_euclid(AUTOMATION_GRID) * AUTOMATION_GRID;
    let mut a = value_at(cell);
    let mut k = 0;
    while k < frames {
        let b = value_at(cell + AUTOMATION_GRID);
        let end = usize::try_from(cell + AUTOMATION_GRID - start)
            .unwrap_or(0)
            .min(frames);
        for i in k..end {
            let t = (start + i as i64 - cell) as f32 / AUTOMATION_GRID as f32;
            apply(i, std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t));
        }
        k = end;
        cell += AUTOMATION_GRID;
        a = b;
    }
}

/// A node's automation as the audio thread plays it: the project's enabled lanes with points,
/// built with the graph. Send-level lanes sit on their [`NodeSend`].
#[derive(Default)]
pub struct NodeAutomation {
    /// The fader, in dB.
    pub volume: Option<AutomationLane>,
    pub pan: Option<AutomationLane>,
    /// Muted where its value is 0.5 or more.
    pub mute: Option<AutomationLane>,
    pub parameters: Vec<ParameterLane>,
}

/// A lane moving one parameter of one insert.
pub struct ParameterLane {
    /// Index in the node's insert chain.
    pub insert: usize,
    /// The plugin's parameter id.
    pub parameter: u32,
    pub lane: AutomationLane,
    /// Set while the parameter is being written (its editor knob held in a write mode): the
    /// lane sends nothing, so the knob is what plays. Shared with the session.
    pub writing: Arc<AtomicBool>,
}

impl NodeAutomation {
    /// Whether the fader stage (volume, pan or mute) is automated.
    fn mixes(&self) -> bool {
        self.volume.is_some() || self.pan.is_some() || self.mute.is_some()
    }

    /// Silent at `position`: muted by the mixer (solo, or the button without a mute lane), or
    /// by the mute lane.
    fn muted_at(&self, params: &NodeParams, meter: &Meter, position: i64) -> bool {
        params.muted.load(Ordering::Relaxed)
            || self
                .lane(&self.mute, params, WRITE_MUTE)
                .is_some_and(|l| lane_value(l, meter, position) >= 0.5)
    }

    /// `lane`, unless its target (`bit`) is being written: then the control plays.
    fn lane<'a>(
        &self,
        lane: &'a Option<AutomationLane>,
        params: &NodeParams,
        bit: u8,
    ) -> Option<&'a AutomationLane> {
        let writing = params.writing.load(Ordering::Relaxed) & bit != 0;
        lane.as_ref().filter(|_| !writing)
    }

    /// Left/right fader gains at `position`, from the lanes where there are some and the
    /// mixer where not.
    fn gains_at(&self, params: &NodeParams, meter: &Meter, position: i64) -> [f32; CHANNELS] {
        if self.muted_at(params, meter, position) {
            return [0.0; CHANNELS];
        }
        let value = |lane: &Option<AutomationLane>, fixed: &AtomicF32, bit: u8| {
            self.lane(lane, params, bit)
                .map_or(fixed.load(), |l| lane_value(l, meter, position) as f32)
        };
        balance(
            value(&self.volume, &params.gain_db, WRITE_VOLUME),
            value(&self.pan, &params.pan, WRITE_PAN),
        )
    }
}

impl ParameterLane {
    /// Queue this block's points on `insert`: the value at the block's start every block (so a
    /// knob turned by hand, or a locate, is overruled at once), and while the lane moves, one
    /// on every grid line inside the block, for the plugin to draw lines between. Points are
    /// never thinned: a plugin interpolates between them, so a held value needs its ends.
    fn send(&self, insert: &mut dyn Processor, at: &Transport, frames: usize) {
        if self.writing.load(Ordering::Relaxed) {
            return;
        }
        let value = |p: i64| {
            let v = lane_value(&self.lane, &at.meter, p);
            if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            }
        };
        let start = at.position;
        let end = start + frames as i64;
        let first = value(start);
        insert.set_parameter_at(self.parameter, first, 0);
        let first_line = start.div_euclid(AUTOMATION_GRID) * AUTOMATION_GRID + AUTOMATION_GRID;
        let lines = (first_line..end).step_by(AUTOMATION_GRID as usize);
        if lines.clone().chain([end]).all(|p| value(p) == first) {
            return;
        }
        for p in lines {
            insert.set_parameter_at(self.parameter, value(p), (p - start) as usize);
        }
        if insert.wants_lookahead() {
            // The first grid line at or after the block's end.
            let ahead = (end + AUTOMATION_GRID - 1).div_euclid(AUTOMATION_GRID) * AUTOMATION_GRID;
            insert.set_parameter_at(self.parameter, value(ahead), (ahead - start) as usize);
        }
    }
}

/// A clip as the audio thread sees it.
pub struct ClipSource {
    pub data: Arc<AudioData>,
    pub start: u64,
    pub offset: u64,
    pub length: u64,
    pub gain: f32,
    /// Fade in and fade out, in timeline samples from each end.
    pub fades: (Fade, Fade),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeInput {
    None,
    Audio {
        channel: usize,
        stereo: bool,
    },
    /// `port` indexes the engine's open MIDI ports; `None` accepts every port.
    Midi {
        port: Option<usize>,
        channel: Option<u8>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// Mixed into another node (by index in processing order).
    Node(usize),
    /// Written to the hardware output pair starting at this channel.
    Hardware(usize),
    /// Dropped: a render needs an instrument's extra outputs but not its own main output.
    Nowhere,
}

/// Where along a node's chain a send takes its signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tap {
    /// After the inserts, before the fader and pan: the send level ignores the fader.
    PreFader,
    /// After the fader and pan, like the main output.
    PostFader,
    /// A plug-in's extra output `bus` (a multi-out instrument's kick, snare...), right after
    /// insert `insert` (counting the node's inserts as built) processed: at unity, whatever
    /// the node's fader, mute or later inserts, since it is a track of its own. Waits only
    /// for the latency in front of it.
    Output { insert: usize, bus: usize },
}

/// A send's level, shared between the UI and the graph like [`NodeParams`], so moving it
/// never requires a rebuild.
#[derive(Debug)]
pub struct SendParams {
    pub gain_db: AtomicF32,
    pub muted: AtomicBool,
    /// The level is being written (automation write modes): `gain_db` wins over the lane.
    pub writing: AtomicBool,
}

impl Default for SendParams {
    fn default() -> Self {
        Self {
            gain_db: AtomicF32::new(0.0),
            muted: AtomicBool::new(false),
            writing: AtomicBool::new(false),
        }
    }
}

/// A send as the audio thread runs it: an extra output with its own level, buffer and delay
/// compensation.
pub struct NodeSend {
    /// Stable identity within its node across rebuilds (the send id), used to hand state over.
    pub key: u64,
    pub target: Target,
    pub tap: Tap,
    pub params: Arc<SendParams>,
    /// Delay compensation for this edge alone.
    pub delay: DelayLine,
    /// Automation of the send's level, in dB.
    pub lane: Option<AutomationLane>,
    /// The gain reached at the end of the last block, ramped from like the fader.
    gain: f32,
    buf: Vec<Vec<f32>>,
}

impl NodeSend {
    /// The level to ramp to. A muted node (mute after solo) silences its sends too, pre-fader
    /// ones included, so muting a track never leaves its reverb playing.
    fn target_gain(&self, node_muted: bool) -> f32 {
        if node_muted || self.params.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            db_to_gain(self.params.gain_db.load())
        }
    }

    /// Fill the send's buffer from `from` at its level, ramping from the last block's.
    fn take(&mut self, from: &[Vec<f32>], frames: usize, node_muted: bool) {
        let target = self.target_gain(node_muted);
        let step = (target - self.gain) / frames.max(1) as f32;
        for (dst, src) in self.buf.iter_mut().zip(from) {
            for (k, (d, s)) in dst[..frames].iter_mut().zip(&src[..frames]).enumerate() {
                *d = *s * (self.gain + step * (k + 1) as f32);
            }
        }
        self.gain = target;
    }

    /// [`Self::take`] when the level or the node's mute is automated: the gain follows the
    /// automation grid at the timeline position `at` of the audio in `from`.
    fn take_automated(
        &mut self,
        from: &[Vec<f32>],
        frames: usize,
        at: &Transport,
        node: (&NodeAutomation, &NodeParams),
    ) {
        let (automation, node_params) = node;
        let gain_at = |p: i64| {
            if self.params.muted.load(Ordering::Relaxed)
                || automation.muted_at(node_params, &at.meter, p)
            {
                return [0.0];
            }
            // While the level is written, the control plays, not the lane.
            let db = self
                .lane
                .as_ref()
                .filter(|_| !self.params.writing.load(Ordering::Relaxed))
                .map_or(self.params.gain_db.load(), |l| {
                    lane_value(l, &at.meter, p) as f32
                });
            [db_to_gain(db)]
        };
        let buf = &mut self.buf;
        on_grid(at.position, frames, &gain_at, |k, [g]| {
            for (dst, src) in buf.iter_mut().zip(from) {
                dst[k] = src[k] * g;
            }
        });
        self.gain = gain_at(at.position + frames as i64)[0];
    }
}

/// Fixed delay per channel, used for delay compensation.
pub struct DelayLine {
    buffers: Vec<Vec<f32>>,
    pos: usize,
}

impl DelayLine {
    pub fn new(delay: usize, channels: usize) -> Self {
        Self {
            buffers: vec![vec![0.0; delay]; channels],
            pos: 0,
        }
    }

    pub fn delay(&self) -> usize {
        self.buffers.first().map_or(0, Vec::len)
    }

    pub fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let delay = self.delay();
        if delay == 0 {
            return;
        }
        for (ring, buf) in self.buffers.iter_mut().zip(io.iter_mut()) {
            let mut pos = self.pos;
            for s in &mut buf[..frames] {
                std::mem::swap(s, &mut ring[pos]);
                pos += 1;
                if pos == delay {
                    pos = 0;
                }
            }
        }
        self.pos = (self.pos + frames) % delay;
    }

    pub fn clear(&mut self) {
        self.buffers.iter_mut().for_each(|b| b.fill(0.0));
    }
}

pub struct Node {
    /// Stable identity across rebuilds (the track id), used to hand state over.
    pub key: u64,
    pub label: String,
    pub params: Arc<NodeParams>,
    pub clips: Vec<ClipSource>,
    pub input: NodeInput,
    pub inserts: Vec<Box<dyn Processor>>,
    /// Instrument tracks: their MIDI clips, played into the first insert.
    pub sequence: Option<SequencePlayer>,
    pub automation: NodeAutomation,
    /// The main output, post-fader.
    pub target: Target,
    /// Delay compensation for the main output.
    pub delay: DelayLine,
    pub sends: Vec<NodeSend>,
    /// How far behind the playhead the node's input is, in samples: the compensation its
    /// sources went through on the way in. Zero for a track; a bus hears its inputs late.
    pub offset: usize,
    buf: Vec<Vec<f32>>,
    gains: [f32; CHANNELS],
    /// The main output's own gain at the end of the last block (1, or 0 while
    /// [`NodeParams::main_muted`]), ramped between like the fader.
    main_gain: f32,
    /// Recent processing time in seconds (smoothed).
    cost: f32,
}

impl Node {
    pub fn latency(&self) -> usize {
        self.inserts.iter().map(|p| p.latency()).sum()
    }
}

/// Per-block data the engine hands to [`Graph::process`].
pub struct BlockContext<'a> {
    pub frames: usize,
    /// The playhead. Clips play only while it plays; plugins see it either way.
    pub transport: Transport,
    /// Hardware input, planar, at least `frames` long per channel.
    pub input: &'a [Vec<f32>],
    /// Hardware output, planar, zeroed by the caller.
    pub output: &'a mut [Vec<f32>],
    /// Live MIDI for this block, in time order.
    pub midi: &'a [LiveMidi],
}

/// A live MIDI event (from a keyboard) for one block.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LiveMidi {
    /// Index of the MIDI input port it came from.
    pub port: usize,
    pub event: MidiEvent,
    /// Where in the block it plays, in samples.
    pub offset: usize,
}

/// A level whose nodes took less than this in total (seconds per block) runs on the audio
/// thread alone: handing it out would cost more than it saves, and keeping the workers idle
/// lets them park instead of spinning through a session with nothing heavy in it.
const MIN_PARALLEL_COST: f32 = 20e-6;

/// Nodes `start..end` share a depth: none feeds another, so they can run in parallel.
struct Level {
    start: usize,
    end: usize,
    /// The level's node indices, heaviest first, so the slowest tracks start earliest.
    order: Vec<usize>,
}

pub struct Graph {
    nodes: Vec<Node>,
    levels: Vec<Level>,
    max_block: usize,
    latency: usize,
    /// For each node, the index of the same node (by key) in the graph this one replaces.
    inherit: Vec<Option<usize>>,
}

/// What every node reads during a block.
struct Inputs<'a> {
    frames: usize,
    transport: Transport,
    input: &'a [Vec<f32>],
    midi: &'a [LiveMidi],
}

/// Lets pool tasks reach the node array. Each task index maps to a distinct node, so no two
/// threads ever touch the same node.
struct NodesPtr(*mut Node);
// SAFETY: see above; `Node` itself is `Send`.
unsafe impl Sync for NodesPtr {}

impl NodesPtr {
    /// Going through a method makes closures capture the whole (`Sync`) wrapper.
    fn at(&self, index: usize) -> *mut Node {
        self.0.wrapping_add(index)
    }
}

impl Graph {
    /// A graph that outputs silence.
    pub fn empty(max_block: usize) -> Self {
        Self {
            nodes: Vec::new(),
            levels: Vec::new(),
            max_block,
            latency: 0,
            inherit: Vec::new(),
        }
    }

    /// Node keys in processing order; pass them to the next graph's [`Self::link_previous`].
    pub fn keys(&self) -> Vec<u64> {
        self.nodes.iter().map(|n| n.key).collect()
    }

    /// Record where each node sat in the graph this one will replace (whose keys are
    /// `previous`), so the swap on the audio thread needs no searching. UI thread.
    pub fn link_previous(&mut self, previous: &[u64]) {
        let index: std::collections::HashMap<u64, usize> =
            previous.iter().enumerate().map(|(i, k)| (*k, i)).collect();
        self.inherit = self
            .nodes
            .iter()
            .map(|n| index.get(&n.key).copied())
            .collect();
    }

    /// Continue where `old` left off: take over each surviving node's fader ramp, delay-line
    /// contents and plugin-insert state, so a rebuild during playback is inaudible. A node
    /// whose compensation delay changed starts its delay line empty. Audio thread;
    /// allocation-free (buffers are swapped, not copied).
    pub fn take_state_from(&mut self, old: &mut Graph) {
        for (i, node) in self.nodes.iter_mut().enumerate() {
            let Some(prev) = self
                .inherit
                .get(i)
                .copied()
                .flatten()
                .and_then(|j| old.nodes.get_mut(j))
            else {
                continue;
            };
            if prev.key != node.key {
                continue;
            }
            node.gains = prev.gains;
            node.main_gain = prev.main_gain;
            node.cost = prev.cost;
            if node.delay.delay() == prev.delay.delay() {
                std::mem::swap(&mut node.delay, &mut prev.delay);
            }
            for insert in &mut node.inserts {
                for old_insert in &mut prev.inserts {
                    if insert.take_state_from(old_insert.as_mut()) {
                        break;
                    }
                }
            }
            if let (Some(player), Some(old)) = (&mut node.sequence, &mut prev.sequence) {
                player.take_state_from(old);
            }
            for send in &mut node.sends {
                let Some(old) = prev.sends.iter_mut().find(|s| s.key == send.key) else {
                    continue;
                };
                send.gain = old.gain;
                if send.delay.delay() == old.delay.delay() {
                    std::mem::swap(&mut send.delay, &mut old.delay);
                }
            }
        }
    }

    pub fn max_block(&self) -> usize {
        self.max_block
    }

    /// Total delay from the timeline to the hardware outputs, in samples.
    pub fn latency(&self) -> usize {
        self.latency
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    /// Play `event` on the first insert of the node with `key` (a track's instrument) at the
    /// start of its next block, whatever the track's input, arming or monitoring: previewing a
    /// note from the editor. Audio thread; allocation-free.
    pub fn send_midi_to(&mut self, key: u64, event: MidiEvent) {
        if let Some(instrument) = self
            .nodes
            .iter_mut()
            .find(|n| n.key == key)
            .and_then(|n| n.inserts.first_mut())
        {
            instrument.send_midi(event);
        }
    }

    /// Clear delay lines, e.g. after a locate, so stale audio is not replayed.
    pub fn reset(&mut self) {
        for node in &mut self.nodes {
            node.delay.clear();
            for send in &mut node.sends {
                send.delay.clear();
            }
        }
    }

    /// Run one block on the calling thread. Allocation-free.
    pub fn process(&mut self, ctx: &mut BlockContext) {
        self.process_with(ctx, None);
    }

    /// Run one block, spreading each level's nodes across `pool` when there is one.
    /// Allocation-free. The result is bit-identical with or without a pool: nodes only write
    /// their own buffers in parallel, and are summed into buses and outputs afterwards in a
    /// fixed order.
    pub fn process_with(&mut self, ctx: &mut BlockContext, pool: Option<&WorkerPool>) {
        let frames = ctx.frames.min(self.max_block);
        for node in &mut self.nodes {
            for c in &mut node.buf {
                c[..frames].fill(0.0);
            }
        }
        let inputs = Inputs {
            frames,
            transport: ctx.transport,
            input: ctx.input,
            midi: ctx.midi,
        };
        let nodes = &mut self.nodes;
        let tracing = trace::on();
        for level in &mut self.levels {
            let cost: f32 = level.order.iter().map(|&i| nodes[i].cost).sum();
            match pool {
                Some(pool)
                    if level.order.len() > 1 && pool.workers() > 0 && cost > MIN_PARALLEL_COST =>
                {
                    level
                        .order
                        .sort_unstable_by(|a, b| nodes[*b].cost.total_cmp(&nodes[*a].cost));
                    let ptr = NodesPtr(nodes.as_mut_ptr());
                    let order = &level.order;
                    let traced = tracing
                        .then(|| trace::level_open(order.iter().copied()))
                        .flatten();
                    pool.run(order.len(), &|k| {
                        let begun = traced.map(|_| trace::task_begin());
                        // SAFETY: `order` holds distinct indices into `nodes`, and each task
                        // index is claimed by exactly one thread.
                        let node = unsafe { &mut *ptr.at(order[k]) };
                        render_node(node, &inputs);
                        if let (Some(level), Some(begun)) = (traced, begun) {
                            trace::task_end(level, k, begun);
                        }
                    });
                    if let Some(level) = traced {
                        trace::level_close(level);
                    }
                }
                _ => {
                    let traced = tracing
                        .then(|| trace::level_open(level.start..level.end))
                        .flatten();
                    for (k, node) in nodes[level.start..level.end].iter_mut().enumerate() {
                        let begun = traced.map(|_| trace::task_begin());
                        render_node(node, &inputs);
                        if let (Some(level), Some(begun)) = (traced, begun) {
                            trace::task_end(level, k, begun);
                        }
                    }
                    if let Some(level) = traced {
                        trace::level_close(level);
                    }
                }
            }
            // Sum in a fixed order (node by node; main output, then sends as added), whatever
            // order the nodes ran in, so the mix is bit-identical with or without a pool.
            for i in level.start..level.end {
                let (head, tail) = nodes.split_at_mut(i + 1);
                let node = &head[i];
                mix_into(node.target, &node.buf, i, tail, ctx.output, frames);
                for send in &node.sends {
                    mix_into(send.target, &send.buf, i, tail, ctx.output, frames);
                }
            }
        }
    }
}

/// Add node `from`'s output `src` into its target. `tail` holds the nodes after `from`: a
/// target is always shallower than its source, so it comes later in processing order.
fn mix_into(
    target: Target,
    src: &[Vec<f32>],
    from: usize,
    tail: &mut [Node],
    output: &mut [Vec<f32>],
    frames: usize,
) {
    match target {
        Target::Node(j) => {
            let dest = &mut tail[j - from - 1];
            for (d, s) in dest.buf.iter_mut().zip(src) {
                add(&mut d[..frames], &s[..frames]);
            }
        }
        Target::Nowhere => {}
        Target::Hardware(first) => {
            for (c, s) in src.iter().enumerate() {
                if let Some(out) = output.get_mut(first + c) {
                    add(&mut out[..frames], &s[..frames]);
                }
            }
        }
    }
}

/// Everything a node does to its own buffer in a block: sources, inserts, fader, meter and
/// delay compensation. Touches no other node, so nodes of one level can run in parallel.
fn render_node(node: &mut Node, inputs: &Inputs) {
    let started = Instant::now();
    let frames = inputs.frames;
    let monitoring = node.params.monitoring.load(Ordering::Relaxed);
    if inputs.transport.playing {
        if let Ok(position) = u64::try_from(inputs.transport.position) {
            mix_clips(&node.clips, position, &mut node.buf, frames);
        }
    }
    match node.input {
        NodeInput::Audio { channel, stereo } if monitoring => {
            let right = if stereo { channel + 1 } else { channel };
            if let (Some(l), Some(r)) = (inputs.input.get(channel), inputs.input.get(right)) {
                add(&mut node.buf[0][..frames], &l[..frames]);
                add(&mut node.buf[1][..frames], &r[..frames]);
            }
        }
        NodeInput::Midi { port, channel } if monitoring => {
            if let Some(first) = node.inserts.first_mut() {
                for live in inputs.midi {
                    let port_ok = port.is_none_or(|want| want == live.port);
                    let channel_ok =
                        channel.is_none_or(|want| midi_channel(&live.event) == Some(want));
                    if port_ok && channel_ok {
                        first.send_midi_at(live.event, live.offset);
                    }
                }
            }
        }
        _ => {}
    }
    // MIDI clips, at the playhead, into the instrument. With no insert the player still runs,
    // so its position stays right for when one is added.
    if let Some(player) = &mut node.sequence {
        let mut first = node.inserts.first_mut();
        player.render(&inputs.transport, frames, &mut |event, offset| {
            if let Some(instrument) = first.as_mut() {
                instrument.send_midi_at(event, offset);
            }
        });
    }

    // Each insert hears audio as old as the compensation and latency in front of it, and its
    // parameter lanes are read at that position. A plug-in's extra outputs are taken as soon
    // as it has run, silent when it didn't (bypassed, skipped, gone).
    let mut transport = inputs.transport.earlier(node.offset);
    for send in &mut node.sends {
        if let Tap::Output { .. } = send.tap {
            for c in &mut send.buf {
                c[..frames].fill(0.0);
            }
        }
    }
    for (index, insert) in node.inserts.iter_mut().enumerate() {
        for lane in node
            .automation
            .parameters
            .iter()
            .filter(|l| l.insert == index)
        {
            lane.send(insert.as_mut(), &transport, frames);
        }
        insert.process(&mut node.buf, frames, &transport);
        transport = transport.earlier(insert.latency());
        for send in &mut node.sends {
            if let Tap::Output { insert: at, bus } = send.tap {
                if at == index {
                    if let Some(out) = insert.output_bus(bus) {
                        crate::plugins::adapt_channels(out, &mut send.buf, frames);
                    }
                }
            }
        }
    }
    // Where the audio now in the buffer sits on the timeline. The fader's and sends'
    // automation is read here, so a move lands on the audio it was drawn over whatever the
    // chain's latency.
    let at = transport;

    // The order below matters. Sends copy the node's signal before the main output's delay
    // line runs in place on it, so each edge gets only its own compensation; pre-fader sends
    // copy before the fader, post-fader ones after it.
    let muted = node.params.muted.load(Ordering::Relaxed);
    let node_mute_automated = node.automation.mute.is_some();
    let take = |send: &mut NodeSend, buf: &[Vec<f32>], node: (&NodeAutomation, &NodeParams)| {
        if send.lane.is_some() || node_mute_automated {
            send.take_automated(buf, frames, &at, node);
        } else {
            send.take(buf, frames, muted);
        }
    };
    for send in node.sends.iter_mut().filter(|s| s.tap == Tap::PreFader) {
        take(send, &node.buf, (&node.automation, &node.params));
    }

    if node.automation.mixes() {
        // Automated: the gains follow the automation grid, sample by sample. The last gains
        // are kept so the ramp below starts from them if the lanes go.
        let (automation, params) = (&node.automation, &node.params);
        let gains_at = |p: i64| automation.gains_at(params, &at.meter, p);
        let mut peak = [0.0f32; CHANNELS];
        let buf = &mut node.buf;
        on_grid(at.position, frames, &gains_at, |k, gains| {
            for c in 0..CHANNELS {
                let s = &mut buf[c][k];
                *s *= gains[c];
                peak[c] = peak[c].max(s.abs());
            }
        });
        for (c, peak) in peak.iter().enumerate() {
            node.params.peak[c].fetch_max_positive(*peak);
        }
        node.gains = gains_at(at.position + frames as i64);
    } else {
        // Ramp from the last block's gains to the new ones to avoid zipper noise.
        let target = node.params.target_gains();
        for (c, buf) in node.buf.iter_mut().enumerate() {
            let (from, to) = (node.gains[c], target[c]);
            let step = (to - from) / frames.max(1) as f32;
            let mut peak = 0.0f32;
            for (k, s) in buf[..frames].iter_mut().enumerate() {
                *s *= from + step * (k + 1) as f32;
                peak = peak.max(s.abs());
            }
            node.params.peak[c].fetch_max_positive(peak);
        }
        node.gains = target;
    }

    for send in node.sends.iter_mut().filter(|s| s.tap == Tap::PostFader) {
        take(send, &node.buf, (&node.automation, &node.params));
    }
    // Solo in place: a track heard only through its sends silences its main output here,
    // after the sends have their copies. Untouched (so bit-identical) while it plays.
    let main = if node.params.main_muted.load(Ordering::Relaxed) {
        0.0
    } else {
        1.0
    };
    if main != 1.0 || node.main_gain != 1.0 {
        let step = (main - node.main_gain) / frames.max(1) as f32;
        for buf in &mut node.buf {
            for (k, s) in buf[..frames].iter_mut().enumerate() {
                *s *= node.main_gain + step * (k + 1) as f32;
            }
        }
    }
    node.main_gain = main;
    node.delay.process(&mut node.buf, frames);
    for send in &mut node.sends {
        send.delay.process(&mut send.buf, frames);
    }

    // Smoothed cost, used to start the heaviest nodes first; and the CPU panel's reading.
    let elapsed = started.elapsed();
    node.params.usage.add(elapsed, frames);
    let cost = elapsed.as_secs_f32();
    node.cost += (cost - node.cost) * 0.1;
}

fn add(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += *s;
    }
}

pub(crate) fn midi_channel(event: &MidiEvent) -> Option<u8> {
    let channel = match event {
        MidiEvent::NoteOn { channel, .. }
        | MidiEvent::NoteOff { channel, .. }
        | MidiEvent::ControlChange { channel, .. }
        | MidiEvent::ProgramChange { channel, .. }
        | MidiEvent::PitchBend { channel, .. } => channel,
        _ => return None,
    };
    Some(channel.as_index())
}

fn mix_clips(clips: &[ClipSource], position: u64, buf: &mut [Vec<f32>], frames: usize) {
    let block_end = position + frames as u64;
    for clip in clips {
        let clip_end = clip.start + clip.length;
        if clip_end <= position || clip.start >= block_end {
            continue;
        }
        let from = clip.start.max(position);
        let to = clip_end.min(block_end);
        let out_start = (from - position) as usize;
        let src_start = (from - clip.start + clip.offset) as usize;
        let count = (to - from) as usize;
        let channels = &clip.data.channels;
        if channels.is_empty() {
            continue;
        }
        // The fades, clamped to fit the clip (a following clip's length moves with the
        // tempo). Each sample's gain depends only on its place in the clip.
        let fade_in = clip.fades.0.length.min(clip.length);
        let fade_out = clip.fades.1.length.min(clip.length - fade_in);
        let into = from - clip.start;
        let plain_from = fade_in.max(into);
        let plain_to = (clip.length - fade_out)
            .min(into + count as u64)
            .max(plain_from);
        for (c, out) in buf.iter_mut().enumerate() {
            let src = &channels[c.min(channels.len() - 1)];
            let end = (src_start + count).min(src.len());
            if src_start >= end {
                continue;
            }
            let out = &mut out[out_start..out_start + (end - src_start)];
            let src = &src[src_start..end];
            // Unfaded, the plain part: exactly the loop clips always had.
            let (a, b) = (
                ((plain_from - into) as usize).min(src.len()),
                ((plain_to - into) as usize).min(src.len()),
            );
            for (o, s) in out[a..b].iter_mut().zip(&src[a..b]) {
                *o += *s * clip.gain;
            }
            for i in (0..a).chain(b..src.len()) {
                let at = into + i as u64;
                let gain = if at < fade_in {
                    clip.fades.0.shape.gain(at as f32 / fade_in as f32)
                } else {
                    let left = clip.length - at;
                    clip.fades.1.shape.gain(left as f32 / fade_out as f32)
                };
                out[i] += src[i] * clip.gain * gain;
            }
        }
    }
}

/// Description of one node before compilation. `target` refers to other specs by index.
pub struct NodeSpec {
    /// Stable identity across rebuilds (the track id).
    pub key: u64,
    pub label: String,
    pub params: Arc<NodeParams>,
    pub clips: Vec<ClipSource>,
    pub input: NodeInput,
    pub inserts: Vec<Box<dyn Processor>>,
    pub target: SpecTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecTarget {
    Spec(usize),
    Hardware(usize),
    /// Dropped (see [`Target::Nowhere`]).
    Nowhere,
}

/// A send before compilation: an extra output of a node, to another spec or to hardware.
pub struct SendSpec {
    /// Stable identity within its node across rebuilds (the send id).
    pub key: u64,
    pub target: SpecTarget,
    pub tap: Tap,
    pub params: Arc<SendParams>,
    /// Automation of its level, in dB.
    pub lane: Option<AutomationLane>,
}

/// Orders nodes, solves delay compensation and allocates buffers.
pub struct GraphBuilder {
    specs: Vec<NodeSpec>,
    /// Each spec's sends, by spec index.
    sends: Vec<Vec<SendSpec>>,
    /// Each spec's MIDI clips, by spec index.
    sequences: Vec<Option<Sequence>>,
    /// Each spec's automation, by spec index.
    automation: Vec<NodeAutomation>,
}

impl GraphBuilder {
    pub fn new() -> Self {
        Self {
            specs: Vec::new(),
            sends: Vec::new(),
            sequences: Vec::new(),
            automation: Vec::new(),
        }
    }

    /// Add a node and return its spec index.
    pub fn add(&mut self, spec: NodeSpec) -> usize {
        self.specs.push(spec);
        self.sends.push(Vec::new());
        self.sequences.push(None);
        self.automation.push(NodeAutomation::default());
        self.specs.len() - 1
    }

    /// Automate node `index`'s fader, pan, mute and insert parameters.
    pub fn set_automation(&mut self, index: usize, automation: NodeAutomation) {
        self.automation[index] = automation;
    }

    /// Play `sequence` (a track's MIDI clips) into node `index`'s first insert. Give every
    /// instrument track one, even an empty one, so notes still sounding are released when its
    /// last clip goes.
    pub fn set_sequence(&mut self, index: usize, sequence: Sequence) {
        self.sequences[index] = Some(sequence);
    }

    /// Re-point a node once its target exists.
    pub fn set_target(&mut self, index: usize, target: SpecTarget) {
        self.specs[index].target = target;
    }

    /// Give node `from` a send. Sends are summed in the order they are added.
    pub fn add_send(&mut self, from: usize, send: SendSpec) {
        self.sends[from].push(send);
    }

    pub fn build(self, max_block: usize) -> Graph {
        let n = self.specs.len();
        let mut targets: Vec<SpecTarget> = self.specs.iter().map(|s| s.target).collect();
        let mut sends = self.sends;
        // Depth is the longest path to a hardware output, so every node sits deeper than
        // everything it feeds and runs before it. A cycle (which the project model prevents)
        // is cut where the search meets it: a main output that closes one goes to hardware 0,
        // a send is dropped.
        let mut depth = vec![0usize; n];
        let mut state = vec![Visit::New; n];
        for i in 0..n {
            if state[i] == Visit::New {
                visit(i, &mut targets, &mut sends, &mut state, &mut depth);
            }
        }
        // Deepest first; stable, so equal depths keep project order. Without sends this is the
        // order the graph has always had, which keeps every bus's sum in the same order.
        let mut order: Vec<usize> = (0..n).collect();
        order.sort_by_key(|&i| std::cmp::Reverse(depth[i]));
        let mut position = vec![0usize; n];
        for (pos, &i) in order.iter().enumerate() {
            position[i] = pos;
        }

        // Delay compensation. A node's output is "ready" at in_time + its latency; a bus's
        // in_time is the latest ready time over every edge into it (main outputs and sends),
        // and hardware outputs share one in_time, the latest of everything feeding them. Each
        // edge then waits from its node's ready time to its target's in_time.
        // A plug-in's extra output leaves partway along the chain: it is ready after the
        // latency of the inserts up to its own (`through[i][k]`: inserts 0..=k), not all.
        let through: Vec<Vec<usize>> = self
            .specs
            .iter()
            .map(|s| {
                s.inserts
                    .iter()
                    .scan(0, |sum, p| {
                        *sum += p.latency();
                        Some(*sum)
                    })
                    .collect()
            })
            .collect();
        let latency: Vec<usize> = through
            .iter()
            .map(|t| t.last().copied().unwrap_or(0))
            .collect();
        let behind = |i: usize, tap: Tap| match tap {
            Tap::Output { insert, .. } => through[i].get(insert).copied().unwrap_or(latency[i]),
            _ => latency[i],
        };
        // Each edge of node `i`: its target and the latency in front of it.
        let edges = |i: usize| -> Vec<(SpecTarget, usize)> {
            std::iter::once((targets[i], latency[i]))
                .chain(sends[i].iter().map(|s| (s.target, behind(i, s.tap))))
                .collect()
        };
        let mut in_time = vec![0usize; n];
        for &i in &order {
            for (target, late) in edges(i) {
                if let SpecTarget::Spec(t) = target {
                    in_time[t] = in_time[t].max(in_time[i] + late);
                }
            }
        }
        let output_time = order
            .iter()
            .flat_map(|&i| {
                let start = in_time[i];
                edges(i)
                    .into_iter()
                    .filter(|(t, _)| matches!(t, SpecTarget::Hardware(_)))
                    .map(move |(_, late)| start + late)
            })
            .max()
            .unwrap_or(0);
        // Where an edge goes in the compiled graph, and how long it waits on the way.
        let compile = |i: usize, target: SpecTarget, late: usize| {
            let ready = in_time[i] + late;
            match target {
                SpecTarget::Spec(t) => (Target::Node(position[t]), in_time[t] - ready),
                SpecTarget::Hardware(ch) => (Target::Hardware(ch), output_time - ready),
                SpecTarget::Nowhere => (Target::Nowhere, 0),
            }
        };

        let mut specs: Vec<Option<NodeSpec>> = self.specs.into_iter().map(Some).collect();
        let mut send_specs: Vec<Vec<SendSpec>> = std::mem::take(&mut sends);
        let mut sequences = self.sequences;
        let mut automation = self.automation;
        let nodes = order
            .iter()
            .map(|&i| {
                let spec = specs[i].take().expect("each spec is used once");
                let (target, wait) = compile(i, targets[i], latency[i]);
                let sends = std::mem::take(&mut send_specs[i])
                    .into_iter()
                    .map(|send| {
                        let (target, wait) = compile(i, send.target, behind(i, send.tap));
                        NodeSend {
                            key: send.key,
                            target,
                            tap: send.tap,
                            params: send.params,
                            delay: DelayLine::new(wait, CHANNELS),
                            lane: send.lane,
                            gain: 0.0,
                            buf: vec![vec![0.0; max_block]; CHANNELS],
                        }
                    })
                    .collect();
                Node {
                    key: spec.key,
                    label: spec.label,
                    params: spec.params,
                    clips: spec.clips,
                    input: spec.input,
                    inserts: spec.inserts,
                    sequence: sequences[i].take().map(SequencePlayer::new),
                    automation: std::mem::take(&mut automation[i]),
                    target,
                    delay: DelayLine::new(wait, CHANNELS),
                    sends,
                    offset: in_time[i],
                    buf: vec![vec![0.0; max_block]; CHANNELS],
                    gains: [0.0; CHANNELS],
                    main_gain: 1.0,
                    cost: 0.0,
                }
            })
            .collect();
        let mut levels: Vec<Level> = Vec::new();
        for (pos, &i) in order.iter().enumerate() {
            match levels.last_mut() {
                Some(level) if depth[order[level.start]] == depth[i] => {
                    level.end = pos + 1;
                    level.order.push(pos);
                }
                _ => levels.push(Level {
                    start: pos,
                    end: pos + 1,
                    order: vec![pos],
                }),
            }
        }
        Graph {
            nodes,
            levels,
            max_block,
            latency: output_time,
            inherit: Vec::new(),
        }
    }
}

impl Default for GraphBuilder {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Visit {
    New,
    /// On the search path: an edge back to it closes a cycle.
    Open,
    Done,
}

/// Depth-first search from spec `i`: sets `depth[i]` to its longest path to an output, and
/// cuts any edge that closes a cycle.
fn visit(
    i: usize,
    targets: &mut [SpecTarget],
    sends: &mut [Vec<SendSpec>],
    state: &mut [Visit],
    depth: &mut [usize],
) {
    state[i] = Visit::Open;
    let mut d = 0;
    if let SpecTarget::Spec(t) = targets[i] {
        if state[t] == Visit::Open {
            targets[i] = SpecTarget::Hardware(0);
        } else {
            if state[t] == Visit::New {
                visit(t, targets, sends, state, depth);
            }
            d = d.max(depth[t] + 1);
        }
    }
    let mut k = 0;
    while k < sends[i].len() {
        if let SpecTarget::Spec(t) = sends[i][k].target {
            if state[t] == Visit::Open {
                sends[i].remove(k);
                continue;
            }
            if state[t] == Visit::New {
                visit(t, targets, sends, state, depth);
            }
            d = d.max(depth[t] + 1);
        }
        k += 1;
    }
    depth[i] = d;
    state[i] = Visit::Done;
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Stand-in plugin: a pure delay.
    struct Delay(DelayLine);

    impl Processor for Delay {
        fn latency(&self) -> usize {
            self.0.delay()
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
            self.0.process(io, frames);
        }
        // A real plugin keeps its state across rebuilds because the instance is shared; the
        // stand-in hands its delay line over to behave the same.
        fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
            Some(self)
        }
        fn take_state_from(&mut self, old: &mut dyn Processor) -> bool {
            match old.as_any_mut().and_then(|a| a.downcast_mut::<Delay>()) {
                Some(old) if old.0.delay() == self.0.delay() => {
                    std::mem::swap(&mut self.0, &mut old.0);
                    true
                }
                _ => false,
            }
        }
    }

    /// Two tracks with plugin latency into a folder, one direct to the master; keys are stable.
    fn keyed_graph(extra_track: bool) -> Graph {
        let mut b = GraphBuilder::new();
        let keyed = |key: u64, mut s: NodeSpec| {
            s.key = key;
            s
        };
        let master = b.add(keyed(100, spec("master", vec![], vec![delay(3)])));
        let folder = b.add(keyed(10, spec("folder", vec![], vec![delay(5)])));
        let a = b.add(keyed(1, spec("a", vec![noise_clip(0, 7)], vec![delay(11)])));
        let c = b.add(keyed(2, spec("b", vec![noise_clip(300, 9)], vec![])));
        let d = b.add(keyed(
            3,
            spec("c", vec![noise_clip(100, 3)], vec![delay(2)]),
        ));
        b.set_target(folder, SpecTarget::Spec(master));
        b.set_target(a, SpecTarget::Spec(folder));
        b.set_target(c, SpecTarget::Spec(folder));
        b.set_target(d, SpecTarget::Spec(master));
        // A pre-fader send whose delay line must be handed over too.
        b.add_send(a, send(7, SpecTarget::Spec(master), Tap::PreFader).0);
        if extra_track {
            // A new, silent, zero-latency track: adding it must not disturb the others.
            let e = b.add(keyed(4, spec("new", vec![], vec![])));
            b.set_target(e, SpecTarget::Spec(folder));
        }
        b.build(256)
    }

    fn block(graph: &mut Graph, pos: u64, frames: usize) -> Vec<f32> {
        let input = vec![vec![0.0; 256]; 2];
        let mut output = vec![vec![0.0; 256]; 2];
        graph.process(&mut BlockContext {
            frames,
            transport: Transport::playing_at(pos),
            input: &input,
            output: &mut output,
            midi: &[],
        });
        output.iter().flat_map(|c| c[..frames].to_vec()).collect()
    }

    #[test]
    fn rebuilding_during_playback_is_seamless() {
        let frames = 128;
        let mut reference = keyed_graph(false);
        let expected: Vec<f32> = (0..40u64)
            .flat_map(|k| block(&mut reference, k * 128, frames))
            .collect();

        for (handover, extra) in [(true, false), (true, true), (false, false)] {
            let mut graph = keyed_graph(false);
            let mut got = Vec::new();
            for k in 0..40u64 {
                if k % 7 == 3 {
                    // Rebuild mid-playback, as moving a clip or adding a track does.
                    let mut next = keyed_graph(extra);
                    if handover {
                        next.link_previous(&graph.keys());
                        next.take_state_from(&mut graph);
                    }
                    graph = next;
                }
                got.extend(block(&mut graph, k * 128, frames));
            }
            let same = got
                .iter()
                .zip(&expected)
                .all(|(a, b)| a.to_bits() == b.to_bits());
            assert_eq!(same, handover, "handover {handover}, extra track {extra}");
        }
    }

    fn delay(samples: usize) -> Box<dyn Processor> {
        Box::new(Delay(DelayLine::new(samples, CHANNELS)))
    }

    fn click_clip(at: u64) -> ClipSource {
        let mut samples = vec![0.0; 16];
        samples[0] = 1.0;
        ClipSource {
            data: Arc::new(AudioData::new(48_000, vec![samples])),
            start: at,
            offset: 0,
            length: 16,
            gain: 1.0,
            fades: Default::default(),
        }
    }

    fn spec(label: &str, clips: Vec<ClipSource>, inserts: Vec<Box<dyn Processor>>) -> NodeSpec {
        let params = Arc::new(NodeParams::default());
        NodeSpec {
            key: 0,
            label: label.into(),
            params,
            clips,
            input: NodeInput::None,
            inserts,
            target: SpecTarget::Hardware(0),
        }
    }

    fn send(key: u64, target: SpecTarget, tap: Tap) -> (SendSpec, Arc<SendParams>) {
        let params = Arc::new(SendParams::default());
        let spec = SendSpec {
            key,
            target,
            tap,
            params: params.clone(),
            lane: None,
        };
        (spec, params)
    }

    /// Stand-in multi-out instrument: everything it hears goes to its extra output 1, none to
    /// its main pair (a drum kit with the kick routed to an output of its own).
    struct Split {
        out: Vec<Vec<f32>>,
        frames: usize,
    }

    impl Processor for Split {
        fn latency(&self) -> usize {
            0
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
            for (o, i) in self.out.iter_mut().zip(io.iter_mut()) {
                o[..frames].copy_from_slice(&i[..frames]);
                i[..frames].fill(0.0);
            }
            self.frames = frames;
        }
        fn output_bus(&self, bus: usize) -> Option<&[Vec<f32>]> {
            (bus == 1).then_some(self.out.as_slice())
        }
    }

    #[test]
    fn a_plug_ins_extra_output_plays_on_its_track_in_time() {
        // A plain track's click, and the same click through an instrument with 64 samples of
        // latency before its multi-out insert and 100 after: the output track hears it after
        // the 64 only, waits the 100 its host's main output takes, and lands on the plain
        // track's sample. At unity, though the host is muted and its fader is down.
        let output_of = |multi: bool, block: usize| {
            let mut b = GraphBuilder::new();
            let master = b.add(spec("master", vec![], vec![]));
            if multi {
                let split = Box::new(Split {
                    out: vec![vec![0.0; 1024]; CHANNELS],
                    frames: 0,
                });
                let mut host = spec(
                    "kit",
                    vec![click_clip(1000)],
                    vec![delay(64), split, delay(100)],
                );
                host.key = 1;
                host.target = SpecTarget::Spec(master);
                host.params.muted.store(true, Ordering::Relaxed);
                host.params.gain_db.store(-30.0);
                let host = b.add(host);
                let mut kick = spec("kick", vec![], vec![]);
                kick.key = 2;
                kick.target = SpecTarget::Spec(master);
                let kick = b.add(kick);
                let tap = Tap::Output { insert: 1, bus: 1 };
                b.add_send(host, send(1 << 40, SpecTarget::Spec(kick), tap).0);
            } else {
                let mut plain = spec("plain", vec![click_clip(1000)], vec![]);
                plain.target = SpecTarget::Spec(master);
                b.add(plain);
            }
            let mut graph = b.build(1024);
            let latency = graph.latency();
            let out = render(&mut graph, 4096, block);
            (out, latency)
        };
        let (plain, plain_latency) = output_of(false, 256);
        let at = plain.iter().position(|&x| x > 0.5).unwrap();
        assert_eq!(at, 1000 + plain_latency);
        for block in [64, 256, 1000] {
            let (multi, latency) = output_of(true, block);
            assert_eq!(latency, 164, "the host's main output sets the latency");
            let hit = multi.iter().position(|&x| x != 0.0).unwrap();
            assert_eq!(hit - latency, at - plain_latency, "block {block}");
            assert_eq!(multi[hit], 1.0, "at unity");
            assert!(multi.iter().filter(|&&x| x != 0.0).count() == 1);
        }
    }

    /// A lane of `(tick, value)` points. At the default 120 BPM and 48 kHz a tick is 25
    /// samples.
    fn lane(target: crate::automation::AutomationTarget, points: &[(u64, f64)]) -> AutomationLane {
        let mut lane = AutomationLane {
            id: 1,
            target,
            points: points
                .iter()
                .enumerate()
                .map(|(i, &(tick, value))| crate::automation::AutomationPoint {
                    id: i as u32 + 1,
                    tick,
                    value,
                    curve: 0.0,
                })
                .collect(),
            enabled: true,
        };
        lane.sort();
        lane
    }

    /// One second of a constant 1.0 from position 0.
    /// `clip` alone through `mix_clips`, `block` samples at a time.
    fn mixed(clip: &ClipSource, frames: usize, block: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; frames];
        let mut at = 0;
        while at < frames {
            let n = block.min(frames - at);
            let mut buf = vec![vec![0.0f32; n]];
            mix_clips(std::slice::from_ref(clip), at as u64, &mut buf, n);
            out[at..at + n].copy_from_slice(&buf[0]);
            at += n;
        }
        out
    }

    #[test]
    fn fades_shape_a_clip_by_position_in_any_block_size() {
        use crate::project::FadeShape;
        let plain = dc_clip();
        let ones = mixed(&plain, 48_000, 512);
        assert!(
            ones.iter().all(|v| *v == 1.0),
            "no fades: the clip as it was"
        );
        let mut faded = dc_clip();
        faded.fades = (
            Fade {
                length: 1000,
                shape: FadeShape::Linear,
            },
            Fade {
                length: 3000,
                shape: FadeShape::EqualPower,
            },
        );
        let a = mixed(&faded, 48_000, 64);
        assert!(a == mixed(&faded, 48_000, 512));
        assert!(a == mixed(&faded, 48_000, 1000));
        assert_eq!((a[0], a[500], a[1000], a[44_999]), (0.0, 0.5, 1.0, 1.0));
        let half = (std::f32::consts::FRAC_PI_4).sin();
        assert!((a[46_500] - half).abs() < 1e-6, "{}", a[46_500]);
        assert!(a[47_999] > 0.0 && a[47_999] < 1e-3);
        // Fades longer than the clip share it.
        faded.fades.0.length = 40_000;
        faded.fades.1.length = 40_000;
        let both = mixed(&faded, 48_000, 512);
        assert_eq!((both[0], both[40_000 - 1] < 1.0), (0.0, true));
        assert!(both[47_999] > 0.0);
    }
    fn dc_clip() -> ClipSource {
        ClipSource {
            data: Arc::new(AudioData::new(48_000, vec![vec![1.0; 48_000]])),
            start: 0,
            offset: 0,
            length: 48_000,
            gain: 1.0,
            fades: Default::default(),
        }
    }

    /// Jump fader and send ramps (which start from silence) to their targets.
    fn settle(graph: &mut Graph) {
        for node in &mut graph.nodes {
            node.gains = node.params.target_gains();
            node.main_gain = if node.params.main_muted.load(Ordering::Relaxed) {
                0.0
            } else {
                1.0
            };
            let muted = node.params.muted.load(Ordering::Relaxed);
            for send in &mut node.sends {
                send.gain = send.target_gain(muted);
            }
        }
    }

    /// Run the graph from position 0, settled, and return the left hardware output.
    fn render(graph: &mut Graph, len: usize, block: usize) -> Vec<f32> {
        render_outputs(graph, len, block, 2).swap_remove(0)
    }

    /// Like [`render`], returning `channels` hardware outputs.
    fn render_outputs(
        graph: &mut Graph,
        len: usize,
        block: usize,
        channels: usize,
    ) -> Vec<Vec<f32>> {
        settle(graph);
        let input = vec![vec![0.0; block]; 2];
        let mut out = vec![Vec::new(); channels];
        let mut pos = 0;
        while out[0].len() < len {
            let mut output = vec![vec![0.0; block]; channels];
            graph.process(&mut BlockContext {
                frames: block,
                transport: Transport::playing_at(pos),
                input: &input,
                output: &mut output,
                midi: &[],
            });
            for (all, block) in out.iter_mut().zip(&output) {
                all.extend_from_slice(block);
            }
            pos += block as u64;
        }
        for channel in &mut out {
            channel.truncate(len);
        }
        out
    }

    /// Where a labelled node sits in processing order.
    fn position_of(graph: &Graph, label: &str) -> usize {
        graph.nodes.iter().position(|n| n.label == label).unwrap()
    }

    /// What the fader gives a 1.0 at `position` under `lane`, computed independently of the
    /// graph: the lane's dB value there, as a gain.
    fn gain_at(lane: &AutomationLane, position: i64) -> f32 {
        db_to_gain(lane.value_at(tick_of(position)).unwrap() as f32)
    }

    /// The tick at `position` under the default meter.
    fn tick_of(position: i64) -> f64 {
        Meter::default().quarter_notes(position) * f64::from(TICKS_PER_QUARTER)
    }

    #[test]
    fn a_volume_ramp_and_jump_land_on_their_samples() {
        use crate::automation::AutomationTarget::Volume;
        // -24 dB at tick 40 (sample 1000) rising to 0 dB at tick 120 (3000), easing to -6 by
        // tick 160 (4000) and jumping to -20 there.
        let volume = lane(
            Volume,
            &[
                (40, -24.0),
                (120, 0.0),
                (160, -6.0),
                (160, -20.0),
                (200, -20.0),
            ],
        );
        // The same track twice, once behind a 37-sample plugin: both reach the outputs 37
        // samples late, and the automation must move with their audio, not the playhead.
        let mut b = GraphBuilder::new();
        let plain = b.add(spec("plain", vec![dc_clip()], vec![]));
        let latent = b.add(spec("latent", vec![dc_clip()], vec![delay(37)]));
        b.set_target(latent, SpecTarget::Hardware(2));
        for node in [plain, latent] {
            b.set_automation(
                node,
                NodeAutomation {
                    volume: Some(volume.clone()),
                    ..Default::default()
                },
            );
        }
        let mut g = b.build(512);
        assert_eq!(g.latency(), 37);
        let out = render_outputs(&mut g, 6000, 64, 4);
        let (plain_out, latent_out) = (&out[0], &out[2]);
        // On every grid line the gain is the lane's value exactly; between lines it is a
        // straight line between them.
        for line in (0..5900).step_by(AUTOMATION_GRID as usize) {
            let at = |p: usize| plain_out[p + 37];
            assert_eq!(at(line), gain_at(&volume, line as i64), "grid line {line}");
            let (a, b) = (at(line), at(line + 32));
            for k in 1..32 {
                let s = at(line + k);
                assert!(
                    s >= a.min(b) - 1e-6 && s <= a.max(b) + 1e-6,
                    "{} between",
                    line + k
                );
            }
        }
        // The jump is complete on its grid line, and fades over the step before it.
        assert_eq!(plain_out[4000 + 37], db_to_gain(-20.0));
        assert_eq!(plain_out[3968 + 37], gain_at(&volume, 3968));
        assert!(plain_out[3990 + 37] < plain_out[3970 + 37]);
        assert_eq!(
            latent_out, plain_out,
            "latency in front doesn't move the ramp"
        );
        for block in [100, 512, 1] {
            g.reset();
            let again = render_outputs(&mut g, 6000, block, 4);
            assert!(
                again[0]
                    .iter()
                    .zip(plain_out)
                    .all(|(a, b)| a.to_bits() == b.to_bits()),
                "{block}-sample blocks"
            );
        }
    }

    #[test]
    fn a_mute_lane_silences_the_track_and_its_sends_on_its_sample() {
        use crate::automation::AutomationTarget::{Mute, SendLevel};
        // Muted from tick 64 (sample 1600, a grid line); a pre-fader send to outputs 2/3 at
        // -12 dB falling to -24 by tick 64.
        let mute = lane(Mute, &[(0, 0.0), (64, 1.0)]);
        let level = lane(SendLevel { send: 1 }, &[(0, -12.0), (64, -24.0)]);
        let mut b = GraphBuilder::new();
        let t = b.add(spec("t", vec![dc_clip()], vec![]));
        let (mut cue, _) = send(1, SpecTarget::Hardware(2), Tap::PreFader);
        cue.lane = Some(level.clone());
        b.add_send(t, cue);
        // And one with a fixed level, to outputs 4/5.
        b.add_send(t, send(2, SpecTarget::Hardware(4), Tap::PreFader).0);
        b.set_automation(
            t,
            NodeAutomation {
                mute: Some(mute),
                ..Default::default()
            },
        );
        let mut g = b.build(512);
        let out = render_outputs(&mut g, 3000, 128, 6);
        let (main, send_out, fixed) = (&out[0], &out[2], &out[4]);
        assert_eq!(fixed[1568], 1.0);
        assert!(
            fixed[1600..].iter().all(|s| *s == 0.0),
            "a send without a lane follows the mute lane too"
        );
        assert_eq!(main[1568], 1.0, "plays up to the grid step before the mute");
        assert!(main[1584] > 0.0 && main[1584] < 1.0, "fades over that step");
        assert!(
            main[1600..].iter().all(|s| *s == 0.0),
            "silent from its sample"
        );
        assert_eq!(send_out[992], gain_at(&level, 992));
        assert!(send_out[1584] > 0.0);
        assert!(
            send_out[1600..].iter().all(|s| *s == 0.0),
            "a mute lane silences the sends too, pre-fader ones included"
        );
    }

    /// Block position, parameter id, value and offset of each point a spy received.
    type SpiedPoints = Arc<std::sync::Mutex<Vec<(i64, u32, f64, usize)>>>;

    /// Records the parameter points each block hands it, with the block's position.
    struct ParamSpy {
        latency: usize,
        points: SpiedPoints,
        queued: Vec<(u32, f64, usize)>,
    }

    impl Processor for ParamSpy {
        fn latency(&self) -> usize {
            self.latency
        }
        fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize, transport: &Transport) {
            let mut points = self.points.lock().unwrap();
            for (id, value, offset) in self.queued.drain(..) {
                points.push((transport.position, id, value, offset));
            }
        }
        fn set_parameter_at(&mut self, id: u32, value: f64, offset: usize) {
            self.queued.push((id, value, offset));
        }
    }

    #[test]
    fn parameter_lanes_reach_their_insert_at_sample_offsets() {
        use crate::automation::AutomationTarget::Parameter;
        // Parameter 7 of the second insert (behind 50 samples of latency) rises from 0 to 1
        // between ticks 0 and 40 (sample 1000), then holds.
        let rising = lane(
            Parameter {
                insert: 1,
                parameter: 7,
            },
            &[(0, 0.0), (40, 1.0)],
        );
        let points = Arc::new(std::sync::Mutex::new(Vec::new()));
        let spy = ParamSpy {
            latency: 0,
            points: points.clone(),
            queued: Vec::new(),
        };
        let mut b = GraphBuilder::new();
        let t = b.add(spec("t", vec![], vec![delay(50), Box::new(spy)]));
        b.set_automation(
            t,
            NodeAutomation {
                parameters: vec![ParameterLane {
                    insert: 1,
                    parameter: 7,
                    lane: rising.clone(),
                    writing: Default::default(),
                }],
                ..Default::default()
            },
        );
        let mut g = b.build(512);
        render(&mut g, 2048, 256);
        let points = points.lock().unwrap();
        let value = |p: i64| rising.value_at(tick_of(p)).unwrap();
        // Every block starts with its value; moving blocks add one point per grid line.
        let blocks: Vec<i64> = points.iter().filter(|p| p.3 == 0).map(|p| p.0).collect();
        assert_eq!(
            blocks,
            [-50, 206, 462, 718, 974, 1230, 1486, 1742],
            "the insert's own position"
        );
        for &(start, id, v, offset) in points.iter() {
            let p = start + offset as i64;
            assert_eq!(id, 7);
            assert_eq!(v, value(p), "point at {p}");
            assert!(
                offset == 0 || p % AUTOMATION_GRID == 0,
                "{p} is on the grid"
            );
        }
        let in_block = |start: i64| points.iter().filter(|p| p.0 == start).count();
        assert_eq!(in_block(206), 9, "moving: the start and 8 grid lines");
        assert_eq!(in_block(1230), 1, "held: just the start");
    }

    /// Write modes (5.4): while a target is written, the control plays, not its lane, and the
    /// switch takes no rebuild.
    #[test]
    fn a_written_target_follows_its_control_not_its_lane() {
        use crate::automation::AutomationTarget::{Parameter, Volume};
        let params = Arc::new(NodeParams::default());
        let points = Arc::new(std::sync::Mutex::new(Vec::new()));
        let spy = ParamSpy {
            latency: 0,
            points: points.clone(),
            queued: Vec::new(),
        };
        let writing = Arc::new(AtomicBool::new(false));
        let mut b = GraphBuilder::new();
        let mut t_spec = spec("t", vec![dc_clip()], vec![Box::new(spy)]);
        t_spec.params = params.clone();
        let t = b.add(t_spec);
        b.set_automation(
            t,
            NodeAutomation {
                volume: Some(lane(Volume, &[(0, -20.0)])),
                parameters: vec![ParameterLane {
                    insert: 0,
                    parameter: 3,
                    lane: lane(
                        Parameter {
                            insert: 1,
                            parameter: 3,
                        },
                        &[(0, 0.25)],
                    ),
                    writing: writing.clone(),
                }],
                ..Default::default()
            },
        );
        let mut g = b.build(512);
        params.gain_db.store(-6.0);
        let lane_gain = db_to_gain(-20.0);
        assert_eq!(render(&mut g, 512, 128)[256], lane_gain, "the lane plays");
        assert!(
            !points.lock().unwrap().is_empty(),
            "and sends its parameter"
        );

        params.writing.store(WRITE_VOLUME, Ordering::Relaxed);
        writing.store(true, Ordering::Relaxed);
        points.lock().unwrap().clear();
        assert_eq!(
            render(&mut g, 512, 128)[256],
            db_to_gain(-6.0),
            "while written, the control's value plays"
        );
        assert!(
            points.lock().unwrap().is_empty(),
            "and the parameter lane leaves the knob alone"
        );

        params.writing.store(0, Ordering::Relaxed);
        assert_eq!(render(&mut g, 512, 128)[256], lane_gain, "the lane again");
    }

    #[test]
    fn each_edge_gets_its_own_compensation() {
        // Track t (no latency) goes to the master directly and through a send to a reverb
        // return with 10 samples of latency. Track s has 40, so the master hears everything
        // 40 late: t's main output must wait 40, its send 0 (the reverb adds 10, then waits
        // 30). One delay line per node would put 40 on the send as well.
        let mut b = GraphBuilder::new();
        let master = b.add(spec("master", vec![], vec![]));
        let reverb = b.add(spec("reverb", vec![], vec![delay(10)]));
        let t = b.add(spec("t", vec![click_clip(10)], vec![]));
        let s = b.add(spec("s", vec![click_clip(10)], vec![delay(40)]));
        b.set_target(reverb, SpecTarget::Spec(master));
        b.set_target(t, SpecTarget::Spec(master));
        b.set_target(s, SpecTarget::Spec(master));
        b.add_send(t, send(1, SpecTarget::Spec(reverb), Tap::PostFader).0);
        let mut g = b.build(64);
        assert_eq!(g.latency(), 40);
        assert!(
            position_of(&g, "t") < position_of(&g, "reverb"),
            "a node runs before everything it feeds, sends included"
        );
        let t_node = &g.nodes[position_of(&g, "t")];
        assert_eq!(
            (t_node.delay.delay(), t_node.sends[0].delay.delay()),
            (40, 0)
        );
        assert_eq!(g.nodes[position_of(&g, "reverb")].delay.delay(), 30);
        let out = render(&mut g, 200, 64);
        assert_eq!(clicks(&out), vec![(50, 3.0)], "three paths, one sample");
        assert_eq!(render(&mut g, 200, 17), out, "and at any block size");
    }

    #[test]
    fn pre_fader_sends_ignore_the_fader_and_mute_silences_every_edge() {
        // Main output on outputs 0/1, a pre-fader send on 2/3 and a post-fader send on 4/5,
        // both at -6 dB.
        let mut b = GraphBuilder::new();
        let s = spec("t", vec![click_clip(0)], vec![]);
        let track = s.params.clone();
        let t = b.add(s);
        let (pre, pre_params) = send(1, SpecTarget::Hardware(2), Tap::PreFader);
        let (post, post_params) = send(2, SpecTarget::Hardware(4), Tap::PostFader);
        b.add_send(t, pre);
        b.add_send(t, post);
        let mut g = b.build(64);
        for params in [&pre_params, &post_params] {
            params.gain_db.store(-6.0);
        }
        let level = db_to_gain(-6.0);
        let peaks = |g: &mut Graph| -> [f32; 3] {
            let out = render_outputs(g, 64, 64, 6);
            [out[0][0], out[2][0], out[4][0]]
        };
        assert_eq!(peaks(&mut g), [1.0, level, level]);

        track.gain_db.store(-12.0);
        let quieter = db_to_gain(-12.0);
        assert_eq!(peaks(&mut g), [quieter, level, quieter * level]);

        post_params.muted.store(true, Ordering::Relaxed);
        assert_eq!(peaks(&mut g), [quieter, level, 0.0], "a muted send");

        post_params.muted.store(false, Ordering::Relaxed);
        track.main_muted.store(true, Ordering::Relaxed);
        assert_eq!(
            peaks(&mut g),
            [0.0, level, quieter * level],
            "solo in place: only the main output goes quiet, both sends play on"
        );

        track.main_muted.store(false, Ordering::Relaxed);
        track.muted.store(true, Ordering::Relaxed);
        assert_eq!(
            peaks(&mut g),
            [0.0, 0.0, 0.0],
            "a muted track silences its pre-fader sends too"
        );
    }

    #[test]
    fn send_levels_ramp_like_faders() {
        // A constant signal, so the output is the gain itself.
        let ones = ClipSource {
            data: Arc::new(AudioData::new(48_000, vec![vec![1.0; 256]])),
            start: 0,
            offset: 0,
            length: 256,
            gain: 1.0,
            fades: Default::default(),
        };
        let mut b = GraphBuilder::new();
        let t = b.add(spec("t", vec![ones], vec![]));
        let (pre, params) = send(1, SpecTarget::Hardware(2), Tap::PreFader);
        b.add_send(t, pre);
        let mut g = b.build(64);
        settle(&mut g);
        params.gain_db.store(-60.0);
        let input = vec![vec![0.0; 64]; 2];
        let mut output = vec![vec![0.0; 64]; 4];
        g.process(&mut BlockContext {
            frames: 64,
            transport: Transport::playing_at(0),
            input: &input,
            output: &mut output,
            midi: &[],
        });
        // Halfway through the block the send is still well above its new -60 dB.
        let gain_at = |k: usize| output[2][k];
        assert!((gain_at(31) - 0.5).abs() < 0.02, "{}", gain_at(31));
        assert!((gain_at(63) - db_to_gain(-60.0)).abs() < 1e-4);
    }

    #[test]
    fn a_send_that_closes_a_cycle_is_dropped() {
        // a → b by main output, and b sends back to a: the send is cut, nothing hangs.
        let mut b = GraphBuilder::new();
        let a = b.add(spec("a", vec![click_clip(0)], vec![]));
        let bus = b.add(spec("b", vec![], vec![]));
        b.set_target(a, SpecTarget::Spec(bus));
        b.add_send(bus, send(1, SpecTarget::Spec(a), Tap::PostFader).0);
        let mut g = b.build(64);
        assert!(g.nodes[position_of(&g, "b")].sends.is_empty());
        assert!(position_of(&g, "a") < position_of(&g, "b"));
        assert_eq!(clicks(&render(&mut g, 64, 64)), vec![(0, 1.0)]);
    }

    fn clicks(out: &[f32]) -> Vec<(usize, f32)> {
        out.iter()
            .enumerate()
            .filter(|(_, s)| s.abs() > 1e-6)
            .map(|(i, s)| (i, *s))
            .collect()
    }

    #[test]
    fn compensates_latency_across_buses() {
        // Two tracks with clicks at the same time; one has a 37-sample plugin. Both go into a
        // folder bus with a 5-sample plugin, which goes to the master with a 3-sample plugin.
        let mut b = GraphBuilder::new();
        let master = b.add(spec("master", vec![], vec![delay(3)]));
        let bus = b.add(spec("bus", vec![], vec![delay(5)]));
        let fast = b.add(spec("fast", vec![click_clip(10)], vec![]));
        let slow = b.add(spec("slow", vec![click_clip(10)], vec![delay(37)]));
        // A third track straight to hardware must line up with the master path too.
        let direct = b.add(spec("direct", vec![click_clip(10)], vec![]));
        b.set_target(bus, SpecTarget::Spec(master));
        b.set_target(fast, SpecTarget::Spec(bus));
        b.set_target(slow, SpecTarget::Spec(bus));
        b.set_target(direct, SpecTarget::Hardware(0));
        let mut g = b.build(64);
        assert_eq!(g.latency(), 37 + 5 + 3);
        let out = render(&mut g, 200, 64);
        // All three clicks land in one sample, at clip time + total latency.
        assert_eq!(clicks(&out), vec![(10 + 45, 3.0)]);
    }

    #[test]
    fn block_size_does_not_change_the_mix() {
        let build = || {
            let mut b = GraphBuilder::new();
            let master = b.add(spec("master", vec![], vec![]));
            let t = b.add(spec("t", vec![click_clip(100)], vec![delay(29)]));
            b.set_target(t, SpecTarget::Spec(master));
            b.build(512)
        };
        let a = render(&mut build(), 600, 64);
        let b = render(&mut build(), 600, 500);
        assert_eq!(a, b);
        assert_eq!(clicks(&a), vec![(129, 1.0)]);
    }

    #[test]
    fn pan_mute_and_meters() {
        let mut b = GraphBuilder::new();
        let s = spec("t", vec![click_clip(0)], vec![]);
        let params = s.params.clone();
        b.add(s);
        let mut g = b.build(64);
        params.pan.store(1.0);
        for node in &mut g.nodes {
            node.gains = node.params.target_gains();
        }
        let input = vec![vec![0.0; 64]; 2];
        let mut output = vec![vec![0.0; 64]; 2];
        g.process(&mut BlockContext {
            frames: 64,
            transport: Transport::playing_at(0),
            input: &input,
            output: &mut output,
            midi: &[],
        });
        assert_eq!(output[0][0], 0.0, "hard right pan silences the left");
        assert_eq!(output[1][0], 1.0);
        assert_eq!(params.peak[1].take(), 1.0);

        params.muted.store(true, Ordering::Relaxed);
        let mut muted_block = || {
            let mut output = vec![vec![0.0; 64]; 2];
            g.process(&mut BlockContext {
                frames: 64,
                transport: Transport::playing_at(0),
                input: &input,
                output: &mut output,
                midi: &[],
            });
            output
        };
        // The first muted block ramps down rather than cutting, then it is silent.
        let ramp = muted_block();
        assert!(ramp[1][0] > 0.9 && ramp[1][0] < 1.0);
        assert!(muted_block().iter().flatten().all(|s| *s == 0.0));
    }

    /// Stand-in plugin that records the transport it is given each block.
    pub(crate) struct TransportSpy {
        pub latency: usize,
        pub seen: Arc<std::sync::Mutex<Vec<Transport>>>,
    }

    impl Processor for TransportSpy {
        fn latency(&self) -> usize {
            self.latency
        }
        fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize, transport: &Transport) {
            self.seen.lock().unwrap().push(*transport);
        }
    }

    pub(crate) fn spy(
        latency: usize,
    ) -> (Box<dyn Processor>, Arc<std::sync::Mutex<Vec<Transport>>>) {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let spy = TransportSpy {
            latency,
            seen: seen.clone(),
        };
        (Box::new(spy), seen)
    }

    #[test]
    fn processors_see_the_transport_of_the_audio_they_hear() {
        // Track a: two plugins, the first with 30 samples of latency. Track c: 100 samples.
        // Both feed a bus, which therefore hears everything 100 samples late, and so does the
        // master after it.
        let (a1, seen_a1) = spy(30);
        let (a2, seen_a2) = spy(0);
        let (on_bus, seen_bus) = spy(0);
        let (on_master, seen_master) = spy(0);
        let mut b = GraphBuilder::new();
        let master = b.add(spec("master", vec![], vec![on_master]));
        let bus = b.add(spec("bus", vec![], vec![on_bus]));
        let a = b.add(spec("a", vec![], vec![a1, a2]));
        let c = b.add(spec("c", vec![], vec![delay(100)]));
        b.set_target(bus, SpecTarget::Spec(master));
        b.set_target(a, SpecTarget::Spec(bus));
        b.set_target(c, SpecTarget::Spec(bus));
        let mut g = b.build(64);

        // 7/8 at 60 quarter notes a minute: one quarter note a second.
        let meter = Meter {
            tempo: 60.0,
            numerator: 7,
            denominator: 8,
            ..Meter::default()
        };
        let input = vec![vec![0.0; 64]; 2];
        let mut output = vec![vec![0.0; 64]; 2];
        for playing in [true, false] {
            g.process(&mut BlockContext {
                frames: 64,
                transport: Transport {
                    playing,
                    position: 48_000,
                    meter,
                },
                input: &input,
                output: &mut output,
                midi: &[],
            });
        }
        let last = |seen: &Arc<std::sync::Mutex<Vec<Transport>>>| {
            let seen = seen.lock().unwrap();
            assert_eq!(seen.len(), 2, "one call per block");
            assert!(
                seen[0].playing && !seen[1].playing,
                "stopped still reaches plugins"
            );
            seen[1]
        };
        assert_eq!(last(&seen_a1).position, 48_000);
        assert_eq!(
            last(&seen_a2).position,
            48_000 - 30,
            "behind the first plugin"
        );
        assert_eq!(
            last(&seen_bus).position,
            48_000 - 100,
            "behind the slowest input"
        );
        assert_eq!(last(&seen_master).position, 48_000 - 100);

        let t = last(&seen_a1);
        assert_eq!(t.quarter_notes(), 1.0);
        assert!((last(&seen_a2).quarter_notes() - (1.0 - 30.0 / 48_000.0)).abs() < 1e-12);
        assert!(Transport::default().earlier(10).quarter_notes() < 0.0);
    }

    /// Stand-in instrument that records the MIDI it receives.
    struct MidiSpy(Arc<std::sync::Mutex<Vec<MidiEvent>>>);

    impl Processor for MidiSpy {
        fn latency(&self) -> usize {
            0
        }
        fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize, _: &Transport) {}
        fn send_midi(&mut self, event: MidiEvent) {
            self.0.lock().unwrap().push(event);
        }
    }

    #[test]
    fn midi_reaches_monitored_instruments_matching_port_and_channel() {
        use vst3_host::MidiChannel;
        let note = |channel, note| MidiEvent::NoteOn {
            channel,
            note,
            velocity: 100,
        };
        let live = |port, event| LiveMidi {
            port,
            event,
            offset: 0,
        };
        let events = [
            live(0, note(MidiChannel::Ch1, 60)),
            live(1, note(MidiChannel::Ch1, 61)),
            live(1, note(MidiChannel::Ch2, 62)),
        ];
        let run = |input: NodeInput, monitoring: bool| {
            let received = Arc::new(std::sync::Mutex::new(Vec::new()));
            let mut s = spec("synth", vec![], vec![Box::new(MidiSpy(received.clone()))]);
            s.input = input;
            s.params.monitoring.store(monitoring, Ordering::Relaxed);
            let mut b = GraphBuilder::new();
            b.add(s);
            let mut g = b.build(16);
            let input = vec![vec![0.0; 16]; 2];
            let mut output = vec![vec![0.0; 16]; 2];
            g.process(&mut BlockContext {
                frames: 16,
                transport: Transport::default(),
                input: &input,
                output: &mut output,
                midi: &events,
            });
            let notes: Vec<u8> = received
                .lock()
                .unwrap()
                .iter()
                .map(|e| match e {
                    MidiEvent::NoteOn { note, .. } => *note,
                    _ => 0,
                })
                .collect();
            notes
        };
        let all = NodeInput::Midi {
            port: None,
            channel: None,
        };
        assert_eq!(run(all, true), vec![60, 61, 62], "omni, all ports");
        assert_eq!(run(all, false), Vec::<u8>::new(), "not armed or monitoring");
        let port1 = NodeInput::Midi {
            port: Some(1),
            channel: None,
        };
        assert_eq!(run(port1, true), vec![61, 62]);
        let port1_ch2 = NodeInput::Midi {
            port: Some(1),
            channel: Some(1),
        };
        assert_eq!(run(port1_ch2, true), vec![62]);
    }

    /// Stand-in for a heavy, stateful plugin: a saturating one-pole filter with some busy work,
    /// so its output depends on everything it processed before.
    struct Busy {
        state: [f32; CHANNELS],
        drive: f32,
    }

    impl Processor for Busy {
        fn latency(&self) -> usize {
            0
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
            for (c, buf) in io.iter_mut().enumerate() {
                for s in &mut buf[..frames] {
                    let mut x = *s * self.drive;
                    for _ in 0..20 {
                        x = (x * 0.999).tanh();
                    }
                    self.state[c] += 0.2 * (x - self.state[c]);
                    *s = self.state[c];
                }
            }
        }
    }

    fn busy(drive: f32) -> Box<dyn Processor> {
        Box::new(Busy {
            state: [0.0; CHANNELS],
            drive,
        })
    }

    fn noise_clip(at: u64, seed: u32) -> ClipSource {
        let mut x = seed.max(1);
        let samples: Vec<f32> = (0..4096)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) * 2.0 - 1.0
            })
            .collect();
        ClipSource {
            data: Arc::new(AudioData::new(48_000, vec![samples.clone(), samples])),
            start: at,
            offset: 0,
            length: 4096,
            gain: 0.5,
            fades: Default::default(),
        }
    }

    /// Twelve tracks in nested folders, with latencies that need compensating, on three levels.
    fn big_graph() -> Graph {
        let mut b = GraphBuilder::new();
        let master = b.add(spec("master", vec![], vec![busy(1.1)]));
        let drums = b.add(spec("drums", vec![], vec![busy(1.3), delay(7)]));
        let guitars = b.add(spec("guitars", vec![], vec![busy(0.9)]));
        let doubles = b.add(spec("doubles", vec![], vec![delay(3)]));
        b.set_target(drums, SpecTarget::Spec(master));
        b.set_target(guitars, SpecTarget::Spec(master));
        b.set_target(doubles, SpecTarget::Spec(guitars));
        // A reverb return fed by sends from tracks and from a bus, with its own latency.
        let reverb = b.add(spec("reverb", vec![], vec![busy(0.7), delay(13)]));
        b.set_target(reverb, SpecTarget::Spec(master));
        b.add_send(doubles, send(1, SpecTarget::Spec(reverb), Tap::PostFader).0);
        for k in 0..12u32 {
            let inserts = vec![busy(1.0 + k as f32 * 0.1), delay((k as usize * 5) % 23)];
            let t = b.add(spec(
                "t",
                vec![noise_clip(u64::from(k) * 100, k + 1)],
                inserts,
            ));
            let target = match k % 4 {
                0 => drums,
                1 => guitars,
                2 => doubles,
                _ => master,
            };
            b.set_target(t, SpecTarget::Spec(target));
            if k % 3 == 0 {
                let tap = if k % 2 == 0 {
                    Tap::PreFader
                } else {
                    Tap::PostFader
                };
                let (to_reverb, params) = send(1, SpecTarget::Spec(reverb), tap);
                params.gain_db.store(-(k as f32));
                b.add_send(t, to_reverb);
            }
            if k == 5 {
                // A cue mix: a pre-fader send straight to the second output pair, automated.
                let (mut cue, _) = send(2, SpecTarget::Hardware(2), Tap::PreFader);
                cue.lane = Some(lane(
                    crate::automation::AutomationTarget::SendLevel { send: 2 },
                    &[(0, -3.0), (300, -30.0)],
                ));
                b.add_send(t, cue);
            }
            // Automation on some tracks: a fade, a pan sweep and a mute.
            if k % 5 == 0 {
                use crate::automation::AutomationTarget::{Mute, Pan, Volume};
                b.set_automation(
                    t,
                    NodeAutomation {
                        volume: Some(lane(Volume, &[(10, -40.0), (200, 3.0)])),
                        pan: (k == 5).then(|| lane(Pan, &[(0, -1.0), (400, 1.0)])),
                        mute: (k == 10).then(|| lane(Mute, &[(0, 0.0), (150, 1.0), (300, 0.0)])),
                        parameters: Vec::new(),
                    },
                );
            }
        }
        // One track straight to a second output pair.
        let direct = b.add(spec("direct", vec![noise_clip(50, 99)], vec![busy(2.0)]));
        b.set_target(direct, SpecTarget::Hardware(2));
        b.build(512)
    }

    fn run_blocks(graph: &mut Graph, pool: Option<&WorkerPool>) -> Vec<f32> {
        let input = vec![vec![0.0; 512]; 2];
        let mut all = Vec::new();
        let mut pos = 0u64;
        // Varying block sizes, like a WASAPI device.
        for frames in [512usize, 480, 1, 300, 512, 64, 333, 512, 512, 100]
            .iter()
            .cycle()
            .take(40)
        {
            let mut output = vec![vec![0.0; 512]; 4];
            graph.process_with(
                &mut BlockContext {
                    frames: *frames,
                    transport: Transport::playing_at(pos),
                    input: &input,
                    output: &mut output,
                    midi: &[],
                },
                pool,
            );
            for channel in &output {
                all.extend_from_slice(&channel[..*frames]);
            }
            pos += *frames as u64;
        }
        all
    }

    #[test]
    fn parallel_mix_is_bit_identical_to_serial() {
        let serial = run_blocks(&mut big_graph(), None);
        assert!(
            serial.iter().any(|s| s.abs() > 0.01),
            "the test graph makes sound"
        );
        for workers in [1, 3, 7] {
            let pool = WorkerPool::new(workers);
            for _ in 0..3 {
                let parallel = run_blocks(&mut big_graph(), Some(&pool));
                let same = serial
                    .iter()
                    .zip(&parallel)
                    .all(|(a, b)| a.to_bits() == b.to_bits());
                assert!(
                    same && serial.len() == parallel.len(),
                    "{workers} workers differ"
                );
            }
        }
    }

    #[test]
    fn levels_hold_independent_nodes() {
        let g = big_graph();
        for level in &g.levels {
            for i in level.start..level.end {
                let node = &g.nodes[i];
                let edges = std::iter::once(node.target).chain(node.sends.iter().map(|s| s.target));
                for target in edges {
                    if let Target::Node(j) = target {
                        assert!(j >= level.end, "node {i} feeds {j} inside its own level");
                    }
                }
            }
            let mut order = level.order.clone();
            order.sort_unstable();
            assert_eq!(order, (level.start..level.end).collect::<Vec<_>>());
        }
        assert_eq!(
            g.levels.len(),
            4,
            "tracks, doubles' children, folders and outputs"
        );
    }

    #[test]
    fn monitoring_routes_hardware_input() {
        let mut b = GraphBuilder::new();
        let mut s = spec("t", vec![], vec![]);
        s.input = NodeInput::Audio {
            channel: 1,
            stereo: false,
        };
        let params = s.params.clone();
        b.add(s);
        let mut g = b.build(8);
        for node in &mut g.nodes {
            node.gains = node.params.target_gains();
        }
        let input = vec![vec![0.0; 8], vec![0.5; 8]];
        let run = |g: &mut Graph| {
            let mut output = vec![vec![0.0; 8]; 2];
            g.process(&mut BlockContext {
                frames: 8,
                transport: Transport::default(),
                input: &input,
                output: &mut output,
                midi: &[],
            });
            output
        };
        assert_eq!(run(&mut g)[0][4], 0.0, "not monitoring");
        params.monitoring.store(true, Ordering::Relaxed);
        let out = run(&mut g);
        assert_eq!(
            (out[0][4], out[1][4]),
            (0.5, 0.5),
            "mono input to both sides"
        );
    }
}
