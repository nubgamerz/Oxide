//! Automation: lanes of points moving a track's or the master's volume, pan, mute, send levels
//! and plug-in parameters along the song. Points are in ticks (960 to a quarter note).

use super::plugins::{find_parameter, ParameterRef};
use super::{check_insert, check_track, error};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::automation::{
    AutomationLane, AutomationMode, AutomationPoint, AutomationTarget, LaneId, LaneOwner, NewPoint,
    PointId,
};
use crate::project::{InsertId, SendId, TrackId};
use crate::session::Session;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub(super) fn register(r: &mut Registry) {
    r.add(
        "set_automation_mode",
        "Set what a track's (or, with `track` omitted, the master's) controls do to its \
         automation lanes while playing: \"read\" (lanes play; the controls follow them), \
         \"touch\" (holding a fader, pan, mute or plug-in knob in the app writes it; letting \
         go glides back to the lane), \"latch\" (like touch, but keeps writing the last value \
         until playback stops) or \"write\" (every playing lane is written from the start of \
         playback). A pass becomes one undo step when playback stops. Writing needs a person \
         at the controls; agents add points with add_automation_points instead.",
        Safety::Edit,
        set_mode,
    );
    r.add(
        "add_automation_lane",
        "Add an automation lane to a track (or the master when `track` is omitted). \
             `target` is {\"type\":\"volume\"} (dB, -60 to +12), {\"type\":\"pan\"} (-1 left to \
             1 right), {\"type\":\"mute\"} (0 plays, 1 muted; steps), \
             {\"type\":\"send_level\",\"send\":id} (dB, one of the track's own sends) or \
             {\"type\":\"parameter\",\"insert\":id,\"parameter\":id or name} (0 to 1, a \
             plug-in on the same track; see list_plugin_parameters). One lane per target. \
             `points` adds points at once (see add_automation_points); without `points` the \
             lane starts with one point at tick 0 holding the current setting (the fader, \
             pan, mute, send level or parameter value), so adding it changes nothing. The \
             track's lanes are shown in the arrangement. Lanes play \
             live and in exports alike, following the audio they move (after plug-in latency), \
             with a 32-sample grid. A mute lane overrules the track's mute button; solo still \
             wins. Returns the lane with its id.",
        Safety::Edit,
        add_lane,
    );
    r.add(
        "get_automation_lane",
        "Read an automation lane by id: its owner (`track`, null for the master), target, \
             units, whether it is enabled, and its points (id, tick, value, curve) in time \
             order.",
        Safety::Read,
        |host, input: LaneArg| {
            let s = host.session();
            let (owner, lane) = find_lane(s, input.lane)?;
            Ok(LaneView::new(s, owner, lane))
        },
    );
    r.add(
        "set_automation_lane",
        "Switch an automation lane on or off (`enabled`): an off lane keeps its points but \
             moves nothing.",
        Safety::Edit,
        |host, input: SetLane| {
            let s = host.session();
            find_lane(s, input.lane)?;
            if let Some(enabled) = input.enabled {
                s.project.find_lane_mut(input.lane).expect("found").enabled = enabled;
            }
            let (owner, lane) = find_lane(s, input.lane)?;
            Ok(LaneView::new(s, owner, lane))
        },
    );
    r.add(
        "remove_automation_lane",
        "Delete an automation lane by id, with its points.",
        Safety::Destructive,
        |host, input: LaneArg| {
            let s = host.session();
            find_lane(s, input.lane)?;
            s.project.remove_lane(input.lane);
            Ok(Done { done: true })
        },
    );
    r.add(
        "add_automation_points",
        "Add points to an automation lane. Each point: `tick` (from the start of the song, \
             960 per quarter note) or `bar` (and `beat`, from 1), `value` in the lane's units \
             (see add_automation_lane), and optionally `curve` (-1 to 1; 0 straight, above 0 \
             starts slowly and ends fast, below 0 the opposite) shaping the segment to the next \
             point. Values hold before the first point and after the last. Two points on one \
             tick make a jump there: the later-added one holds from that tick on. All or \
             nothing: one bad value refuses the call. Returns the new point ids in order.",
        Safety::Edit,
        add_points,
    );
    r.add(
        "set_automation_point",
        "Change an automation point by id: `tick` (or `bar` and `beat`), `value`, `curve`. \
             Only the fields given change.",
        Safety::Edit,
        set_point,
    );
    r.add(
        "remove_automation_points",
        "Delete points from an automation lane: those listed in `points` (ids), and/or \
             those from `from_tick` up to (not including) `to_tick`. Returns how many went.",
        Safety::Destructive,
        remove_points,
    );
    r.add(
        "get_automation_value",
        "The value an automation lane gives at `tick` (from the start of the song, 960 per \
             quarter note; fractions allowed) or at `bar` (and `beat`, from 1), in the lane's \
             units; null for a lane with no points.",
        Safety::Read,
        |host, input: ValueArg| {
            let s = host.session();
            let (_, lane) = find_lane(s, input.lane)?;
            let tick = match (input.tick, input.bar) {
                (Some(_), Some(_)) => return Err(error("give `tick` or `bar`, not both")),
                (Some(tick), None) => tick,
                (None, bar) => position(s, None, bar, input.beat)? as f64,
            };
            Ok(Value {
                tick,
                value: lane.value_at(tick),
            })
        },
    );
}

