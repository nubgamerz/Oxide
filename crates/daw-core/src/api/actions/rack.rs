//! Rack presets: the Waveform view's effects rack saved by name and loaded again (dev plan
//! 18.2). App-wide, in the app's RackPresets folder beside
//! the program, never in a project. The rack itself is kept between runs without any action.

use super::NoArgs;
use crate::api::{ActionOutput, ApiError, ApiResult, Registry, Safety};
use crate::project::InsertId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "list_rack_presets",
        "The saved presets of the Waveform view's effects rack, by name: `name` (what \
         load_rack_preset takes), `path` and `modified` (seconds since 1970).",
        Safety::Read,
        |host, _: NoArgs| {
            let presets = host
                .session()
                .rack_presets()
                .into_iter()
                .map(|p| PresetView {
                    modified: p
                        .modified
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_secs()),
                    name: p.name,
                    path: p.path,
                })
                .collect();
            Ok(PresetList { presets })
        },
    );
    r.add(
        "save_rack_preset",
        "Save the Waveform view's effects rack as it is now as the preset `name`: its slots in \
         order, each plug-in (VST3 or built-in), its bypass and its current settings (a \
         built-in effect's noise print too). Kept in the app's RackPresets folder, for every \
         project. The name is the file's: refused if it is empty or holds < > : \" / \\ | ? * \
         (or ends with a dot), if the rack is empty, or if a preset of that name exists, \
         unless `replace` (default false), which overwrites it. Returns its path.",
        Safety::Destructive,
        |host, input: SavePreset| {
            let path = host
                .session()
                .save_rack_preset(&input.name, input.replace)?;
            Ok(Saved { path })
        },
    );
    r.add_deferred(
        "load_rack_preset",
        "Replace the Waveform view's effects rack with the preset `name` (from \
         list_rack_presets): the plug-ins in the rack now are removed with their settings, and \
         the preset's slots come in with theirs. Plug-ins load one at a time (the app shows its \
         busy bar); the call answers once they are in, with the new slots' insert ids in order \
         (get_audio_editor shows them; a plug-in that failed to load keeps its slot with \
         `loaded` false and why). Refused while an Apply renders.",
        Safety::Destructive,
        |host, input: PresetName, reply| {
            let ids = match host.session().load_rack_preset(&input.name) {
                Ok(ids) => ids,
                Err(e) => return reply(Err(e.into())),
            };
            host.when_opened(
                reply,
                Box::new(move |s| {
                    let slots = ids
                        .iter()
                        .filter_map(|&id| {
                            let slot = s.find_insert(id)?;
                            Some(LoadedSlot {
                                insert: id,
                                name: slot.name.clone(),
                                loaded: s.is_loaded(id),
                                error: s.load_failure(id).map(str::to_string),
                            })
                        })
                        .collect();
                    output(Loaded { slots })
                }),
            );
        },
    );
    r.returns::<Loaded>();
    r.add_deferred(
        "set_rack_sync",
        "Sync the Waveform view's effects rack with the track of the clip `document` was \
         opened from (`sync` true), or stop (false). On, the rack set aside (kept, and still \
         what the app keeps between runs) and the rack becomes copies of the track's inserts \
         with their settings now, so the preview sounds as the clip does in the Multitrack; \
         from then on adding, removing, moving or bypassing an effect on either side does the \
         same on the other (an edit of the rack mirrored onto the track is one undo step; \
         undoing a track's edit brings the rack along), a parameter set on either side \
         (set_plugin_parameter, an editor's knob) is set on the other, and a change no \
         parameter shows (a preset loaded inside a plug-in, a noise print) copies the whole \
         settings across. The track's automation lanes are not copied: the rack holds the \
         settings set by hand. Off, the rack set aside comes back. Plug-ins load one at a time \
         (the busy bar); the call answers once they are in, with the rack's slots and the \
         sync (as get_audio_editor's `sync`). Refused for a file not opened from a clip \
         (open_audio_editor with `clip`), while an Apply renders, and while other plug-ins \
         load; closing the file or removing the track turns it off. load_rack_preset is \
         refused while it is on.",
        Safety::Edit,
        |host, input: SetRackSync, reply| {
            let ids = match host.session().set_rack_sync(input.document, input.sync) {
                Ok(ids) => ids,
                Err(e) => return reply(Err(e.into())),
            };
            host.when_opened(
                reply,
                Box::new(move |s| {
                    let slots = ids
                        .iter()
                        .filter_map(|&id| {
                            let slot = s.find_insert(id)?;
                            Some(LoadedSlot {
                                insert: id,
                                name: slot.name.clone(),
                                loaded: s.is_loaded(id),
                                error: s.load_failure(id).map(str::to_string),
                            })
                        })
                        .collect();
                    output(Synced {
                        slots,
                        sync: super::editor::sync_view(s),
                    })
                }),
            );
        },
    );
    r.returns::<Synced>();
    r.add(
        "delete_rack_preset",
        "Delete the rack preset `name` for good (the rack as it is now is untouched).",
        Safety::Destructive,
        |host, input: PresetName| {
            host.session().delete_rack_preset(&input.name)?;
            Ok(Done { done: true })
        },
    );
}

fn output(answer: impl Serialize) -> ApiResult<ActionOutput> {
    serde_json::to_value(answer)
        .map(ActionOutput::Json)
        .map_err(|e| ApiError(format!("encoding the result: {e}")))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SavePreset {
    /// The preset's name: its file is named after it.
    name: String,
    /// Overwrite a preset of that name.
    #[serde(default)]
    replace: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PresetName {
    name: String,
}

#[derive(Serialize, JsonSchema)]
struct PresetList {
    presets: Vec<PresetView>,
}

#[derive(Serialize, JsonSchema)]
struct PresetView {
    name: String,
    path: PathBuf,
    modified: u64,
}

#[derive(Serialize, JsonSchema)]
struct Saved {
    path: PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct Loaded {
    slots: Vec<LoadedSlot>,
}

#[derive(Serialize, JsonSchema)]
struct LoadedSlot {
    insert: InsertId,
    name: String,
    loaded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRackSync {
    /// An open audio file (get_audio_editor), opened from a clip.
    document: crate::session::DocumentId,
    /// Sync with the clip's track (true) or stop (false).
    sync: bool,
}

#[derive(Serialize, JsonSchema)]
struct Synced {
    slots: Vec<LoadedSlot>,
    sync: Option<super::editor::SyncView>,
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}
