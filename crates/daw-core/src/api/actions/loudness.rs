//! Loudness (dev plan 6.1): the master's live EBU R 128 meter, and measuring a file with the
//! same meter.

use super::error;
use super::NoArgs;
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::loudness::{self, Loudness};
use crate::project::{LOUDNESS_PRESETS, LOUDNESS_TARGET_RANGE};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "get_loudness",
        "Read the master's loudness meter (EBU R 128 / ITU-R BS.1770-4, 'EBU Mode'). It measures \
         the master's hardware output pair after every plug-in and fader and before the \
         metronome, so tracks routed straight to that pair count too. `momentary` (400 ms) and \
         `short_term` (3 s) are in LUFS and always measured. `integrated` (gated, LUFS), \
         `range` (LRA, LU; not yet stable in the first 60 s), `true_peak` (dBTP, 4x \
         oversampled), `max_momentary` and `max_short_term` accumulate only while the timeline \
         plays, until reset_loudness; `seconds` is how long they have run. -120 means silence; \
         null means not measured yet. Also reports the project's loudness `target` (LUFS) and \
         `offset`, the integrated reading minus the target (LU).",
        Safety::Read,
        |host, _: NoArgs| {
            let s = host.session();
            let target = s.project.loudness_target;
            let reading = s.loudness();
            Ok(Reading {
                offset: reading.integrated.map(|i| i - target),
                reading,
                target,
            })
        },
    );
    r.add(
        "set_loudness_target",
        "Set the integrated loudness the mix aims at, in LUFS: a `preset` (\"streaming\" for -14, \"broadcast\" for -23, EBU R 128), or `lufs` for any value from -40 to -5. The meter shows the master against it, and get_loudness reports how far the integrated reading is from it. Saved with the project.",
        Safety::Edit,
        set_target,
    );
    r.add(
        "reset_loudness",
        "Start a new loudness measurement on the master: clear the integrated loudness, \
         loudness range, true peak and maxima (see get_loudness). Not an edit of the project.",
        Safety::Transport,
        |host, _: NoArgs| {
            host.session().reset_loudness();
            Ok(Done { done: true })
        },
    );
    r.add(
        "measure_loudness",
        "Measure a mono or stereo audio file (WAV, FLAC, MP3 or AIFF; absolute `path`) with the same EBU R 128 meter as \
         the master: integrated loudness (LUFS), loudness range (LU), true peak (dBTP), and the \
         maximum momentary and short-term loudness; momentary and short-term are their values \
         at the end of the file. A mono file is measured as one channel, so it reads about \
         3 dB lower than the same audio panned centre in a stereo mix. Reads the whole file \
         before returning (well under a second for a song).",
        Safety::Read,
        |_host, input: PathArg| Ok(loudness::measure_file(&input.path)?),
    );
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}

/// The meter's readings, the project's target, and how far the integrated reading is from it.
#[derive(Serialize, JsonSchema)]
struct Reading {
    #[serde(flatten)]
    reading: Loudness,
    /// The loudness target, in LUFS.
    target: f64,
    /// Integrated minus target, in LU: positive is louder than the target.
    offset: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
enum Preset {
    Streaming,
    Broadcast,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetTarget {
    #[serde(default)]
    preset: Option<Preset>,
    /// A target in LUFS, -40 to -5, instead of a preset.
    #[serde(default)]
    lufs: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
struct Target {
    target: f64,
}

fn set_target(host: &mut dyn Host, input: SetTarget) -> ApiResult<Target> {
    let (low, high) = LOUDNESS_TARGET_RANGE;
    let target = match (input.preset, input.lufs) {
        (Some(_), Some(_)) => return Err(error("give `preset` or `lufs`, not both")),
        (None, None) => return Err(error("give `preset` or `lufs`")),
        (Some(Preset::Streaming), None) => LOUDNESS_PRESETS[0].1,
        (Some(Preset::Broadcast), None) => LOUDNESS_PRESETS[1].1,
        (None, Some(lufs)) if lufs.is_finite() && (low..=high).contains(&lufs) => lufs,
        (None, Some(lufs)) => return Err(error(format!("lufs {lufs} is outside {low}..{high}"))),
    };
    host.session().project.loudness_target = target;
    Ok(Target { target })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PathArg {
    /// Absolute path of a WAV file.
    path: PathBuf,
}
