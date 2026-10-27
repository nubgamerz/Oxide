//! The registered actions, one module per area. Every user-facing operation belongs here (see
//! CONTRIBUTING.md, "New features need two more things"). Ids are the project's stable ids; positions are samples at the
//! project's sample rate.

mod automation;
mod clips;
mod devices;
mod diagnostics;
mod edit;
mod editor;
mod loudness;
mod manual;
mod markers;
mod midi_clips;
mod plugins;
mod project;
mod rack;
mod sends;
mod templates;
mod tracks;
mod transport;
mod ui;

use super::{ApiError, ApiResult, Registry};
use crate::project::{ClipId, InsertId, SendId, TrackId, TrackKind};
use crate::session::Session;
use schemars::JsonSchema;
use serde::Deserialize;

/// The areas actions are grouped in, in the registry's order: each a module here, and a page
/// of the manual's MCP reference (`mcp-<id>.html`, `-` for `_`). Its id, the page's title and
/// a line on what the area covers.
pub const AREAS: &[(&str, &str, &str)] = &[
    (
        "project",
        "Project actions",
        "The project as a whole: reading it, new, open, save and backups, undo and redo, the          tempo, meter and grid, and what is happening while a project opens or saves.",
    ),
    (
        "tracks",
        "Track actions",
        "Tracks, folders and buses: adding, naming, nesting, routing their inputs and outputs,          the mixer's volume, pan, mute and solo, track renders and the master bus.",
    ),
    (
        "sends",
        "Send actions",
        "Sends: copies of a track's signal into a folder or a bus (a reverb return), beside its          main output.",
    ),
    (
        "plugins",
        "Plug-in actions",
        "Plug-ins and built-in effects: what is installed, insert chains, plug-in windows,          parameters, impulse files and multi-out racks.",
    ),
    (
        "clips",
        "Audio clip actions",
        "Audio clips on the timeline: importing, moving, splitting, trimming, fades and gain,          copy and paste, takes and reading a clip's waveform.",
    ),
    (
        "midi_clips",
        "MIDI actions",
        "MIDI clips and notes in musical time (ticks, 960 to a quarter note, or          \"bar:beat:tick\"): writing and reading notes and controllers, MIDI files, quantizing          and MIDI recording options.",
    ),
    (
        "automation",
        "Automation actions",
        "Automation lanes and points moving volume, pan, mute, send levels and plug-in          parameters along the song, and the write modes.",
    ),
    (
        "edit",
        "Grid editing actions",
        "Analysis and editing on the musical grid: transients, splitting at many places and          quantizing clips.",
    ),
    (
        "editor",
        "Waveform view actions",
        "The Waveform view: opening one audio file, editing it in its own history, its          spectrum, the effects rack's Apply and noise prints, metadata and saving the file.          Positions are the file's own samples.",
    ),
    (
        "diagnostics",
        "Diagnostics actions",
        "The Waveform view's Diagnostics: finding clicks and clipped runs in an open file and          repairing the ones chosen.",
    ),
    (
        "transport",
        "Transport actions",
        "Playback, the loop and punch ranges, the metronome and count-in, recording, and          exporting the mix.",
    ),
    (
        "loudness",
        "Loudness actions",
        "The master's EBU R 128 loudness meter, its target, and measuring a file with the same          meter.",
    ),
    (
        "markers",
        "Marker actions",
        "Markers: named places in the song, kept on their bars when the tempo changes.",
    ),
    (
        "templates",
        "Template actions",
        "Project templates: a project's setup saved to start new projects from.",
    ),
    (
        "rack",
        "Effects rack actions",
        "The Waveform view's effects rack: its presets, and syncing it with a track's inserts.",
    ),
    (
        "devices",
        "Device actions",
        "Audio and MIDI devices, the audio settings, the engine's statistics and CPU use.",
    ),
    (
        "ui",
        "Window actions",
        "What only the app window can do: screenshots, meters, the view and selection, window          layouts, recent projects and the app's version.",
    ),
    (
        "manual",
        "Manual actions",
        "Oxide's manual: searching it for an answer, and showing the user a page of it.",
    ),
];

pub(super) fn register(r: &mut Registry) {
    type Register = fn(&mut Registry);
    let areas: [(&str, Register); 18] = [
        ("project", project::register),
        ("tracks", tracks::register),
        ("sends", sends::register),
        ("plugins", plugins::register),
        ("clips", clips::register),
        ("midi_clips", midi_clips::register),
        ("automation", automation::register),
        ("edit", edit::register),
        ("editor", editor::register),
        ("diagnostics", diagnostics::register),
        ("transport", transport::register),
        ("loudness", loudness::register),
        ("markers", markers::register),
        ("templates", templates::register),
        ("rack", rack::register),
        ("devices", devices::register),
        ("ui", ui::register),
        ("manual", manual::register),
    ];
    debug_assert!(areas.iter().map(|a| a.0).eq(AREAS.iter().map(|a| a.0)));
    for (area, register) in areas {
        r.area = area;
        register(r);
    }
    r.area = "";
}

/// For actions that take no arguments.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

fn error(message: impl Into<String>) -> ApiError {
    ApiError(message.into())
}

fn check_track(s: &Session, id: TrackId) -> ApiResult<()> {
    match s.project.track(id) {
        Some(_) => Ok(()),
        None => Err(error(format!("no track with id {id} (see get_project)"))),
    }
}

fn check_folder(s: &Session, id: TrackId) -> ApiResult<()> {
    match s.project.track(id) {
        Some(t) if t.kind == TrackKind::Folder => Ok(()),
        Some(_) => Err(error(format!("track {id} is not a folder"))),
        None => Err(error(format!("no track with id {id} (see get_project)"))),
    }
}

fn check_clip(s: &Session, id: ClipId) -> ApiResult<()> {
    match s.project.find_clip(id) {
        Some(_) => Ok(()),
        // MIDI clips share the id space; say which actions take them.
        None if s.project.find_midi_clip(id).is_some() => Err(error(format!(
            "clip {id} is a MIDI clip; this action works on audio clips (MIDI clips: \
             get_midi_clip, set_midi_clip, remove_midi_clip)"
        ))),
        None => Err(error(format!("no clip with id {id} (see get_project)"))),
    }
}

fn check_send(s: &Session, id: SendId) -> ApiResult<()> {
    match s.project.find_send(id) {
        Some(_) => Ok(()),
        None => Err(error(format!("no send with id {id} (see get_project)"))),
    }
}

fn check_insert(s: &Session, id: InsertId) -> ApiResult<()> {
    match s.find_insert(id) {
        Some(_) => Ok(()),
        None => Err(error(format!("no insert with id {id} (see get_project)"))),
    }
}

/// Up or down in a list.
#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Up,
    Down,
}

#[cfg(test)]
mod examples;
#[cfg(test)]
mod tests;
