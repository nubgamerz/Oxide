//! `--neutral`: the manual's shots show no machine's details (dev plan 27.3, TD191).
//!
//! A shot for the manual is taken on whatever machine builds it (the developer's, and the
//! installer's build): left alone it would show that machine's audio devices, its MIDI ports,
//! the check's demo path and test port, and its core count. With `--neutral`:
//!
//! - **No audio device is opened.** The session starts with no device (as headless tests do)
//!   and runs on a virtual one (`Session::start_virtual_device`, 256 samples, nothing heard),
//!   so meters and the DSP readout work; what is drawn of a device (the status bar, Settings >
//!   Audio's driver, devices, rates and "Running:") is [`device_info`] and [`settings`]:
//!   WASAPI, "Speakers" and "Microphone".
//! - **The demo's path** is drawn as `Documents\Oxide Projects\Demo\Demo.oxide`
//!   ([`project_path`]); the files stay in `out/check/Demo`. Nothing is written elsewhere.
//! - **The start screen's recent projects** are all shown in [`PROJECTS_FOLDER`].
//! - **MIDI** shows one port, "MIDI Keyboard" ([`MIDI_PORTS`]), whatever is connected.
//! - **MCP** shows its default port (7878) while serving on the shots' own.
//! - **Audio threads**: 4 (three workers), whatever the cores.
//! - **Plug-ins** (27.7): the scenarios put built-in effects where a third-party effect would
//!   do ([`DawApp::demo_effect`](crate::app::DawApp::demo_effect)); a VST3 that is still
//!   needed (an instrument) is shown by what it is, never by its maker's name
//!   ([`plugin_name`], applied once the scenario is set up by [`name_plugins`]).
//!
//! Display only: the session's own settings (`Session::settings`) are left as they run, and
//! check runs never save the app's settings.

use daw_core::device::{AudioSettings, DeviceCaps, DeviceInfo, DeviceList};

pub const HOST: &str = "WASAPI";
pub const OUTPUT: &str = "Speakers";
pub const INPUT: &str = "Microphone";
/// The virtual device's buffer, as shown.
pub const BUFFER: u32 = 256;
/// Worker threads (the status bar says one more: the driver's).
pub const THREADS: usize = 3;
pub const MIDI_PORTS: &[&str] = &["MIDI Keyboard"];
/// MCP's port as shown: the default (`settings::McpSettings`).
pub const MCP_PORT: u16 = 7878;

/// What the session starts with: no device opened (an output no machine has), three workers.
pub fn session_settings() -> AudioSettings {
    AudioSettings {
        output_device: Some("<none>".into()),
        threads: Some(THREADS),
        ..Default::default()
    }
}

/// The settings Settings > Audio shows.
pub fn settings() -> AudioSettings {
    AudioSettings {
        host: HOST.into(),
        output_device: Some(OUTPUT.into()),
        input_device: Some(INPUT.into()),
        sample_rate: None,
        buffer_size: Some(BUFFER),
        threads: Some(THREADS),
        record_offset: 0,
        processing_block: None,
    }
}

/// The drivers, devices and what they support, as Settings > Audio lists them.
pub fn hosts() -> Vec<String> {
    vec![HOST.into(), "ASIO".into()]
}

pub fn devices() -> DeviceList {
    DeviceList {
        outputs: vec![OUTPUT.into()],
        inputs: vec![INPUT.into()],
    }
}

pub fn caps(rate: u32) -> DeviceCaps {
    DeviceCaps {
        sample_rates: vec![44_100, 48_000, 96_000],
        buffer_range: Some((32, 2048)),
        default_rate: rate,
    }
}

/// The device as the status bar and Settings > Audio show it running.
pub fn device_info(rate: u32) -> DeviceInfo {
    DeviceInfo {
        host: HOST.into(),
        output_name: OUTPUT.into(),
        input_name: Some(INPUT.into()),
        sample_rate: rate,
        buffer_size: Some(BUFFER),
        inputs: 2,
        outputs: 2,
        sample_format: "f32".into(),
    }
}

