//! Standard MIDI files: reading them into MIDI clips (and the tempo map they ask for), and
//! writing a clip out. Through `midly`.
//!
//! Positions come in at the file's resolution and are rescaled to [`TICKS_PER_QUARTER`]
//! (exactly when it divides evenly, rounded otherwise).

use crate::midi_clip::{Controller, ControllerEvent, MidiClip, Note, TICKS_PER_QUARTER};
use crate::project::TempoChange;
use anyhow::{bail, Context, Result};
use midly::num::{u15, u24, u28, u4, u7};
use midly::{
    Format, Header, MetaMessage, MidiMessage, PitchBend, Smf, Timing, TrackEvent, TrackEventKind,
};
use std::path::Path;

/// Whether `bytes` start like a Standard MIDI file (`MThd`), whatever the file is called.
pub fn is_midi_file(bytes: &[u8]) -> bool {
    bytes.starts_with(b"MThd")
}

/// A MIDI file, read.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiFile {
    /// Quarter notes per minute from these ticks on, in order.
    pub tempos: Vec<(u64, f64)>,
    /// Time signatures from these ticks on, in order.
    pub meters: Vec<(u64, (u32, u32))>,
    /// The file's tracks that hold notes or controller events, in file order.
    pub tracks: Vec<MidiFileTrack>,
}

/// One track of a MIDI file, ready to become a clip.
#[derive(Debug, Clone, PartialEq)]
pub struct MidiFileTrack {
    pub name: Option<String>,
    /// Ticks from the file's start; ids not yet given.
    pub notes: Vec<Note>,
    pub controllers: Vec<ControllerEvent>,
    /// The track's last event.
    pub end: u64,
}

/// The tempo map a file asks for, in the project's terms: the opening tempo and meter, and
/// later changes in bars and beats from the file's start.
#[derive(Debug, Clone, PartialEq)]
pub struct TempoSettings {
    pub tempo: f64,
    pub time_signature: (u32, u32),
    pub changes: Vec<TempoChange>,
    /// Changes moved to fit: a meter change onto its nearest bar line, a tempo change onto
    /// its nearest beat.
    pub moved: usize,
}

pub fn read_path(path: &Path) -> Result<MidiFile> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    read(&bytes).with_context(|| format!("reading {}", path.display()))
}

