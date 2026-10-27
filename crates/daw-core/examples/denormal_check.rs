//! Denormals in the built-in effects (dev plan 18.1, TD128): every effect's cost on a silent
//! tail against its cost on noise, block by block.
//!
//! A recursive filter, envelope or feedback loop whose input stops decays towards zero, and
//! f32/f64 values under the normal range (denormals, below about 1e-38 / 1e-308) cost the CPU
//! tens of times a normal operation. An effect that lets its state decay into them gets
//! dearer when the music stops, for seconds or (a limit cycle) for ever, which is exactly the
//! worst block the CPU-scaling goal judges. Each effect flushes its state where it decays,
//! under 1e-25 in f32 and 1e-30 in f64 (`native::flush`, `native::flush_all`), or (the Hard
//! Limiter) no longer scans its window when its level holds flat.
//!
//! For every kind in `native::KINDS`, at its defaults and at a long-tail setting where it has
//! one, this runs 4 s of stereo noise at -20 dBFS RMS and then 20 s of exact silence through
//! the inserts' own `NativeInsert` (offline, transport playing, on a thread at audio priority),
//! in 64- and 512-sample blocks at 48 kHz, timing each block. It prints per effect the mean,
//! 99.9th percentile and worst block on the noise (after its first half second) and on the tail
//! (after its first second), and the tail's worst 250 ms window (its mean), each the median of
//! `--runs` runs (default 3; every case once, then every case again, so a case's runs lie a
//! whole pass apart), then the tail over the noise for the mean, the 99.9th percentile and the
//! worst window (the noise's own worst window), each the least of the runs: a denormal costs
//! in every run, the scheduler's interruptions and the machine's other work don't.
//!
//! An effect fails when, in every run, its tail's mean is over 1.5 times the noise's, its worst
//! window over 2 times or its 99.9th percentile over 3 times, each beyond a slack of 0.5 µs a
//! block so a cheap effect's timer jitter cannot fail it (`--margins 1.5,2,3`). Over 1.25, 1.5
//! and 2 in every run (the tight margins) prints a `WARN:` line. Single worst blocks are
//! printed, not judged: the scheduler owns them. Measured (18.1): with every effect fixed the
//! least ratios reach 1.16 (mean), 1.37 (window) and 1.31 (99.9th); before, the Limiter and
//! Clip Repair cost 30 to 70 times as much on the tail, the Multiband Compressor, Dynamics
//! Processing and the De-esser 1.4 to 2.1 times.
//!
//! cargo run --release -p daw-core --example denormal_check [-- --runs 3] [--only parametric-eq,...]
//!   [--blocks 64,512] [--tail 20]
//! cargo run --release -p daw-core --example denormal_check -- --list
//! cargo run --release -p daw-core --example denormal_check -- --render out/check/denormal/before --audio in.wav
//! cargo run --release -p daw-core --example denormal_check -- --null <before> <after>
//!
//! `--render` writes each effect's output (every setting) on a recording (`--audio`, read
//! only) and on the noise, each followed by 10 s of silence, as 32-bit float WAVs; `--null`
//! compares the files of the second folder with their namesakes in the first and fails if any
//! sample differs by more than -300 dBFS (a flush moves only values under its floor and what
//! follows from them).
//!
//! Prints `RESULT: PASS` or `FAIL:` lines and exits non-zero on failure.

