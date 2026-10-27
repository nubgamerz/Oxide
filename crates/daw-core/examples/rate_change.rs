//! Changing the sample rate with plug-ins loaded: every plug-in must reload with its state, clip
//! positions must scale, and export must still work. Drives the change through the action
//! registry, as an agent would. Prints `RESULT: PASS` or `FAIL:` lines.
//!
//! cargo run --release -p daw-core --example rate_change -- <in.wav> "<plugin>"

use anyhow::{bail, Result};
use daw_core::api::{ActionOutput, Registry, SessionHost};
use daw_core::device::AudioSettings;
use daw_core::session::Session;
use serde_json::{json, Value};
use std::time::Duration;

fn call(r: &Registry, s: &mut Session, name: &str, args: Value) -> Result<Value> {
    match r.call_now(&mut SessionHost(s), name, args) {
        Ok(ActionOutput::Json(v)) => Ok(v),
        Ok(ActionOutput::Png(_)) => bail!("unexpected image"),
        Err(e) => bail!("{name}: {e}"),
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input =
        std::path::absolute(args.next().ok_or_else(|| {
            anyhow::anyhow!("give the audio file (a WAV) as the first argument")
        })?)?;
    let plugin = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("name the plug-in as the second argument"))?;

    let mut s = Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        sample_rate: Some(48_000),
        threads: Some(0),
        ..Default::default()
    });
    let r = Registry::new();
    let track = call(&r, &mut s, "add_track", json!({"kind": "audio"}))?["track"].clone();
    let clip = call(
        &r,
        &mut s,
        "import_audio",
        json!({"track": track, "path": input, "position": 96_000}),
    )?;
    let insert = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": track, "plugin": plugin}),
    )?;
    // Change a parameter so we can see the state survive the reload.
    // A continuous parameter (toggles would round 0.3 away): the first unlocked one.
    let page = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert["insert"], "limit": 1000}),
    )?;
    let chosen = page["parameters"]
        .as_array()
        .and_then(|ps| {
            ps.iter()
                .find(|p| p["steps"] == 0 && p["read_only"] == false && p["automatable"] == true)
        })
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("{plugin} has no continuous parameter"))?;
    let param = chosen["id"].clone();
    let name = chosen["name"].as_str().unwrap_or_default().to_string();
    let set = call(
        &r,
        &mut s,
        "set_plugin_parameter",
        json!({"insert": insert["insert"], "parameter": param, "value": 0.3}),
    )?;
    println!("set '{name}' to {}", set["display"]);

    let changed = call(
        &r,
        &mut s,
        "set_audio_settings",
        json!({"sample_rate": 44_100}),
    )?;
    println!("after the change: {changed}");

    let mut failures = Vec::new();
    if s.sample_rate() != 44_100 {
        failures.push(format!("session runs at {} Hz", s.sample_rate()));
    }
    let project = call(&r, &mut s, "get_project", Value::Null)?;
    let t = &project["tracks"][0];
    if t["inserts"][0]["loaded"] != true {
        failures.push(format!("the plug-in did not reload: {}", t["inserts"][0]));
    }
    let scale = |v: &Value| (v.as_f64().unwrap_or(0.0) * 44_100.0 / 48_000.0).round();
    let start = t["clips"][0]["start"].as_f64().unwrap_or(-1.0);
    let length = t["clips"][0]["length"].as_f64().unwrap_or(-1.0);
    if start != scale(&clip["start"]) || (length - scale(&clip["length"])).abs() > 1.0 {
        failures.push(format!(
            "clip not rescaled: start {start} length {length} (from {} / {})",
            clip["start"], clip["length"]
        ));
    }
    let after = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert["insert"], "filter": name, "limit": 1}),
    )?;
    println!("after the reload: {}", after["parameters"][0]);
    let value = after["parameters"][0]["value"].as_f64().unwrap_or(-1.0);
    if (value - 0.3).abs() > 0.01 {
        failures.push(format!(
            "plug-in state was not restored: parameter is {value}, not 0.3"
        ));
    }
    let out = std::env::temp_dir().join("daw-rate-change.wav");
    call(
        &r,
        &mut s,
        "export_mixdown",
        json!({"path": out, "tail_seconds": 0}),
    )?;
    match s.wait_for_export(Duration::from_secs(60)) {
        Ok(path) => {
            let audio = daw_core::wav::read(&path)?;
            println!(
                "exported {:.2} s at {} Hz",
                audio.seconds(),
                audio.sample_rate
            );
            if audio.sample_rate != 44_100 {
                failures.push(format!("export is at {} Hz", audio.sample_rate));
            }
        }
        Err(e) => failures.push(format!("export: {e:#}")),
    }

    // Back to 48 kHz staged, as the app does it (18.3): the change returns before the plug-in
    // is back, which reloads in an update of its own, its setting kept.
    s.stage_loads = true;
    let mut settings = s.settings.clone();
    settings.sample_rate = Some(48_000);
    let asked = std::time::Instant::now();
    s.apply_audio_settings(settings);
    let returned = asked.elapsed();
    println!(
        "staged change returned in {:.1} ms",
        returned.as_secs_f64() * 1e3
    );
    let reloading = s.loading().map(|l| l.kind);
    if reloading != Some(daw_core::session::LoadKind::SampleRate) {
        failures.push(format!(
            "the staged change didn't reload staged: {reloading:?}"
        ));
    }
    let mut updates = 0;
    while s.loading().is_some() && updates < 10_000 {
        std::thread::sleep(Duration::from_millis(2));
        s.update();
        updates += 1;
    }
    println!("reloaded in {updates} update(s)");
    let after = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert["insert"], "filter": name, "limit": 1}),
    )?;
    let value = after["parameters"][0]["value"].as_f64().unwrap_or(-1.0);
    if s.sample_rate() != 48_000 || (value - 0.3).abs() > 0.01 {
        failures.push(format!(
            "after the staged change: {} Hz, the parameter {value}",
            s.sample_rate()
        ));
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("rate change check failed")
    }
}
