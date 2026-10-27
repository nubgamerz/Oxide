//! A musical reading of MIDI notes, for agents writing parts (dev plan 17.6): note names
//! (C4 = 60), song positions as bar:beat:tick through the tempo map, the key a part is in and
//! the chords it outlines, bar by bar.
//!
//! Pure: it reads notes at song ticks and a [`TempoMap`] and changes nothing, so the
//! `describe_midi_clip` action and the tests see the same reading.
//!
//! - **Key:** the Krumhansl–Schmuckler estimate. The notes make a pitch-class profile
//!   (each note weighted by its length, capped at a whole note so one held pedal can't
//!   outvote a line, and half as much again when it starts a bar, where tonics sit), which
//!   is correlated (Pearson) with the Krumhansl–Kessler probe-tone profiles turned to each of
//!   the 24 keys. A part using only a scale's notes fits its major key and the relative
//!   minor almost alike; which wins depends on the tonic's weight, so the runner-up is given
//!   with its score.
//! - **Chords:** template matching over pitch classes weighted by how long they sound
//!   (major, minor, diminished and augmented triads; dominant, major, minor, half-diminished
//!   and diminished sevenths). Each bar is tried whole, in halves and beat by beat, equal
//!   neighbours merged, and the cheapest reading kept: the cost is the weight the chord
//!   leaves out plus small penalties for each chord tone not heard, an inversion, a tone
//!   outside the key, a seventh (it must be heard to be named) and each extra chord in the
//!   bar. So a bass line playing roots reads as the key's triad on each root ("implied": its
//!   other tones are missing), an arpeggio as its one chord, and a bar of two block chords as
//!   both. "N" when nothing fits half the weight, or the bar is silent.

use crate::midi_clip::{MidiClip, Note};
use crate::tempo::{TempoMap, TICKS_PER_QUARTER};
use serde::Serialize;

const SHARP_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];
const FLAT_NAMES: [&str; 12] = [
    "C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B",
];

/// A pitch class's name: "C#" (sharps) or "Db" (flats) for 1.
pub fn pitch_class_name(pc: u8, flats: bool) -> &'static str {
    let names = if flats { &FLAT_NAMES } else { &SHARP_NAMES };
    names[usize::from(pc % 12)]
}

/// A note's name with its octave, C4 = 60 (middle C), C-1 = 0: "A#3" or "Bb3" for 58.
pub fn note_name(pitch: u8, flats: bool) -> String {
    format!(
        "{}{}",
        pitch_class_name(pitch % 12, flats),
        i32::from(pitch / 12) - 1
    )
}

/// The MIDI note number of a name: a letter A to G (either case), up to two `#` or `b`
/// (`♯`, `♭` too), then the octave (C4 = 60, C-1 = 0, G9 = 127). The error names `text`.
pub fn parse_note_name(text: &str) -> Result<u8, String> {
    let bad = || {
        format!(
            "\"{text}\" is not a note name: a letter A to G, then # or b if any, then the \
             octave, as in \"C4\" (60, middle C), \"A#3\" or \"Bb3\" (58), \"C-1\" (0)"
        )
    };
    let mut chars = text.trim().chars();
    let base: i32 = match chars.next().map(|c| c.to_ascii_uppercase()) {
        Some('C') => 0,
        Some('D') => 2,
        Some('E') => 4,
        Some('F') => 5,
        Some('G') => 7,
        Some('A') => 9,
        Some('B') => 11,
        _ => return Err(bad()),
    };
    let mut rest = chars.as_str();
    let mut shift = 0;
    for _ in 0..2 {
        let step = match rest.chars().next() {
            Some(c @ ('#' | '♯')) => (1, c.len_utf8()),
            Some(c @ ('b' | '♭')) => (-1, c.len_utf8()),
            _ => break,
        };
        shift += step.0;
        rest = &rest[step.1..];
    }
    if rest.is_empty() {
        return Err(format!(
            "\"{text}\" has no octave: add one, as in \"{text}4\" (C4 = 60, middle C)"
        ));
    }
    let octave: i32 = rest.parse().map_err(|_| bad())?;
    let pitch = (octave + 1) * 12 + base + shift;
    if !(0..=127).contains(&pitch) {
        return Err(format!(
            "\"{text}\" is note {pitch}, outside 0..127 (C-1 to G9)"
        ));
    }
    Ok(pitch as u8)
}

/// Song tick `tick` as "bar:beat:tick" (bars and beats from 1, ticks into the beat, 960 to
/// a quarter note): "2:3:240". Through the tempo map, so bars follow its meter changes.
pub fn format_position(map: &TempoMap, tick: u64) -> String {
    let (bar, beat, into) = map.bar_beat_tick(tick);
    format!("{}:{}:{into:03}", bar + 1, beat + 1)
}

/// The song tick of "bar:beat:tick" (or "bar:beat", or "bar"; `.` separates too, as the
/// app's clock shows it): bars and beats from 1, the beat one of its bar's (4/4 has 4, 6/8
/// has 6 eighths), the ticks within the beat. The error names `text`.
pub fn parse_position(map: &TempoMap, text: &str) -> Result<u64, String> {
    let parts: Vec<&str> = text.trim().split([':', '.']).collect();
    let number = |part: &str, what: &str| {
        part.trim().parse::<u64>().map_err(|_| {
            format!(
                "\"{text}\" is not a position: give \"bar:beat:tick\" (from 1, as in \
                 \"5:1:000\", \"5:3:480\") or \"bar:beat\" ({what} \"{part}\" is not a number)"
            )
        })
    };
    if parts.is_empty() || parts.len() > 3 {
        return Err(format!(
            "\"{text}\" is not a position: give \"bar:beat:tick\" (from 1, as in \"5:1:000\")"
        ));
    }
    let bar = number(parts[0], "the bar")?;
    let beat = parts.get(1).map_or(Ok(1), |p| number(p, "the beat"))?;
    let tick = parts.get(2).map_or(Ok(0), |p| number(p, "the tick"))?;
    if bar == 0 || beat == 0 {
        return Err(format!("\"{text}\": bars and beats count from 1"));
    }
    let bar = i64::try_from(bar - 1).map_err(|_| format!("\"{text}\": bar {bar} is too far"))?;
    let (numerator, denominator) = map.meter_of_bar(bar);
    if beat > u64::from(numerator) {
        return Err(format!(
            "\"{text}\": bar {} is in {numerator}/{denominator}, with beats 1 to \
             {numerator}; beat {beat} is not one of them",
            bar + 1
        ));
    }
    let beat_ticks = map.beat_ticks(bar);
    if tick >= beat_ticks {
        return Err(format!(
            "\"{text}\": a beat of bar {} lasts {beat_ticks} ticks (0 to {}); tick {tick} \
             is past it",
            bar + 1,
            beat_ticks - 1
        ));
    }
    Ok(map.tick_of_bar(bar, beat as u32 - 1) + tick)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Major,
    Minor,
}