/// Settings > Projects' settings file, as shown.
pub const SETTINGS_FILE: &str = r"Oxide\settings.json";

/// The folder the start screen's recent projects (and a new project) are shown in.
pub const PROJECTS_FOLDER: &str = r"Documents\Oxide Projects";

/// A project's file as the menu bar shows it: in the Documents folder's `Oxide Projects`.
pub fn project_path(name: &str) -> String {
    format!(r"Documents\Oxide Projects\{name}\{name}.oxide")
}

/// A file in project `name`'s folder (`folder`) as the shots show it: under
/// `Documents\Oxide Projects\<name>`; a file elsewhere by its name alone.
pub fn file_path(name: &str, folder: Option<&std::path::Path>, path: &std::path::Path) -> String {
    match folder.and_then(|f| path.strip_prefix(f).ok()) {
        Some(inside) => format!(r"{PROJECTS_FOLDER}\{name}\{}", inside.display()),
        None => path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
    }
}

/// What a third-party (VST3) plug-in is shown as: an instrument (the first insert of an
/// instrument track) "Synth", any other "Plug-in". Built-in effects keep their names (they are
/// Oxide's own).
pub fn plugin_name(instrument: bool) -> &'static str {
    if instrument {
        "Synth"
    } else {
        "Plug-in"
    }
}

/// The one place a neutral run's third-party plug-ins get their shown names: every VST3
/// insert loaded in the project (tracks, the master) and in the Waveform view's rack is renamed
/// by [`plugin_name`], and a track named after its instrument (as Insert MIDI track names it)
/// takes the new name too. A slot that isn't loaded (a scenario's missing plug-in) keeps the
/// name the scenario gave it. Run once, after the `--show` scenario is set up; check runs
/// never save the settings, and the demo is remade each run.
pub fn name_plugins(session: &mut daw_core::session::Session) {
    let shown = |session: &daw_core::session::Session, slot: &daw_core::project::InsertSlot| {
        daw_core::native::kind_of(&slot.path).is_none() && session.is_loaded(slot.id)
    };
    // (insert, its track if it is that track's instrument)
    let mut renamed = Vec::new();
    for track in &session.project.tracks {
        let instrument = track.kind == daw_core::project::TrackKind::Instrument;
        for (k, slot) in track.inserts.iter().enumerate() {
            if shown(session, slot) {
                renamed.push((slot.id, (instrument && k == 0).then_some(track.id)));
            }
        }
    }
    let others = session
        .project
        .master
        .inserts
        .iter()
        .chain(session.editor_rack());
    renamed.extend(others.filter(|s| shown(session, s)).map(|s| (s.id, None)));
    for (id, instrument_of) in renamed {
        let name = plugin_name(instrument_of.is_some());
        let Some(slot) = session.insert_mut(id) else {
            continue;
        };
        let was = std::mem::replace(&mut slot.name, name.to_string());
        if let Some(track) = instrument_of.and_then(|t| session.project.track_mut(t)) {
            if track.name == was {
                track.name = name.to_string();
            }
        }
    }
}

/// MCP's status line with the port shown as the default one.
pub fn mcp_text(text: &str, port: u16) -> String {
    text.replace(&format!(":{port}"), &format!(":{MCP_PORT}"))
        .replace(&format!("port {port}"), &format!("port {MCP_PORT}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_names_are_neutral() {
        assert_eq!(
            project_path("Demo"),
            r"Documents\Oxide Projects\Demo\Demo.oxide"
        );
        assert_eq!(
            mcp_text("MCP 127.0.0.1:7896/mcp", 7896),
            "MCP 127.0.0.1:7878/mcp"
        );
        assert_eq!(
            mcp_text("MCP off: port 7896 is in use", 7896),
            "MCP off: port 7878 is in use"
        );
        // The session opens no device; the dialog shows one.
        assert_eq!(session_settings().output_device.as_deref(), Some("<none>"));
        assert_eq!(settings().output_device.as_deref(), Some(OUTPUT));
        assert!(caps(48_000).sample_rates.contains(&48_000));
    }
}