#[derive(Serialize, JsonSchema)]
struct Done {
    done: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMode {
    /// Track id; omit for the master bus.
    #[serde(default)]
    track: Option<TrackId>,
    mode: AutomationMode,
}

#[derive(Serialize, JsonSchema)]
struct ModeView {
    track: Option<TrackId>,
    mode: AutomationMode,
}

fn set_mode(host: &mut dyn Host, input: SetMode) -> ApiResult<ModeView> {
    let s = host.session();
    match input.track {
        Some(track) => {
            check_track(s, track)?;
            s.project.track_mut(track).expect("checked").automation_mode = input.mode;
        }
        None => s.project.master.automation_mode = input.mode,
    }
    Ok(ModeView {
        track: input.track,
        mode: input.mode,
    })
}

fn find_lane(s: &Session, id: LaneId) -> ApiResult<(LaneOwner, &AutomationLane)> {
    s.project
        .find_lane(id)
        .ok_or_else(|| error(format!("no automation lane with id {id} (see get_project)")))
}

/// A position as ticks, or a bar and beat (from 1), in ticks.
fn position(s: &Session, tick: Option<u64>, bar: Option<u32>, beat: Option<u32>) -> ApiResult<u64> {
    match (tick, bar, beat) {
        (Some(_), Some(_), _) => Err(error("give `tick` or `bar`, not both")),
        (_, _, Some(0)) => Err(error("beats count from 1")),
        (_, Some(0), _) => Err(error("bars count from 1")),
        (Some(tick), None, None) => Ok(tick),
        (Some(_), None, Some(_)) | (None, None, Some(_)) => Err(error("`beat` goes with `bar`")),
        (None, Some(bar), beat) => Ok(s
            .tempo_map()
            .tick_of_bar(i64::from(bar) - 1, beat.unwrap_or(1) - 1)),
        (None, None, None) => Err(error("give `tick`, or `bar` (and `beat`)")),
    }
}

/// What a lane's units are, for agents reading it.
fn units(target: &AutomationTarget) -> &'static str {
    match target {
        AutomationTarget::Volume | AutomationTarget::SendLevel { .. } => "dB, -60 to +12",
        AutomationTarget::Pan => "-1 (left) to 1 (right)",
        AutomationTarget::Mute => "0 plays, 1 muted",
        AutomationTarget::Parameter { .. } => "normalized, 0 to 1",
    }
}

/// A plug-in parameter's name, if its plug-in is loaded.
fn parameter_name(s: &Session, insert: InsertId, parameter: u32) -> Option<String> {
    s.parameters(insert)
        .ok()?
        .into_iter()
        .find(|p| p.id == parameter)
        .map(|p| p.name)
}