use anyhow::{anyhow, bail, Context, Result};
use daw_core::graph::{Processor, Transport, CHANNELS};
use daw_core::native::{self, Kind};
use daw_core::wav;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The engine's rate for the timing runs.
const RATE: f64 = 48_000.0;
/// Seconds of noise before the silence.
const NOISE_SECONDS: f64 = 4.0;
/// Noise blocks before this are warm-up, not measured.
const NOISE_SKIP: f64 = 0.5;
/// Tail blocks in the first second after the noise are not measured (a real tail rings).
const TAIL_SKIP: f64 = 1.0;
/// The window whose mean cost catches a stretch of dear blocks (a decay through denormals).
const WINDOW_SECONDS: f64 = 0.25;
/// A block's excess under this (µs) never fails: a cheap effect's timer jitter.
const SLACK_US: f64 = 0.5;
/// Where the generated impulse for the Convolution Reverb goes.
const WORK: &str = "out/check/denormal";

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--list") {
        list();
        return Ok(());
    }
    if let Some(at) = args.iter().position(|a| a == "--null") {
        let before = PathBuf::from(args.get(at + 1).ok_or_else(|| anyhow!("--null <a> <b>"))?);
        let after = PathBuf::from(args.get(at + 2).ok_or_else(|| anyhow!("--null <a> <b>"))?);
        return null(&before, &after);
    }
    let only: Option<Vec<String>> =
        take(&mut args, "--only").map(|s| s.split(',').map(str::to_owned).collect());
    let kinds: Vec<&'static Kind> = native::KINDS
        .iter()
        .copied()
        .filter(|k| only.as_ref().is_none_or(|o| o.iter().any(|n| n == k.id)))
        .collect();
    if kinds.is_empty() {
        bail!("no effect matches --only");
    }
    std::fs::create_dir_all(WORK)?;
    let impulse = write_impulse()?;
    let audio = take(&mut args, "--audio");
    if let Some(dir) = take(&mut args, "--render") {
        let audio = audio.ok_or_else(|| anyhow!("--render <dir> --audio <wav>"))?;
        return render_all(&kinds, Path::new(&dir), &impulse, Path::new(&audio));
    }
    let runs: usize = take(&mut args, "--runs").map_or(Ok(3), |s| s.parse())?;
    let tail: f64 = take(&mut args, "--tail").map_or(Ok(20.0), |s| s.parse())?;
    let margins: Vec<f64> = take(&mut args, "--margins")
        .unwrap_or_else(|| "1.5,2,3".into())
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    let &[margin_mean, margin_window, margin_p999] = margins.as_slice() else {
        bail!("--margins takes three: mean, worst window, 99.9th percentile");
    };
    let blocks: Vec<usize> = take(&mut args, "--blocks")
        .unwrap_or_else(|| "64,512".into())
        .split(',')
        .map(str::parse)
        .collect::<Result<_, _>>()?;
    if let Some(extra) = args.first() {
        bail!("unknown argument '{extra}'");
    }
    let noise = noise(RATE, NOISE_SECONDS, 0x5eed);
    // As the engine's audio thread runs, so the scheduler interrupts it as little.
    daw_core::cpu::make_realtime();
    let clock = Clock::calibrate();
    let mut failures = Vec::new();
    let mut warnings = Vec::new();
    println!(
        "{:<26} {:<9} {:>5} | {:>7} {:>7} {:>7} | {:>7} {:>7} {:>7} {:>7} | {:>6} {:>6} {:>6}",
        "effect",
        "setting",
        "block",
        "noise",
        "p99.9",
        "max",
        "tail",
        "p99.9",
        "max",
        "window",
        "x mean",
        "x p999",
        "x win"
    );
    // Every case once, then every case again: a run's repeats lie a whole pass apart, so no
    // burst of other work on the machine can land on all of them.
    let mut cases = Vec::new();
    for kind in &kinds {
        for (setting, params) in settings(kind) {
            let print = kind.capture.map(|_| noise.clone());
            let file = (kind.reads.is_some() && !kind.reads.is_some_and(|r| r.makes()))
                .then_some(impulse.as_path());
            for &block in &blocks {
                cases.push((*kind, setting, params.clone(), print.clone(), file, block));
            }
        }
    }
    let mut results: Vec<Vec<Run>> = vec![Vec::with_capacity(runs); cases.len()];
    for _ in 0..runs {
        for ((kind, _, params, print, file, block), each) in cases.iter().zip(&mut results) {
            each.push(time(
                kind,
                params,
                print.as_deref(),
                *file,
                &noise,
                tail,
                *block,
                &clock,
            )?);
        }
    }
    for ((kind, setting, _, _, _, block), each) in cases.iter().zip(&results) {
        let s = Run::median_of(each);
        let least = |f: &dyn Fn(&Run) -> f64| each.iter().map(f).fold(f64::INFINITY, f64::min);
        let x_mean = least(&|r| r.tail.mean / r.noise.mean);
        let x_p999 = least(&|r| r.tail.p999 / r.noise.p999);
        let x_win = least(&|r| r.tail.window / r.noise.window);
        println!(
            "{:<26} {:<9} {:>5} | {:>7.2} {:>7.2} {:>7.2} | {:>7.2} {:>7.2} {:>7.2} {:>7.2} | {:>6.2} {:>6.2} {:>6.2}",
            kind.id,
            setting,
            block,
            s.noise.mean,
            s.noise.p999,
            s.noise.max,
            s.tail.mean,
            s.tail.p999,
            s.tail.max,
            s.tail.window,
            x_mean,
            x_p999,
            x_win
        );
        let name = format!("{} ({setting}, {block})", kind.id);
        let figures = format!("mean x{x_mean:.2}, 99.9th x{x_p999:.2}, worst window x{x_win:.2}");
        if each
            .iter()
            .all(|r| r.over(margin_mean, margin_window, margin_p999))
        {
            failures.push(format!(
                "{name}: the silent tail costs more than the noise ({figures})"
            ));
        } else if each.iter().all(|r| r.over(1.25, 1.5, 2.0)) {
            warnings.push(format!("{name}: over the tight margins ({figures})"));
        }
    }
    println!(
        "(us a block, the median of {runs} runs; window: the tail's worst {WINDOW_SECONDS} s \
         window's mean; x: tail over noise, the least of the runs)"
    );
    warnings.iter().for_each(|w| println!("WARN: {w}"));
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("denormal check failed")
    }
}

