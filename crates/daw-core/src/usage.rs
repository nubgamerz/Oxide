//! CPU use per track and per insert, for the DSP readout's panel and `get_cpu_usage`.
//!
//! The audio thread (or the pool worker rendering a node) adds each block's time to a
//! [`Usage`]: a few relaxed atomics, no lock and no allocation. A track's total lives in its
//! [`crate::graph::NodeParams`] (timed in `render_node`), an insert's in its plug-in, wrapped
//! in [`Timed`] when the graph is built; both outlive graph rebuilds, so a swap loses nothing.
//! The main thread takes the sums every [`WINDOW`] ([`crate::session::Session::cpu_usage`])
//! and turns them into time per buffer and share of the buffer's deadline.

use crate::graph::{Processor, Transport};
use crate::project::{InsertId, TrackId};
use serde::Serialize;
use std::any::Any;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::{Duration, Instant};
use vst3_host::MidiEvent;

/// How long each reading averages over.
pub const WINDOW: Duration = Duration::from_millis(500);

/// Processing time summed since the main thread last took it.
#[derive(Debug, Default)]
pub struct Usage {
    nanos: AtomicU64,
    frames: AtomicU64,
    blocks: AtomicU64,
}

/// What a [`Usage`] summed over a window.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Sample {
    pub nanos: u64,
    pub frames: u64,
    pub blocks: u64,
}

impl Usage {
    /// One block of `frames` took `elapsed`. Real-time safe.
    pub fn add(&self, elapsed: Duration, frames: usize) {
        self.nanos.fetch_add(elapsed.as_nanos() as u64, Relaxed);
        self.frames.fetch_add(frames as u64, Relaxed);
        self.blocks.fetch_add(1, Relaxed);
    }

    /// The sums so far, starting again from zero.
    pub fn take(&self) -> Sample {
        Sample {
            nanos: self.nanos.swap(0, Relaxed),
            frames: self.frames.swap(0, Relaxed),
            blocks: self.blocks.swap(0, Relaxed),
        }
    }
}

/// A reading of one track or insert.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct Load {
    /// Average processing time per buffer, in microseconds.
    pub per_buffer_us: f64,
    /// That time as a share of the buffer's duration (its deadline): 1.0 is all of it.
    pub share: f64,
    /// Buffers measured in the window.
    pub buffers: u64,
}

impl Sample {
    pub fn load(&self, sample_rate: u32) -> Load {
        if self.blocks == 0 || self.frames == 0 {
            return Load::default();
        }
        let duration_ns = self.frames as f64 / f64::from(sample_rate) * 1e9;
        Load {
            per_buffer_us: self.nanos as f64 / self.blocks as f64 / 1e3,
            share: self.nanos as f64 / duration_ns,
            buffers: self.blocks,
        }
    }
}

/// A track's (or the master's) reading, with its inserts'.
#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct TrackUsage {
    /// None for the master.
    pub track: Option<TrackId>,
    pub name: String,
    /// The whole track: its clips, inserts, fader, sends and compensation.
    pub load: Load,
    /// Its inserts, most costly first.
    pub inserts: Vec<InsertUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, schemars::JsonSchema)]
pub struct InsertUsage {
    pub insert: InsertId,
    pub name: String,
    pub load: Load,
    /// Bypassed: not called at all, so its load is (close to) nothing.
    pub bypass: bool,
    /// Blocks it has missed since it loaded (its track silent for that block): its plug-in
    /// was held by something else when the audio thread came for it.
    pub misses: u64,
}

/// One reading of the whole project.
#[derive(Debug, Clone, Default, PartialEq, Serialize, schemars::JsonSchema)]
pub struct CpuUsage {
    /// Seconds the reading averages over (0 before the first).
    pub window: f64,
    /// Tracks and the master, most costly first.
    pub tracks: Vec<TrackUsage>,
    /// The whole app's CPU time over the window ([`crate::cpu::process_time`], counted in
    /// cycles), in cores (1.0: one core all the time): every thread, so also what plug-ins do
    /// on threads of their own (a sampler's disk streaming, say), which the tracks' figures (time inside
    /// the audio callback) can't see.
    pub process: f64,
}