/// A lane as get_project lists it.
#[derive(Serialize, JsonSchema)]
pub(super) struct LaneSummary {
    lane: LaneId,
    target: AutomationTarget,
    enabled: bool,
    points: usize,
}

impl LaneSummary {
    pub(super) fn all(lanes: &[AutomationLane]) -> Vec<Self> {
        lanes
            .iter()
            .map(|l| Self {
                lane: l.id,
                target: l.target,
                enabled: l.enabled,
                points: l.points.len(),
            })
            .collect()
    }
}

#[derive(Serialize, JsonSchema)]
struct LaneView {
    lane: LaneId,
    /// The track it belongs to; null for the master.
    track: Option<TrackId>,
    target: AutomationTarget,
    /// The plug-in's name for a parameter lane's parameter, when the plug-in is loaded.
    #[serde(skip_serializing_if = "Option::is_none")]
    parameter_name: Option<String>,
    units: &'static str,
    enabled: bool,
    points: Vec<PointView>,
}

impl LaneView {
    fn new(s: &Session, owner: LaneOwner, lane: &AutomationLane) -> Self {
        Self {
            lane: lane.id,
            track: owner,
            target: lane.target,
            parameter_name: match lane.target {
                AutomationTarget::Parameter { insert, parameter } => {
                    parameter_name(s, insert, parameter)
                }
                _ => None,
            },
            units: units(&lane.target),
            enabled: lane.enabled,
            points: lane.points.iter().map(PointView::from).collect(),
        }
    }
}

#[derive(Serialize, JsonSchema)]
struct PointView {
    point: PointId,
    tick: u64,
    value: f64,
    curve: f32,
}

impl From<&AutomationPoint> for PointView {
    fn from(p: &AutomationPoint) -> Self {
        Self {
            point: p.id,
            tick: p.tick,
            value: p.value,
            curve: p.curve,
        }
    }
}

/// What a new lane moves, as agents give it: a parameter may be named.
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum TargetArg {
    Volume,
    Pan,
    Mute,
    SendLevel {
        send: SendId,
    },
    Parameter {
        insert: InsertId,
        /// The parameter's id, or its name (exact or a unique part).
        parameter: ParameterRef,
    },
}

/// A point as agents give it: at a tick, or at a bar and beat.
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PointArg {
    /// Ticks from the start of the song (960 per quarter note).
    #[serde(default)]
    tick: Option<u64>,
    /// Bar, from 1, instead of `tick`.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
    /// In the lane's units.
    value: f64,
    /// -1 to 1; 0 (straight) when omitted.
    #[serde(default)]
    curve: f32,
}

