//! Serial vs parallel CPU benchmark over independent plugin instances.
//!
//! Each instance is its own [`Chain`] and is only ever driven by one thread at a time;
//! different instances run concurrently. That is the VST3 threading contract and the model
//! the engine will use for parallel tracks.

use crate::engine::Chain;
use anyhow::Result;
use std::time::{Duration, Instant};

/// One instance plus its preallocated I/O block.
pub struct Instance {
    pub chain: Chain,
    io: Vec<Vec<f32>>,
}

impl Instance {
    pub fn new(chain: Chain) -> Self {
        let io = vec![vec![0.0; chain.block_size()]; chain.channels()];
        Self { chain, io }
    }

    /// Process `blocks` blocks of `signal` (looped) and return the slowest block time.
    fn run(&mut self, signal: &[Vec<f32>], blocks: usize) -> Result<Duration> {
        let block = self.chain.block_size();
        let len = signal[0].len();
        let mut worst = Duration::ZERO;
        let mut pos = 0;
        for _ in 0..blocks {
            for (buf, src) in self.io.iter_mut().zip(signal.iter().cycle()) {
                for (i, s) in buf.iter_mut().enumerate() {
                    *s = src[(pos + i) % len];
                }
            }
            let t = Instant::now();
            self.chain.process_block(&mut self.io)?;
            worst = worst.max(t.elapsed());
            pos = (pos + block) % len;
        }
        Ok(worst)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Timing {
    pub wall: Duration,
    /// Slowest single block seen on any instance.
    pub worst_block: Duration,
}

#[derive(Debug, Clone, Copy)]
pub struct BenchReport {
    pub instances: usize,
    pub threads: usize,
    pub blocks: usize,
    pub audio_seconds_per_instance: f64,
    pub block_deadline: Duration,
    pub serial: Timing,
    pub parallel: Timing,
}

impl BenchReport {
    pub fn speedup(&self) -> f64 {
        self.serial.wall.as_secs_f64() / self.parallel.wall.as_secs_f64()
    }

    /// Total audio seconds processed (all instances) per wall-clock second.
    pub fn realtime_factor(t: &Timing, audio_seconds_total: f64) -> f64 {
        audio_seconds_total / t.wall.as_secs_f64()
    }

    pub fn audio_seconds_total(&self) -> f64 {
        self.audio_seconds_per_instance * self.instances as f64
    }
}

/// Number of blocks needed to cover `seconds` of audio.
pub fn blocks_for(seconds: f64, sample_rate: f64, block: usize) -> usize {
    ((seconds * sample_rate) / block as f64).ceil().max(1.0) as usize
}

/// Split `n` instances over `threads` threads as evenly as possible (chunk sizes).
pub fn chunk_size(n: usize, threads: usize) -> usize {
    n.div_ceil(threads.clamp(1, n.max(1))).max(1)
}

pub fn run(
    instances: &mut [Instance],
    signal: &[Vec<f32>],
    sample_rate: f64,
    seconds: f64,
    threads: usize,
) -> Result<BenchReport> {
    let block = instances[0].chain.block_size();
    let blocks = blocks_for(seconds, sample_rate, block);

    // Warm up every instance (caches, lazy allocations, denormal ramps) outside the clock.
    let warmup = blocks_for(0.5, sample_rate, block);
    for inst in instances.iter_mut() {
        inst.run(signal, warmup)?;
    }

    let t = Instant::now();
    let mut worst = Duration::ZERO;
    for inst in instances.iter_mut() {
        worst = worst.max(inst.run(signal, blocks)?);
    }
    let serial = Timing {
        wall: t.elapsed(),
        worst_block: worst,
    };

    let chunk = chunk_size(instances.len(), threads);
    let used_threads = instances.len().div_ceil(chunk);
    let t = Instant::now();
    let results: Vec<Result<Duration>> = std::thread::scope(|scope| {
        let handles: Vec<_> = instances
            .chunks_mut(chunk)
            .map(|group| {
                scope.spawn(move || {
                    let mut worst = Duration::ZERO;
                    for inst in group {
                        worst = worst.max(inst.run(signal, blocks)?);
                    }
                    Ok(worst)
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("benchmark thread panicked"))
            .collect()
    });
    let wall = t.elapsed();
    let mut worst = Duration::ZERO;
    for r in results {
        worst = worst.max(r?);
    }

    Ok(BenchReport {
        instances: instances.len(),
        threads: used_threads,
        blocks,
        audio_seconds_per_instance: (blocks * block) as f64 / sample_rate,
        block_deadline: Duration::from_secs_f64(block as f64 / sample_rate),
        serial,
        parallel: Timing {
            wall,
            worst_block: worst,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_counts() {
        assert_eq!(blocks_for(1.0, 48_000.0, 512), 94);
        assert_eq!(blocks_for(0.0, 48_000.0, 512), 1);
    }

    #[test]
    fn chunking() {
        assert_eq!(chunk_size(8, 8), 1);
        assert_eq!(chunk_size(8, 3), 3);
        assert_eq!(chunk_size(4, 16), 1);
        assert_eq!(chunk_size(5, 1), 5);
    }
}
