//! The manual (dev plan 27.3): search it for an answer, or show a page of it to the user.

use super::error;
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::manual::{self, Index};
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

/// The most passages `search_manual` returns.
const MOST: usize = 20;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "search_manual",
        "Search Oxide's manual (the HTML user guide installed beside the program) for a \
         question or a few words, to answer \"how do I…\" from it: the best passages first, \
         each a page (`page`: its id, for open_manual; `title`) or a section of one (`anchor` \
         and `heading`), with a `snippet` around the first word matched and the passage's \
         whole `text`, and its `score` (words of the question matched times 1000, plus where: \
         a page's title counts most, then a heading, then the text). Common words (\"how\", \
         \"the\") are left out; words match by their start (\"proc\" finds \"processing\"). \
         `limit`: how many (default 5, at most 20). Fails when the manual isn't there (a \
         build without it), saying where it was looked for.",
        Safety::Read,
        |host, input: SearchInput| {
            let (dir, index) = read(host)?;
            let limit = input.limit.unwrap_or(5).clamp(1, MOST);
            let results = manual::search(&index, &input.question, limit);
            Ok(json!({
                "question": input.question,
                "manual": dir,
                "version": index.version,
                "results": results,
            }))
        },
    );
    r.add(
        "open_manual",
        "Show the user a page of Oxide's manual in their default web browser (needs the app \
         window). `page`: a page's id as search_manual gives it (`choppy-audio`; may carry \
         `#anchor`), with `anchor` for a section on it; or `topic`: a question or a few words, \
         opened at the best passage search_manual would give; neither: the manual's first \
         page. Returns the `page`, `anchor`, `title` and `heading` shown, the `file` (absolute) \
         and `opened` (false in the app's check runs, which only note the request). Changes \
         nothing in the project; not an undo step. Errors name a page or section the manual \
         doesn't have, and list those it does.",
        Safety::Edit,
        |host, input: OpenInput| {
            let (dir, index) = read(host)?;
            let target = match (&input.page, &input.topic) {
                (Some(_), Some(_)) => {
                    return Err(error("give `page` or `topic`, not both"));
                }
                (Some(page), None) => manual::check(&index, page, input.anchor.as_deref())?,
                (None, Some(topic)) => {
                    if input.anchor.is_some() {
                        return Err(error("`anchor` goes with `page`, not `topic`"));
                    }
                    manual::topic(&index, topic)?
                }
                (None, None) => manual::check(&index, manual::INDEX_PAGE, input.anchor.as_deref())?,
            };
            let shown = host.open_manual(&dir, &target)?;
            let mut answer = json!({
                "page": target.page,
                "anchor": target.anchor,
                "title": target.title,
                "heading": target.heading,
                "file": target.file(&dir),
            });
            if let (Some(answer), Value::Object(shown)) = (answer.as_object_mut(), shown) {
                answer.extend(shown);
            }
            Ok(answer)
        },
    );
}

/// The manual's folder and index, or why there are none.
fn read(host: &mut dyn Host) -> ApiResult<(PathBuf, Index)> {
    let Some(dir) = host.session().manual_dir.clone() else {
        return Err(error(
            "Oxide's manual isn't installed beside the program (a build without it: \
             `cargo run -p manual` builds it into out/manual)",
        ));
    };
    let index = manual::load(&dir)?;
    Ok((dir, index))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchInput {
    /// What to look for: a question ("how do I stop choppy audio?") or a few words
    /// ("processing block").
    question: String,
    /// How many passages, best first (default 5, at most 20).
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenInput {
    /// A page's id (search_manual's `page`, e.g. `choppy-audio`), optionally with
    /// `#anchor`.
    #[serde(default)]
    page: Option<String>,
    /// A section's anchor on `page` (search_manual's `anchor`).
    #[serde(default)]
    anchor: Option<String>,
    /// Words or a question: the manual opens at the best passage for it.
    #[serde(default)]
    topic: Option<String>,
}