/// A key: its tonic's pitch class (0 = C) and mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    pub tonic: u8,
    pub mode: Mode,
}

impl Key {
    pub const fn new(tonic: u8, mode: Mode) -> Self {
        Self { tonic, mode }
    }

    /// Whether the key is written with flats (F, Bb, Eb, Ab, Db major; D, G, C, F, Bb, Eb
    /// minor). C major and A minor have neither and read with sharps; of the keys six
    /// sharps or flats from C, F# major and Eb minor are the ones used.
    pub fn flats(self) -> bool {
        match self.mode {
            Mode::Major => matches!(self.tonic, 1 | 3 | 5 | 8 | 10),
            Mode::Minor => matches!(self.tonic, 0 | 2 | 3 | 5 | 7 | 10),
        }
    }

    /// "A minor", "Bb major".
    pub fn name(self) -> String {
        let mode = match self.mode {
            Mode::Major => "major",
            Mode::Minor => "minor",
        };
        format!("{} {mode}", pitch_class_name(self.tonic, self.flats()))
    }

    /// The scale's pitch classes from the tonic (natural minor for minor keys).
    pub fn scale(self) -> [u8; 7] {
        let steps = match self.mode {
            Mode::Major => [0, 2, 4, 5, 7, 9, 11],
            Mode::Minor => [0, 2, 3, 5, 7, 8, 10],
        };
        steps.map(|s| (self.tonic + s) % 12)
    }

    /// The key signature: "no sharps or flats", "1 sharp", "3 flats".
    pub fn signature(self) -> String {
        // The relative major's place on the circle of fifths: sharps to the right.
        let major = match self.mode {
            Mode::Major => self.tonic,
            Mode::Minor => (self.tonic + 3) % 12,
        };
        let sharps = (u32::from(major) * 7) % 12;
        let (count, kind) = if self.flats() {
            (12 - sharps, "flat")
        } else {
            (sharps, "sharp")
        };
        match count {
            0 => "no sharps or flats".into(),
            1 => format!("1 {kind}"),
            n => format!("{n} {kind}s"),
        }
    }

    /// Whether pitch class `pc` belongs to the key: its scale, and in minor the raised
    /// seventh too (the leading tone of harmonic minor, as in E major's G# in A minor).
    pub fn contains(self, pc: u8) -> bool {
        self.scale().contains(&(pc % 12))
            || (self.mode == Mode::Minor && pc % 12 == (self.tonic + 11) % 12)
    }
}

/// Krumhansl and Kessler's probe-tone ratings (1982, "Tracing the dynamic changes in
/// perceived tonal organization…"), from the tonic up by semitones.
const MAJOR_PROFILE: [f64; 12] = [
    6.35, 2.23, 3.48, 2.33, 4.38, 4.09, 2.52, 5.19, 2.39, 3.66, 2.29, 2.88,
];
const MINOR_PROFILE: [f64; 12] = [
    6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17,
];

/// A key and how well the notes fit it: the correlation, -1 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyFit {
    pub key: Key,
    pub score: f64,
}

/// The best key and the runner-up.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyEstimate {
    pub best: KeyFit,
    pub runner_up: KeyFit,
}

fn correlation(a: &[f64; 12], b: &[f64; 12]) -> f64 {
    let (ma, mb) = (a.iter().sum::<f64>() / 12.0, b.iter().sum::<f64>() / 12.0);
    let (mut ab, mut aa, mut bb) = (0.0, 0.0, 0.0);
    for (x, y) in a.iter().zip(b) {
        ab += (x - ma) * (y - mb);
        aa += (x - ma) * (x - ma);
        bb += (y - mb) * (y - mb);
    }
    if aa <= 0.0 || bb <= 0.0 {
        return 0.0;
    }
    ab / (aa * bb).sqrt()
}

/// Every key, best first, for a pitch-class profile (`weights[pc]`, any scale). `None` when
/// the profile says nothing: silent, or every pitch class alike.
pub fn rank_keys(weights: &[f64; 12]) -> Option<Vec<KeyFit>> {
    let total: f64 = weights.iter().sum();
    let spread = weights.iter().any(|&w| (w - total / 12.0).abs() > 1e-9);
    if total <= 0.0 || !spread {
        return None;
    }
    let mut fits: Vec<KeyFit> = [Mode::Major, Mode::Minor]
        .into_iter()
        .flat_map(|mode| (0..12).map(move |tonic| Key::new(tonic, mode)))
        .map(|key| {
            let profile = match key.mode {
                Mode::Major => &MAJOR_PROFILE,
                Mode::Minor => &MINOR_PROFILE,
            };
            let turned: [f64; 12] =
                std::array::from_fn(|pc| profile[(pc + 12 - usize::from(key.tonic)) % 12]);
            KeyFit {
                key,
                score: correlation(weights, &turned),
            }
        })
        .collect();
    // Stable: equal scores keep major before minor, C upwards.
    fits.sort_by(|a, b| b.score.total_cmp(&a.score));
    Some(fits)
}

