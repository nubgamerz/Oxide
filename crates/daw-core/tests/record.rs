//! Recording end to end without a device: hardware input through the engine's record tap,
//! the ring, the disk writer, to a WAV file with exactly what came in.

use daw_core::engine::{new_engine, Command, Garbage};
use daw_core::record::{self, TakeSpec};
use daw_core::wav;
use rtrb::RingBuffer;
use std::time::{Duration, Instant};

/// A simulated loopback cable: a fake device whose output plays 700 samples after its
/// callback and whose input arrives 300 samples after capture, with the input callbacks out
/// of phase with the output ones, and what is played fed straight back into the input. A
/// click on the timeline, recorded back through the engine behind 300 samples of delay
/// compensation and placed with the session's shift (compensation + measured round trip),
/// lands within a sample of where it was. A burst of stale input at the start makes the engine
/// trim its backlog, which the measurement must count.
#[test]
fn a_looped_back_click_is_recorded_where_it_was_played() {
    use daw_core::engine::InputAnchor;
    use daw_core::graph::{ClipSource, DelayLine, GraphBuilder, NodeInput, NodeParams};
    use daw_core::graph::{NodeSpec, Processor, SpecTarget, Transport};
    use daw_core::media::AudioData;
    use std::collections::VecDeque;
    use std::sync::atomic::Ordering;
    use std::sync::Arc;

    struct Lag(DelayLine);
    impl Processor for Lag {
        fn latency(&self) -> usize {
            self.0.delay()
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
            self.0.process(io, frames);
        }
    }
    const RATE: u64 = 48_000;
    const BLOCK: u64 = 256;
    const OUT_LATENCY: u64 = 700;
    const IN_LATENCY: u64 = 300;
    const PHASE: u64 = 100; // input callbacks run 100 samples after output ones
    const COMPENSATION: u64 = 300;
    const CLICK: u64 = 96_000;
    let ns = |t: u64| (t as f64 * 1e9 / RATE as f64).round() as u64;

    let (mut core, mut handle) = new_engine(256);
    core.configure_io(1, 2);
    core.shared().set_sample_rate(RATE as u32);
    let mut impulse = vec![0.0f32; 150_000];
    impulse[CLICK as usize] = 1.0;
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
    b.add(spec(
        1,
        vec![Box::new(Lag(DelayLine::new(COMPENSATION as usize, 2)))],
        vec![],
    ));
    let clip = ClipSource {
        data: Arc::new(AudioData::new(RATE as u32, vec![impulse])),
        start: 0,
        offset: 0,
        length: 150_000,
        gain: 1.0,
        fades: Default::default(),
    };
    b.add(spec(2, vec![], vec![clip]));
    for c in [Command::SetGraph(Box::new(b.build(256))), Command::Play] {
        assert!(handle.commands.push(c).is_ok());
    }

    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut anchors: VecDeque<InputAnchor> = VecDeque::new();
    let mut written = 0u64;
    // Stale input from before playback: the engine trims it on its first callback.
    for _ in 0..5 * BLOCK {
        tx.push(0.0).unwrap();
    }
    anchors.push_back(InputAnchor {
        frame: 0,
        capture_ns: 0,
    });
    written += 5 * BLOCK;

    // What reaches the cable, by time in samples.
    let mut played = vec![0.0f32; 200_000];
    let dir = std::env::temp_dir().join("daw-record-loopback-test");
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("take.wav");
    let mut files = None;
    let mut anchor = None;
    // Start a few buffers in, so the first input buffer was captured after time zero.
    let (mut next_out, mut next_in) = (4 * BLOCK, 4 * BLOCK + PHASE);
    while next_out < 170_000 {
        if next_in < next_out {
            // An input callback: the buffer captured IN_LATENCY ago.
            let first = next_in - IN_LATENCY - BLOCK;
            anchors.push_back(InputAnchor {
                frame: written,
                capture_ns: ns(first),
            });
            for t in first..first + BLOCK {
                tx.push(played[t as usize]).unwrap();
            }
            written += BLOCK;
            next_in += BLOCK;
            continue;
        }
        // An output callback; start recording once the round trip has settled.
        if files.is_none() && core.shared().round_trip.load(Ordering::Relaxed) != i64::MIN {
            let (job, done) = record::start(
                vec![TakeSpec {
                    track: 9,
                    channel: 0,
                    stereo: false,
                    path: path.clone(),
                }],
                RATE as u32,
            )
            .unwrap();
            assert!(handle.commands.push(Command::StartRecording(job)).is_ok());
            files = Some(done);
        }
        while let Some(a) = anchors.pop_front() {
            anchor = Some(a);
        }
        core.set_clock(ns(next_out), ns(next_out + OUT_LATENCY), anchor);
        let mut out = vec![0.0f32; BLOCK as usize * 2];
        core.process(&mut out, 2, Some(&mut rx), 1);
        for (k, frame) in out.chunks(2).enumerate() {
            played[(next_out + OUT_LATENCY) as usize + k] = frame[0];
        }
        next_out += BLOCK;
    }
    let round_trip = core.shared().round_trip.load(Ordering::Relaxed);
    assert!(handle.commands.push(Command::Stop).is_ok());
    core.process(&mut [0.0f32; 2], 2, Some(&mut rx), 1);
    let mut tapped_at = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            tapped_at = job.start;
        }
    }
    let files = files.expect("recording started");
    let deadline = Instant::now() + Duration::from_secs(5);
    while files.try_recv().is_err() {
        assert!(Instant::now() < deadline, "disk writer never finished");
        std::thread::sleep(Duration::from_millis(10));
    }
    let take = &wav::read(&path).unwrap().channels[0];
    let hit = take
        .iter()
        .position(|v| *v > 0.5)
        .expect("the click was recorded");
    // Placed as the session places takes.
    let shift = COMPENSATION as i64 + round_trip;
    let landed = tapped_at.unwrap() as i64 - shift + hit as i64;
    println!("round trip {round_trip} samples; click recorded at {landed}, played at {CLICK}");
    assert!(
        (landed - CLICK as i64).abs() <= 1,
        "click landed at {landed}, expected {CLICK} (round trip {round_trip})"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Loop recording through 300 samples of delay compensation, with wraps inside device
/// buffers: about 3.5 passes, cut into takes, each exactly the input played while that pass
/// was heard.
#[test]
fn loop_recording_cuts_one_take_per_pass_through_latency() {
    use daw_core::graph::{DelayLine, GraphBuilder, NodeInput, NodeParams, NodeSpec};
    use daw_core::graph::{Processor, SpecTarget, Transport};
    use std::sync::Arc;

    struct Lag(DelayLine);
    impl Processor for Lag {
        fn latency(&self) -> usize {
            self.0.delay()
        }
        fn process(&mut self, io: &mut [Vec<f32>], frames: usize, _: &Transport) {
            self.0.process(io, frames);
        }
    }
    const LATENCY: u64 = 300;
    let (start, end) = (1000u64, 3000u64);

    let dir = std::env::temp_dir().join("daw-record-loop-test");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut core, mut handle) = new_engine(256);
    core.configure_io(1, 2);
    let mut b = GraphBuilder::new();
    b.add(NodeSpec {
        key: 1,
        label: "slow".into(),
        params: Arc::new(NodeParams::default()),
        clips: vec![],
        input: NodeInput::None,
        inserts: vec![Box::new(Lag(DelayLine::new(LATENCY as usize, 2)))],
        target: SpecTarget::Hardware(0),
    });
    let path = dir.join("take.wav");
    let (mut job, files) = record::start(
        vec![TakeSpec {
            track: 5,
            channel: 0,
            stereo: false,
            path: path.clone(),
        }],
        48_000,
    )
    .unwrap();
    // As the session sets it up: record from the loop start, as heard, and only from there.
    job.from = start + LATENCY;
    job.exact_from = true;
    for c in [
        Command::SetGraph(Box::new(b.build(256))),
        Command::SetLoop(Some((start, end))),
        Command::Locate(start),
        Command::StartRecording(job),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    // 333-sample buffers put every wrap inside one.
    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut sent = Vec::new();
    while sent.len() < 7700 {
        for _ in 0..333 {
            let value = sent.len() as f32 * 1e-4;
            tx.push(value).unwrap();
            sent.push(value);
        }
        let mut out = vec![0.0f32; 333 * 2];
        core.process(&mut out, 2, Some(&mut rx), 1);
    }
    assert!(handle.commands.push(Command::Stop).is_ok());
    core.process(&mut [0.0f32; 2], 2, Some(&mut rx), 1);
    let mut tapped_at = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            tapped_at = job.start;
        }
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let file = loop {
        if let Ok(file) = files.try_recv() {
            break file;
        }
        assert!(Instant::now() < deadline, "disk writer never finished");
        std::thread::sleep(Duration::from_millis(10));
    };
    let audio = wav::read(&path).unwrap();
    let recorded = &audio.channels[0];
    assert_eq!(recorded.len() as u64, file.frames);

    // Placed as the session places takes: the tap position minus the compensation.
    let heard_start = tapped_at.unwrap() - LATENCY;
    let takes = record::slice_passes(file.frames, heard_start, (start, end), None).unwrap();
    assert_eq!((takes.start, takes.length), (start, end - start));
    assert_eq!(takes.passes.len(), 4);
    assert_eq!(takes.active, 2, "pass 3 plays; the unfinished 4th is kept");
    // The output runs 300 behind the engine, which started at the loop start, so pass k is
    // heard while input samples 300 + 2000k .. 300 + 2000(k+1) come in.
    let len = (end - start) as usize;
    for (k, pass) in takes.passes.iter().enumerate() {
        let take = &recorded[pass.offset as usize..];
        let heard = &sent[LATENCY as usize + k * len..];
        let n = len.min(take.len());
        assert_eq!(take[..n], heard[..n], "pass {k}");
        assert_eq!(pass.complete, n == len, "pass {k}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Loop recording started while already playing inside the loop, past its start: the tap
/// waits for the next pass, so the take still starts at the loop start.
#[test]
fn loop_recording_started_mid_pass_waits_for_the_next_pass() {
    let (mut core, mut handle) = new_engine(256);
    core.configure_io(1, 2);
    let dir = std::env::temp_dir().join("daw-record-loop-late-test");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut job, _files) = record::start(
        vec![TakeSpec {
            track: 6,
            channel: 0,
            stereo: false,
            path: dir.join("take.wav"),
        }],
        48_000,
    )
    .unwrap();
    job.from = 1000;
    job.exact_from = true;
    for c in [
        Command::SetLoop(Some((1000, 3000))),
        Command::Locate(2000),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    // Playing, then record mid-pass.
    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut block = |core: &mut daw_core::engine::EngineCore| {
        for _ in 0..200 {
            tx.push(0.5).unwrap();
        }
        core.process(&mut [0.0f32; 400], 2, Some(&mut rx), 1);
    };
    block(&mut core);
    assert!(handle.commands.push(Command::StartRecording(job)).is_ok());
    for _ in 0..10 {
        block(&mut core);
    }
    assert!(handle.commands.push(Command::Stop).is_ok());
    core.process(&mut [0.0f32; 2], 2, Some(&mut rx), 1);
    let mut tapped_at = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            tapped_at = job.start;
        }
    }
    // 2200 → wraps at 3000 back to 1000: the tap opens there, not at 2200.
    assert_eq!(tapped_at, Some(1000));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Count-in: the transport starts before the timeline (a pre-roll) and runs through to the
/// record point, which falls inside a device buffer. The take holds exactly the input from
/// the record point on.
#[test]
fn a_count_in_records_from_the_record_point_to_the_sample() {
    let dir = std::env::temp_dir().join("daw-record-count-in-test");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut core, mut handle) = new_engine(256);
    core.configure_io(1, 2);
    let path = dir.join("take.wav");
    let (mut job, files) = record::start(
        vec![TakeSpec {
            track: 3,
            channel: 0,
            stereo: false,
            path: path.clone(),
        }],
        48_000,
    )
    .unwrap();
    // Record point 1000, with 1500 samples of count-in: 500 of them before the timeline.
    job.from = 1000;
    job.count_in_to = Some(1000);
    for c in [
        Command::Locate(0),
        Command::PreRoll(500),
        Command::StartRecording(job),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut sent = Vec::new();
    for _ in 0..20 {
        for _ in 0..200 {
            let value = sent.len() as f32 * 1e-4;
            tx.push(value).unwrap();
            sent.push(value);
        }
        let mut out = vec![0.0f32; 200 * 2];
        core.process(&mut out, 2, Some(&mut rx), 1);
    }
    assert_eq!(
        core.position(),
        4000 - 500,
        "the pre-roll did not move the timeline"
    );
    assert!(handle.commands.push(Command::Stop).is_ok());
    core.process(&mut [0.0f32; 2], 2, Some(&mut rx), 1);
    let mut start = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            start = job.start;
        }
    }
    assert_eq!(start, Some(1000), "the take starts at the record point");

    let deadline = Instant::now() + Duration::from_secs(5);
    let file = loop {
        if let Ok(file) = files.try_recv() {
            break file;
        }
        assert!(Instant::now() < deadline, "disk writer never finished");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(file.error, None);
    let audio = wav::read(&path).unwrap();
    // 500 samples of pre-roll and 1000 of timeline went by before the record point.
    assert_eq!(audio.channels[0], sent[1500..]);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Punch in and out, both inside device buffers: the take is exactly the input between them,
/// and playback carries on past punch-out.
#[test]
fn punch_records_only_inside_the_range_to_the_sample() {
    let dir = std::env::temp_dir().join("daw-record-punch-test");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut core, mut handle) = new_engine(256);
    core.configure_io(1, 2);
    let path = dir.join("take.wav");
    let (mut job, files) = record::start(
        vec![TakeSpec {
            track: 4,
            channel: 0,
            stereo: false,
            path: path.clone(),
        }],
        48_000,
    )
    .unwrap();
    (job.from, job.to) = record::punch(0, (1100, 1650), 0).unwrap();
    for c in [
        Command::Locate(0),
        Command::StartRecording(job),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }
    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut sent = Vec::new();
    for _ in 0..15 {
        for _ in 0..200 {
            let value = sent.len() as f32 * 1e-4;
            tx.push(value).unwrap();
            sent.push(value);
        }
        let mut out = vec![0.0f32; 200 * 2];
        core.process(&mut out, 2, Some(&mut rx), 1);
    }
    assert_eq!(core.position(), 3000, "playback carries on past punch-out");
    assert!(core.is_playing());
    assert!(handle.commands.push(Command::Stop).is_ok());
    core.process(&mut [0.0f32; 2], 2, Some(&mut rx), 1);
    let mut start = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            start = job.start;
        }
    }
    assert_eq!(start, Some(1100), "the take starts at punch-in");
    let deadline = Instant::now() + Duration::from_secs(5);
    let file = loop {
        if let Ok(file) = files.try_recv() {
            break file;
        }
        assert!(Instant::now() < deadline, "disk writer never finished");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(file.error, None);
    let audio = wav::read(&path).unwrap();
    assert_eq!(audio.channels[0], sent[1100..1650]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn records_the_armed_input_channel_to_disk() {
    let dir = std::env::temp_dir().join("daw-record-test");
    let _ = std::fs::remove_dir_all(&dir);
    let (mut core, mut handle) = new_engine(256);
    core.configure_io(2, 2);

    let path = dir.join("take.wav");
    let (job, files) = record::start(
        vec![TakeSpec {
            track: 7,
            channel: 1,
            stereo: false,
            path: path.clone(),
        }],
        48_000,
    )
    .unwrap();
    assert!(handle.commands.push(Command::Locate(1000)).is_ok());
    assert!(handle.commands.push(Command::StartRecording(job)).is_ok());
    assert!(handle.commands.push(Command::Play).is_ok());

    // Input as a device would deliver it: left is noise we must not record, right is a ramp.
    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut expected = Vec::new();
    let mut n = 0u32;
    for _ in 0..20 {
        for _ in 0..200 {
            let value = (n % 1000) as f32 / 1000.0;
            tx.push(0.9).unwrap();
            tx.push(value).unwrap();
            expected.push(value);
            n += 1;
        }
        let mut out = vec![0.0f32; 200 * 2];
        core.process(&mut out, 2, Some(&mut rx), 2);
    }
    assert!(handle.commands.push(Command::Stop).is_ok());
    let mut out = vec![0.0f32; 2];
    core.process(&mut out, 2, Some(&mut rx), 2);

    // The engine hands the job back; dropping it lets the writer finish the file.
    let mut start = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            start = job.start;
            assert_eq!(job.dropped, 0);
        }
    }
    assert_eq!(start, Some(1000), "take starts where the transport started");

    let deadline = Instant::now() + Duration::from_secs(5);
    let file = loop {
        if let Ok(file) = files.try_recv() {
            break file;
        }
        assert!(Instant::now() < deadline, "disk writer never finished");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(file.error, None);
    assert_eq!(file.track, 7);
    assert_eq!(file.frames as usize, expected.len());
    let audio = wav::read(&path).unwrap();
    assert_eq!(audio.channels.len(), 1);
    assert_eq!(audio.channels[0], expected);
    let _ = std::fs::remove_dir_all(&dir);
}
