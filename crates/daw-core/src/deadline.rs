//! The DSP load against the deadlines the audio driver really sets (TD91).
//!
//! Measuring each callback against its own period assumes the driver calls back evenly, once a
//! period. Some don't: FlexASIO on shared-mode WASAPI asks for about thirty 16-sample blocks back
//! to back, then waits about 10 ms. A 16-sample block there may take longer than 16 samples'
//! worth of time and still be in time, because the burst as a whole has the 10 ms.
//!
//! So callbacks are grouped into *windows*: a callback that starts right after the previous one
//! ended (less than an eighth of a period later) continues its window. A callback's deadline is
//! the window's start plus the audio the window has delivered so far, or the driver's *cadence*
//! if that is longer, and its load is the time from the window's start to its end over that
//! span. Over 1.0 is late.
//!
//! - An even driver's callbacks start one slot apart, so the deadline is the end of the
//!   callback's own slot, even when a heavy load runs them into one window.
//! - A bursty driver gets the whole burst: the cadence is the audio of a window that ends in a
//!   *wait* (a gap of more than two periods), the median of the last [`HISTORY`] of them. A
//!   window counts as a burst only if it delivered its audio in under 90% of its duration: its
//!   callbacks came faster than real time. An even driver's never do (each starts in its own
//!   slot, so a window of k of them lasts at least k − 1 periods plus a heavy last block), so its
//!   rare late callbacks (ASIO4ALL pauses for more than two periods now and then) count as
//!   windows of one block and its cadence stays a block.
//! - Back to back with no wait at all (a driver kept behind), the load is the window's
//!   time over its audio: steady above 1.0.
//!
//! Known looseness: a callback delayed by more than an eighth of a period in the middle of a
//! burst (the thread preempted between callbacks) starts a new window, whose deadline is then
//! later than the driver's.
//!
//! Times are nanoseconds from any fixed origin, so this is a pure calculation the engine feeds.

/// How many windows ending in a wait the cadence is the median of.
pub const HISTORY: usize = 16;

#[derive(Debug, Clone)]
pub struct DeadlineMeter {
    rate: f64,
    block: usize,
    last_end: Option<u64>,
    /// The current window: when it started and the frames it has delivered.
    window_start: u64,
    window_frames: u64,
    spans: [u64; HISTORY],
    span_count: usize,
    next_span: usize,
    cadence: u64,
    running: f32,
    closed: f32,
}

impl Default for DeadlineMeter {
    fn default() -> Self {
        Self {
            rate: 0.0,
            block: 0,
            last_end: None,
            window_start: 0,
            window_frames: 0,
            spans: [0; HISTORY],
            span_count: 0,
            next_span: 0,
            cadence: 0,
            running: 0.0,
            closed: 0.0,
        }
    }
}

impl DeadlineMeter {
    /// A callback of `frames` at `rate` Hz starts at `start` ns. Returns how long it has until
    /// its deadline, in ns (for the trace). A new block size or rate starts over.
    pub fn begin(&mut self, start: u64, frames: usize, rate: f64) -> u64 {
        if frames != self.block || rate != self.rate {
            *self = Self {
                rate,
                block: frames,
                ..Self::default()
            };
        }
        let period = self.ns(frames as u64);
        match self.last_end {
            Some(last_end) => {
                let gap = start.saturating_sub(last_end);
                if gap > 2 * period {
                    self.driver_waited(last_end);
                }
                if gap > period / 8 {
                    self.closed = self.running;
                    self.window_start = start;
                    self.window_frames = 0;
                }
            }
            None => {
                self.window_start = start;
                self.window_frames = 0;
            }
        }
        self.window_frames += frames as u64;
        let deadline = self.window_start + self.ns(self.window_frames.max(self.cadence));
        deadline.saturating_sub(start)
    }

    /// The callback ends at `end` ns. Returns its load: over 1.0 is late.
    pub fn end(&mut self, end: u64) -> f32 {
        let span = self.ns(self.window_frames.max(self.cadence)).max(1);
        self.running = end.saturating_sub(self.window_start) as f32 / span as f32;
        self.last_end = Some(end);
        self.running
    }

    /// The load to show: the current window's, or the last finished window's if higher (the
    /// current one only climbs while a burst runs).
    pub fn shown(&self) -> f32 {
        self.running.max(self.closed)
    }

    /// The audio the driver asks for in a burst, in frames; 0 for a driver that calls back
    /// evenly (or before it has waited).
    pub fn cadence(&self) -> u64 {
        if self.cadence > self.block as u64 {
            self.cadence
        } else {
            0
        }
    }

    /// The driver waited after the window that ended at `last_end`: count it as a burst if its
    /// callbacks came faster than real time, else as one block.
    fn driver_waited(&mut self, last_end: u64) {
        let took = last_end.saturating_sub(self.window_start) as f64;
        let audio = self.ns(self.window_frames) as f64;
        let burst = self.window_frames > self.block as u64 && took < 0.9 * audio;
        self.spans[self.next_span] = if burst {
            self.window_frames
        } else {
            self.block as u64
        };
        self.next_span = (self.next_span + 1) % HISTORY;
        self.span_count = (self.span_count + 1).min(HISTORY);
        let mut sorted = self.spans;
        let sorted = &mut sorted[..self.span_count];
        sorted.sort_unstable();
        self.cadence = sorted[sorted.len() / 2];
    }

