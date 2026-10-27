//! Playing MIDI clips into a track's instrument, sample-accurately.
//!
//! A [`Sequence`] is built on the UI thread from a track's clips: every event in time order, in
//! ticks, cropped at clip ends. The graph node runs a [`SequencePlayer`] over it on the audio
//! thread. It turns ticks into samples with the chunk's [`Meter`]: the engine splits blocks
//! at tempo changes, so a chunk lies inside one tempo segment, and tempo edits need no
//! rebuild.
//!
//! Each event fires once, at the first sample at or after its exact time, from a cursor that
//! only moves forward while playback runs on: never lost or doubled where chunks split. A jump
//! (start, locate, loop wrap) or a stop releases everything this player holds (sustain first,
//! then notes) at the jump's first sample, then optionally re-triggers the notes the new
//! position falls inside ("chase").

use crate::graph::{Meter, Transport};
use crate::midi_clip::{Controller, MidiClip, TICKS_PER_QUARTER};
use vst3_host::{MidiChannel, MidiEvent};

/// At one tick: releases first (so a note can be re-struck on the tick it ends), then
/// controllers (a pedal or patch change applies to the notes that start there), then notes.
const RELEASE: u8 = 0;
const CONTROL: u8 = 1;
const STRIKE: u8 = 2;

#[derive(Debug, Clone, Copy)]
struct Timed {
    tick: u64,
    order: u8,
    event: MidiEvent,
}

/// A note as an interval, for chasing and for reconciling after a rebuild.
#[derive(Debug, Clone, Copy)]
struct Span {
    on: u64,
    off: u64,
    channel: MidiChannel,
    pitch: u8,
    velocity: u8,
}

/// A track's MIDI clips flattened for playback. Built on the UI thread.
#[derive(Debug, Default)]
pub struct Sequence {
    /// In time order (ties: releases, controllers, notes; then clip order).
    events: Vec<Timed>,
    /// In order of their start.
    spans: Vec<Span>,
    /// Re-trigger notes a jump lands inside.
    chase: bool,
}

fn channel(index: u8) -> MidiChannel {
    MidiChannel::from_index(index.min(15)).expect("16 MIDI channels")
}

impl Sequence {
    pub fn new(clips: &[MidiClip], chase: bool) -> Self {
        let mut events = Vec::new();
        let mut spans = Vec::new();
        for clip in clips {
            for note in clip.audible_notes() {
                let on = clip.start + note.tick;
                // Cropped at the clip's end, so a note can't outlive its clip.
                let off = clip.start + (note.tick + note.length).min(clip.length);
                let channel = channel(note.channel);
                events.push(Timed {
                    tick: on,
                    order: STRIKE,
                    event: MidiEvent::NoteOn {
                        channel,
                        note: note.pitch,
                        velocity: note.velocity,
                    },
                });
                events.push(Timed {
                    tick: off,
                    order: RELEASE,
                    event: MidiEvent::NoteOff {
                        channel,
                        note: note.pitch,
                        velocity: 0,
                    },
                });
                spans.push(Span {
                    on,
                    off,
                    channel,
                    pitch: note.pitch,
                    velocity: note.velocity,
                });
            }
            for e in clip.controllers.iter().filter(|e| e.tick < clip.length) {
                let channel = channel(e.channel);
                let event = match e.kind {
                    Controller::Cc { number, value } => MidiEvent::ControlChange {
                        channel,
                        controller: number,
                        value,
                    },
                    Controller::PitchBend { value } => MidiEvent::PitchBend {
                        channel,
                        value: (i32::from(value) + 8192).clamp(0, 16383) as u16,
                    },
                    Controller::ProgramChange { program } => {
                        MidiEvent::ProgramChange { channel, program }
                    }
                };
                events.push(Timed {
                    tick: clip.start + e.tick,
                    order: CONTROL,
                    event,
                });
            }
        }
        // Stable: equal keys keep clip order.
        events.sort_by_key(|t| (t.tick, t.order));
        spans.sort_by_key(|s| s.on);
        Self {
            events,
            spans,
            chase,
        }
    }
}

/// The exact sample of `tick` by this segment's tempo.
fn sample_of(meter: &Meter, tick: u64) -> f64 {
    meter.sample_at_quarter_notes(tick as f64 / f64::from(TICKS_PER_QUARTER))
}

/// The sample an event at `tick` fires on: the first at or after its exact time.
fn fire_at(meter: &Meter, tick: u64) -> i64 {
    sample_of(meter, tick).ceil() as i64
}

