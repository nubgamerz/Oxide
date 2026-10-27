//! Worst-case DSP load during live playback, single-threaded vs the worker pool. Opens a real
//! audio device with the master muted (nothing is heard), plays N tracks that each run their
//! own instance of a plugin, and samples the worst block every 100 ms. Prints `RESULT: PASS`
//! and exits 0 when the pool lowers the load without adding dropouts.
//!
//! cargo run --release -p daw-core --features asio --example live_bench -- \
//!     <in.wav> "<plugin>" <tracks> <seconds> [host] [device] [buffer]
//!
//! With `DAW_TRACE=1` it also traces the audio thread ([`daw_core::trace`]) and prints, for
//! each 100 ms interval whose worst block ran over its period, where that block spent its time.
//! `DAW_WORKERS=7,6,7t` runs those worker counts after the serial run instead of the pool's
//! default (`t`: plain time-critical priority instead of MMCSS), to compare them in one sitting.

use anyhow::Result;
use daw_core::device::AudioSettings;
use daw_core::plugins;
use daw_core::project::TrackKind;
use daw_core::session::{InsertTarget, Session};
use daw_core::trace::{self, BlockReport};
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("give the audio file (a WAV) as the first argument"))?,
    );
    let plugin = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("name the plug-in as the second argument"))?;
    let tracks: usize = args.next().map_or(Ok(8), |s| s.parse())?;
    let seconds: u64 = args.next().map_or(Ok(8), |s| s.parse())?;
    let settings = AudioSettings {
        host: args.next().unwrap_or_default(),
        output_device: args.next().filter(|d| !d.is_empty()),
        buffer_size: args.next().map(|s| s.parse()).transpose()?,
        ..Default::default()
    };

    let tracing = std::env::var_os("DAW_TRACE").is_some_and(|v| v == "1");
    if tracing {
        trace::enable();
        println!(
            "tracing: {:.3} thread cycles per ns",
            trace::cycles_per_ns()
        );
    }
    let mut session = Session::new(settings);
    if let Some(error) = &session.device_error {
        anyhow::bail!("audio device: {error}");
    }
    session.project.master.mixer.mute = true;
    let path = plugins::resolve(&plugin, session.installed_plugins())?;
    let folder = session.add_track(TrackKind::Folder, None);
    for _ in 0..tracks {
        let t = session.add_track(TrackKind::Audio, Some(folder));
        for r in 0..4 {
            let at = r * u64::from(session.sample_rate()) * 6;
            session.add_clip_from_file(t, &input, at)?;
        }
        session.add_insert(InsertTarget::Track(t), &path)?;
    }
    session.update();
    let info = &session.device.as_ref().expect("device running").info;
    println!(
        "{} · {} · {} Hz · {tracks} x {plugin}",
        info.host, info.output_name, info.sample_rate
    );

    // (workers, mean load, intervals over 100%) per run.
    let mut runs = Vec::new();
    // `DAW_WORKERS=7,6,7,6` compares worker counts (after the serial run); the default is the
    // pool's own count. A count ending in `t` gives its workers plain time-critical priority
    // instead of MMCSS (`7t`).
    let counts: Vec<(Option<usize>, bool)> = match std::env::var("DAW_WORKERS") {
        Ok(list) => list
            .split(',')
            .map(|n| {
                let n = n.trim();
                let plain = n.ends_with('t');
                n.trim_end_matches('t').parse().map(|n| (Some(n), !plain))
            })
            .collect::<Result<_, _>>()?,
        Err(_) => vec![(None, true)],
    };
    for (threads, mmcss) in std::iter::once((Some(0), true)).chain(counts) {
        daw_core::cpu::use_mmcss(mmcss);
        session.set_threads(threads);
        if !mmcss {
            println!("(workers at plain time-critical priority, no MMCSS)");
        }
        session.locate(0)?;
        session.play();
        // Let plugins settle and the workers warm up before measuring.
        let warmup = Instant::now();
        while warmup.elapsed() < Duration::from_secs(1) {
            session.update();
            std::thread::sleep(Duration::from_millis(20));
        }
        session.take_dsp_peak();
        trace::take_worst();
        trace::take_gaps();
        let mut spikes = Vec::new();
        let mut peaks = Vec::new();
        let mut loads = Vec::new();
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(seconds) {
            std::thread::sleep(Duration::from_millis(100));
            session.update();
            peaks.push(session.take_dsp_peak());
            loads.push(session.stats().dsp_load);
            if let Some(block) = trace::take_worst().filter(|b| b.total_us > b.budget_us) {
                spikes.push((started.elapsed().as_secs_f64(), block));
            }
        }
        session.stop();
        let stats = session.stats();
        peaks.sort_by(f32::total_cmp);
        let worst = peaks.last().copied().unwrap_or(0.0);
        let p95 = peaks[(peaks.len() * 95 / 100).min(peaks.len() - 1)];
        let mean = loads.iter().sum::<f32>() / loads.len().max(1) as f32;
        let over = peaks.iter().filter(|p| **p > 1.0).count();
        if stats.cadence != stats.block_size {
            println!(
                "   (the driver calls back in bursts of {} frames: loads count against them)",
                stats.cadence
            );
        }
        println!(
            "{:>2} workers: block {:>4} · mean load {:>5.1}% · p95 peak {:>5.1}% · worst {:>5.1}% · \
             {over} of {} intervals over 100%",
            stats.workers,
            stats.block_size,
            mean * 100.0,
            p95 * 100.0,
            worst * 100.0,
            peaks.len()
        );
        runs.push((stats.workers, mean, over));
        if tracing {
            let gaps = trace::take_gaps();
            let edges = trace::GAP_EDGES;
            println!(
                "  callback gaps in periods: <{} {}, <{} {}, <{} {}, <{} {}, <{} {}, more {}",
                edges[0],
                gaps[0],
                edges[1],
                gaps[1],
                edges[2],
                gaps[2],
                edges[3],
                gaps[3],
                edges[4],
                gaps[4],
                gaps[5]
            );
            print_spikes(&spikes);
        }
    }
    for notice in &session.notices {
        println!("notice: {notice}");
    }
    // The pool must lower the mean load and must not add dropouts. One stray interval is
    // tolerated: at 64-sample buffers Windows sometimes takes an audio thread off the CPU for
    // longer than a period, in bursts (TD9).
    let serial = runs[0];
    if runs[1..].iter().all(|r| r.0 == 0) {
        println!("RESULT: PASS (no worker threads on this machine; nothing to compare)");
        return Ok(());
    }
    let mut failed = false;
    for parallel in &runs[1..] {
        if !(parallel.1 < serial.1 && parallel.2 <= serial.2.max(1)) {
            println!(
                "FAIL: {} workers: mean {:.1}% vs serial {:.1}%, overruns {} vs {}",
                parallel.0,
                parallel.1 * 100.0,
                serial.1 * 100.0,
                parallel.2,
                serial.2
            );
            failed = true;
        }
    }
    if failed {
        anyhow::bail!("live check failed")
    }
    println!("RESULT: PASS");
    Ok(())
}