fn new_points(s: &Session, points: &[PointArg]) -> ApiResult<Vec<NewPoint>> {
    points
        .iter()
        .map(|p| {
            Ok(NewPoint {
                tick: position(s, p.tick, p.bar, p.beat)?,
                value: p.value,
                curve: p.curve,
            })
        })
        .collect()
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddLane {
    /// Track id; omit for the master bus.
    #[serde(default)]
    track: Option<TrackId>,
    target: TargetArg,
    /// The lane's points; without them it starts with one holding the current setting.
    #[serde(default)]
    points: Option<Vec<PointArg>>,
}

fn add_lane(host: &mut dyn Host, input: AddLane) -> ApiResult<LaneView> {
    let s = host.session();
    if let Some(track) = input.track {
        check_track(s, track)?;
    }
    let target = match input.target {
        TargetArg::Volume => AutomationTarget::Volume,
        TargetArg::Pan => AutomationTarget::Pan,
        TargetArg::Mute => AutomationTarget::Mute,
        TargetArg::SendLevel { send } => AutomationTarget::SendLevel { send },
        TargetArg::Parameter { insert, parameter } => {
            check_insert(s, insert)?;
            let parameter = match s.is_loaded(insert) {
                true => {
                    let params = s.parameters(insert).map_err(|e| error(format!("{e:#}")))?;
                    let p = find_parameter(&params, &parameter)?;
                    if p.is_read_only || !p.can_automate {
                        return Err(error(format!(
                            "'{}' can't be automated (the plug-in says so)",
                            p.name
                        )));
                    }
                    p.id
                }
                // Its parameters are unknown: trust an id, so the lane survives a missing
                // plug-in the way its saved state does.
                false => match parameter {
                    ParameterRef::Id(id) => id,
                    ParameterRef::Name(_) => {
                        return Err(error(format!(
                            "insert {insert} failed to load, so give the parameter by id"
                        )))
                    }
                },
            };
            AutomationTarget::Parameter { insert, parameter }
        }
    };
    let points = input
        .points
        .as_deref()
        .map(|p| new_points(s, p))
        .transpose()?;
    let id = s
        .add_automation_lane(input.track, target, points.as_deref())
        .map_err(error)?;
    let (owner, lane) = find_lane(s, id)?;
    Ok(LaneView::new(s, owner, lane))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct LaneArg {
    lane: LaneId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetLane {
    lane: LaneId,
    #[serde(default)]
    enabled: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddPoints {
    lane: LaneId,
    points: Vec<PointArg>,
}

#[derive(Serialize, JsonSchema)]
struct PointIds {
    points: Vec<PointId>,
}

fn add_points(host: &mut dyn Host, input: AddPoints) -> ApiResult<PointIds> {
    let s = host.session();
    find_lane(s, input.lane)?;
    let points = new_points(s, &input.points)?;
    let points = s.project.add_points(input.lane, &points).map_err(error)?;
    Ok(PointIds { points })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetPoint {
    point: PointId,
    /// Ticks from the start of the song (960 per quarter note).
    #[serde(default)]
    tick: Option<u64>,
    /// Bar, from 1, instead of `tick`.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
    #[serde(default)]
    value: Option<f64>,
    #[serde(default)]
    curve: Option<f32>,
}

fn set_point(host: &mut dyn Host, input: SetPoint) -> ApiResult<PointView> {
    let s = host.session();
    let tick = match (input.tick, input.bar, input.beat) {
        (None, None, None) => None,
        (tick, bar, beat) => Some(position(s, tick, bar, beat)?),
    };
    s.project
        .set_point(input.point, tick, input.value, input.curve)
        .map_err(error)?;
    let (_, point) = s.project.find_point(input.point).expect("just set");
    Ok(PointView::from(point))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RemovePoints {
    lane: LaneId,
    /// Point ids to delete.
    #[serde(default)]
    points: Vec<PointId>,
    /// Delete points from this tick on…
    #[serde(default)]
    from_tick: Option<u64>,
    /// …up to (not including) this one.
    #[serde(default)]
    to_tick: Option<u64>,
}

#[derive(Serialize, JsonSchema)]
struct Removed {
    removed: usize,
}

fn remove_points(host: &mut dyn Host, input: RemovePoints) -> ApiResult<Removed> {
    let s = host.session();
    find_lane(s, input.lane)?;
    let range = match (input.from_tick, input.to_tick) {
        (None, None) => None,
        (from, to) => {
            let (from, to) = (from.unwrap_or(0), to.unwrap_or(u64::MAX));
            if to <= from {
                return Err(error(format!("to_tick {to} is not after from_tick {from}")));
            }
            Some((from, to))
        }
    };
    if input.points.is_empty() && range.is_none() {
        return Err(error("give `points`, or `from_tick`/`to_tick`"));
    }
    let removed = s
        .project
        .remove_points(input.lane, &input.points, range)
        .map_err(error)?;
    Ok(Removed { removed })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ValueArg {
    lane: LaneId,
    /// Ticks from the start of the song (960 per quarter note); fractions allowed.
    #[serde(default)]
    tick: Option<f64>,
    /// Bar, from 1, instead of `tick`.
    #[serde(default)]
    bar: Option<u32>,
    /// Beat of that bar, from 1 (default 1).
    #[serde(default)]
    beat: Option<u32>,
}

#[derive(Serialize, JsonSchema)]
struct Value {
    tick: f64,
    /// Null for a lane with no points.
    value: Option<f64>,
}
