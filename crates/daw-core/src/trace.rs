//! An opt-in timing trace of the audio thread's worst block (TD9): where a block that ran past
//! its period spent its time. Off unless [`enable`]d (one atomic load per block then). On, the
//! audio thread and the pool's workers write fixed slots of atomics (no allocation, no lock),
//! and at each block's end the block is copied into the "worst" slot if it took longest so far.
//!
//! Per block: its start, when the graph started and ended, its end, and its budget (the
//! period). Per graph level: when it opened and closed (for a parallel level, closing waits for
//! the last worker), and per node the thread that rendered it (0 the audio thread, 1… a pool
//! worker), when it started, its wall time and its CPU time (thread cycles: a node whose CPU
//! time is far below its wall time was off the CPU, preempted; one whose CPU time matches did
//! the work).

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};
use std::sync::OnceLock;
use std::time::Instant;

use std::cell::Cell;

pub const MAX_LEVELS: usize = 16;
pub const MAX_TASKS: usize = 64;

static ON: AtomicBool = AtomicBool::new(false);
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// Thread cycles per nanosecond, measured when tracing starts (as `f64` bits).
static CYCLES_PER_NS: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// 0 for the audio thread, 1… for pool workers ([`set_thread`]).
    static THREAD: Cell<u32> = const { Cell::new(0) };
}

/// Name the calling thread in the trace: `n` for pool worker `n - 1`.
pub(crate) fn set_thread(n: u32) {
    THREAD.with(|t| t.set(n));
}

struct TaskSlot {
    node: AtomicU32,
    thread: AtomicU32,
    start: AtomicU64,
    wall: AtomicU64,
    cycles: AtomicU64,
}

impl TaskSlot {
    const fn new() -> Self {
        Self {
            node: AtomicU32::new(0),
            thread: AtomicU32::new(0),
            start: AtomicU64::new(0),
            wall: AtomicU64::new(0),
            cycles: AtomicU64::new(0),
        }
    }
}

struct LevelSlot {
    open: AtomicU64,
    close: AtomicU64,
    count: AtomicU32,
    tasks: [TaskSlot; MAX_TASKS],
}

impl LevelSlot {
    const fn new() -> Self {
        Self {
            open: AtomicU64::new(0),
            close: AtomicU64::new(0),
            count: AtomicU32::new(0),
            tasks: [const { TaskSlot::new() }; MAX_TASKS],
        }
    }
}

struct BlockSlot {
    start: AtomicU64,
    graph_start: AtomicU64,
    graph_end: AtomicU64,
    end: AtomicU64,
    budget: AtomicU64,
    levels: AtomicU32,
    level: [LevelSlot; MAX_LEVELS],
}

impl BlockSlot {
    const fn new() -> Self {
        Self {
            start: AtomicU64::new(0),
            graph_start: AtomicU64::new(0),
            graph_end: AtomicU64::new(0),
            end: AtomicU64::new(0),
            budget: AtomicU64::new(0),
            levels: AtomicU32::new(0),
            level: [const { LevelSlot::new() }; MAX_LEVELS],
        }
    }

    fn copy_from(&self, other: &BlockSlot) {
        let copy = |to: &AtomicU64, from: &AtomicU64| to.store(from.load(Relaxed), Relaxed);
        copy(&self.start, &other.start);
        copy(&self.graph_start, &other.graph_start);
        copy(&self.graph_end, &other.graph_end);
        copy(&self.end, &other.end);
        copy(&self.budget, &other.budget);
        let levels = other.levels.load(Relaxed);
        self.levels.store(levels, Relaxed);
        for (to, from) in self.level.iter().zip(&other.level).take(levels as usize) {
            copy(&to.open, &from.open);
            copy(&to.close, &from.close);
            let count = from.count.load(Relaxed);
            to.count.store(count, Relaxed);
            for (t, f) in to.tasks.iter().zip(&from.tasks).take(count as usize) {
                t.node.store(f.node.load(Relaxed), Relaxed);
                t.thread.store(f.thread.load(Relaxed), Relaxed);
                copy(&t.start, &f.start);
                copy(&t.wall, &f.wall);
                copy(&t.cycles, &f.cycles);
            }
        }
    }
}

static CURRENT: BlockSlot = BlockSlot::new();
static WORST: BlockSlot = BlockSlot::new();

/// Start tracing (main thread). Measures the thread cycle rate first, for about 20 ms.
pub fn enable() {
    EPOCH.get_or_init(Instant::now);
    let (t, c) = (Instant::now(), thread_cycles());
    while t.elapsed().as_millis() < 20 {
        std::hint::spin_loop();
    }
    let rate = thread_cycles().saturating_sub(c) as f64 / t.elapsed().as_nanos() as f64;
    CYCLES_PER_NS.store(rate.to_bits(), Relaxed);
    ON.store(true, Relaxed);
    #[cfg(test)]
    TRACED_HERE.with(|t| t.set(true));
}

