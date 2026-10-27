//! Device buffers gathered into larger blocks (23.1, [`Stage`]).

use super::*;
use crate::graph::{
    ClipSource, GraphBuilder, NodeInput, NodeParams, NodeSpec, Processor, SpecTarget,
};
use crate::media::AudioData;

/// Adds a value made of its block's length and how many blocks it has run to every sample, so
/// two runs null only if they cut the audio into the same blocks.
struct Blocky(u32);
impl Processor for Blocky {
    fn latency(&self) -> usize {
        0
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
        self.0 += 1;
        for c in io.iter_mut() {
            for s in &mut c[..frames] {
                *s += frames as f32 * 1e-3 + self.0 as f32 * 1e-6;
            }
        }
    }
}

fn graph(max_block: usize) -> Graph {
    let ramp: Vec<f32> = (0..20_000).map(|i| (i % 977) as f32 * 1e-4).collect();
    let mut b = GraphBuilder::new();
    b.add(NodeSpec {
        key: 1,
        label: String::new(),
        params: Arc::new(NodeParams::default()),
        clips: vec![ClipSource {
            data: Arc::new(AudioData::new(48_000, vec![ramp.clone()])),
            start: 100,
            offset: 0,
            length: ramp.len() as u64,
            gain: 1.0,
            fades: Default::default(),
        }],
        input: NodeInput::None,
        inserts: vec![Box::new(Blocky(0))],
        target: SpecTarget::Hardware(0),
    });
    b.build(max_block)
}

