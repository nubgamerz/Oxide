//! Project templates: a project's setup saved to start new projects from (new_project's
//! `template`). App-wide, in the app's Templates folder, never in a project.

use super::NoArgs;
use crate::api::{Registry, Safety};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "save_template",
        "Save the open project's setup as a template called `name`, to start new projects from \
         (new_project's `template`): every track, folder, bus, route, send, mixer setting and \
         plug-in with its current settings (an amp sim's chosen amp too), automation lanes without \
         their points, the tempo map, time signature, loudness target and markers. No clips, \
         recordings or audio files. Kept in the app's Templates folder, for every project. \
         Refused if a template of that name exists, unless `replace` (default false), which \
         overwrites it. Returns its path.",
        Safety::Destructive,
        |host, input: SaveTemplate| {
            let path = host.session().save_template(&input.name, input.replace)?;
            Ok(Path { path })
        },
    );
    r.add(
        "list_templates",
        "The saved project templates by name: `name` (what new_project's `template` takes), \
         `path`, `modified` (seconds since 1970) and `size` in bytes.",
        Safety::Read,
        |host, _: NoArgs| {
            let secs = |t: std::time::SystemTime| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |d| d.as_secs())
            };
            let templates = host
                .session()
                .templates()
                .into_iter()
                .map(|t| TemplateView {
                    modified: secs(t.modified),
                    name: t.name,
                    path: t.path,
                    size: t.size,
                })
                .collect();
            Ok(TemplateList { templates })
        },
    );
    r.add(
        "rename_template",
        "Rename the template `name` to `new_name`; refused if one is called that already. \
         Returns its new path.",
        Safety::Edit,
        |host, input: RenameTemplate| {
            let path = host
                .session()
                .rename_template(&input.name, &input.new_name)?;
            Ok(Path { path })
        },
    );
    r.add(
        "delete_template",
        "Delete the template `name` for good (projects made from it keep their setup).",
        Safety::Destructive,
        |host, input: TemplateName| {
            host.session().delete_template(&input.name)?;
            Ok(Done { done: true })
        },
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveTemplate {
    /// The template's name: its file is named after it.
    name: String,
    /// Overwrite a template of that name.
    #[serde(default)]
    replace: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RenameTemplate {
    name: String,
    new_name: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TemplateName {
    name: String,
}

#[derive(Serialize, JsonSchema)]
struct Path {
    path: PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct TemplateList {
    templates: Vec<TemplateView>,
}

#[derive(Serialize, JsonSchema)]
struct TemplateView {
    name: String,
    path: PathBuf,
    modified: u64,
    size: u64,
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}