/// The best key for a pitch-class profile and the runner-up ([`rank_keys`]).
pub fn estimate_key(weights: &[f64; 12]) -> Option<KeyEstimate> {
    let fits = rank_keys(weights)?;
    Some(KeyEstimate {
        best: fits[0],
        runner_up: fits[1],
    })
}

/// A note as it sounds in the song: from tick `start` to `end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sounding {
    pub start: u64,
    pub end: u64,
    pub pitch: u8,
}

impl Sounding {
    /// A clip's notes that play, at song ticks, cut at its end.
    pub fn of_clip(clip: &MidiClip) -> Vec<Self> {
        clip.audible_notes()
            .map(|n| Self {
                start: clip.start + n.tick,
                end: clip.start + (n.tick + n.length).min(clip.length),
                pitch: n.pitch,
            })
            .collect()
    }
}

/// How many ticks of a whole note count at most toward a note's weight in the key profile.
const KEY_WEIGHT_CAP: u64 = 4 * TICKS_PER_QUARTER as u64;

/// The key profile of `notes`: each note's length in quarter notes (at most a whole note),
/// half as much again when it starts a bar.
pub fn key_weights(notes: &[Sounding], map: &TempoMap) -> [f64; 12] {
    let mut weights = [0.0; 12];
    for n in notes {
        let length =
            n.end.saturating_sub(n.start).min(KEY_WEIGHT_CAP) as f64 / f64::from(TICKS_PER_QUARTER);
        let (_, beat, into) = map.bar_beat_tick(n.start);
        let downbeat = if beat == 0 && into == 0 { 1.5 } else { 1.0 };
        weights[usize::from(n.pitch % 12)] += length * downbeat;
    }
    weights
}

/// A chord's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    Major,
    Minor,
    Diminished,
    Augmented,
    Dominant7,
    Major7,
    Minor7,
    HalfDiminished7,
    Diminished7,
}

const QUALITIES: [Quality; 9] = [
    Quality::Major,
    Quality::Minor,
    Quality::Diminished,
    Quality::Augmented,
    Quality::Dominant7,
    Quality::Major7,
    Quality::Minor7,
    Quality::HalfDiminished7,
    Quality::Diminished7,
];

impl Quality {
    /// Semitones above the root.
    pub fn intervals(self) -> &'static [u8] {
        match self {
            Self::Major => &[0, 4, 7],
            Self::Minor => &[0, 3, 7],
            Self::Diminished => &[0, 3, 6],
            Self::Augmented => &[0, 4, 8],
            Self::Dominant7 => &[0, 4, 7, 10],
            Self::Major7 => &[0, 4, 7, 11],
            Self::Minor7 => &[0, 3, 7, 10],
            Self::HalfDiminished7 => &[0, 3, 6, 10],
            Self::Diminished7 => &[0, 3, 6, 9],
        }
    }

    /// What follows the root in a chord symbol: "" (C), "m", "dim", "aug", "7", "maj7",
    /// "m7", "m7b5", "dim7".
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Major => "",
            Self::Minor => "m",
            Self::Diminished => "dim",
            Self::Augmented => "aug",
            Self::Dominant7 => "7",
            Self::Major7 => "maj7",
            Self::Minor7 => "m7",
            Self::HalfDiminished7 => "m7b5",
            Self::Diminished7 => "dim7",
        }
    }

    fn is_seventh(self) -> bool {
        self.intervals().len() == 4
    }
}

/// A chord: its root and bass pitch classes and its quality.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chord {
    pub root: u8,
    pub quality: Quality,
    /// The lowest note's pitch class; a slash chord when it isn't the root.
    pub bass: u8,
}

impl Chord {
    /// Its pitch classes, root first.
    pub fn tones(self) -> Vec<u8> {
        self.quality
            .intervals()
            .iter()
            .map(|i| (self.root + i) % 12)
            .collect()
    }

    /// "Am", "C/E", "Bb7".
    pub fn name(self, flats: bool) -> String {
        let mut name = format!(
            "{}{}",
            pitch_class_name(self.root, flats),
            self.quality.suffix()
        );
        if self.bass != self.root {
            name.push('/');
            name.push_str(pitch_class_name(self.bass, flats));
        }
        name
    }
}

/// A stretch of a bar and the chord heard there (`None`: "N", nothing fits or silence).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChordSpan {
    /// Song ticks.
    pub start: u64,
    pub end: u64,
    /// The bar it is in, from 0.
    pub bar: i64,
    pub chord: Option<Chord>,
    /// Some of the chord's tones are not heard: it is read from the notes there are and the
    /// key (a bass line's roots).
    pub implied: bool,
}

// The chord reading's costs: weight left out counts as a fraction of the bar's; the rest are
// constants per chord read, so finer readings must earn their extra chords.
const MISSING_TONE: f64 = 0.05;
const INVERSION: f64 = 0.03;
const OUTSIDE_KEY: f64 = 0.02;
const SEVENTH: f64 = 0.08;
const RARE_TRIAD: f64 = 0.015;
const EXTRA_CHORD: f64 = 0.04;
/// Below this share of a stretch's weight in its chord, the stretch reads "N".
const MIN_FIT: f64 = 0.5;

/// The best chord for one stretch and its cost (for a bar of weight `bar_weight`).
struct Reading {
    chord: Option<Chord>,
    implied: bool,
    cost: f64,
}

