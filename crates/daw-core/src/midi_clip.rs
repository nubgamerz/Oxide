//! MIDI clips: notes and controller events in musical time.
//!
//! Positions are MIDI ticks ([`TICKS_PER_QUARTER`] to a quarter note), so a clip and its notes
//! stay on their beats when the tempo changes; the tempo map
//! ([`crate::tempo::TempoMap::sample_at_tick`]) says where they fall in samples. A clip's
//! start counts from the timeline start, its events from the clip's start.
//!
//! Events past a clip's end are kept but don't play, so shortening a clip and lengthening it
//! again loses nothing.

use crate::project::ClipId;
pub use crate::tempo::TICKS_PER_QUARTER;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub type NoteId = u32;
pub type EventId = u32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MidiClip {
    /// Shares the namespace of audio clip ids.
    pub id: ClipId,
    pub name: String,
    /// Ticks from the timeline start.
    pub start: u64,
    /// In ticks; at least 1.
    pub length: u64,
    /// In time order (then pitch).
    #[serde(default)]
    pub notes: Vec<Note>,
    /// In time order.
    #[serde(default)]
    pub controllers: Vec<ControllerEvent>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Note {
    /// Unique in the project, so agents and the editor can name a note.
    pub id: NoteId,
    /// Ticks from the clip's start.
    pub tick: u64,
    /// In ticks; at least 1.
    pub length: u64,
    /// 0-127; 60 is middle C.
    pub pitch: u8,
    /// 1-127.
    pub velocity: u8,
    /// 0-15.
    pub channel: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ControllerEvent {
    /// Unique in the project.
    pub id: EventId,
    /// Ticks from the clip's start.
    pub tick: u64,
    /// 0-15.
    pub channel: u8,
    pub kind: Controller,
}

/// What a controller event sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Controller {
    /// A control change: `number` 0-127 (1 is the mod wheel, 64 the sustain pedal), `value`
    /// 0-127.
    Cc { number: u8, value: u8 },
    /// -8192 (down) to 8191 (up); 0 is centred.
    PitchBend { value: i16 },
    /// 0-127.
    ProgramChange { program: u8 },
}

impl MidiClip {
    pub fn end(&self) -> u64 {
        self.start + self.length
    }

    /// Restore time order after an edit, as playback and the editor expect.
    pub fn sort(&mut self) {
        self.notes.sort_by_key(|n| (n.tick, n.pitch, n.id));
        self.controllers.sort_by_key(|e| (e.tick, e.id));
    }

    pub fn note_mut(&mut self, id: NoteId) -> Option<&mut Note> {
        self.notes.iter_mut().find(|n| n.id == id)
    }

    /// The notes that play: those starting before the clip's end.
    pub fn audible_notes(&self) -> impl Iterator<Item = &Note> {
        self.notes.iter().filter(|n| n.tick < self.length)
    }

    /// Cut the clip `cut` ticks from its start (strictly inside it): this clip keeps the part
    /// before and the returned clip (its id from `new_id`, the same name) starts at the cut.
    ///
    /// - A note starts in one piece only: those from the cut on move to the second piece, ids
    ///   kept, and those that were past the end (kept but silent) stay past its end.
    /// - A note across the cut ends at the cut, and a copy (a new id; the same pitch, velocity
    ///   and channel) plays on from the second piece's start for the rest of its length: what
    ///   a split does in most DAWs, so either piece alone plays every note it
    ///   covers. Played together, the pieces strike such a note again at the cut.
    /// - Controller events from the cut on move to the second piece. Each controller (a CC
    ///   number, pitch bend, program, per channel) set before the cut is set again at the
    ///   second piece's start to its value in force there, unless an event at the cut sets it
    ///   anyway. The re-sent values come first at that tick, in the order they were set, then
    ///   the cut's own events (given new ids for that order), so a bank select still precedes
    ///   its program change.
    pub fn split(&mut self, cut: u64, new_id: &mut dyn FnMut() -> u32) -> MidiClip {
        debug_assert!(
            cut > 0 && cut < self.length,
            "a cut strictly inside the clip"
        );
        // "The last value before the cut" needs time order.
        self.sort();
        let mut second = MidiClip {
            id: new_id(),
            name: self.name.clone(),
            start: self.start + cut,
            length: self.length - cut,
            notes: Vec::new(),
            controllers: Vec::new(),
        };
        let mut first = Vec::with_capacity(self.notes.len());
        for note in std::mem::take(&mut self.notes) {
            let end = note.tick + note.length;
            if note.tick >= cut {
                second.notes.push(Note {
                    tick: note.tick - cut,
                    ..note
                });
            } else if end > cut {
                first.push(Note {
                    length: cut - note.tick,
                    ..note
                });
                second.notes.push(Note {
                    id: new_id(),
                    tick: 0,
                    length: end - cut,
                    ..note
                });
            } else {
                first.push(note);
            }
        }
        self.notes = first;
        let (before, after): (Vec<ControllerEvent>, Vec<ControllerEvent>) =
            std::mem::take(&mut self.controllers)
                .into_iter()
                .partition(|e| e.tick < cut);
        // The last event of each controller before the cut, in the order they were set.
        let mut in_force: Vec<ControllerEvent> = Vec::new();
        for event in &before {
            in_force.retain(|e| e.key() != event.key());
            in_force.push(*event);
        }
        in_force.sort_by_key(|e| (e.tick, e.id));
        let at_cut: Vec<ControllerEvent> =
            after.iter().filter(|e| e.tick == cut).copied().collect();
        for event in in_force {
            if at_cut.iter().all(|e| e.key() != event.key()) {
                second.controllers.push(ControllerEvent {
                    id: new_id(),
                    tick: 0,
                    ..event
                });
            }
        }
        let carried = !second.controllers.is_empty();
        for event in after {
            let id = if carried && event.tick == cut {
                new_id()
            } else {
                event.id
            };
            second.controllers.push(ControllerEvent {
                id,
                tick: event.tick - cut,
                ..event
            });
        }
        self.controllers = before;
        self.length = cut;
        self.sort();
        second.sort();
        second
    }
}

impl Note {
    /// Why the note's values are out of range, if they are.
    pub fn check(&self) -> Result<(), String> {
        if self.pitch > 127 {
            return Err(format!("pitch {} is outside 0..127", self.pitch));
        }
        if !(1..=127).contains(&self.velocity) {
            return Err(format!("velocity {} is outside 1..127", self.velocity));
        }
        check_channel(self.channel)?;
        if self.length == 0 {
            return Err("a note's length must be at least 1 tick".into());
        }
        Ok(())
    }
}

/// Which controller an event sets: a later event with the same key replaces its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ControllerKey {
    Cc(u8),
    PitchBend,
    Program,
}