/// Plays a [`Sequence`] on the audio thread. Allocation-free.
#[derive(Debug, Default)]
pub struct SequencePlayer {
    sequence: Sequence,
    /// The next event to fire.
    cursor: usize,
    /// Notes this player started and hasn't released: a bit per pitch, per channel.
    sounding: [u128; 16],
    /// Channels where it holds the sustain pedal down.
    sustained: u16,
    /// Where the next chunk starts if playback runs on; anything else is a jump.
    next: Option<i64>,
    /// Handed over from the graph this one replaced: line the cursor up again, release what
    /// the new sequence no longer holds, and don't treat it as a jump.
    reseek: bool,
}

impl SequencePlayer {
    pub fn new(sequence: Sequence) -> Self {
        Self {
            sequence,
            ..Self::default()
        }
    }

    /// Continue from the player in the graph this one replaces: notes that are still in the
    /// sequence keep sounding, without a re-strike; the rest are released.
    pub fn take_state_from(&mut self, old: &mut SequencePlayer) {
        self.sounding = old.sounding;
        self.sustained = old.sustained;
        self.next = old.next;
        self.reseek = true;
        // The old graph won't release them now.
        old.sounding = [0; 16];
        old.sustained = 0;
    }

    /// One chunk of `frames` samples: hand each event that falls in it to `send` with its
    /// offset in the chunk, in time order.
    pub fn render(
        &mut self,
        transport: &Transport,
        frames: usize,
        send: &mut dyn FnMut(MidiEvent, usize),
    ) {
        if !transport.playing {
            if self.next.take().is_some() || self.reseek {
                self.release_all(send);
            }
            self.reseek = false;
            return;
        }
        let start = transport.position;
        let meter = &transport.meter;
        let jumped = self.next != Some(start);
        if jumped {
            self.release_all(send);
            self.seek(meter, start);
            if self.sequence.chase {
                self.chase(meter, start, send);
            }
        } else if self.reseek {
            self.seek(meter, start);
            self.release_stale(meter, start, send);
        }
        self.reseek = false;
        let end = start + frames as i64;
        while let Some(&timed) = self.sequence.events.get(self.cursor) {
            let at = fire_at(meter, timed.tick);
            if at >= end {
                break;
            }
            // Anything earlier than this chunk (it can't be, past rounding) plays at its
            // start rather than never.
            let offset = (at - start).max(0) as usize;
            self.track(timed.event);
            send(timed.event, offset);
            self.cursor += 1;
        }
        self.next = Some(end);
    }

    /// The first event at or after `start`.
    fn seek(&mut self, meter: &Meter, start: i64) {
        self.cursor = self
            .sequence
            .events
            .partition_point(|t| fire_at(meter, t.tick) < start);
    }

    /// Strike the notes `start` falls inside: begun before it, not yet ended. A note that
    /// starts exactly at `start` is the cursor's, not the chase's.
    fn chase(&mut self, meter: &Meter, start: i64, send: &mut dyn FnMut(MidiEvent, usize)) {
        for i in 0..self.sequence.spans.len() {
            let span = self.sequence.spans[i];
            if fire_at(meter, span.on) >= start {
                break;
            }
            if fire_at(meter, span.off) > start {
                let event = MidiEvent::NoteOn {
                    channel: span.channel,
                    note: span.pitch,
                    velocity: span.velocity,
                };
                self.track(event);
                send(event, 0);
            }
        }
    }

    /// After a handover: release sounding notes whose release isn't still ahead in the new
    /// sequence (deleted, moved or shortened notes), and keep the rest ringing.
    fn release_stale(&mut self, meter: &Meter, start: i64, send: &mut dyn FnMut(MidiEvent, usize)) {
        let mut pending = [0u128; 16];
        for span in &self.sequence.spans {
            if fire_at(meter, span.on) >= start {
                break;
            }
            if fire_at(meter, span.off) >= start {
                pending[usize::from(span.channel.as_index())] |= 1 << span.pitch;
            }
        }
        for (c, pending) in pending.iter().enumerate() {
            let stale = self.sounding[c] & !pending;
            self.release_notes(c, stale, send);
        }
    }

    /// Lift every sustain pedal this player pressed, then release every note it started.
    fn release_all(&mut self, send: &mut dyn FnMut(MidiEvent, usize)) {
        for c in 0..16u8 {
            if self.sustained & (1 << c) != 0 {
                send(
                    MidiEvent::ControlChange {
                        channel: channel(c),
                        controller: 64,
                        value: 0,
                    },
                    0,
                );
            }
        }
        self.sustained = 0;
        for c in 0..16 {
            let all = self.sounding[c];
            self.release_notes(c, all, send);
        }
    }