/// Each traced block that ran over its period: its phases, then per level its span and nodes.
/// A node's `cpu` is its thread's CPU time over its wall time: well under 100% means the thread
/// was off the CPU (preempted) while rendering it.
fn print_spikes(spikes: &[(f64, BlockReport)]) {
    let rate = trace::cycles_per_ns();
    println!("  {} traced blocks over their period", spikes.len());
    for (at, b) in spikes {
        println!(
            "  t={at:5.1}s block {:6.0} us of {:4.0} us: before graph {:5.0}, graph {:6.0},              after {:5.0}",
            b.total_us, b.budget_us, b.before_us, b.graph_us, b.after_us
        );
        for (i, l) in b.levels.iter().enumerate() {
            let tasks: Vec<String> = l
                .tasks
                .iter()
                .map(|t| {
                    let cpu = if rate > 0.0 && t.wall_us > 0.0 {
                        t.cycles as f64 / rate / (t.wall_us * 1000.0) * 100.0
                    } else {
                        0.0
                    };
                    format!(
                        "n{}@t{} +{:.0} {:.0}us cpu{:.0}%",
                        t.node, t.thread, t.start_us, t.wall_us, cpu
                    )
                })
                .collect();
            println!(
                "    level {i} at {:5.0} us, {:5.0} us: {}",
                l.open_us,
                l.span_us,
                tasks.join(" | ")
            );
        }
    }
}
