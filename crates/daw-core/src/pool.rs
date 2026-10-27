//! A small real-time worker pool for processing independent graph nodes in parallel.
//!
//! The audio thread hands out a batch of `count` tasks with [`WorkerPool::run`] and works on
//! them itself alongside the workers; tasks are claimed one at a time from a shared counter,
//! so a slow task never holds up the rest. Nothing here allocates or takes a lock once the
//! pool is running.
//!
//! Workers spin for a short while after each batch (the next level of the same block usually
//! follows within microseconds), then park until woken. Completion never waits on a parked
//! worker: unclaimed tasks are simply done by the caller.
//!
//! # Protocol
//!
//! The batch description lives in an `UnsafeCell` that only the dispatching thread writes.
//! `state` packs `(generation << 32) | next_task`; an odd generation means "closed, being
//! rewritten". A worker joins a batch by bumping `inside`, then re-checking that the
//! generation it saw is still current; the dispatcher closes the batch and waits for
//! `inside == 0` before rewriting the description. The two sides use SeqCst on these accesses,
//! so either the worker sees the batch closed or the dispatcher sees the worker inside.

use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

/// Bounds on how long an idle worker spins before parking. Within them it spins for about two
/// dispatch periods, so during playback workers stay awake from one audio block to the next
/// (waking a parked thread costs tens of microseconds, a third of a 16-sample block), and
/// park once the engine stops dispatching.
const MIN_SPIN: Duration = Duration::from_micros(200);
const MAX_SPIN: Duration = Duration::from_millis(5);

type Task = dyn Fn(usize) + Sync;

struct Batch {
    /// Lifetime-erased; valid only while the batch is open and its dispatcher waits in `run`.
    task: Option<*const Task>,
    count: usize,
}

struct Shared {
    state: AtomicU64,
    inside: AtomicUsize,
    remaining: AtomicUsize,
    batch: UnsafeCell<Batch>,
    sleeping: Vec<AtomicBool>,
    shutdown: AtomicBool,
    epoch: Instant,
    /// Dispatcher-only bookkeeping for the spin time (atomics so `run` can take `&self`).
    last_run_ns: AtomicU64,
    period_ns: AtomicU64,
    /// How long workers spin before parking, in nanoseconds.
    spin_ns: AtomicU64,
}

// SAFETY: `batch` is only written by the single dispatcher while no worker is inside (see the
// module docs), and the task it points to is `Sync`.
unsafe impl Sync for Shared {}
unsafe impl Send for Shared {}

fn generation(state: u64) -> u64 {
    state >> 32
}

/// Even generations are open batches; 0 means nothing has been published yet.
fn is_open(gen: u64) -> bool {
    gen != 0 && gen.is_multiple_of(2)
}

fn next_task(state: u64) -> usize {
    (state & 0xffff_ffff) as usize
}

pub struct WorkerPool {
    shared: Arc<Shared>,
    threads: Vec<Thread>,
    handles: Vec<JoinHandle<()>>,
}

impl WorkerPool {
    /// Start `workers` threads at audio priority.
    pub fn new(workers: usize) -> Self {
        let shared = Arc::new(Shared {
            state: AtomicU64::new(0),
            inside: AtomicUsize::new(0),
            remaining: AtomicUsize::new(0),
            batch: UnsafeCell::new(Batch {
                task: None,
                count: 0,
            }),
            sleeping: (0..workers).map(|_| AtomicBool::new(false)).collect(),
            shutdown: AtomicBool::new(false),
            epoch: Instant::now(),
            last_run_ns: AtomicU64::new(0),
            period_ns: AtomicU64::new(0),
            spin_ns: AtomicU64::new(MIN_SPIN.as_nanos() as u64),
        });
        let mut handles = Vec::new();
        for index in 0..workers {
            let shared = shared.clone();
            let handle = std::thread::Builder::new()
                .name(format!("audio-worker-{index}"))
                .spawn(move || {
                    crate::cpu::make_realtime();
                    crate::trace::set_thread(index as u32 + 1);
                    worker(&shared, index);
                })
                .expect("spawning an audio worker thread");
            handles.push(handle);
        }
        let threads = handles.iter().map(|h| h.thread().clone()).collect();
        Self {
            shared,
            threads,
            handles,
        }
    }

    pub fn workers(&self) -> usize {
        self.threads.len()
    }

    /// Run `task(i)` for every `i` in `0..count`, on the workers and the calling thread, and
    /// return once all have finished. Only one thread may call this at a time.
    pub fn run(&self, count: usize, task: &(dyn Fn(usize) + Sync)) {
        if count == 0 {
            return;
        }
        let s = &*self.shared;
        if self.threads.is_empty() || count == 1 {
            (0..count).for_each(task);
            return;
        }
        self.track_period();

        // Close the previous batch and wait for stragglers to leave it.
        // Generations alternate: odd while the description is being rewritten, even when open.
        let gen = generation(s.state.load(Ordering::SeqCst));
        let closed = if gen.is_multiple_of(2) {
            gen + 1
        } else {
            gen + 2
        };
        s.state.store(closed << 32, Ordering::SeqCst);
        while s.inside.load(Ordering::SeqCst) != 0 {
            std::hint::spin_loop();
        }
        // SAFETY: the batch is closed and no worker is inside it.
        unsafe {
            let batch = &mut *s.batch.get();
            // Erase the borrow's lifetime: workers only call it while this function waits.
            batch.task = Some(
                std::mem::transmute::<&(dyn Fn(usize) + Sync), &'static Task>(task) as *const Task,
            );
            batch.count = count;
        }
        s.remaining.store(count, Ordering::SeqCst);
        let open = closed + 1;
        s.state.store(open << 32, Ordering::SeqCst);

        // Wake parked workers, but no more than there is work for.
        let mut to_wake = count - 1;
        for (flag, thread) in s.sleeping.iter().zip(&self.threads) {
            if to_wake == 0 {
                break;
            }
            if flag.load(Ordering::SeqCst) {
                thread.unpark();
            }
            to_wake -= 1;
        }

        while let Some(i) = claim(s, open, count) {
            task(i);
            s.remaining.fetch_sub(1, Ordering::AcqRel);
        }
        while s.remaining.load(Ordering::Acquire) != 0 {
            std::hint::spin_loop();
        }
    }

