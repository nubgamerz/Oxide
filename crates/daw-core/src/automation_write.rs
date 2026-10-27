//! Writing automation from the controls while playing (dev plan 5.4): touch, latch and write.
//!
//! A [`Writer`] follows one pass of playback. Controls report what they do (`touch`, `release`)
//! and every frame `advance` records each written control's value at the playhead's tick. The
//! project's lanes don't change during the pass: the engine plays the controls themselves for
//! the targets being written (see `graph::NodeParams::writing`), and, after a touch is let go,
//! the unchanged lane at the positions still to come. When playback stops, `finish` hands over
//! what was written, segment by segment, to go into the lanes as one undo step
//! ([`crate::project::Project::write_segments`]).
//!
//! Pure data, so the modes are pinned by the tests below without a device.

use crate::automation::{AutomationMode, AutomationTarget, LaneOwner};

/// A control being written: whose, and what.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteKey {
    pub owner: LaneOwner,
    pub target: AutomationTarget,
}

/// One stretch of writing, from the tick it started to the tick it ended.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    /// (tick, value), in time order; never empty.
    pub points: Vec<(u64, f64)>,
    /// Ended by letting go in touch mode, or by the end of the pass: the lane glides back
    /// from the last written value to what it was before, so the curve after it is kept.
    pub glide: bool,
}

impl Segment {
    pub fn start(&self) -> u64 {
        self.points.first().map_or(0, |p| p.0)
    }

    pub fn end(&self) -> u64 {
        self.points.last().map_or(0, |p| p.0)
    }

    /// Record `value` at `tick`, replacing a point already at that tick.
    fn record(&mut self, tick: u64, value: f64) {
        match self.points.last_mut() {
            Some(last) if last.0 >= tick => last.1 = value,
            _ => self.points.push((tick, value)),
        }
    }
}

/// What one control wrote in a pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Pass {
    pub key: WriteKey,
    /// Its value before it was first touched: what a lane made for it starts at.
    pub before: f64,
    /// In time order.
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone)]
struct Written {
    key: WriteKey,
    mode: AutomationMode,
    before: f64,
    /// The control's value now.
    live: f64,
    /// Being written: held (touch), held or let go (latch), or armed (write).
    active: bool,
    segments: Vec<Segment>,
}

impl Written {
    fn open(&mut self, tick: u64) {
        self.segments.push(Segment {
            points: vec![(tick, self.live)],
            glide: false,
        });
    }

    fn close(&mut self, tick: u64) {
        if let Some(segment) = self.segments.last_mut() {
            segment.record(tick, self.live);
            segment.glide = true;
        }
    }
}

/// One pass of writing. See the module docs.
#[derive(Debug, Clone, Default)]
pub struct Writer {
    written: Vec<Written>,
    /// The tick of the last frame.
    tick: u64,
}

impl Writer {
    fn find(&mut self, key: WriteKey) -> Option<&mut Written> {
        self.written.iter_mut().find(|w| w.key == key)
    }

    /// A control under `mode` was moved (or grabbed) to `value` at `tick`: start writing it, or
    /// carry on with the new value. `before` is what it was set to before its first touch in
    /// the pass (later ones keep the first).
    pub fn touch(
        &mut self,
        key: WriteKey,
        mode: AutomationMode,
        value: f64,
        tick: u64,
        before: f64,
    ) {
        if !mode.writes() {
            return;
        }
        match self.find(key) {
            Some(w) if w.active => {
                w.live = value;
                if let Some(segment) = w.segments.last_mut() {
                    segment.record(tick, value);
                }
            }
            Some(w) => {
                w.live = value;
                w.mode = mode;
                w.active = true;
                w.open(tick);
            }
            None => {
                let mut w = Written {
                    key,
                    mode,
                    before,
                    live: value,
                    active: true,
                    segments: Vec::new(),
                };
                w.open(tick);
                self.written.push(w);
            }
        }
    }

    /// The control was let go at `tick`. In touch mode, writing stops and the lane takes over
    /// again; in latch and write modes it carries on at the last value until the pass ends.
    pub fn release(&mut self, key: WriteKey, tick: u64) {
        if let Some(w) = self.find(key) {
            if w.active && w.mode == AutomationMode::Touch {
                w.close(tick);
                w.active = false;
            }
        }
    }