impl CpuUsage {
    /// Sort tracks, and each one's inserts, most costly first.
    pub fn sort(&mut self) {
        let by_cost = |a: &Load, b: &Load| b.per_buffer_us.total_cmp(&a.per_buffer_us);
        for track in &mut self.tracks {
            track.inserts.sort_by(|a, b| by_cost(&a.load, &b.load));
        }
        self.tracks.sort_by(|a, b| by_cost(&a.load, &b.load));
    }
}

/// An insert that times its own processing into `usage`. Every other call passes straight
/// through, so the processor behaves exactly as unwrapped: [`Processor::as_any_mut`] and
/// [`Processor::take_state_from`] reach the inner one (a new graph's insert hands over from
/// the old graph's wrapped insert). A method added to [`Processor`] must be forwarded here.
pub struct Timed {
    inner: Box<dyn Processor>,
    usage: Arc<Usage>,
}

impl Timed {
    pub fn new(inner: Box<dyn Processor>, usage: Arc<Usage>) -> Self {
        Self { inner, usage }
    }
}

impl Processor for Timed {
    fn latency(&self) -> usize {
        self.inner.latency()
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize, transport: &Transport) {
        let started = Instant::now();
        self.inner.process(io, frames, transport);
        self.usage.add(started.elapsed(), frames);
    }
    fn send_midi(&mut self, event: MidiEvent) {
        self.inner.send_midi(event);
    }
    fn send_midi_at(&mut self, event: MidiEvent, offset: usize) {
        self.inner.send_midi_at(event, offset);
    }
    fn set_parameter_at(&mut self, id: u32, value: f64, offset: usize) {
        self.inner.set_parameter_at(id, value, offset);
    }
    fn wants_lookahead(&self) -> bool {
        self.inner.wants_lookahead()
    }
    fn output_bus(&self, bus: usize) -> Option<&[Vec<f32>]> {
        self.inner.output_bus(bus)
    }
    fn as_any_mut(&mut self) -> Option<&mut dyn Any> {
        self.inner.as_any_mut()
    }
    fn take_state_from(&mut self, old: &mut dyn Processor) -> bool {
        self.inner.take_state_from(old)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{BlockContext, GraphBuilder, NodeInput, NodeParams, NodeSpec, SpecTarget};

    /// A stand-in plug-in of known cost: it spins for `cost` each block.
    struct Spin {
        cost: Duration,
    }

    impl Processor for Spin {
        fn latency(&self) -> usize {
            0
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize, _: &Transport) {
            let started = Instant::now();
            while started.elapsed() < self.cost {
                std::hint::spin_loop();
            }
        }
    }

    struct Nothing;

    impl Processor for Nothing {
        fn latency(&self) -> usize {
            0
        }
        fn process(&mut self, _: &mut [Vec<f32>], _: usize, _: &Transport) {}
    }

    fn spec(params: Arc<NodeParams>, inserts: Vec<Box<dyn Processor>>) -> NodeSpec {
        NodeSpec {
            key: 0,
            label: "t".into(),
            params,
            clips: Vec::new(),
            input: NodeInput::None,
            inserts,
            target: SpecTarget::Hardware(0),
        }
    }

    /// Three tracks of timed stand-ins: the inserts' times add up to the block's, within a few
    /// percent (the rest is mixing), and each track's total holds its inserts'.
    #[test]
    fn insert_times_add_up_to_the_block() {
        let costs = [[300u64, 100], [200, 50], [400, 0]];
        let mut b = GraphBuilder::new();
        let mut tracks = Vec::new();
        for track in costs {
            let params = Arc::new(NodeParams::default());
            let usages: Vec<Arc<Usage>> = track.iter().map(|_| Arc::default()).collect();
            let inserts = track
                .iter()
                .zip(&usages)
                .map(|(&us, usage)| {
                    let spin = Box::new(Spin {
                        cost: Duration::from_micros(us),
                    });
                    Box::new(Timed::new(spin, usage.clone())) as Box<dyn Processor>
                })
                .collect();
            b.add(spec(params.clone(), inserts));
            tracks.push((params, usages));
        }
        let mut graph = b.build(64);
        let input = vec![vec![0.0; 64]; 2];
        let mut output = vec![vec![0.0; 64]; 2];
        let blocks = 20;
        // One block at a time, the best of them: a test run beside others can lose the CPU
        // between two inserts, which no insert's time would hold.
        let mut best: Option<(f64, f64)> = None;
        for k in 0..blocks {
            let started = Instant::now();
            graph.process(&mut BlockContext {
                frames: 64,
                transport: Transport::playing_at(k * 64),
                input: &input,
                output: &mut output,
                midi: &[],
            });
            let whole = started.elapsed().as_nanos() as f64;
            let mut inserts = 0.0;
            for (params, usages) in &tracks {
                let mine: f64 = usages
                    .iter()
                    .map(|u| {
                        let s = u.take();
                        assert_eq!((s.blocks, s.frames), (1, 64));
                        s.nanos as f64
                    })
                    .sum();
                let total = params.usage.take();
                assert_eq!(total.blocks, 1);
                assert!(
                    total.nanos as f64 >= mine,
                    "a track holds its inserts' time"
                );
                inserts += mine;
            }
            if best.is_none_or(|(_, w)| whole < w) {
                best = Some((inserts, whole));
            }
        }
        let (inserts, whole) = best.unwrap();
        let share = inserts / whole;
        assert!(
            (0.95..=1.0).contains(&share),
            "inserts {inserts} ns of {whole} ns ({share})"
        );
        // 1.05 ms of spinning, as the stand-ins cost.
        assert!((1050e3..1150e3).contains(&inserts), "{inserts} ns");
    }

    /// The timing itself costs well under 1 % of a 64-sample block (1.33 ms at 48 kHz), for a
    /// heavy project's 32 inserts: the best of many runs, timed against the same inserts bare.
    #[test]
    fn timing_costs_under_a_percent_of_a_small_block() {
        let usage = Arc::new(Usage::default());
        let mut bare: Vec<Box<dyn Processor>> = (0..32).map(|_| Box::new(Nothing) as _).collect();
        let mut timed: Vec<Box<dyn Processor>> = (0..32)
            .map(|_| Box::new(Timed::new(Box::new(Nothing), usage.clone())) as _)
            .collect();
        let mut io = vec![vec![0.0; 64]; 2];
        let transport = Transport::playing_at(0);
        let mut best = |inserts: &mut Vec<Box<dyn Processor>>| {
            (0..200)
                .map(|_| {
                    let started = Instant::now();
                    for p in inserts.iter_mut() {
                        p.process(&mut io, 64, &transport);
                    }
                    started.elapsed()
                })
                .min()
                .unwrap()
        };
        let overhead = best(&mut timed).saturating_sub(best(&mut bare));
        let block = Duration::from_secs_f64(64.0 / 48_000.0);
        assert!(overhead < block / 100, "{overhead:?} of a {block:?} block");
    }

    #[test]
    fn loads_are_time_per_buffer_and_share_of_its_duration() {
        let usage = Usage::default();
        for _ in 0..4 {
            usage.add(Duration::from_micros(500), 480);
        }
        // 480 frames at 48 kHz is 10 ms: 0.5 ms of it is 5 %.
        let load = usage.take().load(48_000);
        assert!((load.per_buffer_us - 500.0).abs() < 1e-9);
        assert!((load.share - 0.05).abs() < 1e-12);
        assert_eq!(load.buffers, 4);
        assert_eq!(usage.take(), Sample::default(), "taken");
        assert_eq!(Sample::default().load(48_000), Load::default());
    }
}
