//! A take's waveform while it records (21.1): the engine's record tap sends peak bins that the
//! main thread drains as they come, equal to the finished file's peaks at the end, and the
//! audio thread allocates nothing doing it.
//!
//! This file is its own test binary, so the counting allocator below sees only these tests.

use daw_core::engine::{new_engine, Command, Garbage};
use daw_core::media::AudioData;
use daw_core::record::{self, TakeSpec};
use daw_core::wav;
use rtrb::RingBuffer;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// The system allocator, counting the allocations and frees made on a thread while it is
/// watched ([`allocations_in`]); other threads (the disk writer) are not counted.
struct Counting;

thread_local! {
    static WATCHED: Cell<bool> = const { Cell::new(false) };
}
static COUNTED: AtomicU64 = AtomicU64::new(0);

fn note() {
    if WATCHED.try_with(Cell::get).unwrap_or(false) {
        COUNTED.fetch_add(1, Ordering::Relaxed);
    }
}

// SAFETY: every call is passed straight to `System`, which upholds `GlobalAlloc`'s contract;
// counting touches only an atomic and a const-initialised thread local, which never allocate.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: the caller's contract is `System`'s.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note();
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note();
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note();
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

/// Allocations and frees on this thread while `f` runs.
fn allocations_in(f: impl FnOnce()) -> u64 {
    let before = COUNTED.load(Ordering::Relaxed);
    WATCHED.with(|w| w.set(true));
    f();
    WATCHED.with(|w| w.set(false));
    COUNTED.load(Ordering::Relaxed) - before
}

/// Two armed tracks, one mono on input 2 and one stereo on inputs 1-2, recorded through a punch
/// range that opens and closes inside 200-frame device buffers (neither lines up with the
/// 256-frame peak bins). Peaks reach the main thread between callbacks while the take grows;
/// once the engine lets go, they are the finished files' peaks bit for bit, the last partial
/// bin included. No callback allocates or frees, recording or stopping.
#[test]
fn a_takes_peaks_arrive_while_it_records_and_are_the_files() {
    const PUNCH: (u64, u64) = (1100, 9000);
    // The counter counts: one allocation and its free.
    assert_eq!(
        allocations_in(|| drop(std::hint::black_box(vec![1u8; 10]))),
        2
    );
    let dir = std::env::temp_dir().join(format!("daw-record-peaks-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let (mut core, mut handle) = new_engine(256);
    core.configure_io(2, 2);
    core.shared().set_sample_rate(48_000);
    let paths = [dir.join("mono.wav"), dir.join("stereo.wav")];
    let started = record::begin(
        vec![
            TakeSpec {
                track: 1,
                channel: 1,
                stereo: false,
                path: paths[0].clone(),
            },
            TakeSpec {
                track: 2,
                channel: 0,
                stereo: true,
                path: paths[1].clone(),
            },
        ],
        48_000,
    )
    .unwrap();
    let (mut job, files, mut live) = (started.job, started.files, started.peaks);
    assert_eq!(live.len(), 2);
    assert_eq!((live[0].channels, live[1].channels), (1, 2));
    (job.from, job.to) = record::punch(0, PUNCH, 0).unwrap();
    for c in [
        Command::Locate(0),
        Command::StartRecording(job),
        Command::Play,
    ] {
        assert!(handle.commands.push(c).is_ok());
    }

    let (mut tx, mut rx) = RingBuffer::<f32>::new(1 << 16);
    let mut out = vec![0.0f32; 200 * 2];
    let mut n = 0u32;
    let mut allocations = 0;
    let mut seen = Vec::new();
    for _ in 0..60 {
        for _ in 0..200 {
            tx.push(((n * 37) % 1000) as f32 / 1000.0 - 0.5).unwrap();
            tx.push(((n * 91) % 977) as f32 / 977.0 * 1.6 - 0.8)
                .unwrap();
            n += 1;
        }
        allocations += allocations_in(|| core.process(&mut out, 2, Some(&mut rx), 2));
        for take in &mut live {
            take.drain();
        }
        seen.push(live[0].frames);
        assert_eq!(live[0].frames, live[1].frames);
        assert!(
            live[0].frames.is_multiple_of(256),
            "whole bins until the end"
        );
    }
    assert!(
        seen.iter().any(|&f| f > 0 && f < 7000),
        "peaks arrived while it recorded: {seen:?}"
    );
    assert!(seen.windows(2).all(|w| w[0] <= w[1]), "{seen:?}");

    assert!(handle.commands.push(Command::Stop).is_ok());
    allocations += allocations_in(|| core.process(&mut out[..2], 2, Some(&mut rx), 2));
    assert_eq!(allocations, 0, "the audio thread allocated or freed");

    let mut start = None;
    while let Ok(garbage) = handle.garbage.pop() {
        if let Garbage::Recording(job) = garbage {
            start = job.start;
            assert_eq!(job.dropped, 0);
            assert!(job
                .takes
                .iter()
                .all(|t| t.peaks.as_ref().unwrap().dropped == 0));
        }
    }
    assert_eq!(start, Some(PUNCH.0));
    for take in &mut live {
        take.drain();
    }
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut finished = Vec::new();
    while finished.len() < 2 {
        if let Ok(file) = files.try_recv() {
            finished.push(file);
        }
        assert!(Instant::now() < deadline, "disk writer never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
    for (path, take) in paths.iter().zip(&live) {
        let file = finished.iter().find(|f| f.track == take.track).unwrap();
        assert_eq!(file.error, None);
        assert_eq!(file.frames, PUNCH.1 - PUNCH.0);
        assert_eq!(take.frames, file.frames, "the last partial bin too");
        let audio = wav::read(path).unwrap();
        assert_eq!(audio.channels.len(), take.channels);
        let data = AudioData::new(48_000, audio.channels);
        assert_eq!(take.peaks, data.peaks, "track {}", take.track);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