    /// Write mode at the start of a pass: write `key` from `tick` at `value` (the lane's value
    /// there), whether or not its control is touched.
    pub fn arm(&mut self, key: WriteKey, value: f64, tick: u64) {
        self.touch(key, AutomationMode::Write, value, tick, value);
    }

    /// A frame at `tick`. `jumped` says the playhead jumped (a loop wrap, a locate) since the
    /// last frame: every segment being written ends where it was and a new one starts here.
    pub fn advance(&mut self, tick: u64, jumped: bool) {
        let last = self.tick;
        for w in self.written.iter_mut().filter(|w| w.active) {
            if jumped {
                w.close(last);
                w.open(tick);
            } else if let Some(segment) = w.segments.last_mut() {
                segment.record(tick, w.live);
            }
        }
        self.tick = tick;
    }

    /// The value a control is being written at, while it is.
    pub fn live(&self, key: WriteKey) -> Option<f64> {
        self.written
            .iter()
            .find(|w| w.key == key && w.active)
            .map(|w| w.live)
    }

    /// Whether `target` (of any owner) is being written.
    pub fn writes(&self, target: AutomationTarget) -> bool {
        self.written
            .iter()
            .any(|w| w.key.target == target && w.active)
    }

    /// The tick of the last frame.
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// Whether `key` has been touched in this pass.
    pub fn has(&self, key: WriteKey) -> bool {
        self.written.iter().any(|w| w.key == key)
    }

    /// Whether anything was written in this pass.
    pub fn is_empty(&self) -> bool {
        self.written.is_empty()
    }

    /// The pass ended at `tick`: what each control wrote, its segments in time order, every
    /// segment's points thinned.
    pub fn finish(&mut self, tick: u64) -> Vec<Pass> {
        let mut out = Vec::new();
        for mut w in std::mem::take(&mut self.written) {
            if w.active {
                w.close(tick);
            }
            let tolerance = tolerance(&w.key.target);
            for segment in &mut w.segments {
                segment.points = thin(&segment.points, tolerance);
            }
            out.push(Pass {
                key: w.key,
                before: w.before,
                segments: w.segments,
            });
        }
        self.tick = 0;
        out
    }
}

/// How far a written point may be from the line between its neighbours and still go: well
/// below what anyone hears or sees.
fn tolerance(target: &AutomationTarget) -> f64 {
    match target {
        AutomationTarget::Volume | AutomationTarget::SendLevel { .. } => 0.05,
        AutomationTarget::Pan => 0.002,
        AutomationTarget::Mute => 0.0,
        AutomationTarget::Parameter { .. } => 0.001,
    }
}