    fn ns(&self, frames: u64) -> u64 {
        (frames as f64 * 1e9 / self.rate) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 48_000.0;
    const FRAMES: usize = 48;
    /// One period of `FRAMES`: 1 ms.
    const P: u64 = 1_000_000;

    /// Run callbacks `(start, duration)` in ns; returns each one's load.
    fn run(meter: &mut DeadlineMeter, calls: &[(u64, u64)]) -> Vec<f32> {
        calls
            .iter()
            .map(|&(start, took)| {
                meter.begin(start, FRAMES, RATE);
                meter.end(start + took)
            })
            .collect()
    }

    /// An even driver: one callback per period, each taking `load` of it, except `slow`.
    fn even(blocks: u64, load: f64, slow: Option<(u64, f64)>) -> Vec<(u64, u64)> {
        (0..blocks)
            .map(|k| {
                let load = match slow {
                    Some((at, l)) if at == k => l,
                    _ => load,
                };
                (k * P, (load * P as f64) as u64)
            })
            .collect()
    }

    #[test]
    fn an_even_driver_is_measured_per_period() {
        let mut m = DeadlineMeter::default();
        let loads = run(&mut m, &even(100, 0.5, None));
        assert!(loads.iter().all(|l| (l - 0.5).abs() < 1e-3), "{loads:?}");
        assert_eq!(m.cadence(), 0);
    }

    #[test]
    fn a_late_block_reads_over_one_even_when_blocks_run_together() {
        // At 90% the gaps (0.1 P) are under an eighth of a period: one long window.
        for load in [0.5, 0.8, 0.9] {
            let mut m = DeadlineMeter::default();
            let loads = run(&mut m, &even(60, load, Some((50, 1.15))));
            assert!(loads[50] > 1.0, "{load}: {:?}", &loads[48..53]);
            assert!(loads[..50].iter().all(|&l| l <= 1.0), "{load}");
            assert_eq!(m.cadence(), 0, "{load}");
        }
    }

    #[test]
    fn an_even_driver_that_sometimes_pauses_keeps_a_cadence_of_one_block() {
        // ASIO4ALL now and then calls back more than two periods late; at 50% or 92% load.
        for load in [0.5, 0.92] {
            let mut m = DeadlineMeter::default();
            let calls: Vec<_> = even(400, load, Some((390, 1.15)))
                .into_iter()
                .enumerate()
                .map(|(k, (start, took))| (start + (k as u64 / 40) * 3 * P, took))
                .collect();
            let loads = run(&mut m, &calls);
            assert_eq!(m.cadence(), 0, "{load}");
            assert!(loads[390] > 1.0, "{load}: {}", loads[390]);
        }
    }

    /// FlexASIO's shape: `burst` blocks back to back (each `took`), then a wait, every
    /// `burst` periods.
    fn bursts(count: u64, burst: u64, took: u64, slow: Option<(u64, u64)>) -> Vec<(u64, u64)> {
        let mut calls = Vec::new();
        for b in 0..count {
            let mut at = b * burst * P;
            for k in 0..burst {
                let took = match slow {
                    Some((i, t)) if i == b * burst + k => t,
                    _ => took,
                };
                calls.push((at, took));
                at += took + 2_000; // 2 µs between callbacks
            }
        }
        calls
    }

    #[test]
    fn a_burst_has_the_whole_cadence() {
        let mut m = DeadlineMeter::default();
        // 30 blocks of 0.07 P back to back, then the driver waits.
        let slow_at = 30 * 20 + 10;
        let loads = run(
            &mut m,
            &bursts(40, 30, P * 7 / 100, Some((slow_at, P * 13 / 10))),
        );
        assert_eq!(m.cadence(), 30 * FRAMES as u64);
        // Once learned, a burst reads its share of the cadence (2.1 of 30 periods)...
        let late = &loads[30 * 5..];
        assert!(late.iter().all(|&l| l < 0.12), "{:?}", &late[..31]);
        // ...and a block taking 1.3 periods inside one is no dropout.
        assert!(loads[slow_at as usize] < 0.3, "{}", loads[slow_at as usize]);
        assert!(m.shown() < 0.12);
    }

    #[test]
    fn a_driver_kept_behind_reads_its_overload() {
        let mut m = DeadlineMeter::default();
        // Each block takes 1.22 periods and the driver calls straight back: no wait ever.
        let calls: Vec<_> = (0..200)
            .map(|k| (k * (P * 122 / 100 + 2_000), P * 122 / 100))
            .collect();
        let loads = run(&mut m, &calls);
        assert!(
            loads[100..].iter().all(|&l| l > 1.2 && l < 1.24),
            "{:?}",
            &loads[100..103]
        );
        assert_eq!(m.cadence(), 0);
    }

    #[test]
    fn a_new_block_size_starts_over() {
        let mut m = DeadlineMeter::default();
        run(&mut m, &bursts(10, 30, P / 10, None));
        assert_eq!(m.cadence(), 30 * FRAMES as u64);
        m.begin(100 * P, 64, RATE);
        assert_eq!(m.cadence(), 0);
    }
}