/// The value after `flag`, removing both.
fn take(args: &mut Vec<String>, flag: &str) -> Option<String> {
    let at = args.iter().position(|a| a == flag)?;
    if at + 1 >= args.len() {
        return None;
    }
    let value = args.remove(at + 1);
    args.remove(at);
    Some(value)
}

/// Every effect's parameters, to pick the long-tail settings by name.
fn list() {
    for kind in native::KINDS {
        println!("{} ({})", kind.id, kind.name);
        for (i, p) in kind.params.iter().enumerate() {
            println!(
                "  {i:>2} {:<40} {:>10} .. {:<10} default {:<10} {} steps {}",
                p.name, p.min, p.max, p.default, p.unit, p.steps
            );
        }
    }
}

/// The settings each effect is timed at: its defaults, and one that rings longest where it
/// has one (long decays, high feedback, high Q), as plain values by name.
fn settings(kind: &Kind) -> Vec<(&'static str, BTreeMap<String, f64>)> {
    let mut out = vec![("default", BTreeMap::new())];
    let long: &[(&str, f64)] = match kind.id {
        "parametric-eq" => &[
            ("Band 1 Gain", 18.0),
            ("Band 1 Q", 40.0),
            ("Band 2 Gain", -18.0),
            ("Band 2 Q", 40.0),
            ("Band 3 Gain", 12.0),
            ("Band 3 Q", 100.0),
            ("High-Pass Enable", 1.0),
            ("High-Pass Frequency", 30.0),
            ("High-Pass Slope", 8.0),
            ("Low-Pass Enable", 1.0),
            ("Low-Pass Frequency", 6000.0),
            ("Low-Pass Slope", 8.0),
            ("Low Shelf Gain", 9.0),
            ("Low Shelf Second Order", 1.0),
            ("High Shelf Gain", -9.0),
        ],
        "limiter" => &[("Input Boost", 20.0), ("Release Time", 200.0)],
        "compressor" => &[("Threshold", -40.0), ("Ratio", 10.0), ("Release", 5000.0)],
        "de-esser" => &[("Threshold", -60.0), ("Release", 500.0)],
        "dynamics" => &[
            ("Level Detector Release", 2000.0),
            ("Gain Processor Release", 2000.0),
            ("Lookahead Time", 50.0),
            ("X2", -40.0),
            ("Y2", -60.0),
        ],
        "multiband-compressor" => &[
            ("Band 1 Threshold", -40.0),
            ("Band 1 Ratio", 10.0),
            ("Band 1 Release", 5000.0),
            ("Band 2 Threshold", -40.0),
            ("Band 2 Ratio", 10.0),
            ("Band 2 Release", 5000.0),
            ("Band 3 Threshold", -40.0),
            ("Band 3 Ratio", 10.0),
            ("Band 3 Release", 5000.0),
            ("Band 4 Threshold", -40.0),
            ("Band 4 Ratio", 10.0),
            ("Band 4 Release", 5000.0),
            ("Limiter Threshold", -10.0),
            ("Brickwall Limiter", 1.0),
        ],
        "noise-reduction" | "hiss-reduction" => &[("Spectral Decay Rate", 100.0)],
        "auto-noise-reduction" => &[("Spectral Decay Rate", 750.0)],
        "hum-remover" => &[("Q", 3000.0), ("Gain", -80.0)],
        "room-reverb" => &[
            ("Decay", 10000.0),
            ("Room Size", 100.0),
            ("Damping", 0.0),
            ("Low Frequency Cut", 20.0),
            ("High Frequency Cut", 20000.0),
        ],
        "convolution-reverb" => &[("Pre-Delay", 100.0), ("Damping LF", 100.0)],
        "space-reverb" => &[("Reverberation Decay Time", 6000.0)],
        "echo" => &[
            ("Feedback (Left Channel)", 95.0),
            ("Feedback (Right Channel)", 95.0),
            ("EQ Band 1", 0.0),
            ("EQ Band 2", 0.0),
            ("EQ Band 3", 0.0),
            ("EQ Band 4", 0.0),
            ("EQ Band 5", 0.0),
            ("EQ Band 6", 0.0),
            ("EQ Band 7", 0.0),
        ],
        "vintage-delay" => &[("Feedback", 95.0), ("Delay", 2000.0)],
        "chorus" => &[("Feedback", 50.0), ("Voices", 8.0)],
        "flanger" => &[("Feedback", 100.0)],
        "phaser" => &[("Feedback", 90.0), ("Stages", 6.0)],
        _ => &[],
    };
    let mut long: BTreeMap<String, f64> = long.iter().map(|&(n, v)| (n.to_owned(), v)).collect();
    // The graphic EQs' bands by their own names: alternate boosts and cuts at full range.
    if kind.id.starts_with("graphic-eq") {
        for (i, p) in kind.params.iter().skip(2).enumerate() {
            long.insert(p.name.to_owned(), if i % 2 == 0 { 24.0 } else { -24.0 });
        }
    }
    if !long.is_empty() {
        out.push(("long-tail", long));
    }
    out
}

