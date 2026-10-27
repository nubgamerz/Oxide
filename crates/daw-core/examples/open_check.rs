//! Opening a project behind its loading bar (dev plan 17.1), headless, with real plug-ins: a
//! project of nine audio files (eight long stereo tones and the audio given, which is resampled) and
//! the plug-ins named on the command line is opened over itself as the app opens it (staged),
//! and every `update` is timed. Checks that `open` returns within a frame's budget (nothing
//! decoded or destroyed in it), that every update while the files read stays within it, that
//! each VST3 is closed or loaded in an update of its own, that everything arrives, and that a
//! cancel partway through the plug-ins goes back to the project open before, whole. Prints
//! `RESULT: PASS` and exits 0, or `FAIL:` lines and exits 1.
//!
//! Writes its project to `out/check/OpenCheck` (remade each run).
//!
//! cargo run --release -p daw-core --example open_check -- <input.wav> [plugin name ...]

use anyhow::{bail, Result};
use daw_core::device::AudioSettings;
use daw_core::plugins;
use daw_core::project::TrackKind;
use daw_core::session::{InsertTarget, LoadStage, Session};
use daw_core::wav;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// A frame's budget at 60 Hz.
const BUDGET: Duration = Duration::from_micros(16_667);

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let input = PathBuf::from(
        args.next()
            .ok_or_else(|| anyhow::anyhow!("give the audio file (a WAV) as the first argument"))?,
    );
    let wanted: Vec<String> = args.collect();
    let settings = AudioSettings {
        output_device: Some("<none>".into()),
        sample_rate: Some(48_000),
        ..Default::default()
    };
    let mut session = Session::new(settings);
    let parent = std::path::absolute("out/check")?;
    let _ = std::fs::remove_dir_all(parent.join("OpenCheck"));
    let path = session.new_project_in("OpenCheck", &parent, None)?;
    let imported = session.project_folder(daw_core::folder::IMPORTED)?;
    let mut tracks = Vec::new();
    for k in 0..8 {
        let file = imported.join(format!("tone {k}.wav"));
        let frames = 48_000 * 60;
        let channels = (0..2)
            .map(|c| {
                (0..frames)
                    .map(|i| (i as f32 * 0.01 * (k + c + 1) as f32).sin() * 0.3)
                    .collect()
            })
            .collect();
        let audio = wav::Audio {
            sample_rate: 48_000,
            channels,
        };
        wav::write_f32(&file, &audio)?;
        let track = session.add_track(TrackKind::Audio, None);
        session.add_clip_from_file(track, &file, 0)?;
        tracks.push(track);
    }
    let di = session.add_track(TrackKind::Audio, None);
    session.add_clip_from_file(di, &input, 0)?;
    let installed = session.installed_plugins().to_vec();
    let mut vst3 = 0;
    for (k, name) in wanted.iter().enumerate() {
        let plugin = plugins::resolve(name, &installed)?;
        vst3 += usize::from(!plugin.to_string_lossy().starts_with("native:"));
        session.add_insert(InsertTarget::Track(tracks[k % tracks.len()]), &plugin)?;
    }
    session.update();
    session.save(&path)?;
    let mut failures = Vec::new();

    // The app's open: over the same project, its plug-ins live.
    session.stage_loads = true;
    let started = Instant::now();
    session.open(&path)?;
    let returned = started.elapsed();
    println!("open returned in {:.2} ms", returned.as_secs_f64() * 1000.0);
    if returned > BUDGET {
        failures.push(format!(
            "open took {:.1} ms on the calling thread (budget {:.1} ms)",
            ms(returned),
            ms(BUDGET)
        ));
    }
    let timed = run(&mut session)?;
    let reading = timed
        .iter()
        .filter(|u| u.from == LoadStage::Reading && u.to == Some(LoadStage::Reading));
    let worst_reading = reading.map(|u| u.took).max().unwrap_or_default();
    let closing = timed
        .iter()
        .filter(|u| u.from == LoadStage::Closing)
        .count();
    let loading = timed.iter().filter(|u| u.loaded > 0).count();
    let worst = timed.iter().map(|u| u.took).max().unwrap_or_default();
    println!(
        "{} updates; reading files: worst {:.2} ms; {closing} updates closing plug-ins, {loading} loading them, worst {:.1} ms; done in {:.0} ms",
        timed.len(),
        ms(worst_reading),
        ms(worst),
        ms(started.elapsed())
    );
    for u in timed.iter().filter(|u| u.took > BUDGET) {
        println!(
            "  {:.1} ms {:?} -> {:?}, {} loaded",
            ms(u.took),
            u.from,
            u.to,
            u.loaded
        );
    }
    if worst_reading > BUDGET {
        failures.push(format!(
            "an update while the files read took {:.1} ms (budget {:.1} ms)",
            ms(worst_reading),
            ms(BUDGET)
        ));
    }
    // Each VST3 loaded is an update of its own (the bar paints between them).
    if loading < vst3 {
        failures.push(format!(
            "{vst3} VST3 plug-ins loaded in {loading} updates: the window couldn't paint between them"
        ));
    }
    failures.extend(whole(&session, "the open"));

    // Cancel partway through the plug-ins of another project: back to this one, whole.
    let other = path.with_file_name("OpenCheck other.oxide");
    session.save(&other)?;
    session.save(&path)?;
    // With no plug-ins to load, partway through the files (held) instead.
    let plugins = session.project.all_inserts().count();
    if plugins == 0 {
        session.hold_file_reads(Some(0));
    }
    session.open(&other)?;
    while plugins > 0
        && session
            .loading()
            .is_some_and(|l| l.stage != LoadStage::Loading || l.stage_done == 0)
    {
        session.update();
    }
    let partway = session.loading();
    let reopened = session.cancel_loading()?;
    println!(
        "cancelled at {partway:?}: {}",
        reopened
            .as_deref()
            .map_or("no project".into(), |p| p.display().to_string())
    );
    if reopened.as_deref() != Some(path.as_path()) {
        failures.push(format!(
            "a cancel went to {reopened:?}, not back to {}",
            path.display()
        ));
    }
    run(&mut session)?;
    failures.extend(whole(&session, "the project a cancel went back to"));
    if session.path.as_deref() != Some(path.as_path()) {
        failures.push("the cancel's project isn't the one open before".into());
    }

    for notice in &session.notices {
        println!("notice: {notice}");
    }
    if failures.is_empty() {
        println!("RESULT: PASS");
        Ok(())
    } else {
        failures.iter().for_each(|f| println!("FAIL: {f}"));
        bail!("open check failed")
    }
}

