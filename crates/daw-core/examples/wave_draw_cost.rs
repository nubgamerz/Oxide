//! What drawing waveforms costs a frame (23.2), in the release build: a 10-minute stereo file
//! (48 kHz) given its peak levels as a load does, then laid out ([`daw_core::peaks::layout`],
//! what both the Multitrack and the Waveform view draw from) for 40 lanes 1900 points wide
//! (20 stereo tracks) at every kind of zoom, from the widest the Multitrack allows to 32
//! points a sample. Each zoom is timed over 30 frames and its worst frame kept. Painting the
//! layout (one mesh or line per lane) is timed in the app by the `--show clip-*` shots.
//!
//! Prints `RESULT: PASS`, or `FAIL:` lines and exits 1 when a frame's layout takes more than
//! a quarter of a 60 Hz frame or building the levels takes more than a second. Plays nothing,
//! opens no device, writes nothing.
//!
//! `cargo run --release -p daw-core --example wave_draw_cost`

use daw_core::media::AudioData;
use daw_core::peaks::{layout, Layout, Place};
use std::time::{Duration, Instant};

const RATE: u32 = 48_000;
const LANES: u64 = 40;
const WIDTH: f32 = 1900.0;
/// A quarter of a 60 Hz frame: the rest is painting, the other panes and egui.
const BUDGET: Duration = Duration::from_micros(4_167);

fn signal(len: usize, seed: u32) -> Vec<f32> {
    let mut state = seed | 1;
    (0..len)
        .map(|i| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            let noise = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
            0.5 * (i as f32 * 0.002).sin() + 0.3 * noise
        })
        .collect()
}

fn main() {
    let len = RATE as usize * 600;
    let channels = vec![signal(len, 0x2545_f491), signal(len, 0x9e37_79b9)];
    let started = Instant::now();
    let audio = AudioData::new(RATE, channels);
    let built = started.elapsed();
    println!(
        "levels for 10 minutes of stereo built in {:.1} ms",
        built.as_secs_f64() * 1000.0
    );
    let mut failures = Vec::new();
    if built > Duration::from_secs(1) {
        failures.push(format!("building the levels took {built:?}"));
    }
    let mut out = Layout::default();
    let widest = f64::from(RATE) * 2.0;
    for spp in [
        widest,
        len as f64 / f64::from(WIDTH),
        2_000.0,
        300.0,
        200.0,
        40.0,
        15.0,
        4.0,
        1.5,
        0.5,
        1.0 / 8.0,
        1.0 / 32.0,
    ] {
        let mut worst = Duration::ZERO;
        let mut drawn = 0;
        for frame in 0..30u64 {
            let started = Instant::now();
            drawn = 0;
            for lane in 0..LANES {
                // Each lane somewhere else in the file, scrolled a little each frame.
                let from = (lane * 7_919 + frame * 1_013) % (len as u64 / 2);
                let place = Place {
                    origin: 200.0 - from as f64 / spp,
                    spp,
                    from: 0,
                    to: len as u64,
                    left: 200.0,
                    right: 200.0 + WIDTH,
                };
                layout(&audio.wave((lane % 2) as usize), &place, &mut out);
                drawn += out.columns.len() + out.points.len();
            }
            worst = worst.max(started.elapsed());
        }
        println!(
            "{spp:>10.3} samples a point: worst frame {:>7.3} ms for {LANES} lanes ({drawn} columns or points)",
            worst.as_secs_f64() * 1000.0
        );
        if worst > BUDGET {
            failures.push(format!(
                "{spp} samples a point: a frame's layout took {:.2} ms (budget {:.2} ms)",
                worst.as_secs_f64() * 1000.0,
                BUDGET.as_secs_f64() * 1000.0
            ));
        }
        if drawn == 0 {
            failures.push(format!("{spp} samples a point: nothing laid out"));
        }
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
    } else {
        for f in &failures {
            println!("FAIL: {f}");
        }
        std::process::exit(1);
    }
}
