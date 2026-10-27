//! Analysis and editing on the musical grid: transients, batch splits and quantizing.

use super::{check_clip, error};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::grid::{Division, Rounding};
use crate::project::ClipId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "detect_transients",
        "Find the transients (note attacks, hits) in a clip's audio. Returns their timeline \
         positions in samples, where each attack starts, with a strength from 0 to 1. \
         `sensitivity` 0 to 1 (default 0.5; higher finds softer attacks), `min_gap_ms` the \
         shortest distance between two (default 50). Typical use: split_clip_at these \
         positions, then quantize_clips the pieces.",
        Safety::Read,
        detect,
    );
    r.add(
        "split_clip_at",
        "Cut a clip at several timeline positions (samples) in one go, e.g. at every \
         transient. Returns all the pieces in time order (the original clip id first) and any \
         positions that were not inside the clip.",
        Safety::Edit,
        split_at,
    );
    r.add(
        "quantize_positions",
        "Snap timeline positions (samples) to the grid. `division` is bar, beat, 1/4, 1/8, \
         1/16, 1/32, or a triplet like 1/8t (fractions of a whole note, following the tempo \
         map: grid lines run from each bar's start, so they follow tempo and time-signature \
         changes); `rounding` nearest (default), previous or next. Returns each position with \
         its grid position.",
        Safety::Read,
        quantize_positions,
    );
    r.add(
        "quantize_clips",
        "Move clips so they start on the grid, like audio quantize: after split_clip_at at \
         the transients, each piece starts at its transient, so this lines the hits up with \
         the beat. Set the project tempo to the performance's tempo first (set_tempo, or \
         add_tempo_change where it changes), or the hits are pulled to the wrong beats. \
         `division` as for quantize_positions; `strength` 0 to 1 (default 1 = fully on the \
         grid, 0.5 = halfway). A piece that would now overlap the next is shortened to \
         meet it; with `fill_gaps` (default true) a piece with a gap before it starts a little \
         earlier, playing the audio just before its hit, so no hit is heard twice and no \
         silent gaps open. Neighbours are then joined by `crossfade_ms` (default 5, 0 = hard \
         cuts) linear crossfades over the quiet audio just before each hit, so moved slices \
         don't click; a quantized piece therefore starts that much before its hit (its hit \
         stays on the grid). Only clips quantized together are joined, so include the piece \
         before the first hit. Returns where each piece's hit moved from and to.",
        Safety::Edit,
        quantize_clips,
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Detect {
    clip: ClipId,
    /// 0 to 1, default 0.5.
    #[serde(default)]
    sensitivity: Option<f32>,
    /// Shortest distance between two transients, in milliseconds (default 50).
    #[serde(default)]
    min_gap_ms: Option<f32>,
}

#[derive(Serialize, JsonSchema)]
struct Transient {
    position: u64,
    strength: f32,
}

#[derive(Serialize, JsonSchema)]
struct Detected {
    clip: ClipId,
    count: usize,
    transients: Vec<Transient>,
}

fn detect(host: &mut dyn Host, input: Detect) -> ApiResult<Detected> {
    let s = host.session();
    check_clip(s, input.clip)?;
    let transients: Vec<Transient> = s
        .detect_transients(
            input.clip,
            input.sensitivity.unwrap_or(0.5),
            input.min_gap_ms.unwrap_or(50.0),
        )?
        .into_iter()
        .map(|(position, strength)| Transient { position, strength })
        .collect();
    Ok(Detected {
        clip: input.clip,
        count: transients.len(),
        transients,
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SplitAt {
    clip: ClipId,
    /// Timeline positions in samples.
    positions: Vec<u64>,
}

#[derive(Serialize, JsonSchema)]
struct SplitResult {
    /// All pieces in time order, the original clip first.
    clips: Vec<ClipId>,
    skipped: Vec<u64>,
}

fn split_at(host: &mut dyn Host, input: SplitAt) -> ApiResult<SplitResult> {
    let s = host.session();
    check_clip(s, input.clip)?;
    if input.positions.len() > 10_000 {
        return Err(error("at most 10000 positions per call"));
    }
    let (clips, skipped) = s
        .project
        .split_clip_at(input.clip, &input.positions)
        .expect("checked");
    s.mark_dirty();
    Ok(SplitResult { clips, skipped })
}

#[derive(Deserialize, JsonSchema, Clone, Copy, Default)]
#[serde(rename_all = "lowercase")]
enum RoundingArg {
    #[default]
    Nearest,
    Previous,
    Next,
}

/// The crossfade quantized slices get unless asked otherwise: short enough to sit in the quiet
/// before a hit, long enough not to click.
pub const DEFAULT_CROSSFADE_MS: f64 = 5.0;

fn division(text: &str) -> ApiResult<Division> {
    text.parse().map_err(error)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QuantizePositions {
    positions: Vec<u64>,
    /// bar, beat, 1/4, 1/8, 1/16, 1/32, or a triplet like 1/8t.
    division: String,
    #[serde(default)]
    rounding: RoundingArg,
}

#[derive(Serialize, JsonSchema)]
struct Quantized {
    position: u64,
    quantized: u64,
}

#[derive(Serialize, JsonSchema)]
struct QuantizedPositions {
    positions: Vec<Quantized>,
}

fn quantize_positions(
    host: &mut dyn Host,
    input: QuantizePositions,
) -> ApiResult<QuantizedPositions> {
    let s = host.session();
    let division = division(&input.division)?;
    let rounding = match input.rounding {
        RoundingArg::Nearest => Rounding::Nearest,
        RoundingArg::Previous => Rounding::Previous,
        RoundingArg::Next => Rounding::Next,
    };
    let map = s.tempo_map();
    Ok(QuantizedPositions {
        positions: input
            .positions
            .iter()
            .map(|&position| Quantized {
                position,
                quantized: map.snap(position, division, rounding),
            })
            .collect(),
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct QuantizeClips {
    clips: Vec<ClipId>,
    division: String,
    /// 0 to 1, default 1.
    #[serde(default)]
    strength: Option<f64>,
    /// Lengthen pieces to close gaps (default true).
    #[serde(default)]
    fill_gaps: Option<bool>,
    /// Crossfade between neighbouring pieces, in milliseconds (default 5; 0 = hard cuts).
    #[serde(default)]
    crossfade_ms: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
struct Moved {
    clip: ClipId,
    from: u64,
    to: u64,
}

fn quantize_clips(host: &mut dyn Host, input: QuantizeClips) -> ApiResult<Vec<Moved>> {
    let s = host.session();
    for &clip in &input.clips {
        check_clip(s, clip)?;
    }
    let strength = input.strength.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&strength) {
        return Err(error("strength must be between 0 and 1"));
    }
    let division = division(&input.division)?;
    let moves = s.quantize_clips(
        &input.clips,
        division,
        strength,
        input.fill_gaps.unwrap_or(true),
        input.crossfade_ms.unwrap_or(DEFAULT_CROSSFADE_MS),
    )?;
    Ok(moves
        .into_iter()
        .map(|(clip, from, to)| Moved { clip, from, to })
        .collect())
}
