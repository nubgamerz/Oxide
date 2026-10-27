//! Serial vs parallel processing of a real session: N audio tracks, each with the same clip
//! and its own instance of a plugin, mixed through a folder bus. Exports the same range with
//! the audio thread alone and with the worker pool, reports the speedup and checks the two
//! files are identical. Prints `RESULT: PASS` and exits 0 when the parallel export adds no
//! difference beyond the plugin's own run-to-run variation.
//!
//! cargo run --release -p daw-core --example parallel_bench -- <in.wav> "<plugin>" [tracks] [repeat]

use anyhow::{bail, Result};
use daw_core::device::AudioSettings;
use daw_core::plugins;
use daw_core::project::TrackKind;
use daw_core::session::{InsertTarget, Session};
use daw_core::wav;
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
    // Repeat the clip so the export is long enough to time.
    let repeat: u64 = args.next().map_or(Ok(4), |s| s.parse())?;

    let settings = AudioSettings {
        output_device: Some("<none>".into()),
        sample_rate: Some(48_000),
        ..Default::default()
    };
    let mut session = Session::new(settings);
    let path = plugins::resolve(&plugin, session.installed_plugins())?;
    let folder = session.add_track(TrackKind::Folder, None);
    let clip_len = daw_core::media::load(&input, 48_000)?.len() as u64;
    let started = Instant::now();
    for _ in 0..tracks {
        let t = session.add_track(TrackKind::Audio, Some(folder));
        for r in 0..repeat {
            session.add_clip_from_file(t, &input, r * clip_len)?;
        }
        session.add_insert(InsertTarget::Track(t), &path)?;
    }
    println!("loaded {tracks} x {plugin} in {:.1?}", started.elapsed());
    let seconds = (repeat * clip_len) as f64 / 48_000.0;

    // Serial twice (the baseline for how repeatable the plugin itself is), then parallel.
    let mut results = Vec::new();
    for threads in [Some(0), Some(0), None] {
        session.set_threads(threads);
        session.update();
        let workers = session.stats().workers;
        let out = std::env::temp_dir().join(format!("daw-parallel-{workers}.wav"));
        let started = Instant::now();
        session.export(&out, None, 0.0)?;
        let path = session.wait_for_export(Duration::from_secs(600))?;
        let wall = started.elapsed().as_secs_f64();
        println!(
            "{:>2} workers + caller: {seconds:.1} s of audio in {wall:.2} s ({:.1}x realtime)",
            workers,
            seconds / wall
        );
        results.push((wall, wav::read(&path)?));
    }
    let (serial, serial2, parallel) = (&results[0], &results[1], &results[2]);
    println!("speedup {:.2}x", serial.0 / parallel.0);
    let own = difference(&serial.1, &serial2.1);
    let threaded = difference(&serial2.1, &parallel.1);
    println!("serial vs serial:   {}", describe(own));
    println!("serial vs parallel: {}", describe(threaded));
    for notice in &session.notices {
        println!("notice: {notice}");
    }
    // Parallel processing may not add any difference of its own: bit-identical when the
    // plugin is deterministic, and no worse than the plugin's own run-to-run variation when
    // it is not.
    let ok = match (own, threaded) {
        (_, None) => false,
        (Some(own), Some(threaded)) if own == f32::NEG_INFINITY => threaded == f32::NEG_INFINITY,
        (Some(own), Some(threaded)) => threaded <= own + 6.0,
        (None, Some(_)) => false,
    };
    if ok {
        println!("RESULT: PASS");
        Ok(())
    } else {
        println!("FAIL: parallel export differs more than the plugin's own variation");
        bail!("parallel check failed")
    }
}

/// Peak difference in dB (`-inf` when bit-identical), or `None` if the lengths differ.
fn difference(a: &wav::Audio, b: &wav::Audio) -> Option<f32> {
    if a.len() != b.len() {
        return None;
    }
    let peak = a
        .channels
        .iter()
        .flatten()
        .zip(b.channels.iter().flatten())
        .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
    Some(20.0 * peak.log10())
}

fn describe(diff: Option<f32>) -> String {
    match diff {
        None => "lengths differ".into(),
        Some(d) if d == f32::NEG_INFINITY => "bit-identical".into(),
        Some(d) => format!("differ, peak {d:.1} dB"),
    }
}