/// Read a type 0 or type 1 file. Notes pair first in, first out per pitch and channel (a
/// velocity-0 note-on is a release); a note never released ends at its track's last event.
/// CC, pitch bend and program changes are kept; aftertouch and SysEx aren't.
pub fn read(bytes: &[u8]) -> Result<MidiFile> {
    if !is_midi_file(bytes) {
        bail!("not a MIDI file (no MThd header)");
    }
    let smf = Smf::parse(bytes).context("not a readable MIDI file")?;
    let ppq = match smf.header.timing {
        Timing::Metrical(ppq) => u64::from(ppq.as_int()).max(1),
        Timing::Timecode(..) => bail!("MIDI files timed in SMPTE frames aren't supported"),
    };
    if smf.header.format == Format::Sequential {
        bail!("type 2 (sequential) MIDI files aren't supported");
    }
    let q = u64::from(TICKS_PER_QUARTER);
    let scale = |tick: u64| (tick * q + ppq / 2) / ppq;
    let mut tempos = Vec::new();
    let mut meters = Vec::new();
    let mut tracks = Vec::new();
    for events in &smf.tracks {
        let mut at = 0u64;
        let mut name = None;
        let mut open: Vec<Vec<(u64, u8)>> = vec![Vec::new(); 16 * 128];
        let mut notes = Vec::new();
        let mut controllers = Vec::new();
        for event in events {
            at += u64::from(event.delta.as_int());
            let tick = scale(at);
            match event.kind {
                TrackEventKind::Meta(MetaMessage::Tempo(us)) => {
                    // Files store whole microseconds a quarter note: 140 comes back as
                    // 140.00014. To a thousandth of a beat a minute, it's 140 again.
                    let us = f64::from(us.as_int().max(1));
                    tempos.push((tick, (60e6 / us * 1000.0).round() / 1000.0));
                }
                TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, _, _)) => {
                    meters.push((tick, (u32::from(n.max(1)), 1u32 << d.min(6))));
                }
                TrackEventKind::Meta(MetaMessage::TrackName(bytes)) => {
                    let text = String::from_utf8_lossy(bytes).trim().to_string();
                    if !text.is_empty() {
                        name = Some(text);
                    }
                }
                TrackEventKind::Midi { channel, message } => {
                    let channel = channel.as_int();
                    let slot = |key: u7| usize::from(channel) * 128 + usize::from(key.as_int());
                    match message {
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            open[slot(key)].push((tick, vel.as_int()));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            let list = &mut open[slot(key)];
                            if !list.is_empty() {
                                let (on, velocity) = list.remove(0);
                                notes.push(Note {
                                    id: 0,
                                    tick: on,
                                    length: tick.saturating_sub(on).max(1),
                                    pitch: key.as_int(),
                                    velocity,
                                    channel,
                                });
                            }
                        }
                        MidiMessage::Controller { controller, value } => {
                            controllers.push(ControllerEvent {
                                id: 0,
                                tick,
                                channel,
                                kind: Controller::Cc {
                                    number: controller.as_int(),
                                    value: value.as_int(),
                                },
                            });
                        }
                        MidiMessage::PitchBend { bend } => controllers.push(ControllerEvent {
                            id: 0,
                            tick,
                            channel,
                            kind: Controller::PitchBend {
                                value: bend.as_int(),
                            },
                        }),
                        MidiMessage::ProgramChange { program } => {
                            controllers.push(ControllerEvent {
                                id: 0,
                                tick,
                                channel,
                                kind: Controller::ProgramChange {
                                    program: program.as_int(),
                                },
                            })
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        let end = scale(at);
        // Never released: to the track's end.
        for (slot, held) in open.iter().enumerate() {
            for &(on, velocity) in held {
                notes.push(Note {
                    id: 0,
                    tick: on,
                    length: end.saturating_sub(on).max(1),
                    pitch: (slot % 128) as u8,
                    velocity,
                    channel: (slot / 128) as u8,
                });
            }
        }
        if !notes.is_empty() || !controllers.is_empty() {
            notes.sort_by_key(|n| (n.tick, n.pitch));
            tracks.push(MidiFileTrack {
                name,
                notes,
                controllers,
                end,
            });
        }
    }
    // Type 1 files usually keep these on a first "conductor" track, but needn't.
    tempos.sort_by_key(|t| t.0);
    meters.sort_by_key(|m| m.0);
    Ok(MidiFile {
        tempos,
        meters,
        tracks,
    })
}

impl MidiFile {
    /// The tempo map the file asks for, in bars and beats from its start (`None` when it sets
    /// neither a tempo nor a time signature). A meter change goes on its nearest bar line and
    /// a tempo change on its nearest beat, as the project's tempo map needs; `moved` counts
    /// those that weren't there already.
    pub fn tempo_settings(&self) -> Option<TempoSettings> {
        if self.tempos.is_empty() && self.meters.is_empty() {
            return None;
        }
        let q = u64::from(TICKS_PER_QUARTER);
        let at_zero = |list: &[(u64, f64)]| list.iter().find(|t| t.0 == 0).map(|t| t.1);
        let tempo = at_zero(&self.tempos).unwrap_or(120.0);
        let time_signature = self
            .meters
            .iter()
            .find(|m| m.0 == 0)
            .map_or((4, 4), |m| m.1);
        // Walk the meters to place each change in bars and beats.
        let mut changes: Vec<TempoChange> = Vec::new();
        let mut moved = 0;
        // (start tick, bar it starts, meter)
        let mut segments: Vec<(u64, u32, (u32, u32))> = vec![(0, 0, time_signature)];
        for &(tick, meter) in self.meters.iter().filter(|m| m.0 > 0) {
            let (start, bar, (n, d)) = *segments.last().expect("never empty");
            let bar_ticks = u64::from(n) * 4 * q / u64::from(d.max(1));
            let bars = ((tick - start) as f64 / bar_ticks as f64).round() as u64;
            if (tick - start) % bar_ticks != 0 {
                moved += 1;
            }
            let bar = bar + bars as u32;
            let at = start + bars * bar_ticks;
            if bars == 0 {
                // Rounded onto the segment's own start: it replaces that meter.
                segments.last_mut().expect("never empty").2 = meter;
            } else {
                segments.push((at, bar, meter));
            }
            changes.push(TempoChange {
                bar,
                beat: 0,
                tempo: None,
                time_signature: Some(meter),
            });
        }
        for &(tick, bpm) in self.tempos.iter().filter(|t| t.0 > 0) {
            let &(start, bar, (n, d)) = segments
                .iter()
                .rev()
                .find(|s| s.0 <= tick)
                .expect("the first starts at 0");
            let beat_ticks = 4 * q / u64::from(d.max(1));
            let beats = ((tick - start) as f64 / beat_ticks as f64).round() as u64;
            if (tick - start) % beat_ticks != 0 {
                moved += 1;
            }
            let (bars, beat) = (beats / u64::from(n), (beats % u64::from(n)) as u32);
            let bar = bar + bars as u32;
            match changes.iter_mut().find(|c| (c.bar, c.beat) == (bar, beat)) {
                Some(change) => change.tempo = Some(bpm),
                None => changes.push(TempoChange {
                    bar,
                    beat,
                    tempo: Some(bpm),
                    time_signature: None,
                }),
            }
        }
        changes.sort_by_key(|c| (c.bar, c.beat));
        Some(TempoSettings {
            tempo,
            time_signature,
            changes,
            moved,
        })
    }

    /// A name for each track, in order, that tells them apart: its own (or `fallback`, the
    /// file's name, for a track without one) when no other track shares it. Notation programs
    /// write one track per staff under the instrument's name (Guitar Pro's piano: two
    /// "Acoustic Piano" tracks), so a pair that shares one is told apart by register,
    /// "(treble)" and "(bass)", when their notes sit either side of middle C; any other
    /// tracks that share a name are numbered in file order.
    pub fn track_names(&self, fallback: &str) -> Vec<String> {
        let base: Vec<&str> = self
            .tracks
            .iter()
            .map(|t| t.name.as_deref().unwrap_or(fallback))
            .collect();
        let mut names: Vec<String> = base.iter().map(|b| b.to_string()).collect();
        for (k, name) in base.iter().enumerate() {
            let same: Vec<usize> = (0..base.len()).filter(|&j| base[j] == *name).collect();
            if same.len() < 2 || same[0] != k {
                continue;
            }
            let medians: Vec<Option<u8>> = same
                .iter()
                .map(|&j| median_pitch(&self.tracks[j].notes))
                .collect();
            if let [Some(a), Some(b)] = medians[..] {
                if (a >= MIDDLE_C) != (b >= MIDDLE_C) {
                    for (&j, m) in same.iter().zip([a, b]) {
                        let part = if m >= MIDDLE_C { "treble" } else { "bass" };
                        names[j] = format!("{name} ({part})");
                    }
                    continue;
                }
            }
            for (n, &j) in same.iter().enumerate() {
                names[j] = format!("{name} {}", n + 1);
            }
        }
        names
    }
}

/// MIDI note 60, the line between a grand staff's treble and bass.
const MIDDLE_C: u8 = 60;

/// The middle pitch of `notes` (the upper of the two middles for an even count).
fn median_pitch(notes: &[Note]) -> Option<u8> {
    let mut pitches: Vec<u8> = notes.iter().map(|n| n.pitch).collect();
    pitches.sort_unstable();
    pitches.get(pitches.len() / 2).copied()
}

/// A MIDI clip as a type 0 file at [`TICKS_PER_QUARTER`]: what plays (notes cropped at the
/// clip's end, as playback does), with the tempo and time signature at the clip's start.
pub fn write_clip(clip: &MidiClip, name: &str, tempo: f64, meter: (u32, u32)) -> Result<Vec<u8>> {
    let at_zero = MidiClip {
        start: 0,
        ..clip.clone()
    };
    write_track(&[at_zero], name, &[(0, tempo, meter)])
}

/// A tempo and time signature from a tick of the file on: `(tick, quarter notes a minute,
/// (numerator, denominator))`.
pub type TempoAt = (u64, f64, (u32, u32));

/// MIDI clips as one type 0 file at [`TICKS_PER_QUARTER`], each at its place in the song (tick
/// 0 is the timeline's start): what plays (notes cropped at their clip's end, as playback
/// does) and their controller events, with `tempo`'s changes (the first at tick 0), so another
/// program places every note on the same bar and beat. Overlapping clips both play.
pub fn write_track(clips: &[MidiClip], name: &str, tempo: &[TempoAt]) -> Result<Vec<u8>> {
    let mut timed: Vec<(u64, u8, TrackEventKind)> = Vec::new();
    timed.push((
        0,
        0,
        TrackEventKind::Meta(MetaMessage::TrackName(name.as_bytes())),
    ));
    let mut meter_was = None;
    for &(tick, bpm, meter) in tempo {
        let us = (60e6 / bpm.max(1.0))
            .round()
            .clamp(1.0, f64::from(0xFF_FFFF)) as u32;
        timed.push((
            tick,
            0,
            TrackEventKind::Meta(MetaMessage::Tempo(u24::new(us))),
        ));
        if meter_was != Some(meter) {
            let d = meter.1.max(1).trailing_zeros().min(6) as u8;
            timed.push((
                tick,
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(
                    meter.0.clamp(1, 255) as u8,
                    d,
                    24,
                    8,
                )),
            ));
            meter_was = Some(meter);
        }
    }
    let midi = |channel: u8, message| TrackEventKind::Midi {
        channel: u4::new(channel.min(15)),
        message,
    };
    let mut end = 0;
    for clip in clips {
        end = end.max(clip.start + clip.length);
        for note in clip.audible_notes() {
            let off = (note.tick + note.length).min(clip.length);
            let key = u7::new(note.pitch.min(127));
            timed.push((
                clip.start + note.tick,
                3,
                midi(
                    note.channel,
                    MidiMessage::NoteOn {
                        key,
                        vel: u7::new(note.velocity.clamp(1, 127)),
                    },
                ),
            ));
            timed.push((
                clip.start + off,
                1,
                midi(
                    note.channel,
                    MidiMessage::NoteOff {
                        key,
                        vel: u7::new(0),
                    },
                ),
            ));
        }
        for e in clip.controllers.iter().filter(|e| e.tick < clip.length) {
            let message = match e.kind {
                Controller::Cc { number, value } => MidiMessage::Controller {
                    controller: u7::new(number.min(127)),
                    value: u7::new(value.min(127)),
                },
                Controller::PitchBend { value } => MidiMessage::PitchBend {
                    bend: PitchBend::from_int(value),
                },
                Controller::ProgramChange { program } => MidiMessage::ProgramChange {
                    program: u7::new(program.min(127)),
                },
            };
            timed.push((clip.start + e.tick, 2, midi(e.channel, message)));
        }
    }
    // In time; at one tick: metas, releases, controllers, then notes (as playback orders them).
    timed.sort_by_key(|t| (t.0, t.1));
    let mut events = Vec::with_capacity(timed.len() + 1);
    let mut last = 0;
    for (tick, _, kind) in timed {
        events.push(TrackEvent {
            delta: u28::new((tick - last) as u32),
            kind,
        });
        last = tick;
    }
    let end = end.max(last);
    events.push(TrackEvent {
        delta: u28::new((end - last) as u32),
        kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
    });
    let mut smf = Smf::new(Header::new(
        Format::SingleTrack,
        Timing::Metrical(u15::new(TICKS_PER_QUARTER as u16)),
    ));
    smf.tracks.push(events);
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).context("writing the MIDI file")?;
    Ok(bytes)
}