/// A stretch of one run's per-block costs, summed up (µs).
#[derive(Clone, Copy, Default)]
struct Part {
    mean: f64,
    p999: f64,
    max: f64,
    /// The worst window's mean.
    window: f64,
}

/// One run: the noise's blocks and the silent tail's.
#[derive(Clone, Copy, Default)]
struct Run {
    noise: Part,
    tail: Part,
}

impl Part {
    /// From per-block costs in µs, `per_window` blocks to a window.
    fn of(costs: &[f64], per_window: usize) -> Self {
        let mut sorted = costs.to_vec();
        sorted.sort_by(f64::total_cmp);
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q).round() as usize];
        let per_window = per_window.max(1);
        let window = costs
            .chunks_exact(per_window)
            .map(|w| w.iter().sum::<f64>() / per_window as f64)
            .fold(0.0, f64::max);
        Self {
            mean: costs.iter().sum::<f64>() / costs.len() as f64,
            p999: at(0.999),
            max: at(1.0),
            window,
        }
    }
}

impl Run {
    /// Each figure's median across runs.
    fn median_of(runs: &[Run]) -> Run {
        let med = |f: &dyn Fn(&Run) -> f64| {
            let mut v: Vec<f64> = runs.iter().map(f).collect();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let part = |p: &dyn Fn(&Run) -> Part| Part {
            mean: med(&|s| p(s).mean),
            p999: med(&|s| p(s).p999),
            max: med(&|s| p(s).max),
            window: med(&|s| p(s).window),
        };
        Run {
            noise: part(&|s| s.noise),
            tail: part(&|s| s.tail),
        }
    }

    /// Whether the tail costs more than the noise by any of the margins (for the mean, the
    /// worst window and the 99.9th percentile), beyond the slack.
    fn over(&self, mean: f64, window: f64, p999: f64) -> bool {
        let (t, n) = (&self.tail, &self.noise);
        let over = |tail: f64, noise: f64, m: f64| tail > noise * m + SLACK_US;
        over(t.mean, n.mean, mean) || over(t.window, n.window, window) || over(t.p999, n.p999, p999)
    }
}

/// A cycle counter turned into µs, finer than Windows' 100 ns performance counter, which a
/// cheap effect's 64-sample block would fall under.
struct Clock {
    us_per_tick: f64,
}

impl Clock {
    fn calibrate() -> Self {
        let t0 = Instant::now();
        let c0 = ticks();
        while t0.elapsed().as_millis() < 200 {
            std::hint::spin_loop();
        }
        let c1 = ticks();
        let us = t0.elapsed().as_secs_f64() * 1e6;
        Self {
            us_per_tick: us / (c1 - c0).max(1) as f64,
        }
    }
}

#[cfg(target_arch = "x86_64")]
fn ticks() -> u64 {
    // SAFETY: RDTSC reads the time-stamp counter; it has no preconditions on x86_64.
    unsafe { core::arch::x86_64::_rdtsc() }
}

#[cfg(not(target_arch = "x86_64"))]
fn ticks() -> u64 {
    use std::sync::OnceLock;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

/// One run: noise then `tail` seconds of silence through a fresh instance, each block timed.
#[allow(clippy::too_many_arguments)]
fn time(
    kind: &'static Kind,
    params: &BTreeMap<String, f64>,
    print: Option<&[Vec<f32>]>,
    file: Option<&Path>,
    noise: &[Vec<f32>],
    tail: f64,
    block: usize,
    clock: &Clock,
) -> Result<Run> {
    let mut insert = native::offline_insert(kind, RATE, params, print, file)
        .with_context(|| format!("making {}", kind.id))?;
    let noise_len = noise[0].len();
    let total = noise_len + (tail * RATE) as usize;
    let blocks = total.div_ceil(block);
    let mut costs = Vec::with_capacity(blocks);
    let mut io = vec![vec![0.0f32; block]; CHANNELS];
    let mut pos = 0;
    while pos < total {
        let n = block.min(total - pos);
        for (c, buf) in io.iter_mut().enumerate() {
            for (k, s) in buf[..n].iter_mut().enumerate() {
                *s = noise[c].get(pos + k).copied().unwrap_or(0.0);
            }
        }
        let transport = Transport::playing_at(pos as u64);
        let t0 = ticks();
        insert.process(&mut io, n, &transport);
        let t1 = ticks();
        costs.push(t1.saturating_sub(t0) as f64 * clock.us_per_tick);
        // Keep the optimizer from dropping the output.
        std::hint::black_box(&io);
        pos += n;
    }
    let first_noise = (NOISE_SKIP * RATE) as usize / block;
    let last_noise = noise_len / block;
    let first_tail = (noise_len + (TAIL_SKIP * RATE) as usize).div_ceil(block);
    let per_window = (WINDOW_SECONDS * RATE) as usize / block;
    Ok(Run {
        noise: Part::of(&costs[first_noise..last_noise], per_window),
        tail: Part::of(&costs[first_tail..], per_window),
    })
}

/// Stereo noise, uniform, at -20 dBFS RMS, the same every run.
fn noise(rate: f64, seconds: f64, seed: u64) -> Vec<Vec<f32>> {
    let len = (rate * seconds) as usize;
    let mut state = seed;
    let mut next = move || {
        // xorshift64*
        state ^= state >> 12;
        state ^= state << 25;
        state ^= state >> 27;
        let x = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        (x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0
    };
    // Uniform in ±a has an RMS of a/√3.
    let a = 0.1 * 3f32.sqrt();
    (0..2)
        .map(|_| (0..len).map(|_| next() * a).collect())
        .collect()
}

/// A 2 s stereo impulse of decaying noise (-120 dB at its end) for the Convolution Reverb.
fn write_impulse() -> Result<PathBuf> {
    let path = std::env::current_dir()?.join(WORK).join("impulse.wav");
    let mut channels = noise(RATE, 2.0, 0x1a2b);
    let len = channels[0].len() as f64;
    for ch in &mut channels {
        for (i, s) in ch.iter_mut().enumerate() {
            *s *= 10f64.powf(-6.0 * i as f64 / len) as f32 * 5.0;
        }
    }
    wav::write_f32(
        &path,
        &wav::Audio {
            sample_rate: RATE as u32,
            channels,
        },
    )?;
    Ok(path)
}

/// Every effect's output (each setting) on the recording `audio` and on the noise, each with
/// 10 s of silence after it, to null before against after a change.
fn render_all(kinds: &[&'static Kind], dir: &Path, impulse: &Path, audio: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let di = wav::read(audio).with_context(|| format!("reading {}", audio.display()))?;
    let di_rate = f64::from(di.sample_rate);
    let mut di_audio = di.channels.clone();
    for ch in &mut di_audio {
        ch.extend(std::iter::repeat_n(0.0, (10.0 * di_rate) as usize));
    }
    let noise_print = noise(RATE, NOISE_SECONDS, 0x5eed);
    let mut noise_audio = noise_print.clone();
    for ch in &mut noise_audio {
        ch.extend(std::iter::repeat_n(0.0, (10.0 * RATE) as usize));
    }
    for kind in kinds {
        for (setting, params) in settings(kind) {
            let file =
                (kind.reads.is_some() && !kind.reads.is_some_and(|r| r.makes())).then_some(impulse);
            for (signal, rate, audio, print) in [
                ("di", di_rate, &di_audio, &di.channels),
                ("noise", RATE, &noise_audio, &noise_print),
            ] {
                let print = kind.capture.map(|_| print.as_slice());
                let out = native::render_with_file(kind, rate, &params, print, file, audio, 512)
                    .with_context(|| format!("rendering {} ({setting})", kind.id))?;
                let path = dir.join(format!("{}_{setting}_{signal}.wav", kind.id));
                wav::write_f32(
                    &path,
                    &wav::Audio {
                        sample_rate: rate as u32,
                        channels: out,
                    },
                )?;
            }
        }
        println!("rendered {}", kind.id);
    }
    println!("RESULT: PASS");
    Ok(())
}

/// Compare every WAV in `after` (all of them, or those `--only` rendered) with its namesake in
/// `before`.
fn null(before: &Path, after: &Path) -> Result<()> {
    let mut failures = Vec::new();
    let mut names: Vec<PathBuf> = std::fs::read_dir(after)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "wav"))
        .collect();
    names.sort();
    if names.is_empty() {
        bail!("no renders in {}", after.display());
    }
    println!(
        "{:<48} {:>12} {:>14}",
        "render", "max diff dB", "differ (samples)"
    );
    for path in names {
        let name = path.file_name().unwrap_or_default();
        let a = wav::read(&before.join(name))
            .with_context(|| format!("{} has no {}", before.display(), name.display()))?;
        let b = wav::read(&path)?;
        if a.channels.len() != b.channels.len() || a.len() != b.len() {
            failures.push(format!("{}: lengths differ", name.display()));
            continue;
        }
        let mut worst = 0.0f64;
        let mut differ = 0usize;
        for (x, y) in a.channels.iter().zip(&b.channels) {
            for (&p, &q) in x.iter().zip(y) {
                if p.to_bits() != q.to_bits() {
                    differ += 1;
                    worst = worst.max((f64::from(p) - f64::from(q)).abs());
                }
            }
        }
        let db = if worst > 0.0 {
            20.0 * worst.log10()
        } else {
            f64::NEG_INFINITY
        };
        println!("{:<48} {:>12.1} {:>14}", name.display(), db, differ);
        if db > -300.0 {
            failures.push(format!("{}: differs by {db:.1} dB", name.display()));
        }
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("null failed")
    }
}