    /// Follow the time between dispatches. It rises at once to a longer gap (the gap between
    /// audio blocks) and decays slowly through the short gaps between levels of one block, so
    /// it settles near the block period.
    fn track_period(&self) {
        let s = &*self.shared;
        let now = s.epoch.elapsed().as_nanos() as u64;
        let last = s.last_run_ns.swap(now, Ordering::Relaxed);
        if last == 0 {
            return;
        }
        let gap = now.saturating_sub(last);
        let period = s.period_ns.load(Ordering::Relaxed);
        let period = if gap > period {
            gap
        } else {
            period - (period - gap) / 64
        };
        s.period_ns.store(period, Ordering::Relaxed);
        let spin = (2 * period).clamp(MIN_SPIN.as_nanos() as u64, MAX_SPIN.as_nanos() as u64);
        s.spin_ns.store(spin, Ordering::Relaxed);
    }
}

/// Take the next task of batch `gen`, if any are left and the batch is still current.
fn claim(s: &Shared, gen: u64, count: usize) -> Option<usize> {
    let mut state = s.state.load(Ordering::SeqCst);
    loop {
        if generation(state) != gen || next_task(state) >= count {
            return None;
        }
        match s
            .state
            .compare_exchange_weak(state, state + 1, Ordering::SeqCst, Ordering::SeqCst)
        {
            Ok(_) => return Some(next_task(state)),
            Err(actual) => state = actual,
        }
    }
}

fn worker(s: &Shared, index: usize) {
    let mut last_gen = 0;
    let mut idle_since = Instant::now();
    loop {
        if s.shutdown.load(Ordering::Acquire) {
            return;
        }
        let state = s.state.load(Ordering::SeqCst);
        let gen = generation(state);
        if gen != last_gen && is_open(gen) {
            last_gen = gen;
            join_batch(s, gen);
            idle_since = Instant::now();
            continue;
        }
        let spin = Duration::from_nanos(s.spin_ns.load(Ordering::Relaxed));
        if idle_since.elapsed() < spin {
            std::hint::spin_loop();
            continue;
        }
        // Park; recheck after announcing it so a batch published meanwhile is not missed.
        s.sleeping[index].store(true, Ordering::SeqCst);
        let state = s.state.load(Ordering::SeqCst);
        let gen = generation(state);
        let fresh = gen != last_gen && is_open(gen);
        if !fresh && !s.shutdown.load(Ordering::Acquire) {
            std::thread::park_timeout(Duration::from_millis(100));
        }
        s.sleeping[index].store(false, Ordering::SeqCst);
        idle_since = Instant::now();
    }
}

fn join_batch(s: &Shared, gen: u64) {
    s.inside.fetch_add(1, Ordering::SeqCst);
    if generation(s.state.load(Ordering::SeqCst)) == gen {
        // SAFETY: we are inside batch `gen`, so the dispatcher cannot rewrite it until we
        // leave, and it waits in `run` (keeping the task alive) until every task is done.
        let (task, count) = unsafe {
            let batch = &*s.batch.get();
            (batch.task, batch.count)
        };
        if let Some(task) = task {
            while let Some(i) = claim(s, gen, count) {
                unsafe { (*task)(i) };
                s.remaining.fetch_sub(1, Ordering::AcqRel);
            }
        }
    }
    s.inside.fetch_sub(1, Ordering::SeqCst);
}

impl Drop for WorkerPool {
    fn drop(&mut self) {
        self.shared.shutdown.store(true, Ordering::Release);
        for thread in &self.threads {
            thread.unpark();
        }
        for handle in self.handles.drain(..) {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_every_task_exactly_once() {
        let pool = WorkerPool::new(3);
        for round in 0..2000 {
            let count = 1 + round % 17;
            let hits: Vec<AtomicUsize> = (0..count).map(|_| AtomicUsize::new(0)).collect();
            pool.run(count, &|i| {
                hits[i].fetch_add(1, Ordering::Relaxed);
            });
            assert!(
                hits.iter().all(|h| h.load(Ordering::Relaxed) == 1),
                "round {round}"
            );
            if round % 500 == 0 {
                // Let the workers park, so waking is exercised too.
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    #[test]
    fn work_actually_spreads_across_threads() {
        let pool = WorkerPool::new(3);
        let threads: Vec<std::sync::Mutex<Option<std::thread::ThreadId>>> =
            (0..4).map(|_| std::sync::Mutex::new(None)).collect();
        pool.run(4, &|i| {
            std::thread::sleep(Duration::from_millis(20));
            *threads[i].lock().unwrap() = Some(std::thread::current().id());
        });
        let mut ids: Vec<_> = threads.iter().map(|t| t.lock().unwrap().unwrap()).collect();
        ids.sort_by_key(|id| format!("{id:?}"));
        ids.dedup();
        assert!(ids.len() > 1, "all tasks ran on one thread");
    }
}