/// Files written by [`write_track`] (or any at [`TICKS_PER_QUARTER`]) as one type 1 file, their
/// tracks in order, as a notation program exports a piece with several parts.
pub fn join(files: &[&[u8]]) -> Result<Vec<u8>> {
    let mut smf = Smf::new(Header::new(
        Format::Parallel,
        Timing::Metrical(u15::new(TICKS_PER_QUARTER as u16)),
    ));
    for bytes in files {
        let file = Smf::parse(bytes).context("not a readable MIDI file")?;
        if file.header.timing != smf.header.timing {
            bail!("only files at {TICKS_PER_QUARTER} ticks a quarter note can be joined");
        }
        smf.tracks.extend(file.tracks);
    }
    let mut bytes = Vec::new();
    smf.write_std(&mut bytes).context("writing the MIDI file")?;
    Ok(bytes)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A file at `ppq`: a conductor track (tempo 140 then 70 at the file's bar 3, 4/4 then
    /// 3/4 at bar 2), and two note tracks.
    pub(crate) fn sample_file(ppq: u16) -> Vec<u8> {
        let p = u32::from(ppq);
        let ev = |delta: u32, kind| TrackEvent {
            delta: u28::new(delta),
            kind,
        };
        let tempo =
            |bpm: f64| TrackEventKind::Meta(MetaMessage::Tempo(u24::new((60e6 / bpm) as u32)));
        let meter = |n: u8, d: u8| TrackEventKind::Meta(MetaMessage::TimeSignature(n, d, 24, 8));
        let note = |on: bool, key: u8, vel: u8| TrackEventKind::Midi {
            channel: u4::new(0),
            message: if on {
                MidiMessage::NoteOn {
                    key: u7::new(key),
                    vel: u7::new(vel),
                }
            } else {
                MidiMessage::NoteOff {
                    key: u7::new(key),
                    vel: u7::new(0),
                }
            },
        };
        let end = TrackEventKind::Meta(MetaMessage::EndOfTrack);
        let conductor = vec![
            ev(0, tempo(140.0)),
            ev(0, meter(4, 2)),
            // Bar 2 (4 quarters in): 3/4.
            ev(4 * p, meter(3, 2)),
            // Bar 3 (3 quarters later): 70.
            ev(3 * p, tempo(70.0)),
            ev(0, end),
        ];
        let lead = vec![
            ev(0, TrackEventKind::Meta(MetaMessage::TrackName(b"Lead"))),
            ev(0, note(true, 60, 100)),
            ev(p, note(false, 60, 0)),
            ev(p / 2, note(true, 64, 80)),
            // A velocity-0 note-on releases.
            ev(p / 2, note(true, 64, 0)),
            ev(0, end),
        ];
        let bass = vec![
            ev(0, note(true, 36, 90)),
            // Never released: ends with the track.
            ev(8 * p, end),
        ];
        let mut smf = Smf::new(Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(ppq)),
        ));
        smf.tracks = vec![conductor, lead, bass];
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }

    /// A piano as notation programs export it (Guitar Pro): 90 BPM in 4/4, and one track per
    /// staff, both named "Acoustic Piano": a treble melody (quarter notes from E4 up, one per
    /// beat for two bars) and a bass line (half notes from C2, two a bar), at `ppq`.
    pub(crate) fn staves_file(ppq: u16) -> Vec<u8> {
        let p = u32::from(ppq);
        let ev = |delta: u32, kind| TrackEvent {
            delta: u28::new(delta),
            kind,
        };
        let key = |on: bool, key: u8| TrackEventKind::Midi {
            channel: u4::new(0),
            message: MidiMessage::NoteOn {
                key: u7::new(key),
                vel: u7::new(if on { 90 } else { 0 }),
            },
        };
        let end = TrackEventKind::Meta(MetaMessage::EndOfTrack);
        let name = TrackEventKind::Meta(MetaMessage::TrackName(b"Acoustic Piano"));
        let conductor = vec![
            ev(
                0,
                TrackEventKind::Meta(MetaMessage::Tempo(u24::new(60_000_000 / 90))),
            ),
            ev(
                0,
                TrackEventKind::Meta(MetaMessage::TimeSignature(4, 2, 24, 8)),
            ),
            ev(0, end),
        ];
        let staff = |pitches: &[u8], length: u32| {
            let mut events = vec![ev(0, name)];
            for &pitch in pitches {
                events.push(ev(0, key(true, pitch)));
                events.push(ev(length, key(false, pitch)));
            }
            events.push(ev(0, end));
            events
        };
        let treble = staff(&[64, 65, 67, 69, 71, 72, 74, 76], p);
        let bass = staff(&[36, 43, 41, 48], 2 * p);
        let mut smf = Smf::new(Header::new(
            Format::Parallel,
            Timing::Metrical(u15::new(ppq)),
        ));
        smf.tracks = vec![conductor, treble, bass];
        let mut bytes = Vec::new();
        smf.write_std(&mut bytes).unwrap();
        bytes
    }

    #[test]
    fn tracks_that_share_a_name_are_told_apart() {
        let file = read(&staves_file(480)).unwrap();
        assert_eq!(
            file.track_names("piano"),
            ["Acoustic Piano (treble)", "Acoustic Piano (bass)"]
        );
        // Unique names stay; a track without one takes the file's.
        let file = read(&sample_file(480)).unwrap();
        assert_eq!(file.track_names("song"), ["Lead", "song"]);
        // Three of one name, or two on the same side of middle C: numbered in file order.
        let track = |name: &str, pitch: u8| MidiFileTrack {
            name: Some(name.into()),
            notes: vec![Note {
                id: 0,
                tick: 0,
                length: 1,
                pitch,
                velocity: 100,
                channel: 0,
            }],
            controllers: Vec::new(),
            end: 1,
        };
        let file = MidiFile {
            tempos: Vec::new(),
            meters: Vec::new(),
            tracks: vec![
                track("Gtr", 40),
                track("Gtr", 52),
                track("Bass", 30),
                track("Gtr", 70),
                track("Keys", 72),
                track("Keys", 80),
            ],
        };
        assert_eq!(
            file.track_names("x"),
            ["Gtr 1", "Gtr 2", "Bass", "Gtr 3", "Keys 1", "Keys 2"]
        );
    }

    #[test]
    fn joined_files_keep_each_track() {
        let staves = read(&staves_file(960)).unwrap();
        let one = |k: usize| {
            let t = &staves.tracks[k];
            let clip = MidiClip {
                id: 0,
                name: String::new(),
                start: 0,
                length: t.end,
                notes: t.notes.clone(),
                controllers: Vec::new(),
            };
            write_clip(&clip, "Acoustic Piano", 90.0, (4, 4)).unwrap()
        };
        let joined = read(&join(&[&one(0), &one(1)]).unwrap()).unwrap();
        assert_eq!(joined.tracks, staves.tracks);
        assert_eq!(joined.tempo_settings().unwrap().tempo, 90.0);
        assert!(join(&[&staves_file(480)]).is_err(), "another resolution");
    }

    #[test]
    fn files_read_into_exact_ticks_at_any_resolution() {
        for ppq in [96, 480, 960] {
            let file = read(&sample_file(ppq)).unwrap();
            assert_eq!(file.tracks.len(), 2, "the conductor holds no notes");
            let lead = &file.tracks[0];
            assert_eq!(lead.name.as_deref(), Some("Lead"));
            let notes: Vec<(u64, u64, u8, u8)> = lead
                .notes
                .iter()
                .map(|n| (n.tick, n.length, n.pitch, n.velocity))
                .collect();
            assert_eq!(notes, [(0, 960, 60, 100), (1440, 480, 64, 80)], "ppq {ppq}");
            assert_eq!(
                (file.tracks[1].notes[0].length, file.tracks[1].end),
                (7680, 7680)
            );
            assert_eq!(file.tempos, [(0, 140.0), (6720, 70.0)]);
            assert_eq!(file.meters, [(0, (4, 4)), (3840, (3, 4))]);
        }
        assert!(read(b"RIFF....")
            .unwrap_err()
            .to_string()
            .contains("not a MIDI file"));
    }

    #[test]
    fn the_files_tempo_map_becomes_bars_and_beats() {
        let settings = read(&sample_file(480)).unwrap().tempo_settings().unwrap();
        assert_eq!((settings.tempo, settings.time_signature), (140.0, (4, 4)));
        // 3/4 from bar 2 (index 1), 70 from bar 3 (index 2): 4 + 3 quarter notes in.
        assert_eq!(
            settings.changes,
            [
                TempoChange {
                    bar: 1,
                    beat: 0,
                    tempo: None,
                    time_signature: Some((3, 4))
                },
                TempoChange {
                    bar: 2,
                    beat: 0,
                    tempo: Some(70.0),
                    time_signature: None
                },
            ]
        );
        assert_eq!(settings.moved, 0);
    }

    #[test]
    fn a_clip_writes_what_plays_and_reads_back_the_same() {
        let clip = MidiClip {
            id: 1,
            name: "c".into(),
            start: 3840,
            length: 3840,
            notes: vec![
                Note {
                    id: 1,
                    tick: 0,
                    length: 480,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
                Note {
                    id: 2,
                    tick: 480,
                    length: 240,
                    pitch: 67,
                    velocity: 64,
                    channel: 2,
                },
                // Past the end: doesn't play, isn't written.
                Note {
                    id: 3,
                    tick: 4000,
                    length: 100,
                    pitch: 72,
                    velocity: 90,
                    channel: 0,
                },
            ],
            controllers: vec![
                ControllerEvent {
                    id: 4,
                    tick: 0,
                    channel: 0,
                    kind: Controller::Cc {
                        number: 64,
                        value: 127,
                    },
                },
                ControllerEvent {
                    id: 5,
                    tick: 960,
                    channel: 2,
                    kind: Controller::PitchBend { value: -4096 },
                },
                ControllerEvent {
                    id: 6,
                    tick: 1200,
                    channel: 0,
                    kind: Controller::ProgramChange { program: 5 },
                },
            ],
        };
        let bytes = write_clip(&clip, "Keys", 96.0, (6, 8)).unwrap();
        let back = read(&bytes).unwrap();
        assert_eq!(back.tempos, [(0, 96.0)]);
        assert_eq!(back.meters, [(0, (6, 8))]);
        let track = &back.tracks[0];
        assert_eq!(track.name.as_deref(), Some("Keys"));
        let strip = |n: &Note| (n.tick, n.length, n.pitch, n.velocity, n.channel);
        assert_eq!(
            track.notes.iter().map(strip).collect::<Vec<_>>(),
            clip.notes[..2].iter().map(strip).collect::<Vec<_>>()
        );
        let kinds = |c: &[ControllerEvent]| -> Vec<(u64, u8, Controller)> {
            c.iter().map(|e| (e.tick, e.channel, e.kind)).collect()
        };
        assert_eq!(kinds(&track.controllers), kinds(&clip.controllers));
        assert_eq!(track.end, 3840, "the file runs to the clip's end");
    }
}