fn read_stretch(
    notes: &[Sounding],
    from: u64,
    to: u64,
    bar_weight: f64,
    key: Option<Key>,
) -> Reading {
    let mut weights = [0.0f64; 12];
    for n in notes {
        let overlap = n.end.min(to).saturating_sub(n.start.max(from));
        weights[usize::from(n.pitch % 12)] += overlap as f64;
    }
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return Reading {
            chord: None,
            implied: false,
            cost: 0.0,
        };
    }
    // The bass: the lowest note sounding as the stretch starts, else the lowest of the first
    // to start in it.
    let sounding = notes
        .iter()
        .filter(|n| n.start <= from && n.end > from)
        .map(|n| n.pitch)
        .min();
    let bass = sounding
        .or_else(|| {
            let first = notes
                .iter()
                .filter(|n| n.start >= from && n.start < to)
                .map(|n| n.start)
                .min()?;
            notes
                .iter()
                .filter(|n| n.start == first)
                .map(|n| n.pitch)
                .min()
        })
        .unwrap_or(0)
        % 12;
    let mut best: Option<(f64, Chord, f64, bool)> = None;
    for quality in QUALITIES {
        for root in 0..12u8 {
            let chord = Chord {
                root,
                quality,
                bass,
            };
            let tones = chord.tones();
            let inside: f64 = tones.iter().map(|&pc| weights[usize::from(pc)]).sum();
            let missing = tones
                .iter()
                .filter(|&&pc| weights[usize::from(pc)] <= 0.0)
                .count();
            let mut cost = (total - inside) / bar_weight + missing as f64 * MISSING_TONE;
            if bass != root {
                cost += INVERSION;
            }
            if key.is_some_and(|k| tones.iter().any(|&pc| !k.contains(pc))) {
                cost += OUTSIDE_KEY;
            }
            if quality.is_seventh() {
                cost += SEVENTH;
            }
            if matches!(quality, Quality::Diminished | Quality::Augmented) {
                cost += RARE_TRIAD;
            }
            if best.is_none_or(|(c, ..)| cost < c - 1e-12) {
                best = Some((cost, chord, inside / total, missing > 0));
            }
        }
    }
    let (cost, chord, fit, implied) = best.expect("there are templates");
    if fit < MIN_FIT {
        return Reading {
            chord: None,
            implied: false,
            cost,
        };
    }
    // The bass of a chord it isn't in is a passing note: name the chord as it stands.
    let chord = if chord.tones().contains(&bass) {
        chord
    } else {
        Chord {
            bass: chord.root,
            ..chord
        }
    };
    Reading {
        chord: Some(chord),
        implied,
        cost,
    }
}

/// The chords of `notes` from song tick `from` to `to`, a bar at a time (each bar, or the
/// part of it inside the range): one chord for the bar, or one per half bar or beat where it
/// changes, whichever reads it more cheaply (see the module's notes). In `key` when it is
/// known, which settles a bar of roots.
pub fn chords(
    notes: &[Sounding],
    map: &TempoMap,
    from: u64,
    to: u64,
    key: Option<Key>,
) -> Vec<ChordSpan> {
    let mut out = Vec::new();
    if to <= from {
        return out;
    }
    let mut bar = map.bar_of_tick(from);
    loop {
        let bar_start = map.tick_of_bar(bar, 0);
        if bar_start >= to {
            break;
        }
        let bar_end = map.tick_of_bar(bar + 1, 0);
        let (lo, hi) = (bar_start.max(from), bar_end.min(to));
        let numerator = map.meter_of_bar(bar).0;
        let beat = map.beat_ticks(bar);
        let inside = |cuts: Vec<u64>| -> Vec<u64> {
            let mut edges = vec![lo];
            edges.extend(cuts.into_iter().filter(|&t| t > lo && t < hi));
            edges.push(hi);
            edges
        };
        let mut readings = vec![inside(Vec::new())];
        if numerator >= 4 && numerator.is_multiple_of(2) {
            readings.push(inside(vec![bar_start + u64::from(numerator / 2) * beat]));
        }
        if numerator >= 2 {
            readings.push(inside(
                (1..numerator)
                    .map(|b| bar_start + u64::from(b) * beat)
                    .collect(),
            ));
        }
        let in_bar: Vec<Sounding> = notes
            .iter()
            .filter(|n| n.start < hi && n.end > lo)
            .copied()
            .collect();
        let bar_weight: f64 = in_bar
            .iter()
            .map(|n| (n.end.min(hi) - n.start.max(lo)) as f64)
            .sum();
        let mut best: Option<(f64, Vec<ChordSpan>)> = None;
        for edges in readings {
            let mut spans: Vec<ChordSpan> = Vec::new();
            let mut cost = 0.0;
            for pair in edges.windows(2) {
                let r = read_stretch(&in_bar, pair[0], pair[1], bar_weight.max(1.0), key);
                cost += r.cost;
                let span = ChordSpan {
                    start: pair[0],
                    end: pair[1],
                    bar,
                    chord: r.chord,
                    implied: r.implied,
                };
                match spans.last_mut() {
                    // A silent stretch goes on with the chord before it.
                    Some(last) if r.chord.is_none() && r.cost == 0.0 => last.end = pair[1],
                    Some(last) if last.chord == r.chord => {
                        last.end = pair[1];
                        last.implied &= r.implied;
                    }
                    _ => spans.push(span),
                }
            }
            // A bar that starts silent: its first chord starts with it.
            if spans.len() > 1 && spans[0].chord.is_none() {
                let start = spans[0].start;
                spans.remove(0);
                spans[0].start = start;
            }
            cost += EXTRA_CHORD * (spans.len() - 1) as f64;
            if best.as_ref().is_none_or(|(c, _)| cost < c - 1e-12) {
                best = Some((cost, spans));
            }
        }
        out.extend(best.expect("a bar has a reading").1);
        bar += 1;
    }
    out
}

/// How note names and chords are spelled: as the key is written, or always one way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Spelling {
    /// Flats in a flat key (F, Bb… major; D, G… minor), else sharps.
    #[default]
    Auto,
    Sharps,
    Flats,
}

