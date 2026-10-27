//! Recording latency on real hardware: play clicks out of the device and record them back
//! through a cable from that output to an input, then check where the recorded clicks were
//! placed. They must land within a sample of where they were played.
//!
//! THIS MAKES SOUND and records the input. Patch the master's output pair (outputs 1/2) into
//! `input channel` with a cable first, and turn monitors down. It is never part of a default
//! check tier: `check.ps1 -Tier live -Loopback "<device>"` runs it.
//!
//! cargo run --release -p daw-core --example loopback_check -- <host> "<device>" [input channel]
//!
//! Prints `RESULT: PASS` or `FAIL:` lines.

use anyhow::{bail, Context, Result};
use daw_core::device::AudioSettings;
use daw_core::project::{Monitor, TrackInput, TrackKind, TrackOutput};
use daw_core::session::Session;
use daw_core::wav;
use std::time::{Duration, Instant};

/// Clicks at these seconds; the first is measured, the rest confirm it.
const CLICKS: [f64; 3] = [1.0, 2.0, 3.0];

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let host = args.next().unwrap_or_default();
    let device = args.next().filter(|d| !d.is_empty());
    let channel: u16 = args.next().map_or(Ok(0), |c| c.parse())?;

    let dir = std::env::temp_dir().join("daw-loopback-check");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut s = Session::new(AudioSettings {
        host,
        output_device: device,
        ..Default::default()
    });
    if let Some(error) = &s.device_error {
        bail!("the device did not open: {error}");
    }
    let rate = s.sample_rate();
    // Recordings go next to the project, so keep it out of the user's projects.
    s.save(&dir.join("loopback.dawproj"))?;

    let mut clicks = vec![0.0f32; (rate as f64 * 4.0) as usize];
    for at in CLICKS {
        let i = (at * f64::from(rate)) as usize;
        clicks[i..i + 4].fill(0.7);
    }
    let click_file = dir.join("clicks.wav");
    wav::write_f32(
        &click_file,
        &wav::Audio {
            sample_rate: rate,
            channels: vec![clicks],
        },
    )?;
    let player = s.add_track(TrackKind::Audio, None);
    s.project.track_mut(player).unwrap().output = TrackOutput::Master;
    s.add_clip_from_file(player, &click_file, 0)?;
    let recorder = s.add_track(TrackKind::Audio, None);
    {
        let t = s.project.track_mut(recorder).unwrap();
        t.input = TrackInput::Audio {
            channel,
            stereo: false,
        };
        t.arm = true;
        // Never monitor the cable: it would feed back.
        t.monitor = Monitor::Off;
    }
    s.mark_dirty();

    // Let the device report its timing.
    let settle = Instant::now();
    while s.round_trip().is_none() && settle.elapsed() < Duration::from_secs(5) {
        s.update();
        std::thread::sleep(Duration::from_millis(20));
    }
    let round_trip = s
        .round_trip()
        .context("the device reported no round trip")?;
    println!(
        "round trip {round_trip} samples ({:.2} ms), record shift {} samples",
        round_trip as f64 * 1000.0 / f64::from(rate),
        s.record_shift()
    );

    s.locate(0)?;
    s.record()?;
    let run = Instant::now();
    while run.elapsed() < Duration::from_secs_f64(CLICKS[2] + 0.8) {
        s.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    s.stop();
    let done = Instant::now();
    while s.project.track(recorder).unwrap().clips.is_empty() {
        if done.elapsed() > Duration::from_secs(10) {
            bail!("the take never arrived");
        }
        s.update();
        std::thread::sleep(Duration::from_millis(20));
    }
    let take = s.project.track(recorder).unwrap().clips[0].clone();
    let audio = wav::read(&s.resolve(take.file()))?;
    let samples = &audio.channels[0];

    let mut failures = Vec::new();
    let mut from = take.offset() as usize;
    for at in CLICKS {
        let expected = (at * f64::from(rate)) as i64;
        let Some(i) = samples[from..]
            .iter()
            .position(|v| v.abs() > 0.1)
            .map(|i| i + from)
        else {
            // Its loudest sample tells a quiet loop (or the wrong input) from silence.
            let (peak_at, peak) =
                samples
                    .iter()
                    .enumerate()
                    .fold((0, 0.0f32), |(pi, p), (i, v)| {
                        if v.abs() > p {
                            (i, v.abs())
                        } else {
                            (pi, p)
                        }
                    });
            failures.push(format!(
                "the click at {at} s was not recorded (is the cable in?): the take ({} samples) \
                 peaks at {:.1} dBFS, sample {peak_at}",
                samples.len(),
                20.0 * peak.max(1e-9).log10()
            ));
            break;
        };
        let landed = take.start as i64 + (i as i64 - take.offset() as i64);
        let error = landed - expected;
        println!("click at {at} s: recorded at {landed}, {error:+} samples");
        if error.abs() > 1 {
            failures.push(format!(
                "the click at {at} s landed {error:+} samples off; set the record offset to \
                 {error} (Options > Settings > Audio) if the driver under-reports"
            ));
        }
        // Past this click's tail.
        from = i + (f64::from(rate) * 0.2) as usize;
    }
    let _ = std::fs::remove_dir_all(&dir);
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("loopback check failed")
    }
}