impl ControllerEvent {
    fn key(&self) -> (u8, ControllerKey) {
        let key = match self.kind {
            Controller::Cc { number, .. } => ControllerKey::Cc(number),
            Controller::PitchBend { .. } => ControllerKey::PitchBend,
            Controller::ProgramChange { .. } => ControllerKey::Program,
        };
        (self.channel, key)
    }

    pub fn check(&self) -> Result<(), String> {
        check_channel(self.channel)?;
        match self.kind {
            Controller::Cc { number, value } => {
                if number > 127 {
                    return Err(format!("controller number {number} is outside 0..127"));
                }
                if value > 127 {
                    return Err(format!("controller value {value} is outside 0..127"));
                }
            }
            Controller::PitchBend { value } => {
                if !(-8192..=8191).contains(&value) {
                    return Err(format!("pitch bend {value} is outside -8192..8191"));
                }
            }
            Controller::ProgramChange { program } => {
                if program > 127 {
                    return Err(format!("program {program} is outside 0..127"));
                }
            }
        }
        Ok(())
    }
}

/// Which end of a note an edge drag (or `resize_notes`) moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NoteEdge {
    Start,
    End,
}

/// `notes` with one edge of each note in `ids` moved by `by` ticks (positive: later), as the
/// piano roll's edge drag does: the other edge stays where it is. A note keeps at least `min`
/// ticks (or its own length, if it was shorter) and never starts before the clip's start.
/// The rest are unchanged.
pub fn resize_notes(
    notes: &[Note],
    ids: &BTreeSet<NoteId>,
    edge: NoteEdge,
    by: i64,
    min: u64,
) -> Vec<Note> {
    notes
        .iter()
        .map(|n| {
            if !ids.contains(&n.id) {
                return *n;
            }
            let min = min.min(n.length).max(1);
            let end = n.tick + n.length;
            match edge {
                NoteEdge::End => Note {
                    length: (n.length as i64 + by).max(min as i64) as u64,
                    ..*n
                },
                NoteEdge::Start => {
                    let tick = (n.tick as i64 + by).clamp(0, (end - min) as i64) as u64;
                    Note {
                        tick,
                        length: end - tick,
                        ..*n
                    }
                }
            }
        })
        .collect()
}

