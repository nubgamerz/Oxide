//! The Waveform view's Diagnostics (dev plan 11.5b): scan an open audio file for clicks or
//! clipped runs, list them, repair the chosen ones. Positions are the file's own samples.

use super::error;
use crate::api::{Registry, Safety};
use crate::diagnostics::{Problem, ProblemKind, Settings};
use crate::session::{DocumentId, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "find_audio_problems",
        "Scan an open audio file (`document`) for problems, \
         over `start`..`end` (the file's own samples; the whole file when both are left \
         out). `kind` \"click-repair\" finds clicks and pops with the Click Remover's \
         detector: `threshold` (0..100 %, default 29.3; higher finds fewer) and `complexity` \
         (0..100 %, default 15.2; higher takes longer clicks, up to 2 x (0.99 x complexity + \
         1.5) + 8 samples). `kind` \"clip-repair\" finds clipped runs with Clip Repair's scan: \
         each channel's highest and lowest sample in the range are its clip levels, and a run \
         is at least `min_clip_size` samples (3..20, default 3) within `tolerance` (0..50 %, \
         default 10) of one; nothing under -24 dBFS counts. Each problem: `id`, `channel` \
         (from 0), `start` and `length` (samples) and `level_db` (dBFS: a click's size, the \
         largest change its repair makes; a clipped run's highest sample). The list stays on \
         the file, replacing the last one, until the file's audio changes; ids are never \
         reused while it is open. Changes no audio. Returns the list in order of time.",
        Safety::Edit,
        find_audio_problems,
    );
    r.add(
        "get_audio_problems",
        "The problems the last find_audio_problems on an open audio file (`document`) found \
         that are still to repair, as it returned them; `stale` is true (and the list empty) \
         when there was no scan or the file's audio has changed since (an edit, undo or redo).",
        Safety::Read,
        |host, input: DocumentArg| Ok(listing(host.session(), input.document)),
    );
    r.add(
        "repair_audio_problems",
        "Repair problems find_audio_problems found in an open audio file (`document`): `ids` \
         (from its list), or every one with `all` true. Each is rebuilt as the effect itself \
         rebuilds it (the Click Remover's least-squares AR interpolation, the \
         Clip Repair's cubic and AR fill), with the samples the scan made for it; nothing else \
         changes. No gain is applied, so a rebuilt peak may rise past the old clip level. One \
         step of the file's own history (\"Repair 12 clicks\"; undo_audio_edit takes it back); \
         the repaired ones leave the list and the rest keep their ids. Stops the file's \
         playback. Returns how many were repaired and the list left.",
        Safety::Edit,
        |host, input: Repair| {
            let s = host.session();
            let ids = match (input.ids, input.all) {
                (Some(ids), false) => Some(ids),
                (None, true) => None,
                _ => return Err(error("give `ids`, or `all` true (not both)")),
            };
            let repaired = s.repair_audio_problems(input.document, ids.as_deref())?;
            Ok(Repaired {
                repaired,
                left: listing(s, input.document),
            })
        },
    );
}

fn find_audio_problems(
    host: &mut dyn crate::api::Host,
    input: Find,
) -> crate::api::ApiResult<Listing> {
    let s = host.session();
    let defaults = Settings::default();
    let settings = Settings {
        threshold: input.threshold.unwrap_or(defaults.threshold),
        complexity: input.complexity.unwrap_or(defaults.complexity),
        tolerance: input.tolerance.unwrap_or(defaults.tolerance),
        min_clip_size: input.min_clip_size.unwrap_or(defaults.min_clip_size),
    };
    let range = match (input.start, input.end) {
        (Some(a), Some(b)) => Some((a, b)),
        (None, None) => None,
        _ => return Err(error("give both `start` and `end`, or neither")),
    };
    s.find_audio_problems(input.document, input.kind, &settings, range)?;
    Ok(listing(s, input.document))
}

fn listing(s: &Session, document: DocumentId) -> Listing {
    match s.audio_problems(document) {
        Some((kind, problems)) => Listing {
            kind: Some(kind),
            count: problems.len(),
            problems,
            stale: false,
        },
        None => Listing {
            kind: None,
            count: 0,
            problems: Vec::new(),
            stale: true,
        },
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Find {
    document: DocumentId,
    /// "click-repair" (clicks) or "clip-repair" (clipped runs); the older "declicker" and
    /// "declipper" are read as these.
    kind: ProblemKind,
    /// Click Repair's Threshold, 0..100 % (default 29.3).
    #[serde(default)]
    threshold: Option<f64>,
    /// Click Repair's Complexity, 0..100 % (default 15.2).
    #[serde(default)]
    complexity: Option<f64>,
    /// Clip Repair's Tolerance, 0..50 % (default 10).
    #[serde(default)]
    tolerance: Option<f64>,
    /// Clip Repair's Min. Clip Size, 3..20 samples (default 3).
    #[serde(default)]
    min_clip_size: Option<f64>,
    /// The range's start, the file's own samples.
    #[serde(default)]
    start: Option<u64>,
    /// The range's end.
    #[serde(default)]
    end: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocumentArg {
    document: DocumentId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Repair {
    document: DocumentId,
    /// The problems to repair, by id.
    #[serde(default)]
    ids: Option<Vec<u32>>,
    /// Repair every problem in the list.
    #[serde(default)]
    all: bool,
}

#[derive(Serialize, JsonSchema)]
struct Listing {
    kind: Option<ProblemKind>,
    count: usize,
    problems: Vec<Problem>,
    stale: bool,
}

#[derive(Serialize, JsonSchema)]
struct Repaired {
    repaired: usize,
    left: Listing,
}