/// Drop the points a straight line through their neighbours already gives (within
/// `tolerance`). A frame writes a point each time, so a held value leaves a run of equal points:
/// only its two ends stay, and so does every corner.
pub fn thin(points: &[(u64, f64)], tolerance: f64) -> Vec<(u64, f64)> {
    if points.len() <= 2 {
        return points.to_vec();
    }
    let mut kept = vec![points[0]];
    for i in 1..points.len() - 1 {
        let (a, p, b) = (*kept.last().expect("one kept"), points[i], points[i + 1]);
        let t = (p.0 - a.0) as f64 / (b.0 - a.0).max(1) as f64;
        let on_line = a.1 + (b.1 - a.1) * t;
        if (p.1 - on_line).abs() > tolerance {
            kept.push(p);
        }
    }
    kept.push(points[points.len() - 1]);
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use AutomationMode::{Latch, Read, Touch, Write};
    use AutomationTarget::{Mute, Pan, Volume};

    fn key(target: AutomationTarget) -> WriteKey {
        WriteKey {
            owner: Some(1),
            target,
        }
    }

    #[test]
    fn a_touch_writes_while_held_and_glides_back_when_let_go() {
        let mut w = Writer::default();
        w.advance(0, false);
        w.touch(key(Volume), Touch, -10.0, 100, 0.0);
        assert_eq!(w.live(key(Volume)), Some(-10.0));
        w.advance(132, false);
        w.touch(key(Volume), Touch, -8.0, 150, 0.0);
        w.advance(164, false);
        w.advance(196, false);
        w.release(key(Volume), 200);
        assert_eq!(w.live(key(Volume)), None, "the lane plays again");
        assert!(!w.writes(Volume));
        w.advance(228, false);
        let out = w.finish(400);
        assert_eq!(out.len(), 1);
        let segments = &out[0].segments;
        assert_eq!(segments.len(), 1);
        assert!(segments[0].glide);
        assert_eq!(
            segments[0].points,
            [(100, -10.0), (132, -10.0), (150, -8.0), (200, -8.0)],
            "held values keep their ends; nothing after the release"
        );
    }

    #[test]
    fn the_value_before_the_first_touch_is_kept() {
        let mut w = Writer::default();
        w.touch(key(Pan), Touch, 0.3, 10, -0.5);
        w.touch(key(Pan), Touch, 0.4, 20, 0.3);
        w.release(key(Pan), 30);
        w.touch(key(Pan), Touch, 0.9, 40, 0.4);
        let out = w.finish(50);
        assert_eq!(
            out[0].before, -0.5,
            "the setting before the pass first moved it"
        );
        assert_eq!(out[0].segments.len(), 2);
    }

    #[test]
    fn read_mode_writes_nothing() {
        let mut w = Writer::default();
        w.touch(key(Volume), Read, -3.0, 10, 0.0);
        assert!(w.is_empty());
        assert_eq!(w.live(key(Volume)), None);
    }

    #[test]
    fn latch_keeps_writing_the_last_value_until_the_end() {
        let mut w = Writer::default();
        w.touch(key(Pan), Latch, 0.5, 100, 0.0);
        w.release(key(Pan), 150);
        assert_eq!(
            w.live(key(Pan)),
            Some(0.5),
            "still written after letting go"
        );
        for tick in [200, 300, 400] {
            w.advance(tick, false);
        }
        let out = w.finish(500);
        assert_eq!(out[0].segments[0].points, [(100, 0.5), (500, 0.5)]);
    }

    #[test]
    fn write_mode_arms_lanes_flat_unless_touched() {
        let mut w = Writer::default();
        w.arm(key(Volume), -6.0, 0);
        w.arm(key(Mute), 0.0, 0);
        w.advance(100, false);
        w.touch(key(Volume), Write, -12.0, 120, 0.0);
        w.release(key(Volume), 130);
        assert_eq!(
            w.live(key(Volume)),
            Some(-12.0),
            "write mode doesn't stop on release"
        );
        w.advance(200, false);
        let out = w.finish(300);
        let points = |target| {
            out.iter()
                .find(|p| p.key.target == target)
                .map(|p| p.segments[0].points.clone())
                .unwrap()
        };
        assert_eq!(
            points(Mute),
            [(0, 0.0), (300, 0.0)],
            "untouched: flat over the pass"
        );
        assert_eq!(
            points(Volume),
            [(0, -6.0), (100, -6.0), (120, -12.0), (300, -12.0)]
        );
    }

    #[test]
    fn a_jump_ends_the_segment_and_starts_another() {
        let mut w = Writer::default();
        w.advance(1000, false);
        w.touch(key(Volume), Latch, -4.0, 1000, 0.0);
        w.advance(1900, false);
        // The loop wraps back to 200.
        w.advance(200, true);
        w.advance(500, false);
        let out = w.finish(600);
        let segments = &out[0].segments;
        assert_eq!(segments.len(), 2);
        assert_eq!((segments[0].start(), segments[0].end()), (1000, 1900));
        assert_eq!((segments[1].start(), segments[1].end()), (200, 600));
        assert!(segments.iter().all(|s| s.glide));
    }

    #[test]
    fn thinning_keeps_corners_and_the_ends_of_flat_runs() {
        let ramp: Vec<(u64, f64)> = (0..=10).map(|k| (k * 10, k as f64)).collect();
        assert_eq!(
            thin(&ramp, 0.01),
            [(0, 0.0), (100, 10.0)],
            "a straight ramp: its ends"
        );
        let held = [(0, 1.0), (10, 1.0), (20, 1.0), (30, 5.0), (40, 5.0)];
        assert_eq!(
            thin(&held, 0.01),
            [(0, 1.0), (20, 1.0), (30, 5.0), (40, 5.0)],
            "the hold ends where the change starts"
        );
        let step = [(0, 0.0), (10, 0.0), (20, 1.0), (30, 1.0)];
        assert_eq!(thin(&step, 0.0), step, "mute's steps stay");
    }
}