    fn release_notes(&mut self, c: usize, notes: u128, send: &mut dyn FnMut(MidiEvent, usize)) {
        if notes == 0 {
            return;
        }
        for pitch in 0..128u8 {
            if notes & (1 << pitch) != 0 {
                send(
                    MidiEvent::NoteOff {
                        channel: channel(c as u8),
                        note: pitch,
                        velocity: 0,
                    },
                    0,
                );
            }
        }
        self.sounding[c] &= !notes;
    }

    /// Keep `sounding` and `sustained` in step with what was sent.
    fn track(&mut self, event: MidiEvent) {
        match event {
            MidiEvent::NoteOn {
                channel,
                note,
                velocity,
            } if velocity > 0 => {
                self.sounding[usize::from(channel.as_index())] |= 1 << note;
            }
            MidiEvent::NoteOn { channel, note, .. } | MidiEvent::NoteOff { channel, note, .. } => {
                self.sounding[usize::from(channel.as_index())] &= !(1 << note);
            }
            MidiEvent::ControlChange {
                channel,
                controller: 64,
                value,
            } => {
                let bit = 1 << channel.as_index();
                if value >= 64 {
                    self.sustained |= bit;
                } else {
                    self.sustained &= !bit;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi_clip::{ControllerEvent, Note};

    /// 120 quarter notes a minute at 48 kHz: 24000 samples a quarter note, 25 a tick.
    fn meter() -> Meter {
        Meter::default()
    }

    fn note(id: u32, tick: u64, length: u64, pitch: u8) -> Note {
        Note {
            id,
            tick,
            length,
            pitch,
            velocity: 100,
            channel: 0,
        }
    }

    fn clip(start: u64, length: u64, notes: Vec<Note>) -> MidiClip {
        MidiClip {
            id: 1,
            name: "c".into(),
            start,
            length,
            notes,
            controllers: Vec::new(),
        }
    }

    /// What a player sent: (absolute sample, event).
    type Log = Vec<(i64, MidiEvent)>;

    /// Run chunks of `sizes` (cycled) from `from` to `to`, playing.
    fn play(
        player: &mut SequencePlayer,
        meter: Meter,
        from: i64,
        to: i64,
        sizes: &[usize],
        log: &mut Log,
    ) {
        let mut at = from;
        for &size in sizes.iter().cycle() {
            if at >= to {
                break;
            }
            let frames = size.min((to - at) as usize);
            chunk(player, meter, at, frames, true, log);
            at += frames as i64;
        }
    }

    fn chunk(
        player: &mut SequencePlayer,
        meter: Meter,
        at: i64,
        frames: usize,
        playing: bool,
        log: &mut Log,
    ) {
        let transport = Transport {
            playing,
            position: at,
            meter,
        };
        player.render(&transport, frames, &mut |event, offset| {
            assert!(
                offset < frames.max(1),
                "offset {offset} outside a {frames}-sample chunk"
            );
            log.push((at + offset as i64, event));
        });
    }

    fn on(pitch: u8) -> MidiEvent {
        MidiEvent::NoteOn {
            channel: channel(0),
            note: pitch,
            velocity: 100,
        }
    }

    fn off(pitch: u8) -> MidiEvent {
        MidiEvent::NoteOff {
            channel: channel(0),
            note: pitch,
            velocity: 0,
        }
    }

    #[test]
    fn every_event_fires_once_on_its_sample_however_blocks_split() {
        // Ticks 0, 1 and 959, and one ending on the tick a note starts at the same pitch.
        let notes = vec![
            note(1, 0, 960, 60),
            note(2, 1, 10, 64),
            note(3, 959, 2, 67),
            note(4, 960, 480, 60),
        ];
        let sequence = || Sequence::new(&[clip(0, 3840, notes.clone())], true);
        let mut expected: Log = vec![
            (0, on(60)),
            (25, on(64)),
            (275, off(64)),
            (23_975, on(67)),
            (24_000, off(60)),
            (24_000, on(60)),
            (24_025, off(67)),
            (36_000, off(60)),
        ];
        expected.sort_by_key(|(at, _)| *at);
        for sizes in [&[4096][..], &[1, 64, 333, 512, 7], &[23_975, 25, 1]] {
            let mut player = SequencePlayer::new(sequence());
            let mut log = Log::new();
            play(&mut player, meter(), 0, 48_000, sizes, &mut log);
            assert_eq!(log, expected, "chunks of {sizes:?}");
        }
    }

    #[test]
    fn ticks_turn_into_samples_by_each_chunks_tempo() {
        // 120 until sample 48000 (two quarter notes), then 60: a tick lasts 50 samples.
        let after = Meter {
            tempo: 60.0,
            origin_sample: 48_000.0,
            origin_quarter_notes: 2.0,
            ..meter()
        };
        let mut player = SequencePlayer::new(Sequence::new(
            &[clip(
                0,
                9600,
                vec![note(1, 1920, 10, 72), note(2, 2880, 1, 74)],
            )],
            false,
        ));
        let mut log = Log::new();
        play(&mut player, meter(), 0, 48_000, &[500], &mut log);
        play(&mut player, after, 48_000, 200_000, &[500], &mut log);
        // On the change exactly, then a quarter note (48000 samples at 60) later.
        assert_eq!(
            log,
            [
                (48_000, on(72)),
                (48_500, off(72)),
                (96_000, on(74)),
                (96_050, off(74))
            ]
        );
    }

    #[test]
    fn a_loop_wrap_releases_on_the_wrap_and_never_plays_the_loop_end() {
        // A two-quarter-note loop: note 60 fills the first half, 62 the second (ending on
        // the loop end), and 64 starts on the loop end.
        let notes = vec![
            note(1, 0, 960, 60),
            note(2, 960, 960, 62),
            note(3, 1920, 960, 64),
        ];
        let mut player = SequencePlayer::new(Sequence::new(&[clip(0, 3840, notes)], true));
        let mut log = Log::new();
        play(&mut player, meter(), 0, 48_000, &[700], &mut log);
        play(&mut player, meter(), 0, 48_000, &[700], &mut log);
        assert_eq!(
            log,
            [
                (0, on(60)),
                (24_000, off(60)),
                (24_000, on(62)),
                // The wrap: 62's release is at the loop end, the wrap's first sample.
                (0, off(62)),
                (0, on(60)),
                (24_000, off(60)),
                (24_000, on(62)),
            ]
        );
        assert!(
            log.iter().all(|(_, e)| *e != on(64)),
            "the loop end never plays"
        );
    }

    #[test]
    fn a_jump_chases_the_notes_it_lands_in_once() {
        let notes = vec![note(1, 0, 3840, 60), note(2, 960, 960, 64)];
        for chase in [true, false] {
            let mut player =
                SequencePlayer::new(Sequence::new(&[clip(0, 3840, notes.clone())], chase));
            let mut log = Log::new();
            // Locate into 60's middle, exactly where 64 starts.
            play(&mut player, meter(), 24_000, 30_000, &[512], &mut log);
            let mut want = vec![];
            if chase {
                want.push((24_000, on(60)));
            }
            want.push((24_000, on(64)));
            assert_eq!(log, want, "chase {chase}: 64 is the cursor's, struck once");
        }
    }

    #[test]
    fn stopping_lifts_the_sustain_pedal_then_releases_notes() {
        let mut c = clip(0, 3840, vec![note(1, 0, 3840, 60)]);
        c.controllers.push(ControllerEvent {
            id: 2,
            tick: 0,
            channel: 0,
            kind: Controller::Cc {
                number: 64,
                value: 127,
            },
        });
        let mut player = SequencePlayer::new(Sequence::new(&[c], true));
        let mut log = Log::new();
        play(&mut player, meter(), 0, 1000, &[512], &mut log);
        chunk(&mut player, meter(), 1000, 512, false, &mut log);
        chunk(&mut player, meter(), 1000, 512, false, &mut log);
        let pedal = |value| MidiEvent::ControlChange {
            channel: channel(0),
            controller: 64,
            value,
        };
        // Pedal down before the note at the same tick; on stop, pedal up, then note off,
        // once.
        assert_eq!(
            log,
            [
                (0, pedal(127)),
                (0, on(60)),
                (1000, pedal(0)),
                (1000, off(60))
            ]
        );
    }

    #[test]
    fn a_count_in_before_zero_plays_the_first_note_on_time_and_chases_nothing() {
        let mut player = SequencePlayer::new(Sequence::new(
            &[clip(0, 3840, vec![note(1, 0, 960, 60)])],
            true,
        ));
        let mut log = Log::new();
        play(&mut player, meter(), -48_000, 1000, &[480], &mut log);
        assert_eq!(log, [(0, on(60))]);
    }

    #[test]
    fn notes_are_cropped_at_their_clips_end() {
        // A clip from bar 2 cut to one quarter note: the long note stops at the clip end, and
        // the one past the end never plays.
        let c = clip(3840, 960, vec![note(1, 0, 9999, 60), note(2, 960, 10, 62)]);
        let mut player = SequencePlayer::new(Sequence::new(&[c], true));
        let mut log = Log::new();
        play(&mut player, meter(), 0, 200_000, &[1024], &mut log);
        assert_eq!(log, [(96_000, on(60)), (120_000, off(60))]);
    }

    /// What `clips` play from 0 through a tempo change: 120 until sample 48000 (tick 1920),
    /// then 60 (50 samples a tick), in uneven chunks.
    fn played_across_a_tempo_change(clips: &[MidiClip]) -> Log {
        let after = Meter {
            tempo: 60.0,
            origin_sample: 48_000.0,
            origin_quarter_notes: 2.0,
            ..meter()
        };
        let mut player = SequencePlayer::new(Sequence::new(clips, true));
        let mut log = Log::new();
        play(&mut player, meter(), 0, 48_000, &[333, 512], &mut log);
        play(&mut player, after, 48_000, 600_000, &[333, 512], &mut log);
        log
    }

    /// `whole` split at `cut` ticks (as S splits it) plays exactly what it played, but for
    /// `extras` at sample `at`: a crossing note's release and re-strike, the controllers
    /// re-sent at the second piece's start.
    fn split_plays_like_the_whole(whole: &MidiClip, cut: u64, at: i64, extras: &[MidiEvent]) {
        let mut first = whole.clone();
        let mut next = 1000;
        let second = first.split(cut, &mut || {
            next += 1;
            next
        });
        let want = played_across_a_tempo_change(std::slice::from_ref(whole));
        let mut got = played_across_a_tempo_change(&[first, second]);
        for extra in extras {
            let found = got
                .iter()
                .position(|(sample, e)| *sample == at && e == extra)
                .unwrap_or_else(|| panic!("{extra:?} at {at} in {got:?}"));
            got.remove(found);
        }
        assert_eq!(got, want, "cut at tick {cut}");
    }

    #[test]
    fn split_pieces_play_what_the_whole_clip_played() {
        let channel0 = channel(0);
        let mut whole = clip(
            0,
            9600,
            vec![
                note(1, 0, 1920, 64),   // ends on the tempo change
                note(2, 960, 1920, 60), // across it
                note(3, 1920, 960, 67), // starts on it
                note(4, 4800, 960, 72),
            ],
        );
        whole.controllers = vec![
            ControllerEvent {
                id: 5,
                tick: 100,
                channel: 0,
                kind: Controller::Cc {
                    number: 7,
                    value: 90,
                },
            },
            ControllerEvent {
                id: 6,
                tick: 500,
                channel: 0,
                kind: Controller::PitchBend { value: 1000 },
            },
        ];
        // On the tempo change (sample 48000): 60 is released and struck again, and the
        // volume and bend are set again to the values they already had.
        let cc7 = MidiEvent::ControlChange {
            channel: channel0,
            controller: 7,
            value: 90,
        };
        let bend = MidiEvent::PitchBend {
            channel: channel0,
            value: 9192,
        };
        split_plays_like_the_whole(&whole, 1920, 48_000, &[off(60), cc7, bend, on(60)]);
        // Where nothing sounds across the cut (tick 3840, sample 144000): only the controllers.
        split_plays_like_the_whole(&whole, 3840, 144_000, &[cc7, bend]);
        // With no controllers either, sample for sample the same.
        whole.controllers.clear();
        split_plays_like_the_whole(&whole, 3840, 144_000, &[]);
    }

    #[test]
    fn a_rebuild_keeps_notes_that_stay_and_releases_the_rest() {
        let both = vec![note(1, 0, 3840, 60), note(2, 0, 3840, 64)];
        let mut old = SequencePlayer::new(Sequence::new(&[clip(0, 3840, both)], true));
        let mut log = Log::new();
        play(&mut old, meter(), 0, 10_000, &[512], &mut log);
        // 64 deleted while both sound: the new player releases 64 only, without re-striking
        // 60, whose release comes on time.
        let mut new = SequencePlayer::new(Sequence::new(
            &[clip(0, 3840, vec![note(1, 0, 3840, 60)])],
            true,
        ));
        new.take_state_from(&mut old);
        log.clear();
        play(&mut new, meter(), 10_000, 100_000, &[512], &mut log);
        assert_eq!(log, [(10_000, off(64)), (96_000, off(60))]);
        // And the old player, if it ran on, releases nothing more.
        log.clear();
        chunk(&mut old, meter(), 10_000, 512, false, &mut log);
        assert!(log.is_empty());
    }
}
