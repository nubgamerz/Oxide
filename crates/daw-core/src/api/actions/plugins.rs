//! Plug-ins: what is installed, insert chains, editors and parameters.

use super::{check_insert, check_track, error, Direction};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::plugins;
use crate::project::{InsertId, TrackId};
use crate::session::{InsertTarget, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "list_plugins",
        "List the built-in effects (vendor \"Built in\": Oxide's own effects, path \
         native:<id>; an id from before an effect was renamed still names it) and the installed \
         VST3 plug-ins (name, vendor folder, path). `filter` matches name or vendor, case-insensitively.",
        Safety::Read,
        list_plugins,
    );
    r.add(
        "rescan_plugins",
        "Look for newly installed VST3 plug-ins. Returns how many are installed.",
        Safety::Edit,
        |host, _: super::NoArgs| {
            host.session().rescan_plugins();
            Ok(Count {
                count: host.session().installed_plugins().len(),
            })
        },
    );
    r.add(
        "add_insert",
        "Load a VST3 plug-in or a built-in effect (\"Amplify\") into a track's insert chain \
         (or the master's when `track` is omitted, or the Waveform view's effects rack with \
         `editor` true). `plugin` is a built-in effect's or an installed plug-in's name (exact, \
         or a unique part of it; see list_plugins; a built-in effect's id or name from before \
         it was renamed still finds it, and the insert gets today's) or a path. `position` puts it at that place in the chain (0 is first; \
         past the end appends); without it instruments go first and effects last. Returns the \
         insert id and its latency in samples. Loading takes up to a few seconds.",
        Safety::Edit,
        add_insert,
    );
    r.add(
        "remove_insert",
        "Remove a plug-in from its insert chain, discarding its settings.",
        Safety::Destructive,
        |host, input: InsertArg| {
            check_insert(host.session(), input.insert)?;
            host.session().remove_insert(input.insert);
            Ok(Done { done: true })
        },
    );
    r.add(
        "set_insert",
        "Bypass or enable a plug-in (`bypass`). A bypassed plug-in passes audio through \
         unchanged but keeps its latency, so timing stays aligned.",
        Safety::Edit,
        |host, input: SetInsert| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let slot = s.insert_mut(input.insert).expect("checked");
            if let Some(bypass) = input.bypass {
                slot.bypass = bypass;
            }
            Ok(InsertState {
                insert: slot.id,
                name: slot.name.clone(),
                bypass: slot.bypass,
            })
        },
    );
    r.add(
        "move_insert",
        "Move a plug-in in its insert chain, where the order is the signal's: the first \
         processes first (a tuner before an amp hears the dry input). One place up (earlier) \
         or down (later) with `direction`, or to `position` (0 is first; past the end makes \
         it last). With `track` (or `master` true) it moves to that track's (or the \
         master's) chain instead, at `position` or last, as the same instance with its \
         settings and its parameters' automation lanes. Give `direction` or a place, not \
         both.",
        Safety::Edit,
        |host, input: MoveInsert| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let elsewhere = match (input.track, input.master) {
                (Some(_), true) => return Err(error("give `track` or `master`, not both")),
                (Some(id), false) => {
                    check_track(s, id)?;
                    Some(InsertTarget::Track(id))
                }
                (None, true) => Some(InsertTarget::Master),
                (None, false) => None,
            };
            match (input.direction, input.position, elsewhere) {
                (Some(direction), None, None) => {
                    s.move_insert(input.insert, matches!(direction, Direction::Up))
                }
                (None, Some(position), None) => {
                    s.move_insert_to(input.insert, position);
                }
                (None, position, Some(target)) => {
                    s.move_insert_into(input.insert, target, position.unwrap_or(usize::MAX))?;
                }
                _ => {
                    return Err(error(
                        "give either `direction`, or a place: `position`, `track` or `master`",
                    ))
                }
            }
            Ok(Done { done: true })
        },
    );
    r.add(
        "copy_insert",
        "Copy a plug-in: a new instance of it with the same settings (its state now) and \
         bypass, without its automation, into `track`'s chain (or the master's with `master` \
         true; without either, its own chain, right after it) at `position` (0 is first; \
         default: last). Returns the new insert's id.",
        Safety::Edit,
        |host, input: CopyInsert| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let own = s.insert_owner(input.insert);
            let target = match (input.track, input.master) {
                (Some(_), true) => return Err(error("give `track` or `master`, not both")),
                (Some(id), false) => {
                    check_track(s, id)?;
                    InsertTarget::Track(id)
                }
                (None, true) => InsertTarget::Master,
                (None, false) => own.ok_or_else(|| {
                    error("the Waveform view's rack: give a `track` (or `master`) to copy to")
                })?,
            };
            let after = s
                .insert_slots(target)
                .iter()
                .position(|slot| slot.id == input.insert)
                .map(|i| i + 1);
            let position = input.position.or(after).unwrap_or(usize::MAX);
            let insert = s.copy_insert(input.insert, target, position)?;
            Ok(Copied { insert })
        },
    );
    r.add(
        "copy_plugin",
        "Copy a plug-in insert to the plug-in clipboard, as its menu's Copy does: which plug-in, \
         its name, bypass and its settings as they are now. paste_plugin makes new instances \
         from it, also after the original is removed. Replaces what was copied before; not an \
         edit of the project. Returns what is on the clipboard.",
        Safety::Edit,
        |host, input: InsertArg| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let clip = s.copy_plugin(input.insert)?;
            Ok(ClipView {
                name: clip.name.clone(),
                path: clip.path.clone(),
                bypass: clip.bypass,
            })
        },
    );
    r.add(
        "paste_plugin",
        "Paste the copied plug-in (copy_plugin): a new instance with the settings it had when \
         copied, into `track`'s chain (or the master's with `master` true) at `position` (0 is \
         first; default: last). Paste as often as wanted. Returns the new insert's id.",
        Safety::Edit,
        |host, input: PastePlugin| {
            let s = host.session();
            let target = match (input.track, input.master) {
                (Some(_), true) => return Err(error("give `track` or `master`, not both")),
                (Some(id), false) => {
                    check_track(s, id)?;
                    InsertTarget::Track(id)
                }
                (None, true) => InsertTarget::Master,
                (None, false) => return Err(error("give a `track` (or `master`) to paste to")),
            };
            let insert = s.paste_plugin(target, input.position.unwrap_or(usize::MAX))?;
            s.mark_dirty();
            Ok(Copied { insert })
        },
    );
    r.add(
        "set_inserts_bypassed",
        "Bypass (`bypass` true) or enable every plug-in in a track's insert chain at once, or \
         the master's when `track` is omitted (the Waveform view's rack with `editor` true), as the fx rack's power button does. Bypassed \
         plug-ins keep their latency. Returns how many changed.",
        Safety::Edit,
        |host, input: SetInsertsBypassed| {
            let s = host.session();
            let target = target(s, input.track, input.editor)?;
            let changed = s.set_inserts_bypassed(target, input.bypass)?;
            Ok(Changed { changed })
        },
    );
    r.add(
        "set_plugin_editor",
        "Open or close a plug-in's own editor window (`open`). The window is a separate \
         native window; opening it briefly silences that track.",
        Safety::Edit,
        |host, input: SetEditor| {
            let s = host.session();
            check_insert(s, input.insert)?;
            s.set_editor_open(input.insert, input.open)?;
            let open = s.insert_info(input.insert).is_some_and(|(_, _, o)| o);
            Ok(EditorState { open })
        },
    );
    r.add(
        "list_plugin_parameters",
        "List a plug-in's parameters: id, name, normalized value (0-1), the value as the \
         plug-in displays it, unit, default and step count (0 = continuous, 1 = on/off, n = \
         n+1 choices). `filter` matches names case-insensitively; results are paged by \
         `offset`/`limit` (default 100) because some plug-ins have thousands. A built-in effect \
         that counts what it does adds `counters` (the Click & Pop Repair: clicks corrected, \
         big pops corrected, clicks rejected, since playback last started). Reading never \
         interrupts the plug-in's audio, so it is safe during playback.",
        Safety::Read,
        list_parameters,
    );
    r.add(
        "set_plugin_parameter",
        "Set a plug-in parameter to a normalized `value` from 0 to 1. `parameter` is its id \
         or its name (exact, or a unique part; see list_plugin_parameters). Returns how the \
         plug-in displays that value, e.g. \"-6.0 dB\", so the mapping can be checked. \
         list_plugin_parameters shows the new value at once; the audio takes it at the \
         plug-in's next block (a few milliseconds while audio runs), without interrupting it.",
        Safety::Edit,
        set_parameter,
    );
    r.add(
        "set_impulse_file",
        "Give a Convolution Reverb insert (`insert`) its impulse response: the audio file at \
         `path` (an absolute path; WAV, AIFF or FLAC, up to 30 s). The file is checked, copied \
         into the project's Imported folder (a file already in the project is used in place) \
         and saved with the effect by its project-relative path. A stereo file plays left on \
         left and right on right; any other channel count is averaged to one impulse on both \
         sides; it is resampled to the session's rate, then shaped by Room Size, Width, \
         Damping LF/HF and Pre-Delay and normalised (k = 0.5/mean Σ|h|). It is prepared off \
         the audio thread (waited for here, up to 10 s; exports always wait for it) and the \
         impulse before it plays until it is in. Without an impulse the effect passes its \
         input unchanged. Returns the impulse as get_impulse_file does.",
        Safety::Edit,
        |host, input: SetImpulseFile| {
            let s = host.session();
            check_insert(s, input.insert)?;
            s.set_impulse_file(input.insert, &input.path)?;
            // Usually milliseconds; a long file at a high rate can take a second or two.
            let _ = s.wait_for_impulses(std::time::Duration::from_secs(10));
            impulse_view(s, input.insert)
        },
    );
    r.add(
        "get_impulse_file",
        "The impulse response a Convolution Reverb or Space Reverb insert (`insert`) plays: \
         `file` (as saved, relative to the project's folder; null when none is chosen, and \
         always for the Space Reverb, which makes its impulse from its settings), `preparing` \
         (true while it or a settings change is being prepared; exports wait for it), `error` \
         (why it could not be read, if so), and once prepared `seconds` (its length as played: \
         with Room Size and Pre-Delay; the Space Reverb's as trimmed), `file_channels`, \
         `file_rate` (Hz) and `file_seconds` (the file as it is; for the Space Reverb its \
         impulse channels, 2 or 4 in true stereo, the session's rate and 1.45 x Decay).",
        Safety::Read,
        |host, input: InsertArg| {
            let s = host.session();
            check_insert(s, input.insert)?;
            impulse_view(s, input.insert)
        },
    );
    r.add(
        "list_plugin_outputs",
        "A plug-in insert's audio outputs, as it names them: `bus` (0 is the main pair, which \
         plays on the plug-in's own track; 1 and up are extra outputs, like a drum sampler's \
         kick and snare or a sampler's outputs 2..32), `name`, `channels`, `active` (on in the \
         plug-in: a track listens to it), and `track`, the track listening to it if any. \
         Native effects have none.",
        Safety::Read,
        |host, input: InsertArg| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let outputs = s
                .plugin_outputs(input.insert)?
                .into_iter()
                .map(|o| OutputView {
                    bus: o.bus,
                    name: o.name,
                    channels: o.channels,
                    active: o.active,
                    track: o.track,
                })
                .collect();
            Ok(Outputs { outputs })
        },
    );
    r.add(
        "create_multi_out_rack",
        "Route a multi-out plug-in's extra outputs to tracks of their own (a multi-out rack \
         in a submix): a folder right after the plug-in's track holding one audio track per \
         output in `outputs` (bus numbers from list_plugin_outputs, 1 and up; default every \
         extra output without a track yet), each named after its output and hearing it at \
         unity (0 dB), with its own fader, pan, inserts and sends. The main pair stays on the \
         plug-in's track; the plug-in turns the outputs on. `insert` must be on an audio or \
         instrument track. Returns the folder and the new tracks.",
        Safety::Edit,
        |host, input: CreateRack| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let (folder, tracks) =
                s.create_multi_out_rack(input.insert, input.outputs.as_deref())?;
            s.mark_dirty();
            Ok(Rack { folder, tracks })
        },
    );
    r.add(
        "remove_multi_out_rack",
        "Remove the tracks listening to a plug-in insert's extra outputs (and the folder they \
         leave empty); the plug-in turns those outputs off. Their clips and plug-ins go too. \
         Returns the tracks removed.",
        Safety::Destructive,
        |host, input: InsertArg| {
            let s = host.session();
            check_insert(s, input.insert)?;
            let removed = s.remove_multi_out_rack(input.insert)?;
            s.mark_dirty();
            Ok(Removed { removed })
        },
    );
}

