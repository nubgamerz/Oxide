//! Tracks: creating, naming, nesting, routing and mixing them, and the master bus.

use super::{check_folder, check_track, error, Direction};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::project::{
    Mixer, Monitor, Place, TrackId, TrackInput, TrackKind, TrackOutput, MAX_TRACK_HEIGHT,
    MIN_TRACK_HEIGHT,
};
use crate::session::Session;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "add_track",
        "Add a track and return its id. `kind` is audio, instrument, folder (a submix bus for \
         the tracks inside it) or bus (a return track for reverbs and other shared effects, \
         fed by other tracks' sends or outputs; it holds no clips). `parent` puts it inside \
         that folder track.",
        Safety::Edit,
        add_track,
    );
    r.add(
        "remove_track",
        "Delete a track with its clips and plug-ins. A folder's sub-tracks move up a level.",
        Safety::Destructive,
        |host, input: TrackArg| {
            check_track(host.session(), input.track)?;
            host.session().remove_track(input.track);
            Ok(Done { done: true })
        },
    );
    r.add(
        "set_track",
        "Rename a track, recolour it (`color` as \"#rrggbb\"), collapse/expand a folder in \
         the arrangement, set its row `height` in points (32 to 400; 0 for the default; \
         taller rows draw clearer waveforms), or show or hide its automation lanes \
         (`show_automation`). Only the fields given change. The height and the lanes shown \
         are view state: saved with the project, but not undo steps.",
        Safety::Edit,
        set_track,
    );
    r.add(
        "set_track_input",
        "Set what feeds a track. Audio tracks: {\"type\":\"audio\",\"channel\":0,\"stereo\":false} \
         where channel is the 0-based hardware input (0 is \"In 1\"); stereo takes channel and \
         channel+1. Instrument tracks: {\"type\":\"midi\",\"port\":null,\"channel\":null} where \
         port is a MIDI input name (null = all ports) and channel 0-15 filters one MIDI channel \
         (null = all). {\"type\":\"none\"} disconnects it. A track fed by a plug-in's extra \
         output (get_project shows its input as {\"Plugin\":{\"insert\":..,\"output\":..}}) \
         belongs to a multi-out rack: create_multi_out_rack and remove_multi_out_rack set and \
         clear those.",
        Safety::Edit,
        set_track_input,
    );
    r.add(
        "set_track_output",
        "Route a track's main output: {\"type\":\"parent\"} (its folder, or the master), \
         {\"type\":\"master\"}, {\"type\":\"bus\",\"track\":id} (any folder or bus track that \
         would not create a loop through outputs or sends), or \
         {\"type\":\"hardware\",\"channel\":0} (straight to the output pair starting at that \
         0-based channel, bypassing the master). Sends are separate: see add_send.",
        Safety::Edit,
        set_track_output,
    );
    r.add(
        "set_track_monitor",
        "Input monitoring: auto (hear the input while the track is armed), on, or off. \
         Monitoring runs the input through the track's plug-ins.",
        Safety::Edit,
        |host, input: SetMonitor| {
            let s = host.session();
            check_track(s, input.track)?;
            let track = s.project.track_mut(input.track).expect("checked");
            track.monitor = match input.monitor {
                MonitorArg::Auto => Monitor::Auto,
                MonitorArg::On => Monitor::On,
                MonitorArg::Off => Monitor::Off,
            };
            Ok(Done { done: true })
        },
    );
    r.add(
        "move_track",
        "Move a track (with its sub-tracks) one place up or down among its siblings.",
        Safety::Edit,
        |host, input: MoveTrack| {
            let s = host.session();
            check_track(s, input.track)?;
            s.project
                .move_track(input.track, matches!(input.direction, Direction::Up));
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
    r.add(
        "set_track_parent",
        "Move a track (with its sub-tracks) into a folder track, or to the top level when \
         `folder` is omitted. A folder cannot go inside itself.",
        Safety::Edit,
        |host, input: SetParent| {
            let s = host.session();
            check_track(s, input.track)?;
            if let Some(folder) = input.folder {
                check_folder(s, folder)?;
            }
            s.project
                .set_parent(input.track, input.folder)
                .map_err(error)?;
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
    r.add(
        "move_track_to",
        "Move a track (with its sub-tracks) to a place in the track list, as dragging its \
         header does: `before` or `after` another track (it joins that track's folder, or the \
         top level), or into `folder` as its `position`th sub-track (0 = first; omitted or \
         past the last: at the end), or with none of them to the end of the top level. Give \
         at most one of `before`, `after` and `folder`. Folder sums and routing follow. \
         Refused for a folder inside itself, beside a track inside the one moved, or a move \
         that would close a loop.",
        Safety::Edit,
        |host, input: MoveTrackTo| {
            let s = host.session();
            check_track(s, input.track)?;
            let place = match (input.before, input.after, input.folder) {
                (Some(t), None, None) => Place::Before(t),
                (None, Some(t), None) => Place::After(t),
                (None, None, Some(folder)) => {
                    check_folder(s, folder)?;
                    Place::Into {
                        folder,
                        index: input.position.unwrap_or(usize::MAX),
                    }
                }
                (None, None, None) => Place::End,
                _ => return Err(error("give at most one of `before`, `after` and `folder`")),
            };
            for other in [input.before, input.after].into_iter().flatten() {
                check_track(s, other)?;
            }
            s.project.move_track_to(input.track, place).map_err(error)?;
            s.mark_dirty();
            Ok(Done { done: true })
        },
    );
    r.add(
        "set_track_mixer",
        "Change a track's mixer settings. Only the fields given change: `gain_db` (-60 to +12 \
         dB), `pan` (-1 left to 1 right), `mute`, `solo`, `arm` (record arm; for instrument \
         tracks, play from the MIDI keyboard). Returns the resulting settings.",
        Safety::Edit,
        set_track_mixer,
    );
    r.add(
        "set_track_enabled",
        "Switch a track off (`enabled` false) or on again. Off, the track and whatever reaches \
         the mix only through it (a folder's tracks) leave the audio graph: its plug-ins cost \
         no CPU, unlike a mute, and it is silent; every setting is kept. An undo step.",
        Safety::Edit,
        |host, input: SetEnabled| {
            let s = host.session();
            check_track(s, input.track)?;
            s.project.track_mut(input.track).expect("checked").enabled = input.enabled;
            s.mark_dirty();
            Ok(Enabled {
                track: input.track,
                enabled: input.enabled,
            })
        },
    );
    r.add(
        "render_track",
        "Render a track as it sounds (its clips, or its MIDI through its instrument; its \
         plug-ins, fader, pan and automation; a folder's tracks; not its sends) from its first \
         clip to its last plus `tail_seconds` (default 2, for reverb and note releases) into \
         the project's Rendered folder as a 32-bit float WAV, through the same engine and \
         plug-in pre-roll as an export, so it sounds exactly like playback. When it finishes, \
         the file goes on a new audio track right under it, routed where it goes, with its \
         sends, at 0 dB and centre; with `disable` (default true) the original is switched off \
         (set_track_enabled) in the same undo step, to save its CPU. Stops playback and runs in \
         the background on export's thread, as export_mixdown does: it answers at once with the \
         file's path; poll get_export_status (its `track` names the new track once finished), \
         and cancel_export stops it (no new track, the original left on). Refused while the \
         project's plug-ins are still loading.",
        Safety::Edit,
        |host, input: RenderTrack| {
            let s = host.session();
            check_track(s, input.track)?;
            let path = s.render_track(
                input.track,
                input.tail_seconds.unwrap_or(2.0),
                input.disable.unwrap_or(true),
            )?;
            Ok(Rendered { path })
        },
    );
    r.add(
        "set_master",
        "Change the master bus: `gain_db` (-60 to +12), `pan`, `mute`, and `output_channel` \
         (first channel of the hardware output pair, 0-based and even); and its row at the top \
         of the arrangement: `height` in points (32 to 400; 0 for the default) and \
         `show_automation` (its lanes shown under it). Only the fields given change. The \
         height and the lanes shown are view state: saved with the project, but not undo \
         steps.",
        Safety::Edit,
        set_master,
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetEnabled {
    track: TrackId,
    enabled: bool,
}

#[derive(Serialize, JsonSchema)]
struct Enabled {
    track: TrackId,
    enabled: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenderTrack {
    track: TrackId,
    /// Seconds rendered past the last clip's end (default 2).
    #[serde(default)]
    tail_seconds: Option<f64>,
    /// Switch the original off once the render is in (default true).
    #[serde(default)]
    disable: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
struct Rendered {
    path: std::path::PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TrackArg {
    track: TrackId,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum Kind {
    Audio,
    Instrument,
    Folder,
    Bus,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddTrack {
    kind: Kind,
    /// Display name; a default like "Audio 3" when omitted.
    #[serde(default)]
    name: Option<String>,
    /// Folder track to put it in.
    #[serde(default)]
    parent: Option<TrackId>,
}

#[derive(Serialize, JsonSchema)]
struct AddedTrack {
    track: TrackId,
    name: String,
}

fn add_track(host: &mut dyn Host, input: AddTrack) -> ApiResult<AddedTrack> {
    let s = host.session();
    if let Some(parent) = input.parent {
        check_folder(s, parent)?;
    }
    let kind = match input.kind {
        Kind::Audio => TrackKind::Audio,
        Kind::Instrument => TrackKind::Instrument,
        Kind::Folder => TrackKind::Folder,
        Kind::Bus => TrackKind::Bus,
    };
    let id = s.add_track(kind, input.parent);
    let track = s.project.track_mut(id).expect("just added");
    if let Some(name) = input.name.filter(|n| !n.trim().is_empty()) {
        track.name = name.trim().to_string();
    }
    Ok(AddedTrack {
        track: id,
        name: track.name.clone(),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTrack {
    track: TrackId,
    #[serde(default)]
    name: Option<String>,
    /// Colour as "#rrggbb".
    #[serde(default)]
    color: Option<String>,
    /// Folders only: hide (true) or show the sub-tracks.
    #[serde(default)]
    collapsed: Option<bool>,
    /// Row height in the arrangement, in points (32 to 400); 0 for the default.
    #[serde(default)]
    height: Option<f32>,
    /// Show (true) or hide the track's automation lanes under it in the arrangement.
    #[serde(default)]
    show_automation: Option<bool>,
}

fn parse_color(text: &str) -> ApiResult<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    let bad = || error(format!("colour '{text}' is not #rrggbb"));
    if hex.len() != 6 {
        return Err(bad());
    }
    let byte = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).map_err(|_| bad());
    Ok([byte(0)?, byte(2)?, byte(4)?])
}

#[derive(Serialize, JsonSchema)]
struct TrackSummary {
    track: TrackId,
    name: String,
    color: String,
    collapsed: bool,
    height: f32,
    show_automation: bool,
}

fn set_track(host: &mut dyn Host, input: SetTrack) -> ApiResult<TrackSummary> {
    let s = host.session();
    check_track(s, input.track)?;
    let color = input.color.as_deref().map(parse_color).transpose()?;
    if let Some(name) = &input.name {
        // The header's and the inspector's name fields rename through the same path.
        s.rename_track(input.track, name)?;
    }
    let track = s.project.track_mut(input.track).expect("checked");
    if let Some(color) = color {
        track.color = color;
    }
    if let Some(collapsed) = input.collapsed {
        if track.kind != TrackKind::Folder {
            return Err(error(format!("track {} is not a folder", track.id)));
        }
        track.collapsed = collapsed;
    }
    if let Some(height) = input.height {
        track.height = check_height(height)?;
    }
    if let Some(show) = input.show_automation {
        track.show_automation = show;
    }
    let [r, g, b] = track.color;
    Ok(TrackSummary {
        height: track.height(),
        show_automation: track.show_automation,
        track: track.id,
        name: track.name.clone(),
        color: format!("#{r:02x}{g:02x}{b:02x}"),
        collapsed: track.collapsed,
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
enum InputArg {
    None,
    Audio {
        /// 0-based hardware input channel (0 is "In 1").
        channel: u16,
        /// Take `channel` and `channel + 1` as a stereo pair.
        #[serde(default)]
        stereo: bool,
    },
    Midi {
        /// MIDI input port name; omit or null for all ports.
        #[serde(default)]
        port: Option<String>,
        /// MIDI channel 0-15 to listen to; omit or null for all.
        #[serde(default)]
        channel: Option<u8>,
    },
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInput {
    track: TrackId,
    input: InputArg,
}

fn set_track_input(host: &mut dyn Host, input: SetInput) -> ApiResult<TrackInput> {
    let s = host.session();
    check_track(s, input.track)?;
    let kind = s.project.track(input.track).expect("checked").kind;
    let value = match input.input {
        InputArg::None => TrackInput::None,
        InputArg::Audio { channel, stereo } => {
            if kind != TrackKind::Audio {
                return Err(error("only audio tracks take an audio input"));
            }
            let needed = usize::from(channel) + usize::from(stereo) + 1;
            let available = s.input_channels();
            if available > 0 && needed > available {
                return Err(error(format!(
                    "the audio device has {available} inputs (channels 0..{})",
                    available - 1
                )));
            }
            TrackInput::Audio { channel, stereo }
        }
        InputArg::Midi { port, channel } => {
            if kind != TrackKind::Instrument {
                return Err(error("only instrument tracks take a MIDI input"));
            }
            if channel.is_some_and(|c| c > 15) {
                return Err(error("MIDI channels are 0-15"));
            }
            TrackInput::Midi { port, channel }
        }
    };
    s.project.track_mut(input.track).expect("checked").input = value.clone();
    s.mark_dirty();
    Ok(value)
}

#[derive(Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "lowercase")]
enum OutputArg {
    Parent,
    Master,
    Bus {
        /// The folder or bus track to route into.
        track: TrackId,
    },
    Hardware {
        /// First channel (0-based) of the output pair.
        channel: u16,
    },
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetOutput {
    track: TrackId,
    output: OutputArg,
}

fn set_track_output(host: &mut dyn Host, input: SetOutput) -> ApiResult<TrackOutput> {
    let s = host.session();
    check_track(s, input.track)?;
    let value = match input.output {
        OutputArg::Parent => TrackOutput::Parent,
        OutputArg::Master => TrackOutput::Master,
        OutputArg::Bus { track } => {
            if let Some(problem) = s.project.route_error(input.track, track) {
                return Err(error(problem));
            }
            TrackOutput::Track(track)
        }
        OutputArg::Hardware { channel } => {
            check_output_pair(s, channel)?;
            TrackOutput::Hardware { channel }
        }
    };
    s.project.track_mut(input.track).expect("checked").output = value;
    s.mark_dirty();
    Ok(value)
}

fn check_output_pair(s: &Session, channel: u16) -> ApiResult<()> {
    let outputs = s.output_channels();
    if !channel.is_multiple_of(2) {
        return Err(error(format!(
            "output pairs start on even channels (0, 2, ...); {channel} is odd"
        )));
    }
    if usize::from(channel) + 1 >= outputs {
        return Err(error(format!(
            "the audio device has {outputs} outputs; pairs start at 0, 2, ... up to {}",
            outputs.saturating_sub(2)
        )));
    }
    Ok(())
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "lowercase")]
enum MonitorArg {
    Auto,
    On,
    Off,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMonitor {
    track: TrackId,
    monitor: MonitorArg,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveTrack {
    track: TrackId,
    direction: Direction,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveTrackTo {
    track: TrackId,
    /// Just above this track, in its folder (or the top level).
    #[serde(default)]
    before: Option<TrackId>,
    /// Just below this track and its sub-tracks, in its folder (or the top level).
    #[serde(default)]
    after: Option<TrackId>,
    /// Into this folder.
    #[serde(default)]
    folder: Option<TrackId>,
    /// With `folder`: its place among the folder's sub-tracks (0 = first; default: last).
    #[serde(default)]
    position: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParent {
    track: TrackId,
    /// Folder to move into; omit for the top level.
    #[serde(default)]
    folder: Option<TrackId>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTrackMixer {
    track: TrackId,
    /// Volume in dB, -60 to +12.
    #[serde(default)]
    gain_db: Option<f32>,
    /// Balance, -1 (left) to 1 (right).
    #[serde(default)]
    pan: Option<f32>,
    #[serde(default)]
    mute: Option<bool>,
    #[serde(default)]
    solo: Option<bool>,
    /// Record arm (for instrument tracks: play from the MIDI keyboard).
    #[serde(default)]
    arm: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
struct MixerView {
    track: TrackId,
    mixer: Mixer,
    arm: bool,
}

fn check_mix(gain_db: Option<f32>, pan: Option<f32>) -> ApiResult<()> {
    if gain_db.is_some_and(|g| !g.is_finite()) {
        return Err(error("gain_db must be a number"));
    }
    if let Some(pan) = pan {
        if !(-1.0..=1.0).contains(&pan) {
            return Err(error(format!("pan {pan} is outside -1..1")));
        }
    }
    Ok(())
}

fn set_track_mixer(host: &mut dyn Host, input: SetTrackMixer) -> ApiResult<MixerView> {
    let s = host.session();
    check_track(s, input.track)?;
    check_mix(input.gain_db, input.pan)?;
    let track = s.project.track_mut(input.track).expect("checked above");
    apply_mix(&mut track.mixer, input.gain_db, input.pan, input.mute);
    if let Some(solo) = input.solo {
        track.mixer.solo = solo;
    }
    if let Some(arm) = input.arm {
        track.arm = arm;
    }
    Ok(MixerView {
        track: track.id,
        mixer: track.mixer,
        arm: track.arm,
    })
}

fn apply_mix(mixer: &mut Mixer, gain_db: Option<f32>, pan: Option<f32>, mute: Option<bool>) {
    // Clamped as a value typed into a readout is (`Session::set_mix_level`).
    if let Some(gain) = gain_db {
        mixer.set_gain_db(gain);
    }
    if let Some(pan) = pan {
        mixer.set_pan(pan);
    }
    if let Some(mute) = mute {
        mixer.mute = mute;
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMaster {
    #[serde(default)]
    gain_db: Option<f32>,
    #[serde(default)]
    pan: Option<f32>,
    #[serde(default)]
    mute: Option<bool>,
    /// First channel (0-based, even) of the hardware output pair.
    #[serde(default)]
    output_channel: Option<u16>,
    /// The master's row height in the arrangement, in points (32 to 400); 0 for the default.
    #[serde(default)]
    height: Option<f32>,
    /// Show (true) or hide the master's automation lanes under its row.
    #[serde(default)]
    show_automation: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
struct MasterView {
    mixer: Mixer,
    output_channel: u16,
    height: f32,
    show_automation: bool,
}

/// A row height as `set_track` and `set_master` take it: 0 for the default, else within the
/// limits.
fn check_height(height: f32) -> ApiResult<Option<f32>> {
    if height != 0.0 && !(MIN_TRACK_HEIGHT..=MAX_TRACK_HEIGHT).contains(&height) {
        return Err(error(format!(
            "height {height} is outside {MIN_TRACK_HEIGHT}..{MAX_TRACK_HEIGHT}"
        )));
    }
    Ok((height != 0.0).then_some(height))
}

fn set_master(host: &mut dyn Host, input: SetMaster) -> ApiResult<MasterView> {
    let s = host.session();
    check_mix(input.gain_db, input.pan)?;
    let height = input.height.map(check_height).transpose()?;
    if let Some(channel) = input.output_channel {
        check_output_pair(s, channel)?;
        s.project.master.output_channel = channel;
        s.mark_dirty();
    }
    let master = &mut s.project.master;
    apply_mix(&mut master.mixer, input.gain_db, input.pan, input.mute);
    if let Some(height) = height {
        master.height = height;
    }
    if let Some(show) = input.show_automation {
        master.show_automation = show;
    }
    Ok(MasterView {
        mixer: master.mixer,
        output_channel: master.output_channel,
        height: master.height(),
        show_automation: master.show_automation,
    })
}