#[cfg(test)]
thread_local! {
    /// In tests, only the thread that enabled tracing traces: the trace is global, and other
    /// tests' graphs running alongside would write into the blocks a trace test reads.
    static TRACED_HERE: Cell<bool> = const { Cell::new(false) };
}

/// Thread cycles per nanosecond of a thread that ran all the time (0 before [`enable`]).
pub fn cycles_per_ns() -> f64 {
    f64::from_bits(CYCLES_PER_NS.load(Relaxed))
}

pub fn on() -> bool {
    #[cfg(test)]
    if !TRACED_HERE.with(Cell::get) {
        return false;
    }
    ON.load(Relaxed)
}

fn now() -> u64 {
    EPOCH.get().map_or(0, |e| e.elapsed().as_nanos() as u64)
}

/// The current thread's CPU cycles so far (0 where they can't be read).
pub fn thread_cycles() -> u64 {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::GetCurrentThread;
        use windows_sys::Win32::System::WindowsProgramming::QueryThreadCycleTime;
        let mut cycles = 0u64;
        // SAFETY: the current thread's pseudo-handle and a valid out pointer.
        unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut cycles) };
        cycles
    }
    #[cfg(not(windows))]
    0
}

/// How far apart the device's callbacks start, in multiples of their period:
/// `GAP_EDGES[i - 1] <= gap < GAP_EDGES[i]` counts in `GAPS[i]`. A driver that calls back
/// evenly fills the bucket around 1; one that runs several blocks back to back between
/// longer waits (shared-mode WASAPI behind FlexASIO) fills the first and the last.
pub const GAP_EDGES: [f64; 5] = [0.25, 0.75, 1.25, 2.0, 4.0];
static GAPS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static LAST_START: AtomicU64 = AtomicU64::new(0);

/// The callback gaps counted since the last call (see [`GAP_EDGES`]), and start over.
pub fn take_gaps() -> [u64; 6] {
    GAPS.each_ref().map(|g| g.swap(0, Relaxed))
}

/// A device callback begins: `budget_ns` until its deadline, `period_ns` its own length.
pub(crate) fn block_begin(budget_ns: u64, period_ns: u64) {
    let start = now();
    let last = LAST_START.swap(start, Relaxed);
    if last != 0 && period_ns > 0 {
        let gap = start.saturating_sub(last) as f64 / period_ns as f64;
        let bucket = GAP_EDGES.iter().take_while(|&&e| gap >= e).count();
        GAPS[bucket].fetch_add(1, Relaxed);
    }
    CURRENT.start.store(start, Relaxed);
    CURRENT.budget.store(budget_ns, Relaxed);
    CURRENT.levels.store(0, Relaxed);
    CURRENT.graph_start.store(0, Relaxed);
    CURRENT.graph_end.store(0, Relaxed);
}

/// The graph starts (the first time in this block: a long buffer runs it in several chunks).
pub(crate) fn graph_start() {
    if CURRENT.graph_start.load(Relaxed) == 0 {
        CURRENT.graph_start.store(now(), Relaxed);
    }
}

pub(crate) fn graph_end() {
    CURRENT.graph_end.store(now(), Relaxed);
}

pub(crate) fn block_end() {
    let end = now();
    CURRENT.end.store(end, Relaxed);
    let took = end.saturating_sub(CURRENT.start.load(Relaxed));
    let worst = WORST
        .end
        .load(Relaxed)
        .saturating_sub(WORST.start.load(Relaxed));
    if took > worst {
        WORST.copy_from(&CURRENT);
    }
}

/// A level opens with `nodes` (the node each task index renders); returns its slot, `None`
/// when the block has used them all. Audio thread only.
pub(crate) fn level_open(nodes: impl ExactSizeIterator<Item = usize>) -> Option<usize> {
    let level = CURRENT.levels.load(Relaxed) as usize;
    let slot = CURRENT.level.get(level)?;
    CURRENT.levels.store(level as u32 + 1, Relaxed);
    slot.open.store(now(), Relaxed);
    slot.close.store(0, Relaxed);
    slot.count.store(nodes.len().min(MAX_TASKS) as u32, Relaxed);
    for (task, node) in slot.tasks.iter().zip(nodes) {
        task.node.store(node as u32, Relaxed);
        task.wall.store(0, Relaxed);
    }
    Some(level)
}

pub(crate) fn level_close(level: usize) {
    if let Some(slot) = CURRENT.level.get(level) {
        slot.close.store(now(), Relaxed);
    }
}

/// Time a task: returns what [`task_end`] needs.
pub(crate) fn task_begin() -> (u64, u64) {
    (now(), thread_cycles())
}