/// A clip read as music: what `describe_midi_clip` returns besides the clip's ids.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct Description {
    /// Where the clip starts and ends in the song, "bar:beat:tick".
    pub start: String,
    pub end: String,
    pub bars: BarSpan,
    /// The tempo and time signature where the clip plays, and each change inside it.
    pub meters: Vec<MeterView>,
    /// `null` when there are no notes (or every pitch class alike).
    pub key: Option<KeyView>,
    pub runner_up: Option<KeyView>,
    pub range: Option<RangeView>,
    /// The chords bar by bar: "Am | F | C G" (bars split by "|").
    pub progression: String,
    pub chords: Vec<ChordView>,
    pub notes: Vec<NoteView>,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct BarSpan {
    /// The first and last bar it touches, from 1.
    pub first: i64,
    pub last: i64,
    /// Its length in bars (a part bar as its fraction).
    pub length: f64,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct MeterView {
    /// Where it applies from: the clip's start, or a change inside the clip.
    pub at: String,
    /// Song ticks.
    pub tick: u64,
    /// Quarter notes per minute.
    pub tempo: f64,
    pub time_signature: (u32, u32),
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct KeyView {
    pub name: String,
    pub tonic: &'static str,
    pub mode: Mode,
    /// How well the notes fit the key's profile: a correlation, -1 to 1.
    pub score: f64,
    pub signature: String,
    /// The scale's notes from the tonic (natural minor for minor keys).
    pub scale: Vec<&'static str>,
}

impl KeyView {
    fn new(fit: KeyFit, flats: bool) -> Self {
        Self {
            name: fit.key.name(),
            tonic: pitch_class_name(fit.key.tonic, fit.key.flats()),
            mode: fit.key.mode,
            score: (fit.score * 1000.0).round() / 1000.0,
            signature: fit.key.signature(),
            scale: fit
                .key
                .scale()
                .iter()
                .map(|&pc| pitch_class_name(pc, flats))
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct RangeView {
    pub lowest: String,
    pub highest: String,
    pub lowest_pitch: u8,
    pub highest_pitch: u8,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct ChordView {
    pub at: String,
    /// Ticks from the clip's start, and how long it lasts.
    pub tick: u64,
    pub length: u64,
    /// "Am", "C/E", "G7"; "N" for none.
    pub chord: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub root: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<Quality>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bass: Option<&'static str>,
    /// The chord's notes, root first.
    pub tones: Vec<&'static str>,
    /// Not every tone is heard: read from the notes there are and the key.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub implied: bool,
}

/// A note as `get_midi_clip` lists it, with its place and name.
#[derive(Debug, Serialize, schemars::JsonSchema)]
pub struct NoteView {
    #[serde(flatten)]
    pub note: Note,
    /// Song position, "bar:beat:tick".
    pub at: String,
    pub name: String,
    /// Starts at or past the clip's end, so it doesn't play (kept, as the clip keeps it).
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub past_end: bool,
}

/// Read `clip` as music through `map`.
pub fn describe(clip: &MidiClip, map: &TempoMap, spelling: Spelling) -> Description {
    let (from, to) = (clip.start, clip.end());
    let notes = Sounding::of_clip(clip);
    let keys = estimate_key(&key_weights(&notes, map));
    let key = keys.map(|k| k.best.key);
    let flats = match spelling {
        Spelling::Auto => key.is_some_and(Key::flats),
        Spelling::Sharps => false,
        Spelling::Flats => true,
    };
    let (first, last) = (map.bar_of_tick(from), map.bar_of_tick(to.max(from + 1) - 1));
    let length = (first..=last)
        .map(|bar| {
            let (a, b) = (map.tick_of_bar(bar, 0), map.tick_of_bar(bar + 1, 0));
            let inside = b.min(to).saturating_sub(a.max(from));
            inside as f64 / b.saturating_sub(a).max(1) as f64
        })
        .sum::<f64>();
    let q = f64::from(TICKS_PER_QUARTER);
    let segments = map.segments();
    let meters = segments
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            let start = (s.quarter_notes * q).round().max(0.0) as u64;
            let next = segments
                .get(i + 1)
                .map_or(u64::MAX, |n| (n.quarter_notes * q).round().max(0.0) as u64);
            (start < to.max(from + 1) && next > from).then(|| {
                let tick = start.max(from);
                MeterView {
                    at: format_position(map, tick),
                    tick,
                    tempo: s.tempo,
                    time_signature: (s.numerator, s.denominator),
                }
            })
        })
        .collect();
    let spans = chords(&notes, map, from, to, key);
    let mut progression = String::new();
    for (i, span) in spans.iter().enumerate() {
        if i > 0 {
            progression.push_str(if spans[i - 1].bar == span.bar {
                " "
            } else {
                " | "
            });
        }
        progression.push_str(&span.chord.map_or_else(|| "N".into(), |c| c.name(flats)));
    }
    let chords = spans
        .iter()
        .map(|span| ChordView {
            at: format_position(map, span.start),
            tick: span.start - from,
            length: span.end - span.start,
            chord: span.chord.map_or_else(|| "N".into(), |c| c.name(flats)),
            root: span.chord.map(|c| pitch_class_name(c.root, flats)),
            quality: span.chord.map(|c| c.quality),
            bass: span.chord.map(|c| pitch_class_name(c.bass, flats)),
            tones: span.chord.map_or_else(Vec::new, |c| {
                c.tones()
                    .into_iter()
                    .map(|pc| pitch_class_name(pc, flats))
                    .collect()
            }),
            implied: span.implied,
        })
        .collect();
    let range = notes
        .iter()
        .map(|n| n.pitch)
        .min()
        .zip(notes.iter().map(|n| n.pitch).max())
        .map(|(lo, hi)| RangeView {
            lowest: note_name(lo, flats),
            highest: note_name(hi, flats),
            lowest_pitch: lo,
            highest_pitch: hi,
        });
    Description {
        start: format_position(map, from),
        end: format_position(map, to),
        bars: BarSpan {
            first: first + 1,
            last: last + 1,
            length: (length * 1000.0).round() / 1000.0,
        },
        meters,
        key: keys.map(|k| KeyView::new(k.best, flats)),
        runner_up: keys.map(|k| KeyView::new(k.runner_up, flats)),
        range,
        progression,
        chords,
        notes: clip
            .notes
            .iter()
            .map(|n| NoteView {
                note: *n,
                at: format_position(map, from + n.tick),
                name: note_name(n.pitch, flats),
                past_end: n.tick >= clip.length,
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::TempoChange;

    const Q: u64 = TICKS_PER_QUARTER as u64;
    const BAR: u64 = 4 * Q;

    fn note(start: u64, length: u64, pitch: u8) -> Sounding {
        Sounding {
            start,
            end: start + length,
            pitch,
        }
    }

    fn map44() -> TempoMap {
        TempoMap::constant(48_000, 120.0, 4, 4)
    }

    fn key_of(notes: &[Sounding], map: &TempoMap) -> KeyEstimate {
        estimate_key(&key_weights(notes, map)).expect("notes")
    }

    fn names(spans: &[ChordSpan], flats: bool) -> Vec<String> {
        spans
            .iter()
            .map(|s| s.chord.map_or_else(|| "N".into(), |c| c.name(flats)))
            .collect()
    }

    /// Triads as block chords, one per `length` ticks from 0.
    fn block(chords: &[&[u8]], length: u64) -> Vec<Sounding> {
        chords
            .iter()
            .enumerate()
            .flat_map(|(i, c)| c.iter().map(move |&p| note(i as u64 * length, length, p)))
            .collect()
    }

    #[test]
    fn note_names_parse_and_print_with_sharps_flats_and_octaves() {
        assert_eq!(note_name(60, false), "C4");
        assert_eq!(note_name(58, false), "A#3");
        assert_eq!(note_name(58, true), "Bb3");
        assert_eq!(note_name(0, false), "C-1");
        assert_eq!(note_name(127, false), "G9");
        for (text, pitch) in [
            ("C4", 60),
            ("c4", 60),
            ("A#3", 58),
            ("Bb3", 58),
            ("bb3", 58),
            ("B♭3", 58),
            ("F♯2", 42),
            ("C-1", 0),
            ("G9", 127),
            ("Cb4", 59),
            ("B#3", 60),
            ("Ebb4", 62),
            (" A4 ", 69),
        ] {
            assert_eq!(parse_note_name(text), Ok(pitch), "{text}");
        }
        // Every note name printed parses back, either way.
        for pitch in 0..=127u8 {
            for flats in [false, true] {
                assert_eq!(parse_note_name(&note_name(pitch, flats)), Ok(pitch));
            }
        }
        for (bad, says) in [
            ("H4", "\"H4\" is not a note name"),
            ("", "is not a note name"),
            ("C", "\"C\" has no octave"),
            ("Bb", "\"Bb\" has no octave"),
            ("C#x", "\"C#x\" is not a note name"),
            ("C###4", "is not a note name"),
            ("G#9", "\"G#9\" is note 128, outside 0..127"),
            ("Cb-1", "is note -1"),
            ("4C", "is not a note name"),
        ] {
            let err = parse_note_name(bad).unwrap_err();
            assert!(err.contains(says), "{bad}: {err}");
        }
    }

    /// 4/4 at 120 for bars 1-2, 3/4 at 90 from bar 3, then 180 from beat 2 of bar 5: the
    /// tempo changes move no tick, the meter change moves the bar lines.
    fn changing() -> TempoMap {
        let mut project = crate::project::Project::default();
        project.tempo = 120.0;
        project.time_signature = (4, 4);
        project.tempo_changes = vec![
            TempoChange {
                bar: 2,
                beat: 0,
                tempo: Some(90.0),
                time_signature: Some((3, 4)),
            },
            TempoChange {
                bar: 4,
                beat: 1,
                tempo: Some(180.0),
                time_signature: None,
            },
        ];
        TempoMap::of(&project, 48_000)
    }

    #[test]
    fn positions_round_trip_through_ticks_across_tempo_and_meter_changes() {
        let map = changing();
        let constant = TempoMap::constant(48_000, 120.0, 4, 4);
        for (text, tick) in [
            ("1:1:000", 0),
            ("1:2:000", Q),
            ("2:4:959", BAR + 3 * Q + 959),
            ("3:1:000", 2 * BAR),
            // 3/4 from bar 3: three quarters a bar.
            ("4:1:000", 2 * BAR + 3 * Q),
            ("5:2:000", 2 * BAR + 6 * Q + Q),
            ("5:2:480", 2 * BAR + 7 * Q + 480),
            ("6:3:000", 2 * BAR + 9 * Q + 2 * Q),
        ] {
            assert_eq!(parse_position(&map, text), Ok(tick), "{text}");
            assert_eq!(format_position(&map, tick), text, "{tick}");
        }
        // The tempo changes move no tick: up to bar 3 the ticks are 4/4's at any tempo.
        for tick in [0, Q, BAR + 5, 2 * BAR - 1] {
            assert_eq!(
                format_position(&map, tick),
                format_position(&constant, tick)
            );
        }
        // The meter change does: bar 4 starts a quarter earlier than in 4/4.
        assert_eq!(parse_position(&constant, "4:1:000"), Ok(3 * BAR));
        assert_eq!(parse_position(&map, "4:1:000"), Ok(3 * BAR - Q));
        // Every tick across the changes round-trips.
        for tick in (0..4 * BAR).step_by(37) {
            assert_eq!(parse_position(&map, &format_position(&map, tick)), Ok(tick));
        }
        // Shorter forms and the clock's dots.
        assert_eq!(parse_position(&map, "3"), Ok(2 * BAR));
        assert_eq!(parse_position(&map, "3:2"), Ok(2 * BAR + Q));
        assert_eq!(parse_position(&map, "3.2.240"), Ok(2 * BAR + Q + 240));
        // 6/8: a beat is an eighth, 480 ticks.
        let six = TempoMap::constant(48_000, 120.0, 6, 8);
        assert_eq!(parse_position(&six, "2:6:479"), Ok(3 * Q + 5 * 480 + 479));
        assert_eq!(format_position(&six, 3 * Q + 5 * 480 + 479), "2:6:479");
        for (bad, says) in [
            ("3:4:000", "bar 3 is in 3/4, with beats 1 to 3; beat 4"),
            ("1:5", "beat 5 is not one of them"),
            ("0:1:000", "count from 1"),
            ("2:0", "count from 1"),
            ("2:1:960", "lasts 960 ticks (0 to 959); tick 960"),
            ("2:x", "\"x\" is not a number"),
            ("bar 2", "is not a position"),
            ("1:1:1:1", "is not a position"),
        ] {
            let err = parse_position(&map, bad).unwrap_err();
            assert!(err.contains(says), "{bad}: {err}");
            assert!(err.contains(bad), "{bad}: names the value: {err}");
        }
        assert!(parse_position(&six, "1:1:480")
            .unwrap_err()
            .contains("480 ticks"));
    }

    /// A bass line in A minor: roots on the downbeats, A held longest, a raised seventh
    /// (G#) on the dominant, back to A.
    #[test]
    fn a_bass_line_in_a_minor_reads_as_a_minor_and_its_roots() {
        let map = map44();
        let (a2, c3, d2, e2, f2, g2, gs2) = (45, 48, 38, 40, 41, 43, 44);
        let bar = |n: u64| n * BAR;
        let notes = [
            // Am: A A E A
            note(bar(0), 2 * Q, a2),
            note(bar(0) + 2 * Q, Q, e2),
            note(bar(0) + 3 * Q, Q, a2),
            // Dm: D, F, A
            note(bar(1), 2 * Q, d2),
            note(bar(1) + 2 * Q, Q, f2),
            note(bar(1) + 3 * Q, Q, a2),
            // F: F, C
            note(bar(2), 3 * Q, f2),
            note(bar(2) + 3 * Q, Q, c3),
            // E: E, G#
            note(bar(3), 3 * Q, e2),
            note(bar(3) + 3 * Q, Q, gs2),
            // Am: A held.
            note(bar(4), BAR, a2),
            // G passing into the last bar's A.
            note(bar(5), 2 * Q, g2),
            note(bar(5) + 2 * Q, 2 * Q, a2),
        ];
        let key = key_of(&notes, &map);
        assert_eq!(key.best.key, Key::new(9, Mode::Minor), "{key:?}");
        assert_eq!(key.best.key.name(), "A minor");
        assert!(key.best.score > key.runner_up.score);
        // So much A makes the parallel major the second choice (the relative major, C, would
        // be the other natural one), well behind.
        assert_eq!(key.runner_up.key.name(), "A major", "{key:?}");
        assert!(key.best.score - key.runner_up.score > 0.1, "{key:?}");
        let spans = chords(&notes, &map, 0, 5 * BAR, Some(key.best.key));
        assert_eq!(names(&spans, false), ["Am", "Dm", "F", "E", "Am"]);
        // A bar of roots alone implies its chord.
        assert!(spans[4].implied);
        assert_eq!(spans[0].start, 0);
        assert_eq!(spans[4].end, 5 * BAR);
    }

    /// I-V-vi-IV in C as block chords, a bar each; then two to a bar.
    #[test]
    fn i_v_vi_iv_in_c_reads_as_c_major_with_its_chords() {
        let map = map44();
        let progression: [&[u8]; 4] = [&[60, 64, 67], &[55, 59, 62], &[57, 60, 64], &[53, 57, 60]];
        let notes = block(&progression, BAR);
        let key = key_of(&notes, &map);
        assert_eq!(key.best.key.name(), "C major", "{key:?}");
        let spans = chords(&notes, &map, 0, 4 * BAR, Some(key.best.key));
        assert_eq!(names(&spans, false), ["C", "G", "Am", "F"]);
        assert!(spans.iter().all(|s| !s.implied));
        // Two to a bar: each bar splits at its half.
        let notes = block(&progression, 2 * Q);
        let spans = chords(&notes, &map, 0, 2 * BAR, Some(key.best.key));
        assert_eq!(names(&spans, false), ["C", "G", "Am", "F"]);
        assert_eq!(
            spans.iter().map(|s| (s.bar, s.start)).collect::<Vec<_>>(),
            [(0, 0), (0, 2 * Q), (1, BAR), (1, BAR + 2 * Q)]
        );
        // A first inversion keeps its bass: C/E.
        let notes = block(&[&[52, 60, 67]], BAR);
        let spans = chords(&notes, &map, 0, BAR, Some(key.best.key));
        assert_eq!(names(&spans, false), ["C/E"]);
        // An arpeggio is one chord, a held seventh a seventh.
        let arpeggio: Vec<Sounding> = [57, 60, 64, 60]
            .iter()
            .enumerate()
            .map(|(i, &p)| note(i as u64 * Q, Q, p))
            .collect();
        assert_eq!(names(&chords(&arpeggio, &map, 0, BAR, None), false), ["Am"]);
        let g7 = block(&[&[43, 59, 62, 65]], BAR);
        assert_eq!(names(&chords(&g7, &map, 0, BAR, None), false), ["G7"]);
        // Silence reads N; a cluster too.
        assert_eq!(names(&chords(&[], &map, 0, BAR, None), false), ["N"]);
        let cluster = block(&[&[60, 61, 62, 63, 64, 65, 66]], BAR);
        assert_eq!(names(&chords(&cluster, &map, 0, BAR, None), false), ["N"]);
    }

    /// A part in a flat key names its notes, chords and scale with flats.
    #[test]
    fn a_part_in_a_flat_key_is_spelled_with_flats() {
        let map = map44();
        // Bb major: Bb - Eb - F - Bb, block chords.
        let notes = block(
            &[&[46, 62, 65], &[51, 55, 58], &[53, 57, 60], &[46, 62, 65]],
            BAR,
        );
        let key = key_of(&notes, &map).best.key;
        assert_eq!(key.name(), "Bb major");
        assert!(key.flats());
        assert_eq!(key.signature(), "2 flats");
        let spans = chords(&notes, &map, 0, 4 * BAR, Some(key));
        assert_eq!(names(&spans, true), ["Bb", "Eb", "F", "Bb"]);
        let clip = MidiClip {
            start: 0,
            length: 4 * BAR,
            notes: notes
                .iter()
                .enumerate()
                .map(|(i, n)| Note {
                    id: i as u32 + 1,
                    tick: n.start,
                    length: n.end - n.start,
                    pitch: n.pitch,
                    velocity: 100,
                    channel: 0,
                })
                .collect(),
            id: 1,
            name: "Part".into(),
            controllers: Vec::new(),
        };
        let d = describe(&clip, &map, Spelling::Auto);
        assert_eq!(d.notes[0].name, "Bb2");
        assert_eq!(
            d.key.as_ref().unwrap().scale,
            ["Bb", "C", "D", "Eb", "F", "G", "A"]
        );
        assert_eq!(d.progression, "Bb | Eb | F | Bb");
        // Asked for sharps, the same notes read A#.
        let d = describe(&clip, &map, Spelling::Sharps);
        assert_eq!(d.notes[0].name, "A#2");
        assert_eq!(d.progression, "A# | D# | F | A#");
        // Spelling across the keys: flats only where the signature has them.
        for (key, name, signature) in [
            (Key::new(0, Mode::Major), "C major", "no sharps or flats"),
            (Key::new(9, Mode::Minor), "A minor", "no sharps or flats"),
            (Key::new(2, Mode::Minor), "D minor", "1 flat"),
            (Key::new(3, Mode::Minor), "Eb minor", "6 flats"),
            (Key::new(6, Mode::Major), "F# major", "6 sharps"),
            (Key::new(1, Mode::Major), "Db major", "5 flats"),
            (Key::new(8, Mode::Minor), "G# minor", "5 sharps"),
            (Key::new(4, Mode::Major), "E major", "4 sharps"),
        ] {
            assert_eq!(
                (key.name().as_str(), key.signature().as_str()),
                (name, signature)
            );
        }
    }

    /// The demo's chords (C, Am, F, G, two to a bar) use only C major's notes: C major wins,
    /// A minor is a close second; and a lone fifth is ambiguous between its major and minor.
    #[test]
    fn something_ambiguous_names_a_sensible_runner_up() {
        let map = map44();
        let notes = block(
            &[&[60, 64, 67], &[57, 60, 64], &[53, 57, 60], &[55, 59, 62]],
            2 * Q,
        );
        let key = key_of(&notes, &map);
        assert_eq!(key.best.key.name(), "C major", "{key:?}");
        assert_eq!(key.runner_up.key.name(), "A minor", "{key:?}");
        assert!(key.best.score - key.runner_up.score < 0.2, "{key:?}");
        let spans = chords(&notes, &map, 0, 2 * BAR, Some(key.best.key));
        assert_eq!(names(&spans, false), ["C", "Am", "F", "G"]);
        // A and E alone: A minor and A major are both possible.
        let fifth = [note(0, BAR, 57), note(0, BAR, 64)];
        let ranked = rank_keys(&key_weights(&fifth, &map)).unwrap();
        let top: Vec<String> = ranked[..2].iter().map(|f| f.key.name()).collect();
        assert!(
            top.contains(&"A minor".to_string()) || top.contains(&"A major".to_string()),
            "{top:?}"
        );
        // In a minor key the fifth reads as minor, in a major key as major.
        let minor = chords(&fifth, &map, 0, BAR, Some(Key::new(9, Mode::Minor)));
        let major = chords(&fifth, &map, 0, BAR, Some(Key::new(9, Mode::Major)));
        assert_eq!(
            (names(&minor, false), names(&major, false)),
            (vec!["Am".to_string()], vec!["A".to_string()])
        );
        assert!(minor[0].implied);
        // Nothing to go on: no key.
        assert!(estimate_key(&[0.0; 12]).is_none());
        assert!(estimate_key(&[1.0; 12]).is_none());
    }

    /// A clip starting mid-bar (beat 3 of bar 2) over a meter change: positions, bars,
    /// meters and the chords of the part bar.
    #[test]
    fn a_clip_starting_mid_bar_reads_its_positions_from_the_song() {
        let map = changing();
        let start = parse_position(&map, "2:3:000").unwrap();
        assert_eq!(start, BAR + 2 * Q);
        let clip = MidiClip {
            start,
            // To the end of bar 3 (3/4).
            length: 2 * Q + 3 * Q,
            notes: vec![
                Note {
                    id: 1,
                    tick: 0,
                    length: 2 * Q,
                    pitch: 57,
                    velocity: 100,
                    channel: 0,
                },
                Note {
                    id: 2,
                    tick: 2 * Q + 480,
                    length: Q,
                    pitch: 64,
                    velocity: 100,
                    channel: 0,
                },
                Note {
                    id: 3,
                    tick: 10 * Q,
                    length: Q,
                    pitch: 60,
                    velocity: 100,
                    channel: 0,
                },
            ],
            id: 1,
            name: "Part".into(),
            controllers: Vec::new(),
        };
        let d = describe(&clip, &map, Spelling::Auto);
        assert_eq!((d.start.as_str(), d.end.as_str()), ("2:3:000", "4:1:000"));
        assert_eq!(
            d.notes.iter().map(|n| n.at.as_str()).collect::<Vec<_>>(),
            ["2:3:000", "3:1:480", "5:3:000"]
        );
        assert!(d.notes[2].past_end && !d.notes[0].past_end);
        assert_eq!((d.bars.first, d.bars.last, d.bars.length), (2, 3, 1.5));
        assert_eq!(
            d.meters
                .iter()
                .map(|m| (m.at.as_str(), m.tempo, m.time_signature))
                .collect::<Vec<_>>(),
            [("2:3:000", 120.0, (4, 4)), ("3:1:000", 90.0, (3, 4))]
        );
        // Bar 2's half and bar 3, the note past the end left out.
        assert_eq!(d.chords.len(), 2, "{:?}", d.chords);
        assert_eq!((d.chords[0].at.as_str(), d.chords[0].tick), ("2:3:000", 0));
        assert_eq!(
            (d.chords[1].at.as_str(), d.chords[1].tick),
            ("3:1:000", 2 * Q)
        );
        assert_eq!(d.range.as_ref().unwrap().highest, "E4");
    }
}
