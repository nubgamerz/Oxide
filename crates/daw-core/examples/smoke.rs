//! Headless end-to-end check of the session API with real plugins and no audio device:
//! list devices, build a project with a folder bus, import audio, load plugins, export twice,
//! save, reopen and export again. Prints `RESULT: PASS` and exits 0, or prints `FAIL:` lines
//! and exits non-zero.
//!
//! cargo run -p daw-core --example smoke -- <input.wav> [plugin name ...]

use anyhow::{bail, Result};
use daw_core::device::{self, AudioSettings};
use daw_core::plugins;
use daw_core::project::{TrackKind, TrackOutput};
use daw_core::session::{InsertTarget, Session};
use daw_core::util::db_to_gain;
use daw_core::wav;
use std::path::PathBuf;
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("give the audio file (a WAV) as the first argument"))?,
    );
    let wanted: Vec<String> = args.collect();

    for host in device::host_names() {
        match device::list_devices(&host) {
            Ok(list) => println!(
                "{host}: outputs {:?}, inputs {:?}",
                list.outputs, list.inputs
            ),
            Err(e) => println!("{host}: {e:#}"),
        }
    }

    // A device name that does not exist keeps the session device-less.
    let settings = AudioSettings {
        output_device: Some("<none>".into()),
        sample_rate: Some(48_000),
        ..Default::default()
    };
    let mut session = Session::new(settings);
    println!("device error (expected): {:?}", session.device_error);

    let folder = session.add_track(TrackKind::Folder, None);
    let guitar = session.add_track(TrackKind::Audio, Some(folder));
    let dry = session.add_track(TrackKind::Audio, None);
    session.project.track_mut(dry).unwrap().output = TrackOutput::Master;
    session.add_clip_from_file(guitar, &input, 0)?;
    session.add_clip_from_file(dry, &input, 0)?;
    session.project.track_mut(dry).unwrap().mixer.mute = true;

    let installed = session.installed_plugins().to_vec();
    for name in &wanted {
        let path = plugins::resolve(name, &installed)?;
        let started = Instant::now();
        let id = session.add_insert(InsertTarget::Track(guitar), &path)?;
        let (_, latency, _) = session.insert_info(id).unwrap();
        println!(
            "loaded {name} in {:?}, latency {latency}",
            started.elapsed()
        );
    }
    session.update();
    println!("graph latency {}", session.stats().latency);

    let out = std::env::temp_dir().join("daw-smoke.wav");
    session.export(&out, None, 0.5)?;
    let started = Instant::now();
    let path = session.wait_for_export(Duration::from_secs(120))?;
    let audio = wav::read(&path)?;
    let peak = audio
        .channels
        .iter()
        .flatten()
        .fold(0.0f32, |m, s| m.max(s.abs()));
    let nan = audio.channels.iter().flatten().any(|s| !s.is_finite());
    println!(
        "exported {:.2} s in {:?} to {}: peak {:.2} dBFS, non-finite {nan}",
        audio.seconds(),
        started.elapsed(),
        path.display(),
        20.0 * peak.log10()
    );

    let mut failures = Vec::new();
    if nan {
        failures.push("export contains NaN or infinity".to_string());
    }
    if peak < db_to_gain(-60.0) {
        failures.push(format!(
            "export is silent (peak {:.1} dBFS)",
            20.0 * peak.log10()
        ));
    }

    // Export again in the same session: the render must be repeatable.
    let again = export(&mut session, "daw-smoke-again.wav")?;
    let repeat = compare("second export, same session", &audio, &again);

    // Save, reopen, export again: plugin state and routing must survive the round trip.
    // The save is the app's (the file written on a worker) and the reopen too (15.2): plug-ins
    // load a few per update, so the window can paint between them; each heavy one is an
    // update of its own.
    let project = std::env::temp_dir().join("daw-smoke").join("smoke.dawproj");
    let _ = std::fs::remove_dir_all(project.with_file_name("Backups"));
    let started = Instant::now();
    session.save_in_background(&project)?;
    let returned = started.elapsed();
    session.finish_saving()?;
    println!(
        "background save returned in {returned:?}, written in {:?}",
        started.elapsed()
    );
    session.stage_loads = true;
    session.open(&project)?;
    let mut updates = 0;
    while session.loading().is_some() {
        session.update();
        updates += 1;
    }
    session.stage_loads = false;
    // Built-in effects load in microseconds, several to an update; VST3s one each.
    let vst3 = wanted
        .iter()
        .filter(|name| {
            plugins::resolve(name, &installed)
                .is_ok_and(|p| !p.to_string_lossy().starts_with("native:"))
        })
        .count();
    println!(
        "reopened in {updates} updates for {} plug-ins ({vst3} VST3)",
        wanted.len()
    );
    if updates < vst3 {
        failures.push(format!(
            "the reopen loaded {vst3} VST3 plug-ins in {updates} updates: the window couldn't paint between them"
        ));
    }
    let reopened = export(&mut session, "daw-smoke-reopened.wav")?;
    println!("reopened project: {} tracks", session.project.tracks.len());
    let reopen = compare("export after reopening", &audio, &reopened);
    for notice in &session.notices {
        println!("notice: {notice}");
    }

    // Autosave (13.2): with nothing changed since the reopen, a due autosave writes nothing,
    // which holds only if every plug-in's state reads back the same bytes each time; and a
    // backup (every plug-in's state read, then written) is quick on the main thread.
    // Its states are read an update at a time and the backup written on a worker (20.1): the
    // update that begins it, then updates until it is done (no other begun meanwhile).
    session.autosave = Some(Duration::from_millis(1));
    std::thread::sleep(Duration::from_millis(5));
    session.update();
    let began = session.autosaving();
    session.autosave = None;
    let deadline = Instant::now() + Duration::from_secs(30);
    while session.autosaving() && Instant::now() < deadline {
        session.update();
        std::thread::sleep(Duration::from_millis(1));
    }
    if !began || session.autosaving() {
        failures.push(format!(
            "the autosave {}",
            if began {
                "never finished"
            } else {
                "never began"
            }
        ));
    }
    let unchanged = session.backups().len();
    let started = Instant::now();
    session.save_backup()?;
    println!(
        "backup with {} plug-ins written in {:?}; an unchanged project made {unchanged} backups",
        wanted.len(),
        started.elapsed()
    );
    if unchanged != 0 {
        failures.push(format!(
            "autosave backed up an unchanged project ({unchanged} backups): a plug-in's state              reads back differently each time"
        ));
    }

    // The fx rack's power button bypasses the whole chain: the export must be the dry
    // signal, exactly as with the plug-ins removed (bypass keeps their latency, which the
    // export compensates).
    let changed = session.set_inserts_bypassed(InsertTarget::Track(guitar), true)?;
    session.update();
    let bypassed = export(&mut session, "daw-smoke-bypassed.wav")?;
    let ids: Vec<_> = session
        .project
        .track(guitar)
        .map(|t| t.inserts.iter().map(|i| i.id).collect())
        .unwrap_or_default();
    for id in ids {
        session.remove_insert(id);
    }
    let removed = export(&mut session, "daw-smoke-removed.wav")?;
    let bypass = compare(
        &format!("chain bypassed ({changed} plug-ins) vs removed"),
        &bypassed,
        &removed,
    );
    if bypass > f32::NEG_INFINITY {
        failures.push(format!(
            "a bypassed chain differs from no plug-ins by {bypass:.1} dB"
        ));
    }

    // A high-gain amp sim was measured to vary by about -86 dB between runs; anything louder is
    // a real change.
    for (what, diff) in [
        ("repeat export", repeat),
        ("export after reopening", reopen),
    ] {
        if diff > MAX_DIFF_DB {
            failures.push(format!(
                "{what} differs by {diff:.1} dB (limit {MAX_DIFF_DB} dB)"
            ));
        }
    }
    if session.project.tracks.len() != 3 {
        failures.push("reopened project lost tracks".into());
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("smoke check failed")
    }
}

