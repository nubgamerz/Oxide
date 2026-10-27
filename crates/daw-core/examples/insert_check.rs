//! Moving and copying a real plug-in between tracks (dev plan 13.8): the plug-in named, with a
//! setting changed, on one track of the audio is copied onto a second track of the same audio; each played
//! alone must null (the copy has the original's settings), and the original moved onto a
//! third track must play exactly as it did. Then Copy and Paste (15.1): copied to the
//! clipboard and pasted onto a fourth track, it nulls too. Through the action registry, as an
//! agent does.
//! Prints `RESULT: PASS` or `FAIL:` lines and exits non-zero on a failure.
//!
//! cargo run --release -p daw-core --example insert_check -- <audio.wav> ["<plugin>"]

use anyhow::{bail, Result};
use daw_core::api::{ActionOutput, Registry, SessionHost};
use daw_core::device::AudioSettings;
use daw_core::session::Session;
use daw_core::wav;
use serde_json::{json, Value};
use std::time::Duration;

fn call(r: &Registry, s: &mut Session, name: &str, args: Value) -> Result<Value> {
    match r.call_now(&mut SessionHost(s), name, args) {
        Ok(ActionOutput::Json(v)) => Ok(v),
        Ok(ActionOutput::Png(_)) => bail!("unexpected image"),
        Err(e) => bail!("{name}: {e}"),
    }
}

/// The difference of two renders in dB below their peak (-inf: identical).
fn null_db(a: &[f32], b: &[f32]) -> f64 {
    let peak = a.iter().chain(b).fold(0.0f32, |m, x| m.max(x.abs()));
    let diff = a
        .iter()
        .zip(b)
        .fold(0.0f32, |m, (x, y)| m.max((x - y).abs()));
    if diff == 0.0 {
        f64::NEG_INFINITY
    } else {
        20.0 * f64::from(diff / peak.max(1e-12)).log10()
    }
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let audio =
        std::path::absolute(args.next().ok_or_else(|| {
            anyhow::anyhow!("give the audio file (a WAV) as the first argument")
        })?)?;
    let plugin = args
        .next()
        .ok_or_else(|| anyhow::anyhow!("name the plug-in as the second argument"))?;
    let dir = std::env::temp_dir().join(format!("oxide-insert-check-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut s = Session::new(AudioSettings {
        output_device: Some("<none>".into()),
        threads: Some(0),
        ..Default::default()
    });
    let r = Registry::new();
    let track = |r: &Registry, s: &mut Session| -> Result<u64> {
        let t = call(r, s, "add_track", json!({"kind": "audio"}))?["track"]
            .as_u64()
            .unwrap_or_default();
        call(
            r,
            s,
            "import_audio",
            json!({"track": t, "path": audio, "position": 0}),
        )?;
        Ok(t)
    };
    let (a, b, c, d) = (
        track(&r, &mut s)?,
        track(&r, &mut s)?,
        track(&r, &mut s)?,
        track(&r, &mut s)?,
    );
    let insert = call(
        &r,
        &mut s,
        "add_insert",
        json!({"track": a, "plugin": plugin}),
    )?["insert"]
        .as_u64()
        .unwrap_or_default();
    // A setting away from its default, so a copy that lost its state would show: the output
    // gain if it has one, else its first setting that isn't a bypass.
    let params = call(
        &r,
        &mut s,
        "list_plugin_parameters",
        json!({"insert": insert}),
    )?;
    let list = params["parameters"].as_array().cloned().unwrap_or_default();
    let name = |p: &Value| p["name"].as_str().unwrap_or_default().to_lowercase();
    let chosen = list
        .iter()
        .find(|p| name(p).contains("output gain"))
        .or_else(|| list.iter().find(|p| !name(p).contains("bypass")))
        .cloned()
        .unwrap_or_default();
    let value = if chosen["value"].as_f64().unwrap_or(0.0) > 0.5 {
        0.55
    } else {
        0.8
    };
    let set = json!({"insert": insert, "parameter": chosen["id"], "value": value});
    call(&r, &mut s, "set_plugin_parameter", set)?;
    println!("set {} to {value}", chosen["name"]);
    let end = s.content_end();
    let alone = |r: &Registry, s: &mut Session, only: u64, name: &str| -> Result<Vec<f32>> {
        for t in [a, b, c, d] {
            call(
                r,
                s,
                "set_track_mixer",
                json!({"track": t, "mute": t != only}),
            )?;
        }
        let path = dir.join(name);
        let args = json!({"path": path, "end": end, "tail_seconds": 0});
        call(r, s, "export_mixdown", args)?;
        s.wait_for_export(Duration::from_secs(120))?;
        Ok(wav::read(&path)?.channels.remove(0))
    };
    let original = alone(&r, &mut s, a, "original.wav")?;

    let mut failures = Vec::new();
    call(
        &r,
        &mut s,
        "copy_insert",
        json!({"insert": insert, "track": b}),
    )?;
    let copy = alone(&r, &mut s, b, "copy.wav")?;
    let copied = null_db(&original, &copy);
    println!("{plugin} copied onto another track: nulls to {copied:.1} dB against the original");
    if copied > -120.0 {
        failures.push(format!(
            "the copy differs from the original by {copied:.1} dB"
        ));
    }
    call(
        &r,
        &mut s,
        "move_insert",
        json!({"insert": insert, "track": c}),
    )?;
    let moved = alone(&r, &mut s, c, "moved.wav")?;
    let moved_db = null_db(&original, &moved);
    println!("{plugin} moved onto a third track: nulls to {moved_db:.1} dB against where it was");
    if moved_db > -120.0 {
        failures.push(format!("the moved plug-in differs by {moved_db:.1} dB"));
    }
    // The first track, its plug-in gone, no longer sounds processed.
    let left = alone(&r, &mut s, a, "left.wav")?;
    if null_db(&left, &original) <= -120.0 {
        failures.push("the first track still sounds processed after the move".into());
    }
    // Copy and Paste: through the clipboard onto a fourth track.
    call(&r, &mut s, "copy_plugin", json!({"insert": insert}))?;
    call(&r, &mut s, "paste_plugin", json!({"track": d}))?;
    let pasted = alone(&r, &mut s, d, "pasted.wav")?;
    let pasted_db = null_db(&original, &pasted);
    println!("{plugin} copied and pasted onto a fourth track: nulls to {pasted_db:.1} dB");
    if pasted_db > -120.0 {
        failures.push(format!("the pasted plug-in differs by {pasted_db:.1} dB"));
    }
    for notice in &s.notices {
        println!("notice: {notice}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("insert check failed")
    }
}
