//! Open an audio device for a second (silent: no tracks) and report what it ran with.
//! Prints `RESULT: PASS` and exits 0 when the device opened and its callback ran.
//!
//! cargo run -p daw-core --features asio --example device_check -- ASIO "ASIO4ALL v2"

use daw_core::device::AudioSettings;
use daw_core::session::Session;
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let settings = AudioSettings {
        host: args.next().unwrap_or_default(),
        output_device: args.next().filter(|d| !d.is_empty()),
        ..Default::default()
    };
    let mut session = Session::new(settings);
    if let Some(error) = &session.device_error {
        println!("FAIL: device failed: {error}");
        return ExitCode::FAILURE;
    }
    std::thread::sleep(Duration::from_secs(1));
    session.update();
    let info = &session.device.as_ref().expect("device is running").info;
    let stats = session.stats();
    println!("{info:#?}");
    println!(
        "callback block {} samples, dsp load {:.1}%, input gaps {}, workers {}",
        stats.block_size,
        stats.dsp_load * 100.0,
        stats.input_underruns,
        stats.workers
    );
    // The recording round trip, measured from the device's timestamps (needs an input).
    let rate = f64::from(info.sample_rate.max(1));
    let has_input = info.input_name.is_some();
    for _ in 0..50 {
        if session.round_trip().is_some() || !has_input {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    match session.round_trip() {
        Some(n) => println!(
            "recording round trip {n} samples ({:.2} ms)",
            n as f64 * 1000.0 / rate
        ),
        None if has_input => println!("recording round trip: not reported"),
        None => println!("recording round trip: no input open"),
    }
    for notice in &session.notices {
        println!("notice: {notice}");
    }
    if stats.block_size == 0 {
        println!("FAIL: the device opened but its callback never ran");
        return ExitCode::FAILURE;
    }
    println!("RESULT: PASS");
    ExitCode::SUCCESS
}