fn check_channel(channel: u8) -> Result<(), String> {
    if channel > 15 {
        return Err(format!("MIDI channel {channel} is outside 0..15"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resizing_moves_one_edge_and_keeps_the_other() {
        let notes = vec![note(1, 480, 60), note(2, 960, 64), note(3, 0, 67)];
        let both: BTreeSet<NoteId> = [1, 2].into();
        let spans = |notes: Vec<Note>| -> Vec<(u64, u64)> {
            notes.iter().map(|n| (n.tick, n.tick + n.length)).collect()
        };
        let end = |by| spans(resize_notes(&notes, &both, NoteEdge::End, by, 1));
        let start = |by| spans(resize_notes(&notes, &both, NoteEdge::Start, by, 1));
        // Each 240 long: the end out and back in, then the start back and forward.
        assert_eq!(end(240), [(480, 960), (960, 1440), (0, 240)]);
        assert_eq!(end(-100), [(480, 620), (960, 1100), (0, 240)]);
        assert_eq!(
            end(-1000),
            [(480, 481), (960, 961), (0, 240)],
            "a tick at least"
        );
        assert_eq!(start(-240), [(240, 720), (720, 1200), (0, 240)]);
        assert_eq!(start(200), [(680, 720), (1160, 1200), (0, 240)]);
        assert_eq!(start(1000), [(719, 720), (1199, 1200), (0, 240)]);
        assert_eq!(start(-600)[0], (0, 720), "never before the clip's start");
        // A grid step as the least, unless the note was already shorter.
        let short = vec![
            Note {
                length: 60,
                ..note(1, 0, 60)
            },
            note(2, 960, 64),
        ];
        let sized = spans(resize_notes(&short, &both, NoteEdge::End, -500, 120));
        assert_eq!(sized, [(0, 60), (960, 1080)]);
    }

    fn note(id: NoteId, tick: u64, pitch: u8) -> Note {
        Note {
            id,
            tick,
            length: 240,
            pitch,
            velocity: 100,
            channel: 0,
        }
    }

    #[test]
    fn notes_sort_by_time_then_pitch_and_past_the_end_stay_silent() {
        let mut clip = MidiClip {
            id: 1,
            name: "c".into(),
            start: 0,
            length: 960,
            notes: vec![
                note(3, 480, 64),
                note(4, 0, 67),
                note(5, 0, 60),
                note(6, 1200, 72),
            ],
            controllers: Vec::new(),
        };
        clip.sort();
        let order: Vec<NoteId> = clip.notes.iter().map(|n| n.id).collect();
        assert_eq!(order, [5, 4, 3, 6]);
        let audible: Vec<NoteId> = clip.audible_notes().map(|n| n.id).collect();
        assert_eq!(
            audible,
            [5, 4, 3],
            "the note past the end is kept but silent"
        );
        clip.length = 1920;
        assert_eq!(
            clip.audible_notes().count(),
            4,
            "lengthening brings it back"
        );
    }

    fn cc(id: EventId, tick: u64, number: u8, value: u8) -> ControllerEvent {
        ControllerEvent {
            id,
            tick,
            channel: 0,
            kind: Controller::Cc { number, value },
        }
    }

    /// Ids from 100 on, as the project would hand them out.
    fn ids() -> impl FnMut() -> u32 {
        let mut next = 100;
        move || {
            next += 1;
            next
        }
    }

    #[test]
    fn a_split_shortens_a_note_across_the_cut_and_starts_the_second_piece_with_the_rest() {
        let mut clip = MidiClip {
            id: 1,
            name: "Chords".into(),
            start: 1920,
            length: 3840,
            notes: vec![
                // Across the cut (960): 480..1440.
                Note {
                    length: 960,
                    ..note(2, 480, 60)
                },
                // Ending exactly at the cut: stays whole on the left.
                Note {
                    length: 480,
                    ..note(3, 480, 64)
                },
                // Starting exactly at the cut: the right piece's, at its tick 0.
                note(4, 960, 67),
                // Later, and one past the clip's end (kept, silent): moved, still past the end.
                note(5, 2000, 72),
                note(6, 4000, 74),
            ],
            controllers: Vec::new(),
        };
        let mut new_id = ids();
        let second = clip.split(960, &mut new_id);
        assert_eq!((clip.id, clip.start, clip.length), (1, 1920, 960));
        assert_eq!((second.id, second.start, second.length), (101, 2880, 2880));
        assert_eq!(second.name, "Chords", "both pieces keep the name");
        let spans = |c: &MidiClip| -> Vec<(NoteId, u64, u64, u8)> {
            c.notes
                .iter()
                .map(|n| (n.id, n.tick, n.length, n.pitch))
                .collect()
        };
        assert_eq!(spans(&clip), [(2, 480, 480, 60), (3, 480, 480, 64)]);
        assert_eq!(
            spans(&second),
            [
                (102, 0, 480, 60),
                (4, 0, 240, 67),
                (5, 1040, 240, 72),
                (6, 3040, 240, 74)
            ]
        );
        let copy = second.notes[0];
        assert_eq!((copy.velocity, copy.channel), (100, 0), "the same note");
        assert_eq!(
            second.audible_notes().count(),
            3,
            "the note past the end stays silent"
        );
    }

    #[test]
    fn a_split_starts_the_second_piece_with_each_controller_in_force() {
        let event = |id, tick, channel, kind| ControllerEvent {
            id,
            tick,
            channel,
            kind,
        };
        let mut clip = MidiClip {
            id: 1,
            name: "c".into(),
            start: 0,
            length: 3840,
            notes: Vec::new(),
            controllers: vec![
                cc(10, 0, 7, 100),
                cc(11, 100, 7, 90), // the volume in force at the cut
                cc(12, 200, 64, 127),
                event(13, 300, 0, Controller::PitchBend { value: 2000 }),
                event(14, 310, 1, Controller::PitchBend { value: -500 }),
                cc(15, 400, 0, 2), // a bank select, then its program
                event(16, 410, 0, Controller::ProgramChange { program: 5 }),
                // At the cut: the pedal is set there anyway, so it isn't carried.
                cc(17, 960, 64, 0),
                cc(18, 1500, 1, 64),
            ],
        };
        let second = clip.split(960, &mut ids());
        let left: Vec<EventId> = clip.controllers.iter().map(|e| e.id).collect();
        assert_eq!(left, [10, 11, 12, 13, 14, 15, 16]);
        let right: Vec<(EventId, u64, u8, Controller)> = second
            .controllers
            .iter()
            .map(|e| (e.id, e.tick, e.channel, e.kind))
            .collect();
        let cc_kind = |number, value| Controller::Cc { number, value };
        assert_eq!(
            right,
            [
                // Carried, in the order they were set; the cut's own event after them.
                (102, 0, 0, cc_kind(7, 90)),
                (103, 0, 0, Controller::PitchBend { value: 2000 }),
                (104, 0, 1, Controller::PitchBend { value: -500 }),
                (105, 0, 0, cc_kind(0, 2)),
                (106, 0, 0, Controller::ProgramChange { program: 5 }),
                (107, 0, 0, cc_kind(64, 0)),
                (18, 540, 0, cc_kind(1, 64)),
            ]
        );
    }

    #[test]
    fn values_are_checked() {
        let good = note(1, 0, 60);
        assert!(good.check().is_ok());
        for (bad, says) in [
            (Note { pitch: 128, ..good }, "pitch"),
            (
                Note {
                    velocity: 0,
                    ..good
                },
                "velocity",
            ),
            (
                Note {
                    channel: 16,
                    ..good
                },
                "channel",
            ),
            (Note { length: 0, ..good }, "length"),
        ] {
            assert!(bad.check().unwrap_err().contains(says));
        }
        let event = |kind| ControllerEvent {
            id: 1,
            tick: 0,
            channel: 0,
            kind,
        };
        assert!(event(Controller::PitchBend { value: 8191 }).check().is_ok());
        assert!(event(Controller::PitchBend { value: 8192 })
            .check()
            .is_err());
        assert!(event(Controller::Cc {
            number: 128,
            value: 0
        })
        .check()
        .is_err());
        assert!(event(Controller::ProgramChange { program: 200 })
            .check()
            .is_err());
    }
}