/// Play `frames` from 0 in device buffers of `device`, gathered into `block` (0: not).
fn play(device: usize, block: usize, frames: usize) -> (Vec<f32>, EngineCore) {
    let (mut core, mut handle) = new_engine(512);
    core.configure_io(0, 2);
    core.shared()
        .processing_block
        .store(block as u64, Ordering::Relaxed);
    for c in [
        Command::SetGraph(Box::new(graph(512))),
        Command::Locate(0),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    let mut left = Vec::new();
    while left.len() < frames {
        let mut out = vec![0.0f32; device * 2];
        core.process(&mut out, 2, None, 0);
        left.extend(out.chunks(2).map(|f| f[0]));
    }
    (left, core)
}

#[test]
fn gathered_buffers_play_the_render_in_blocks_late_by_the_stage() {
    let total = 4096;
    let (live, core) = play(16, 64, total + 64);
    let shared = core.shared();
    assert_eq!(shared.block_size.load(Ordering::Relaxed), 64);
    assert_eq!(shared.device_block.load(Ordering::Relaxed), 16);
    assert_eq!(shared.stage_latency.load(Ordering::Relaxed), 48);

    // An export's path: render() in the same blocks, from the same start.
    let (mut offline, _handle) = new_engine(512);
    offline.configure_io(0, 2);
    *offline.graph = graph(512);
    offline.set_transport(true, 0);
    let mut rendered = vec![Vec::new(), Vec::new()];
    offline.render(total, 64, &mut rendered);

    assert!(live[..48].iter().all(|s| *s == 0.0), "{:?}", &live[..48]);
    for n in 0..total {
        assert_eq!(
            live[n + 48].to_bits(),
            rendered[0][n].to_bits(),
            "sample {n}: {} live, {} rendered",
            live[n + 48],
            rendered[0][n]
        );
    }
    // And not what 16-sample blocks make: the processor saw the gathered blocks.
    let (plain, _) = play(16, 0, total);
    assert!(plain[..total - 48]
        .iter()
        .zip(&live[48..total])
        .any(|(a, b)| a.to_bits() != b.to_bits()));
}

#[test]
fn a_block_is_whole_device_buffers_within_the_largest_block() {
    // (device buffer, asked for, block, stage latency)
    for (device, wanted, block, latency) in [
        (16, 64, 64, 48),
        (16, 50, 64, 48),
        (48, 64, 96, 48),
        (64, 64, 64, 0),
        (128, 64, 128, 0),
        (100, 2048, 500, 400),
    ] {
        let (_, core) = play(device, wanted, 2048);
        let shared = core.shared();
        assert_eq!(
            shared.block_size.load(Ordering::Relaxed),
            block as u64,
            "{device} into {wanted}"
        );
        assert_eq!(
            shared.stage_latency.load(Ordering::Relaxed),
            latency as u64,
            "{device} into {wanted}"
        );
    }
}

/// Feed `buffers` device buffers of 16 frames to a monitored mono input, with one impulse at
/// frame `at`; returns the left output.
fn monitor(core: &mut EngineCore, buffers: usize, at: usize) -> Vec<f32> {
    let (mut tx, mut rx) = RingBuffer::<f32>::new(4096);
    let mut left = Vec::new();
    for k in 0..buffers {
        for f in 0..16 {
            tx.push(if k * 16 + f == at { 1.0 } else { 0.0 }).unwrap();
        }
        let mut out = vec![0.0f32; 32];
        core.process(&mut out, 2, Some(&mut rx), 1);
        left.extend(out.chunks(2).map(|f| f[0]));
    }
    left
}

#[test]
fn monitored_input_comes_out_the_stage_later() {
    let (mut core, mut handle) = new_engine(512);
    core.configure_io(1, 2);
    core.shared().processing_block.store(64, Ordering::Relaxed);
    let params = Arc::new(NodeParams::default());
    params.monitoring.store(true, Ordering::Relaxed);
    let mut b = GraphBuilder::new();
    b.add(NodeSpec {
        key: 1,
        label: String::new(),
        params,
        clips: vec![],
        input: NodeInput::Audio {
            channel: 0,
            stereo: false,
        },
        inserts: vec![],
        target: SpecTarget::Hardware(0),
    });
    assert!(handle
        .commands
        .push(Command::SetGraph(Box::new(b.build(512))))
        .is_ok());
    let left = monitor(&mut core, 64, 700);
    let heard: Vec<usize> = (0..left.len()).filter(|&n| left[n] != 0.0).collect();
    assert_eq!(heard, [748], "the impulse in at 700 comes out 48 later");
    assert_eq!(left[748], 1.0);

    // Turned off, it starts over without the stage: straight through again.
    core.shared().processing_block.store(0, Ordering::Relaxed);
    let left = monitor(&mut core, 8, 40);
    let heard: Vec<usize> = (0..left.len()).filter(|&n| left[n] != 0.0).collect();
    assert_eq!(heard, [40]);
    assert_eq!(core.shared().stage_latency.load(Ordering::Relaxed), 0);
    assert_eq!(core.shared().block_size.load(Ordering::Relaxed), 16);
}

#[test]
fn live_midi_lands_where_its_buffer_sits_in_the_gathered_block() {
    use std::sync::Mutex;
    use vst3_host::MidiChannel;

    type Log = Arc<Mutex<Vec<(i64, usize)>>>;
    struct Spy {
        pending: Vec<usize>,
        log: Log,
    }
    impl Processor for Spy {
        fn latency(&self) -> usize {
            0
        }
        fn process(&mut self, _: &mut [Vec<f32>], frames: usize, transport: &Transport) {
            assert_eq!(frames, 64, "the plug-in gets whole gathered blocks");
            for offset in self.pending.drain(..) {
                self.log.lock().unwrap().push((transport.position, offset));
            }
        }
        fn send_midi_at(&mut self, _: MidiEvent, offset: usize) {
            self.pending.push(offset);
        }
    }
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
    core.shared().processing_block.store(64, Ordering::Relaxed);
    let (mut keyboard, port) = RingBuffer::new(64);
    for c in [
        Command::SetGraph(Box::new(b.build(512))),
        Command::SetMidiPorts(vec![port]),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    for callback in 0..8 {
        let now = Instant::now();
        if callback == 5 {
            // In as the callback starts: the end of its buffer, the second of the second block.
            assert!(keyboard
                .push(TimedMidi {
                    event: MidiEvent::NoteOn {
                        channel: MidiChannel::Ch1,
                        note: 60,
                        velocity: 100,
                    },
                    at: now,
                })
                .is_ok());
        }
        core.set_callback_time(now);
        let mut out = vec![0.0f32; 32];
        core.process(&mut out, 2, None, 0);
    }
    assert_eq!(*log.lock().unwrap(), [(64, 16 + 15)]);
}

/// Keys pressed as each of `callbacks` 16-frame callbacks starts, recording from 0 with the
/// buffers gathered into `block` (0: not); each key's `heard` stamp, less the stage latency
/// (which the session takes off with the output latency).
fn recorded_stamps(block: usize, callbacks: usize) -> Vec<i64> {
    use vst3_host::MidiChannel;
    let (mut core, mut handle) = new_engine(512);
    core.configure_io(0, 2);
    core.shared().set_sample_rate(48_000);
    core.shared()
        .processing_block
        .store(block as u64, Ordering::Relaxed);
    let (mut keyboard, port) = RingBuffer::new(64);
    let (recorded, mut midi) = RingBuffer::new(256);
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
        Command::SetGraph(Box::new(graph(512))),
        Command::SetMidiPorts(vec![port]),
        Command::Locate(0),
        Command::StartRecording(Box::new(job)),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    for _ in 0..callbacks {
        let now = Instant::now();
        assert!(keyboard
            .push(TimedMidi {
                event: MidiEvent::NoteOn {
                    channel: MidiChannel::Ch1,
                    note: 60,
                    velocity: 100,
                },
                at: now,
            })
            .is_ok());
        core.set_callback_time(now);
        let mut out = vec![0.0f32; 32];
        core.process(&mut out, 2, None, 0);
    }
    let stage = core.shared().stage_latency.load(Ordering::Relaxed) as i64;
    let mut stamps = Vec::new();
    while let Ok(m) = midi.pop() {
        stamps.push(m.heard - stage);
    }
    stamps
}

#[test]
fn recorded_midi_is_stamped_with_what_was_playing_while_gathering() {
    // Without gathering, a key at callback c was heard over the buffer playing then: 16 c.
    let plain = recorded_stamps(0, 16);
    let expected: Vec<i64> = (0..16).map(|c| 16 * c).collect();
    assert_eq!(plain, expected);
    // Gathered into 64, the device plays a block's output 48 frames later: the buffer playing
    // at callback c holds the engine's 16 c - 48, a buffer at a time, with no jumps.
    let gathered = recorded_stamps(64, 16);
    let expected: Vec<i64> = (0..16).map(|c| 16 * c - 48).collect();
    assert_eq!(gathered, expected);
}