/// Largest acceptable difference between two exports of the same project.
const MAX_DIFF_DB: f32 = -70.0;

fn export(session: &mut Session, name: &str) -> Result<wav::Audio> {
    let out = std::env::temp_dir().join(name);
    session.export(&out, None, 0.5)?;
    let path = session.wait_for_export(Duration::from_secs(120))?;
    wav::read(&path)
}

/// Print the peak difference between two renders and where the worst of it is, and return it
/// in dB (`-inf` when bit-identical).
fn compare(label: &str, a: &wav::Audio, b: &wav::Audio) -> f32 {
    if a.len() != b.len() {
        println!("{label}: lengths differ ({} vs {})", a.len(), b.len());
        return 0.0;
    }
    let mut worst = (0.0f32, 0usize);
    for (ca, cb) in a.channels.iter().zip(&b.channels) {
        for (i, (x, y)) in ca.iter().zip(cb).enumerate() {
            let d = (x - y).abs();
            if d > worst.0 {
                worst = (d, i);
            }
        }
    }
    if worst.0 == 0.0 {
        println!("{label}: bit-identical");
    } else {
        println!(
            "{label}: differs, peak {:.1} dB at {:.3} s",
            20.0 * worst.0.log10(),
            worst.1 as f64 / f64::from(a.sample_rate)
        );
    }
    20.0 * worst.0.log10()
}