/// One update while the project opened.
struct Update {
    took: Duration,
    from: LoadStage,
    to: Option<LoadStage>,
    /// Plug-ins loaded in it.
    loaded: usize,
}

/// Update until the open is over (as the app's frames do), timing each.
fn run(session: &mut Session) -> Result<Vec<Update>> {
    let started = Instant::now();
    let mut timed = Vec::new();
    while let Some(progress) = session.loading() {
        if started.elapsed() > Duration::from_secs(300) {
            bail!("the open never finished: {progress:?}");
        }
        let live = |s: &Session| {
            s.project
                .all_inserts()
                .filter(|i| s.insert_info(i.id).is_some())
                .count()
        };
        let before = live(session);
        let t = Instant::now();
        session.update();
        let took = t.elapsed();
        timed.push(Update {
            took,
            from: progress.stage,
            to: session.loading().map(|l| l.stage),
            loaded: live(session) - before,
        });
    }
    Ok(timed)
}

/// What's missing from the open project: a clip's audio, a plug-in.
fn whole(session: &Session, what: &str) -> Vec<String> {
    let mut failures = Vec::new();
    let clips = session.project.tracks.iter().flat_map(|t| &t.clips);
    let silent = clips.filter(|c| session.clip_audio(c).is_none()).count();
    if silent > 0 {
        failures.push(format!("{what}: {silent} clips have no audio"));
    }
    let missing = session
        .project
        .all_inserts()
        .filter(|s| session.insert_info(s.id).is_none())
        .count();
    if missing > 0 {
        failures.push(format!("{what}: {missing} plug-ins didn't load"));
    }
    if session.project.tracks.len() != 9 {
        failures.push(format!("{what}: {} tracks", session.project.tracks.len()));
    }
    failures
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}