pub(crate) fn task_end(level: usize, task: usize, begun: (u64, u64)) {
    let Some(slot) = CURRENT.level.get(level).and_then(|l| l.tasks.get(task)) else {
        return;
    };
    let (start, cycles) = begun;
    slot.thread.store(THREAD.with(Cell::get), Relaxed);
    slot.start.store(start, Relaxed);
    slot.wall.store(now().saturating_sub(start), Relaxed);
    slot.cycles
        .store(thread_cycles().saturating_sub(cycles), Relaxed);
}

/// One task of the worst block.
#[derive(Debug, Clone)]
pub struct TaskReport {
    pub node: usize,
    /// 0: the audio thread; 1…: a pool worker.
    pub thread: u32,
    /// Since the level opened, in microseconds.
    pub start_us: f64,
    pub wall_us: f64,
    pub cycles: u64,
}

#[derive(Debug, Clone)]
pub struct LevelReport {
    /// Since the block began, in microseconds.
    pub open_us: f64,
    /// From opening to closing (after its last node, wherever it ran).
    pub span_us: f64,
    pub tasks: Vec<TaskReport>,
}

/// The worst block since the last [`take_worst`]: its times in microseconds.
#[derive(Debug, Clone)]
pub struct BlockReport {
    pub total_us: f64,
    pub budget_us: f64,
    /// Before the graph, the graph, after it.
    pub before_us: f64,
    pub graph_us: f64,
    pub after_us: f64,
    pub levels: Vec<LevelReport>,
}

/// The worst block so far, and start over (main thread). Read during playback, it can tear if a
/// slower block replaces it meanwhile: a report whose phases don't add up to its total, or
/// whose nodes look like another block's, is suspect.
pub fn take_worst() -> Option<BlockReport> {
    let w = &WORST;
    let us = |ns: u64| ns as f64 / 1000.0;
    let (start, end) = (w.start.load(Relaxed), w.end.load(Relaxed));
    if end <= start {
        return None;
    }
    let (gs, ge) = (w.graph_start.load(Relaxed), w.graph_end.load(Relaxed));
    let report = BlockReport {
        total_us: us(end - start),
        budget_us: us(w.budget.load(Relaxed)),
        before_us: us(gs.saturating_sub(start)),
        graph_us: us(ge.saturating_sub(gs)),
        after_us: us(end.saturating_sub(ge)),
        levels: w
            .level
            .iter()
            .take(w.levels.load(Relaxed) as usize)
            .map(|l| {
                let open = l.open.load(Relaxed);
                LevelReport {
                    open_us: us(open.saturating_sub(start)),
                    span_us: us(l.close.load(Relaxed).saturating_sub(open)),
                    tasks: l
                        .tasks
                        .iter()
                        .take(l.count.load(Relaxed) as usize)
                        .map(|t| TaskReport {
                            node: t.node.load(Relaxed) as usize,
                            thread: t.thread.load(Relaxed),
                            start_us: us(t.start.load(Relaxed).saturating_sub(open)),
                            wall_us: us(t.wall.load(Relaxed)),
                            cycles: t.cycles.load(Relaxed),
                        })
                        .collect(),
                }
            })
            .collect(),
    };
    w.start.store(0, Relaxed);
    w.end.store(0, Relaxed);
    Some(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Trace one slow block by hand: its level, its two nodes and the threads that ran them.
    fn traced_block() -> Option<BlockReport> {
        block_begin(1_000_000, 1_000_000);
        graph_start();
        let level = level_open([3usize, 5].into_iter())?;
        let begun = task_begin();
        task_end(level, 0, begun);
        std::thread::spawn(move || {
            set_thread(2);
            let begun = task_begin();
            std::thread::sleep(Duration::from_millis(2));
            task_end(level, 1, begun);
        })
        .join()
        .ok()?;
        level_close(level);
        graph_end();
        block_end();
        take_worst()
    }

    #[test]
    fn reports_the_worst_block_by_node_and_thread() {
        enable();
        assert!(cycles_per_ns() > 0.0 || cfg!(not(windows)));
        // Trace state is global and other tests' graphs may write it meanwhile when tracing is
        // on, so one clean block of a few is enough.
        let blocks: Vec<_> = (0..5).filter_map(|_| traced_block()).collect();
        let clean = blocks.iter().any(|b| {
            let Some(level) = b.levels.first() else {
                return false;
            };
            let nodes: Vec<_> = level.tasks.iter().map(|t| (t.node, t.thread)).collect();
            b.total_us >= 2000.0
                && b.levels.len() == 1
                && nodes == [(3, 0), (5, 2)]
                && level.tasks[1].wall_us >= 2000.0
                && level.span_us >= level.tasks[1].wall_us
        });
        assert!(clean, "no traced block came back whole");
        // Five blocks or more since tracing began: at least four gaps counted.
        assert!(take_gaps().iter().sum::<u64>() >= 4);
    }
}
