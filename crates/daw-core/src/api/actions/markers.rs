//! Markers (dev plan 8.3): named places in the song, kept on their bars when the tempo
//! changes, and jumping the playhead to them.

use super::error;
use crate::api::{ApiResult, Registry, Safety};
use crate::session::Session;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "get_markers",
        "List the markers in time order: `number` (1, 2, 3… as the keys 1-9 jump to them), \
         `id`, `name`, `tick` (960 per quarter note; markers keep their bar when the tempo \
         changes), `position` (samples now) and `bar` / `beat` (from 1).",
        Safety::Read,
        |host, _: super::NoArgs| Ok(markers(host.session())),
    );
    r.add(
        "add_marker",
        "Add a marker at `position` (samples), or at `bar` (and `beat`, both from 1); the \
         playhead by default. Unnamed, it's called \"Marker N\". Returns the markers.",
        Safety::Edit,
        |host, input: AddMarker| {
            let s = host.session();
            let at = place(s, input.position, input.bar, input.beat)?;
            let id = s.add_marker(at.unwrap_or_else(|| s.position()), input.name);
            Ok(Added {
                marker: id,
                markers: markers(s).markers,
            })
        },
    );
    r.add(
        "set_marker",
        "Rename a marker (`name`) and/or move it to `position` (samples) or `bar` / `beat` \
         (from 1). Only the fields given change.",
        Safety::Edit,
        |host, input: SetMarker| {
            let s = host.session();
            let at = place(s, input.position, input.bar, input.beat)?;
            s.set_marker(input.marker, input.name, at)?;
            Ok(markers(s))
        },
    );
    r.add(
        "remove_marker",
        "Remove a marker by id.",
        Safety::Destructive,
        |host, input: MarkerArg| {
            let s = host.session();
            s.project.remove_marker(input.marker).map_err(error)?;
            Ok(markers(s))
        },
    );
    r.add(
        "go_to_marker",
        "Move the playhead to a marker: by `marker` id, by `number` (1 is the first in time), \
         or `direction` \"next\" / \"previous\" from the playhead (from a marker, the one after \
         or before it). Give exactly one. Refused while recording. Returns the position \
         (samples) and the marker.",
        Safety::Transport,
        |host, input: GoTo| {
            let s = host.session();
            let id = match (input.marker, input.number, input.direction) {
                (Some(id), None, None) => id,
                (None, Some(n), None) => s.marker_number(n).ok_or_else(|| {
                    error(format!(
                        "there is no marker {n} (there are {})",
                        s.project.markers.len()
                    ))
                })?,
                (None, None, Some(d)) => {
                    let forward = d == Direction::Next;
                    s.adjacent_marker(s.position(), forward).ok_or_else(|| {
                        error(format!(
                            "no marker {} the playhead",
                            if forward { "after" } else { "before" }
                        ))
                    })?
                }
                _ => return Err(error("give exactly one of marker, number or direction")),
            };
            let position = s.go_to_marker(id)?;
            Ok(Went {
                marker: id,
                position,
            })
        },
    );
}

/// A position from samples or a bar and beat (from 1), if either is given.
fn place(
    s: &Session,
    position: Option<u64>,
    bar: Option<u32>,
    beat: Option<u32>,
) -> ApiResult<Option<u64>> {
    match (position, bar) {
        (Some(_), Some(_)) => Err(error("give position or bar, not both")),
        (Some(p), None) => Ok(Some(p)),
        (None, Some(bar)) => {
            if bar == 0 || beat == Some(0) {
                return Err(error("bars and beats count from 1"));
            }
            let map = s.tempo_map();
            let tick = map.tick_of_bar(i64::from(bar) - 1, beat.unwrap_or(1) - 1);
            Ok(Some(map.sample_at_tick(tick as f64).round() as u64))
        }
        (None, None) if beat.is_some() => Err(error("beat needs a bar")),
        (None, None) => Ok(None),
    }
}

fn markers(s: &Session) -> MarkerList {
    let map = s.tempo_map();
    MarkerList {
        markers: s
            .project
            .markers
            .iter()
            .enumerate()
            .map(|(i, m)| {
                let position = map.sample_at_tick(m.tick as f64).round().max(0.0);
                let (bar, beats) = map.bar_at(position);
                MarkerView {
                    number: i + 1,
                    id: m.id,
                    name: m.name.clone(),
                    tick: m.tick,
                    position: position as u64,
                    bar: bar + 1,
                    beat: beats.floor() as i64 + 1,
                }
            })
            .collect(),
    }
}

#[derive(Serialize, JsonSchema)]
pub(super) struct MarkerView {
    number: usize,
    id: u32,
    name: String,
    tick: u64,
    position: u64,
    bar: i64,
    beat: i64,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct MarkerList {
    markers: Vec<MarkerView>,
}

/// The markers as get_project shows them.
pub(super) fn marker_views(s: &Session) -> Vec<MarkerView> {
    markers(s).markers
}

#[derive(Serialize, JsonSchema)]
struct Added {
    marker: u32,
    markers: Vec<MarkerView>,
}

#[derive(Serialize, JsonSchema)]
struct Went {
    marker: u32,
    position: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddMarker {
    /// Samples; the playhead when neither this nor `bar` is given.
    #[serde(default)]
    position: Option<u64>,
    /// Bar, from 1.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMarker {
    marker: u32,
    #[serde(default)]
    name: Option<String>,
    /// Samples.
    #[serde(default)]
    position: Option<u64>,
    /// Bar, from 1.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MarkerArg {
    marker: u32,
}

#[derive(Deserialize, JsonSchema, PartialEq)]
#[serde(rename_all = "snake_case")]
enum Direction {
    Next,
    Previous,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GoTo {
    #[serde(default)]
    marker: Option<u32>,
    /// 1 is the first marker in time.
    #[serde(default)]
    number: Option<usize>,
    #[serde(default)]
    direction: Option<Direction>,
}