#[derive(Serialize, JsonSchema)]
struct Outputs {
    outputs: Vec<OutputView>,
}

#[derive(Serialize, JsonSchema)]
struct OutputView {
    bus: usize,
    name: String,
    channels: usize,
    active: bool,
    track: Option<TrackId>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateRack {
    insert: InsertId,
    /// The extra outputs to give tracks, by bus number (1 and up).
    #[serde(default)]
    outputs: Option<Vec<usize>>,
}

#[derive(Serialize, JsonSchema)]
struct Rack {
    folder: TrackId,
    tracks: Vec<TrackId>,
}

#[derive(Serialize, JsonSchema)]
struct Removed {
    removed: Vec<TrackId>,
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}

#[derive(Serialize, JsonSchema)]
struct Count {
    count: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetImpulseFile {
    insert: InsertId,
    /// Absolute path of a WAV, AIFF or FLAC file.
    path: PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct ImpulseView {
    insert: InsertId,
    effect: String,
    file: Option<String>,
    preparing: bool,
    error: Option<String>,
    seconds: Option<f64>,
    file_channels: Option<usize>,
    file_rate: Option<u32>,
    file_seconds: Option<f64>,
}

/// What a file-playing built-in effect (the Convolution Reverb) has as its impulse.
fn impulse_view(s: &Session, insert: InsertId) -> ApiResult<ImpulseView> {
    let native = s
        .native(insert)
        .filter(|n| n.kind.reads.is_some())
        .ok_or_else(|| error(format!("insert {insert} plays no impulse")))?;
    let overview = native.file_overview();
    Ok(ImpulseView {
        insert,
        effect: native.kind.name.to_string(),
        file: native
            .file()
            .map(|f| f.to_string_lossy().replace('\\', "/")),
        preparing: native.file_pending(),
        error: native.file_error().map(str::to_string),
        seconds: overview.map(|o| o.samples as f64 / o.rate),
        file_channels: overview.map(|o| o.file_channels),
        file_rate: overview.map(|o| o.file_rate),
        file_seconds: overview.map(|o| o.file_frames as f64 / f64::from(o.file_rate)),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct InsertArg {
    insert: InsertId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInsert {
    insert: InsertId,
    #[serde(default)]
    bypass: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
struct InsertState {
    insert: InsertId,
    name: String,
    bypass: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveInsert {
    insert: InsertId,
    #[serde(default)]
    direction: Option<Direction>,
    /// The place in the chain, from 0.
    #[serde(default)]
    position: Option<usize>,
    /// Move it to this track's chain.
    #[serde(default)]
    track: Option<TrackId>,
    /// Move it to the master's chain.
    #[serde(default)]
    master: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CopyInsert {
    insert: InsertId,
    /// Copy it to this track's chain.
    #[serde(default)]
    track: Option<TrackId>,
    /// Copy it to the master's chain.
    #[serde(default)]
    master: bool,
    /// The place in the chain, from 0.
    #[serde(default)]
    position: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PastePlugin {
    /// Paste into this track's chain.
    #[serde(default)]
    track: Option<TrackId>,
    /// Paste into the master's chain.
    #[serde(default)]
    master: bool,
    /// The place in the chain, from 0.
    #[serde(default)]
    position: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct ClipView {
    name: String,
    path: PathBuf,
    bypass: bool,
}

#[derive(Serialize, JsonSchema)]
struct Copied {
    insert: InsertId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetInsertsBypassed {
    /// Track id; omit for the master bus.
    #[serde(default)]
    track: Option<TrackId>,
    /// The Waveform view's effects rack instead.
    #[serde(default)]
    editor: bool,
    bypass: bool,
}

#[derive(Serialize, JsonSchema)]
struct Changed {
    changed: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetEditor {
    insert: InsertId,
    open: bool,
}

#[derive(Serialize, JsonSchema)]
struct EditorState {
    open: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListPlugins {
    /// Case-insensitive text to match in the name or vendor.
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct PluginView {
    name: String,
    vendor: String,
    path: PathBuf,
}

fn list_plugins(host: &mut dyn Host, input: ListPlugins) -> ApiResult<Vec<PluginView>> {
    let needle = input.filter.unwrap_or_default().to_lowercase();
    let native: Vec<PathBuf> = crate::native::KINDS
        .iter()
        .map(|k| crate::native::path_of(k))
        .collect();
    Ok(native
        .iter()
        .chain(host.session().installed_plugins())
        .map(|path| PluginView {
            name: plugins::display_name(path),
            vendor: plugins::vendor(path),
            path: path.clone(),
        })
        .filter(|p| {
            needle.is_empty()
                || p.name.to_lowercase().contains(&needle)
                || p.vendor.to_lowercase().contains(&needle)
        })
        .collect())
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddInsert {
    /// Track id; omit for the master bus.
    #[serde(default)]
    track: Option<TrackId>,
    /// The Waveform view's effects rack instead of a track or the master.
    #[serde(default)]
    editor: bool,
    /// A built-in effect's or installed plug-in's name (or a unique part of it), or a path.
    plugin: String,
    /// The place in the chain, from 0; omit for instruments first, effects last.
    #[serde(default)]
    position: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct AddedInsert {
    insert: InsertId,
    name: String,
    latency: usize,
}

/// The chain an action names: a track's, the Waveform view's rack (`editor`), or the master's.
fn target(s: &Session, track: Option<TrackId>, editor: bool) -> ApiResult<InsertTarget> {
    match (track, editor) {
        (Some(_), true) => Err(error("give `track` or `editor`, not both")),
        (Some(id), false) => {
            check_track(s, id)?;
            Ok(InsertTarget::Track(id))
        }
        (None, true) => Ok(InsertTarget::Editor),
        (None, false) => Ok(InsertTarget::Master),
    }
}

fn add_insert(host: &mut dyn Host, input: AddInsert) -> ApiResult<AddedInsert> {
    let s = host.session();
    let target = target(s, input.track, input.editor)?;
    let path = plugins::resolve(&input.plugin, s.installed_plugins())?;
    let insert = s.add_insert_at(target, &path, input.position)?;
    let latency = s.insert_info(insert).map_or(0, |(_, latency, _)| latency);
    Ok(AddedInsert {
        insert,
        name: plugins::display_name(&path),
        latency,
    })
}

// ------------------------------------------------------------------- parameters

/// An insert's parameters, refusing one that failed to load.
pub(super) fn parameters(
    s: &Session,
    insert: InsertId,
) -> ApiResult<Vec<vst3_host::parameters::Parameter>> {
    check_insert(s, insert)?;
    if !s.is_loaded(insert) {
        return Err(error(format!(
            "insert {insert} failed to load, so it has no parameters"
        )));
    }
    s.parameters(insert).map_err(|e| error(format!("{e:#}")))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListParameters {
    insert: InsertId,
    /// Case-insensitive text to match in parameter names.
    #[serde(default)]
    filter: Option<String>,
    #[serde(default)]
    offset: Option<usize>,
    /// At most this many (default 100).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct ParameterView {
    id: u32,
    name: String,
    value: f64,
    display: String,
    unit: String,
    default: f64,
    steps: i32,
    automatable: bool,
    read_only: bool,
}

#[derive(Serialize, JsonSchema)]
struct ParameterPage {
    /// Parameters matching the filter, before paging.
    total: usize,
    parameters: Vec<ParameterView>,
    /// A built-in effect's counters, if it keeps any (the Click & Pop Repair's clicks
    /// corrected, big pops corrected and clicks rejected since playback started).
    #[serde(skip_serializing_if = "Option::is_none")]
    counters: Option<Vec<u64>>,
}

fn list_parameters(host: &mut dyn Host, input: ListParameters) -> ApiResult<ParameterPage> {
    let s = host.session();
    let needle = input.filter.unwrap_or_default().to_lowercase();
    let matching: Vec<_> = parameters(s, input.insert)?
        .into_iter()
        .filter(|p| needle.is_empty() || p.name.to_lowercase().contains(&needle))
        .collect();
    let total = matching.len();
    let page: Vec<_> = matching
        .into_iter()
        .skip(input.offset.unwrap_or(0))
        .take(input.limit.unwrap_or(100).min(1000))
        .collect();
    let values: Vec<(u32, f64)> = page.iter().map(|p| (p.id, p.value)).collect();
    let displays = s
        .parameter_displays(input.insert, &values)
        .map_err(|e| error(format!("{e:#}")))?;
    let parameters = page
        .into_iter()
        .zip(displays)
        .map(|(p, display)| ParameterView {
            display,
            id: p.id,
            name: p.name,
            value: p.value,
            unit: p.unit,
            default: p.default,
            steps: p.step_count,
            automatable: p.can_automate,
            read_only: p.is_read_only,
        })
        .collect();
    let counters = s
        .native(input.insert)
        .map(|n| n.readouts())
        .filter(|r| !r.is_empty());
    Ok(ParameterPage {
        total,
        parameters,
        counters,
    })
}

/// A parameter id, or a name to look up.
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub(super) enum ParameterRef {
    Id(u32),
    Name(String),
}

/// The parameter `r` names: by id, by exact name, or by a part of the name only one has.
pub(super) fn find_parameter<'a>(
    params: &'a [vst3_host::parameters::Parameter],
    r: &ParameterRef,
) -> ApiResult<&'a vst3_host::parameters::Parameter> {
    let found = match r {
        ParameterRef::Id(id) => params.iter().find(|p| p.id == *id),
        ParameterRef::Name(name) => {
            let needle = name.to_lowercase();
            match params.iter().find(|p| p.name.to_lowercase() == needle) {
                Some(exact) => Some(exact),
                None => {
                    let partial: Vec<_> = params
                        .iter()
                        .filter(|p| p.name.to_lowercase().contains(&needle))
                        .collect();
                    match partial.as_slice() {
                        [one] => Some(*one),
                        [] => None,
                        many => {
                            let names: Vec<&str> =
                                many.iter().take(10).map(|p| p.name.as_str()).collect();
                            return Err(error(format!(
                                "'{name}' matches {} parameters: {}…",
                                many.len(),
                                names.join(", ")
                            )));
                        }
                    }
                }
            }
        }
    };
    found.ok_or_else(|| error("no such parameter (see list_plugin_parameters)"))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetParameter {
    insert: InsertId,
    parameter: ParameterRef,
    /// Normalized value, 0 to 1.
    value: f64,
}

#[derive(Serialize, JsonSchema)]
struct ParameterSet {
    id: u32,
    name: String,
    value: f64,
    display: String,
}

fn set_parameter(host: &mut dyn Host, input: SetParameter) -> ApiResult<ParameterSet> {
    if !(0.0..=1.0).contains(&input.value) {
        return Err(error(format!(
            "value {} is outside 0..1 (parameters are normalized)",
            input.value
        )));
    }
    let s = host.session();
    let params = parameters(s, input.insert)?;
    let param = find_parameter(&params, &input.parameter)?;
    if param.is_read_only {
        return Err(error(format!("'{}' is read-only", param.name)));
    }
    let (id, name) = (param.id, param.name.clone());
    // Some plug-ins only report the new value after their next audio block, so
    // describe the value that was set rather than reading it back.
    let display = s
        .set_parameter(input.insert, id, input.value)
        .map_err(|e| error(format!("setting '{name}': {e:#}")))?;
    Ok(ParameterSet {
        id,
        display,
        name,
        value: input.value,
    })
}
