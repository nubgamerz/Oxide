//! Turning MIDI that arrived while recording into a MIDI clip's notes and controllers.
//!
//! The engine stamps each event with the heard position ([`crate::engine::RecordedMidi`]);
//! the session takes off the device's output latency and folds loop wraps, and hands this
//! module each track's events, in the order they arrived, at their timeline positions.

use crate::grid::{Division, Rounding};
use crate::midi_clip::{Controller, ControllerEvent, Note};
use crate::tempo::TempoMap;
use vst3_host::MidiEvent;

/// A recorded event at its timeline position, in samples.
#[derive(Debug, Clone, Copy)]
pub struct MidiHit {
    pub at: i64,
    pub event: MidiEvent,
}

/// What one track's MIDI recording becomes: a span and its events, in ticks.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiTake {
    /// Ticks from the timeline start.
    pub start: u64,
    pub length: u64,
    /// Ticks from `start`; ids not yet given.
    pub notes: Vec<Note>,
    pub controllers: Vec<ControllerEvent>,
}

/// Build a take from `hits`, in the order they arrived, inside `range` (samples, the recorded
/// window: everything outside is dropped).
///
/// - A note runs from its note-on to the next note-off (or velocity-0 note-on) of its pitch
///   and channel. A pitch struck again before its release ends the earlier note there.
/// - A release that comes before its note (held over a loop wrap, so it lands at the start
///   of the loop) ends the note at the end of `range`, as does no release at all.
/// - A release with no note before it is ignored.
/// - `quantize` snaps note starts to the grid and keeps their lengths.
///
/// Controllers (CC, pitch bend, program change) are kept; aftertouch isn't in the model.
pub fn take(
    hits: &[MidiHit],
    range: (u64, u64),
    map: &TempoMap,
    quantize: Option<Division>,
) -> Option<MidiTake> {
    let tick = |sample: f64| map.tick_at(sample).round().max(0.0) as u64;
    let start = tick(range.0 as f64);
    let end = tick(range.1 as f64).max(start + 1);
    let inside = |at: i64| at >= range.0 as i64 && at < range.1 as i64;
    let place = |on: u64| match quantize {
        Some(division) => map.snap_tick(on, division, Rounding::Nearest),
        None => on,
    };

    let mut open: Vec<Option<(u64, u8)>> = vec![None; 16 * 128];
    let mut notes = Vec::new();
    let mut close = |slot: &mut Option<(u64, u8)>, off: u64, channel: u8, pitch: u8| {
        if let Some((on, velocity)) = slot.take() {
            // Released before it started: held over a loop wrap.
            let off = if off < on { end } else { off };
            let at = place(on);
            notes.push(Note {
                id: 0,
                tick: at.saturating_sub(start),
                length: off.saturating_sub(on).max(1),
                pitch,
                velocity,
                channel,
            });
        }
    };
    let mut controllers = Vec::new();
    for hit in hits.iter().filter(|h| inside(h.at)) {
        let at = tick(hit.at as f64);
        match hit.event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } if velocity > 0 => {
                let c = channel.as_index();
                let slot = &mut open[usize::from(c) * 128 + usize::from(note)];
                close(slot, at, c, note);
                *slot = Some((at, velocity));
            }
            MidiEvent::NoteOn { channel, note, .. } | MidiEvent::NoteOff { channel, note, .. } => {
                let c = channel.as_index();
                close(
                    &mut open[usize::from(c) * 128 + usize::from(note)],
                    at,
                    c,
                    note,
                );
            }
            MidiEvent::ControlChange {
                channel,
                controller,
                value,
            } => controllers.push((
                at,
                channel.as_index(),
                Controller::Cc {
                    number: controller,
                    value,
                },
            )),
            MidiEvent::PitchBend { channel, value } => controllers.push((
                at,
                channel.as_index(),
                Controller::PitchBend {
                    value: (i32::from(value) - 8192) as i16,
                },
            )),
            MidiEvent::ProgramChange { channel, program } => controllers.push((
                at,
                channel.as_index(),
                Controller::ProgramChange { program },
            )),
            _ => {}
        }
    }
    // Still held at the end of the window.
    for (i, slot) in open.iter_mut().enumerate() {
        close(slot, end, (i / 128) as u8, (i % 128) as u8);
    }
    let controllers: Vec<ControllerEvent> = controllers
        .into_iter()
        .map(|(at, channel, kind)| ControllerEvent {
            id: 0,
            tick: at.saturating_sub(start),
            channel,
            kind,
        })
        .collect();
    if notes.is_empty() && controllers.is_empty() {
        return None;
    }
    Some(MidiTake {
        start,
        length: end - start,
        notes,
        controllers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use vst3_host::MidiChannel;

    /// 120 at 48 kHz: 25 samples a tick.
    fn map() -> TempoMap {
        TempoMap::constant(48_000, 120.0, 4, 4)
    }

    fn on(at: i64, pitch: u8) -> MidiHit {
        MidiHit {
            at,
            event: MidiEvent::NoteOn {
                channel: MidiChannel::Ch1,
                note: pitch,
                velocity: 99,
            },
        }
    }

    fn off(at: i64, pitch: u8) -> MidiHit {
        MidiHit {
            at,
            event: MidiEvent::NoteOff {
                channel: MidiChannel::Ch1,
                note: pitch,
                velocity: 0,
            },
        }
    }

    /// (tick from the take's start, length, pitch) of each note.
    fn notes(take: &MidiTake) -> Vec<(u64, u64, u8)> {
        let mut n: Vec<_> = take
            .notes
            .iter()
            .map(|n| (n.tick, n.length, n.pitch))
            .collect();
        n.sort();
        n
    }

    #[test]
    fn notes_pair_up_inside_the_window_and_hanging_ones_end_with_it() {
        // Window 24000..120000: ticks 960..4800.
        let hits = [
            on(10_000, 50),  // before the window (a count-in): dropped
            off(24_100, 50), // its release, with no note: ignored
            on(25_000, 60),
            off(49_000, 60),
            on(50_000, 64), // never released
            on(60_000, 67),
            on(72_000, 67), // struck again: ends the first 67
            off(84_000, 67),
            on(130_000, 72), // after the window
        ];
        let t = take(&hits, (24_000, 120_000), &map(), None).unwrap();
        assert_eq!((t.start, t.length), (960, 3840));
        assert_eq!(
            notes(&t),
            [
                (40, 960, 60),
                (1040, 2800, 64),
                (1440, 480, 67),
                (1920, 480, 67)
            ]
        );
    }

    #[test]
    fn a_note_held_over_a_loop_wrap_ends_at_the_loop_end() {
        // A loop of 0..96000 (3840 ticks). The note starts near the end of one pass; its
        // release arrives early in the next, at an earlier position.
        let hits = [
            on(90_000, 60),
            off(2_000, 60),
            on(10_000, 62),
            off(20_000, 62),
        ];
        let t = take(&hits, (0, 96_000), &map(), None).unwrap();
        assert_eq!(notes(&t), [(400, 400, 62), (3600, 240, 60)]);
    }

    #[test]
    fn quantize_snaps_starts_and_keeps_lengths() {
        // Slightly off the sixteenths (240 ticks = 6000 samples).
        let hits = [
            on(6_100, 60),
            off(9_100, 60),
            on(11_800, 62),
            off(12_800, 62),
        ];
        let t = take(&hits, (0, 48_000), &map(), Some("1/16".parse().unwrap())).unwrap();
        assert_eq!(notes(&t), [(240, 120, 60), (480, 40, 62)]);
    }

    #[test]
    fn controllers_are_kept_and_an_empty_window_makes_nothing() {
        let bend = MidiHit {
            at: 2_500,
            event: MidiEvent::PitchBend {
                channel: MidiChannel::Ch2,
                value: 0,
            },
        };
        let t = take(&[bend], (0, 48_000), &map(), None).unwrap();
        assert_eq!(t.controllers.len(), 1);
        assert_eq!(t.controllers[0].tick, 100);
        assert_eq!(t.controllers[0].channel, 1);
        assert_eq!(
            t.controllers[0].kind,
            Controller::PitchBend { value: -8192 }
        );
        assert!(take(&[on(50_000, 60)], (0, 48_000), &map(), None).is_none());
    }
}
