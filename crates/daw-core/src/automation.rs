//! Automation: lanes of points that move a track's (or the master's) volume, pan, mute, send
//! levels and plug-in parameters along the song.
//!
//! Points sit in musical time (ticks, [`crate::tempo::TICKS_PER_QUARTER`] to a quarter note),
//! so they stay on their beats when the tempo changes. Values are in the target's own units:
//! dB for volume and send levels, −1..1 for pan, 0 or 1 for mute, 0..1 (VST3's normalized
//! range) for plug-in parameters. A segment runs from a point to the next, straight or
//! curved by the first point's `curve`; mute steps.

use crate::project::{InsertId, Project, SendId, TrackId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub type LaneId = u32;
pub type PointId = u32;

/// Whose lanes: a track's, or the master's (`None`), as actions name it.
pub type LaneOwner = Option<TrackId>;

/// What a track's (or the master's) controls do to its lanes while playing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AutomationMode {
    /// Lanes play; the controls show them and take no edits.
    #[default]
    Read,
    /// Holding a control writes it; letting go glides back to the lane.
    Touch,
    /// Like touch, but letting go keeps writing the last value until playback stops.
    Latch,
    /// Every playing lane is written from the start of playback, at the control's value.
    Write,
}

impl AutomationMode {
    /// Whether moving a control while playing writes it.
    pub fn writes(self) -> bool {
        self != Self::Read
    }
}

/// What a lane moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AutomationTarget {
    /// The fader, in dB.
    Volume,
    /// −1 (left) to 1 (right).
    Pan,
    /// 0 plays, 1 mutes; it steps rather than sliding.
    Mute,
    /// A send's level, in dB.
    SendLevel { send: SendId },
    /// A plug-in parameter (its VST3 id), 0..1.
    Parameter { insert: InsertId, parameter: u32 },
}

impl AutomationTarget {
    /// The range its values take.
    pub fn range(&self) -> (f64, f64) {
        match self {
            Self::Volume | Self::SendLevel { .. } => (-60.0, 12.0),
            Self::Pan => (-1.0, 1.0),
            Self::Mute | Self::Parameter { .. } => (0.0, 1.0),
        }
    }

    /// Why `value` can't be a point's value, if it can't.
    pub fn check(&self, value: f64) -> Result<(), String> {
        let (low, high) = self.range();
        if !value.is_finite() || value < low || value > high {
            return Err(format!(
                "{value} is outside {low}..{high} for {}",
                self.describe()
            ));
        }
        if matches!(self, Self::Mute) && value != 0.0 && value != 1.0 {
            return Err(format!(
                "mute points are 0 (plays) or 1 (muted), not {value}"
            ));
        }
        Ok(())
    }

    /// Mute steps from point to point; everything else slides.
    pub fn steps(&self) -> bool {
        matches!(self, Self::Mute)
    }

    pub fn describe(&self) -> String {
        match self {
            Self::Volume => "volume (dB)".into(),
            Self::Pan => "pan".into(),
            Self::Mute => "mute".into(),
            Self::SendLevel { send } => format!("send {send} level (dB)"),
            Self::Parameter { insert, parameter } => {
                format!("parameter {parameter} of plug-in {insert}")
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AutomationPoint {
    /// Unique in the project.
    pub id: PointId,
    /// Ticks from the song's start.
    pub tick: u64,
    /// In the target's units.
    pub value: f64,
    /// The shape of the segment to the next point: 0 straight, above 0 starts slowly and ends
    /// fast, below 0 the opposite (−1 to 1).
    #[serde(default)]
    pub curve: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AutomationLane {
    /// Unique in the project.
    pub id: LaneId,
    pub target: AutomationTarget,
    /// In time order; two at one tick make a jump there.
    pub points: Vec<AutomationPoint>,
    /// Off: the lane is kept but doesn't move anything.
    pub enabled: bool,
}

/// How far along a segment the value is, `t` (0..1) of the way through it in time, for a
/// segment of shape `curve`: straight at 0; a power curve otherwise, mirror images for ±curve.
pub fn shape(t: f64, curve: f32) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if curve == 0.0 {
        return t;
    }
    let power = 2f64.powf(f64::from(curve.abs().min(1.0)) * 3.0);
    if curve > 0.0 {
        t.powf(power)
    } else {
        1.0 - (1.0 - t).powf(power)
    }
}

/// The lane for `target` among `lanes`, if it plays.
pub fn playing(lanes: &[AutomationLane], target: AutomationTarget) -> Option<&AutomationLane> {
    lanes.iter().find(|l| l.target == target && l.plays())
}

impl AutomationLane {
    /// Whether it moves anything: on, with points.
    pub fn plays(&self) -> bool {
        self.enabled && !self.points.is_empty()
    }

    /// Keep the points in time order: by tick, then as added, so of two points on one tick the
    /// later-added one is the value from that tick on.
    pub fn sort(&mut self) {
        self.points.sort_by_key(|p| (p.tick, p.id));
    }

    /// The value at `tick` (fractional: the engine evaluates between ticks), or `None` for a
    /// lane with no points (it moves nothing). Before the first point the first value holds,
    /// after the last the last; exactly on a point, that point's value (the later one of two
    /// on the same tick).
    pub fn value_at(&self, tick: f64) -> Option<f64> {
        let first = self.points.first()?;
        if tick < first.tick as f64 {
            return Some(first.value);
        }
        // The last point at or before `tick`.
        let i = self.points.partition_point(|p| p.tick as f64 <= tick) - 1;
        let a = &self.points[i];
        let Some(b) = self.points.get(i + 1) else {
            return Some(a.value);
        };
        if self.target.steps() || b.tick == a.tick {
            return Some(a.value);
        }
        let t = (tick - a.tick as f64) / (b.tick - a.tick) as f64;
        Some(a.value + (b.value - a.value) * shape(t, a.curve))
    }
}

/// A point to add, before it has an id.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewPoint {
    /// Ticks from the song's start (960 to a quarter note).
    pub tick: u64,
    /// In the lane's units.
    pub value: f64,
    /// Shape of the segment to the next point, −1 to 1; 0 (straight) when omitted.
    #[serde(default)]
    pub curve: f32,
}

fn check_curve(curve: f32) -> Result<(), String> {
    if (-1.0..=1.0).contains(&curve) {
        Ok(())
    } else {
        Err(format!("curve {curve} is outside -1..1"))
    }
}

impl NewPoint {
    /// Why it can't go on a lane for `target`, if it can't.
    pub fn check(&self, target: &AutomationTarget) -> Result<(), String> {
        target.check(self.value)?;
        check_curve(self.curve)
    }
}

fn owner_name(owner: LaneOwner) -> String {
    match owner {
        Some(track) => format!("track {track}"),
        None => "the master".into(),
    }
}

impl Project {
    pub fn lanes(&self, owner: LaneOwner) -> Option<&[AutomationLane]> {
        match owner {
            Some(track) => self.track(track).map(|t| t.automation.as_slice()),
            None => Some(&self.master.automation),
        }
    }

    fn lanes_mut(&mut self, owner: LaneOwner) -> Option<&mut Vec<AutomationLane>> {
        match owner {
            Some(track) => self.track_mut(track).map(|t| &mut t.automation),
            None => Some(&mut self.master.automation),
        }
    }

    /// Every lane in the project with its owner: tracks in order, then the master.
    pub fn all_lanes(&self) -> impl Iterator<Item = (LaneOwner, &AutomationLane)> {
        self.tracks
            .iter()
            .flat_map(|t| t.automation.iter().map(move |l| (Some(t.id), l)))
            .chain(self.master.automation.iter().map(|l| (None, l)))
    }

    fn all_lanes_mut(&mut self) -> impl Iterator<Item = &mut AutomationLane> {
        self.tracks
            .iter_mut()
            .flat_map(|t| t.automation.iter_mut())
            .chain(self.master.automation.iter_mut())
    }

    pub fn find_lane(&self, id: LaneId) -> Option<(LaneOwner, &AutomationLane)> {
        self.all_lanes().find(|(_, l)| l.id == id)
    }

    pub fn find_lane_mut(&mut self, id: LaneId) -> Option<&mut AutomationLane> {
        self.all_lanes_mut().find(|l| l.id == id)
    }

    /// The lane holding point `id`, and the point.
    pub fn find_point(&self, id: PointId) -> Option<(&AutomationLane, &AutomationPoint)> {
        self.all_lanes()
            .find_map(|(_, l)| l.points.iter().find(|p| p.id == id).map(|p| (l, p)))
    }

    /// Why `owner` can't have a lane for `target`, if it can't: the owner must exist, a send
    /// must be one of its own, a plug-in one of its own inserts, and each target gets one lane.
    pub fn lane_target_error(&self, owner: LaneOwner, target: AutomationTarget) -> Option<String> {
        let Some(lanes) = self.lanes(owner) else {
            return Some(format!("no {} (see get_project)", owner_name(owner)));
        };
        match target {
            AutomationTarget::SendLevel { send } => match (owner, self.find_send(send)) {
                (None, _) => return Some("the master has no sends".into()),
                (Some(track), Some((from, _))) if from == track => {}
                (Some(track), Some((from, _))) => {
                    return Some(format!(
                        "send {send} belongs to track {from}, not track {track}"
                    ))
                }
                (_, None) => return Some(format!("no send with id {send}")),
            },
            AutomationTarget::Parameter { insert, .. } => {
                let slots = match owner {
                    Some(track) => &self.track(track).expect("has lanes").inserts,
                    None => &self.master.inserts,
                };
                if !slots.iter().any(|s| s.id == insert) {
                    return Some(format!(
                        "plug-in {insert} is not in {}'s inserts",
                        owner_name(owner)
                    ));
                }
            }
            AutomationTarget::Volume | AutomationTarget::Pan | AutomationTarget::Mute => {}
        }
        lanes.iter().find(|l| l.target == target).map(|l| {
            format!(
                "{} already has a {} lane (lane {})",
                owner_name(owner),
                target.describe(),
                l.id
            )
        })
    }

    /// Give `owner` an empty lane for `target` and return its id.
    pub fn add_lane(
        &mut self,
        owner: LaneOwner,
        target: AutomationTarget,
    ) -> Result<LaneId, String> {
        if let Some(error) = self.lane_target_error(owner, target) {
            return Err(error);
        }
        let id = self.next_id();
        self.lanes_mut(owner)
            .expect("checked")
            .push(AutomationLane {
                id,
                target,
                points: Vec::new(),
                enabled: true,
            });
        Ok(id)
    }

    pub fn remove_lane(&mut self, id: LaneId) -> Option<AutomationLane> {
        let lanes = self
            .tracks
            .iter_mut()
            .map(|t| &mut t.automation)
            .chain(std::iter::once(&mut self.master.automation));
        for lanes in lanes {
            if let Some(at) = lanes.iter().position(|l| l.id == id) {
                return Some(lanes.remove(at));
            }
        }
        None
    }

    /// Add points to a lane, all or none: every value is checked against the lane's target
    /// first. Returns their ids, in the order given.
    pub fn add_points(
        &mut self,
        lane: LaneId,
        points: &[NewPoint],
    ) -> Result<Vec<PointId>, String> {
        let (_, l) = self
            .find_lane(lane)
            .ok_or(format!("no automation lane with id {lane}"))?;
        for p in points {
            p.check(&l.target)?;
        }
        let ids: Vec<PointId> = points.iter().map(|_| self.next_id()).collect();
        let l = self.find_lane_mut(lane).expect("found");
        l.points
            .extend(points.iter().zip(&ids).map(|(p, &id)| AutomationPoint {
                id,
                tick: p.tick,
                value: p.value,
                curve: p.curve,
            }));
        l.sort();
        Ok(ids)
    }

    /// Move a point, or change its value or curve; only what's given changes.
    pub fn set_point(
        &mut self,
        id: PointId,
        tick: Option<u64>,
        value: Option<f64>,
        curve: Option<f32>,
    ) -> Result<(), String> {
        let (lane, _) = self
            .find_point(id)
            .ok_or(format!("no automation point with id {id}"))?;
        if let Some(value) = value {
            lane.target.check(value)?;
        }
        if let Some(curve) = curve {
            check_curve(curve)?;
        }
        let lane = lane.id;
        let l = self.find_lane_mut(lane).expect("found");
        let p = l.points.iter_mut().find(|p| p.id == id).expect("found");
        p.tick = tick.unwrap_or(p.tick);
        p.value = value.unwrap_or(p.value);
        p.curve = curve.unwrap_or(p.curve);
        l.sort();
        Ok(())
    }

    /// Delete a lane's points: those listed, or (with `range`) those from `range.0` up to but
    /// not including `range.1`. Unknown ids refuse the whole call. Returns how many went.
    pub fn remove_points(
        &mut self,
        lane: LaneId,
        ids: &[PointId],
        range: Option<(u64, u64)>,
    ) -> Result<usize, String> {
        let l = self
            .find_lane_mut(lane)
            .ok_or(format!("no automation lane with id {lane}"))?;
        if let Some(missing) = ids.iter().find(|id| !l.points.iter().any(|p| p.id == **id)) {
            return Err(format!("lane {lane} has no point with id {missing}"));
        }
        let before = l.points.len();
        l.points.retain(|p| {
            !ids.contains(&p.id) && !range.is_some_and(|(from, to)| (from..to).contains(&p.tick))
        });
        Ok(before - l.points.len())
    }

    /// Put what a write pass wrote ([`crate::automation_write`]) into lane `lane`, segment by
    /// segment in order. Each segment replaces the lane's points over its span. One that ends
    /// with a glide also gets a point `glide` ticks after its end, at what the lane gave there
    /// before the pass, so the curve after it is kept. The lane is switched on.
    pub fn write_segments(
        &mut self,
        lane: LaneId,
        segments: &[crate::automation_write::Segment],
        glide: u64,
    ) -> Result<(), String> {
        let (_, original) = self
            .find_lane(lane)
            .ok_or(format!("no automation lane with id {lane}"))?;
        let original = original.clone();
        let target = original.target;
        let (low, high) = target.range();
        let fit = |value: f64| {
            if target.steps() {
                f64::from(u8::from(value >= 0.5))
            } else {
                value.clamp(low, high)
            }
        };
        // What the lane gave before the pass is read before anything changes: a later segment
        // must glide back to the old curve, not to what an earlier one just wrote.
        let returns: Vec<Option<(u64, f64)>> = segments
            .iter()
            .map(|s| {
                let at = s.end() + glide;
                s.glide
                    .then(|| original.value_at(at as f64).map(|v| (at, fit(v))))
                    .flatten()
            })
            .collect();
        for (segment, back) in segments.iter().zip(returns) {
            if segment.points.is_empty() {
                continue;
            }
            // The lane as it stands where the segment starts (after any earlier segment): a
            // point holding it there, before the written ones on the same tick, keeps the
            // curve leading up to the touch.
            let lead = self
                .find_lane(lane)
                .and_then(|(_, l)| l.value_at(segment.start() as f64))
                .map(|v| (segment.start(), fit(v)));
            let end = back.map_or(segment.end(), |(at, _)| at);
            self.remove_points(lane, &[], Some((segment.start(), end + 1)))?;
            let points: Vec<NewPoint> = lead
                .into_iter()
                .chain(
                    segment
                        .points
                        .iter()
                        .map(|&(tick, value)| (tick, fit(value))),
                )
                .chain(back)
                .map(|(tick, value)| NewPoint {
                    tick,
                    value,
                    curve: 0.0,
                })
                .collect();
            self.add_points(lane, &points)?;
        }
        if let Some(l) = self.find_lane_mut(lane) {
            l.enabled = true;
        }
        Ok(())
    }

    /// Drop every lane whose target is gone (its send or plug-in was removed).
    pub(crate) fn forget_lanes(&mut self, gone: impl Fn(&AutomationTarget) -> bool) {
        for track in &mut self.tracks {
            track.automation.retain(|l| !gone(&l.target));
        }
        self.master.automation.retain(|l| !gone(&l.target));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lane(target: AutomationTarget, points: &[(u64, f64, f32)]) -> AutomationLane {
        let mut lane = AutomationLane {
            id: 1,
            target,
            points: points
                .iter()
                .enumerate()
                .map(|(i, &(tick, value, curve))| AutomationPoint {
                    id: i as u32 + 10,
                    tick,
                    value,
                    curve,
                })
                .collect(),
            enabled: true,
        };
        lane.sort();
        lane
    }

    #[test]
    fn straight_segments_hold_outside_and_meet_every_point() {
        let l = lane(
            AutomationTarget::Volume,
            &[(960, -12.0, 0.0), (1920, 0.0, 0.0)],
        );
        assert_eq!(l.value_at(0.0), Some(-12.0), "held before the first point");
        assert_eq!(l.value_at(960.0), Some(-12.0));
        assert_eq!(l.value_at(1440.0), Some(-6.0), "the mean halfway");
        assert_eq!(l.value_at(1920.0), Some(0.0));
        assert_eq!(l.value_at(5000.0), Some(0.0), "held after the last");
        assert_eq!(
            lane(AutomationTarget::Pan, &[(100, 0.5, 0.0)]).value_at(9.0),
            Some(0.5)
        );
        assert_eq!(
            lane(AutomationTarget::Pan, &[]).value_at(9.0),
            None,
            "an empty lane moves nothing"
        );
    }

    #[test]
    fn curves_bend_either_way_as_mirror_images() {
        for c in [0.3f32, 1.0] {
            for t in [0.1, 0.25, 0.5, 0.9] {
                assert!((shape(t, c) - (1.0 - shape(1.0 - t, -c))).abs() < 1e-12);
            }
            assert!(shape(0.5, c) < 0.5 && shape(0.5, -c) > 0.5);
            assert_eq!((shape(0.0, c), shape(1.0, c)), (0.0, 1.0), "ends exact");
        }
        let up = lane(
            AutomationTarget::Volume,
            &[(0, -60.0, 0.5), (1000, 0.0, 0.0)],
        );
        assert!(up.value_at(500.0).unwrap() < -30.0, "slow start");
    }

    #[test]
    fn mute_steps_and_two_points_on_a_tick_jump() {
        let m = lane(AutomationTarget::Mute, &[(0, 0.0, 0.8), (960, 1.0, 0.0)]);
        assert_eq!(m.value_at(959.9), Some(0.0), "no slide, curve or not");
        assert_eq!(m.value_at(960.0), Some(1.0));
        // A jump at 960: up to -6 dB there, then down to -20 by 1920.
        let jump = lane(
            AutomationTarget::Volume,
            &[
                (0, -30.0, 0.0),
                (960, -6.0, 0.0),
                (960, -10.0, 0.0),
                (1920, -20.0, 0.0),
            ],
        );
        assert_eq!(jump.value_at(959.0).map(|v| v.round()), Some(-6.0));
        assert_eq!(
            jump.value_at(960.0),
            Some(-10.0),
            "the later point from its tick on"
        );
        assert_eq!(jump.value_at(1440.0), Some(-15.0));
    }

    #[test]
    fn written_segments_replace_their_span_and_glide_back_to_the_old_curve() {
        use crate::automation_write::Segment;
        let mut p = Project::default();
        let track = p.add_track(crate::project::TrackKind::Audio, None);
        let lane = p.add_lane(Some(track), AutomationTarget::Volume).unwrap();
        // A ramp from -20 dB at tick 0 to 0 dB at tick 1000, with a point in the middle.
        p.add_points(
            lane,
            &[
                NewPoint {
                    tick: 0,
                    value: -20.0,
                    curve: 0.0,
                },
                NewPoint {
                    tick: 300,
                    value: -14.0,
                    curve: 0.0,
                },
                NewPoint {
                    tick: 1000,
                    value: 0.0,
                    curve: 0.0,
                },
            ],
        )
        .unwrap();
        let original = p.find_lane(lane).unwrap().1.clone();
        p.find_lane_mut(lane).unwrap().enabled = false;
        // A touch from 200 to 400, then a second one from 450 to 480 whose glide (to 600)
        // reaches over the first one's (to 520): both must land on the old curve.
        let segments = [
            Segment {
                points: vec![(200, -30.0), (400, -40.0)],
                glide: true,
            },
            Segment {
                points: vec![(450, -50.0), (480, -50.0)],
                glide: true,
            },
        ];
        p.write_segments(lane, &segments, 120).unwrap();
        let l = p.find_lane(lane).unwrap().1;
        let ticks: Vec<(u64, f64)> = l.points.iter().map(|p| (p.tick, p.value)).collect();
        let old = |tick: u64| original.value_at(tick as f64).unwrap();
        // Where the second touch starts, the first one's glide is on its way to the old curve.
        let gliding = -40.0 + (old(520) + 40.0) * (450.0 - 400.0) / 120.0;
        let expected = [
            (0, -20.0),
            (200, old(200)),
            (200, -30.0),
            (400, -40.0),
            (450, gliding),
            (450, -50.0),
            (480, -50.0),
            (600, old(600)),
            (1000, 0.0),
        ];
        assert_eq!(ticks.len(), expected.len(), "{ticks:?}");
        for (got, want) in ticks.iter().zip(expected) {
            assert_eq!(got.0, want.0, "{ticks:?}");
            assert!((got.1 - want.1).abs() < 1e-9, "{got:?} vs {want:?}");
        }
        assert!(
            (l.value_at(800.0).unwrap() - old(800)).abs() < 1e-9,
            "the curve after is kept"
        );
        assert_eq!(l.value_at(100.0), original.value_at(100.0), "and before");
        assert!(l.enabled, "writing switches the lane on");
    }

    #[test]
    fn values_are_checked_against_the_target() {
        assert!(AutomationTarget::Volume.check(-61.0).is_err());
        assert!(AutomationTarget::Volume.check(6.0).is_ok());
        assert!(AutomationTarget::Pan.check(1.5).is_err());
        assert!(AutomationTarget::Mute
            .check(0.5)
            .unwrap_err()
            .contains("0 (plays) or 1"));
        assert!(AutomationTarget::Parameter {
            insert: 1,
            parameter: 2
        }
        .check(0.25)
        .is_ok());
        assert!(AutomationTarget::SendLevel { send: 3 }
            .check(f64::NAN)
            .is_err());
    }
}
