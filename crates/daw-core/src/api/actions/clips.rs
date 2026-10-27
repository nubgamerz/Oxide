//! Audio clips on the timeline: importing, moving, splitting, deleting, gain, and reading
//! their waveform.

use super::{check_clip, check_track, error};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::project::{ClipId, Fade, FadeShape, TrackId, TrackKind};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "split_range",
        "Cut the clips on `tracks` (track ids; omit for every track that holds clips) at both \
         edges of the time range `start`..`end` (samples), wherever an edge falls inside a \
         clip: what a time selection's Split does. MIDI clips are cut at the tick nearest each \
         edge, as split_clip cuts them. Returns the new clips' ids.",
        Safety::Edit,
        |host, input: RangeArg| {
            let pieces = host.session().split_range(
                input.start,
                input.end,
                input.tracks.as_deref().unwrap_or(&[]),
            )?;
            Ok(Pieces { clips: pieces })
        },
    );
    r.add(
        "delete_range",
        "Remove everything in the time range `start`..`end` (samples) on `tracks` (track ids; \
         omit for every track that holds clips), as Delete does on a time selection: audio and \
         MIDI clips are cut at the edges (as split_range cuts them) and the parts inside \
         removed, the parts outside kept. Later clips don't move. Returns how many clips (or \
         parts) were removed.",
        Safety::Destructive,
        |host, input: RangeArg| {
            let removed = host.session().delete_range(
                input.start,
                input.end,
                input.tracks.as_deref().unwrap_or(&[]),
            )?;
            Ok(Removed { removed })
        },
    );
    r.add(
        "import_audio",
        "Import an audio file (WAV, FLAC, MP3 or AIFF; absolute `path`) as a clip on an audio track at `position` (samples; default: the \
         playhead). Files at another sample rate are converted. Returns the clip id and length.",
        Safety::Edit,
        import_audio,
    );
    r.add(
        "move_clip",
        "Move a clip so it starts at `start` (samples), and/or onto another audio or \
         instrument `track`. Only the fields given change.",
        Safety::Edit,
        move_clip,
    );
    r.add(
        "split_clip",
        "Cut an audio or MIDI clip in two at timeline `position` (samples, strictly inside the \
         clip). The original keeps the first part and its id; returns both parts (the second \
         with a new id), each as get_clip or get_midi_clip shows it. A MIDI clip is cut at the \
         tick nearest `position` (960 per quarter note, through the tempo map): each note goes \
         to the part it starts in; a note across the cut ends there and a copy (new id, same \
         pitch, velocity and channel) starts the second part with the rest of its length; \
         controller events from the cut on move to the second part, which starts with each \
         controller (CC, pitch bend, program, per channel) set to its value in force at the \
         cut.",
        Safety::Edit,
        split_clip,
    );
    r.add(
        "delete_clip",
        "Remove a clip from the timeline (the audio file on disk is kept).",
        Safety::Destructive,
        |host, input: ClipArg| {
            let s = host.session();
            check_clip(s, input.clip)?;
            s.project.remove_clip(input.clip);
            s.mark_dirty();
            Ok(ClipDone { clip: input.clip })
        },
    );
    r.add(
        "set_clip",
        "Change a clip's `gain_db` (-60 to +24), `name`, `stretch` or `pitch`. Only the fields \
         given change. `stretch` is a time-stretch ratio from 0.25 to 4 (2 = twice as long, \
         same pitch; 1 = untouched): the start stays and the length scales with it. `pitch` \
         transposes by semitones, -24 to +24, at the same length. A stretched or transposed \
         clip renders in the background and plays unchanged until `rendering` is false; \
         export_mixdown refuses while any clip is rendering. `source_tempo` (quarter notes \
         per minute the audio was recorded at, 20 to 999) makes the clip follow the project \
         tempo and its changes: it keeps its start and the audio it plays, and stretches to \
         fit the tempo map, moving with the bars when the tempo changes (its `stretch` is then \
         set by the tempo and can't be set). `source_tempo` 0 stops following: the clip stays \
         where and as long as it is, at the ratio the tempo gave its start. `fade_in` and \
         `fade_out` are fade lengths in samples from each end (0 = none; together at most the \
         clip's length, the fade-in first), with `fade_in_shape` / `fade_out_shape` \
         \"linear\" or \"equal_power\" (the default). `transients` (default true for new clips) \
         keeps attacks sharp and on time when stretching: each plays unstretched and the audio \
         between them takes up the stretch. `trim_start` / `trim_end` move the clip's start or \
         end edge to a timeline sample while its audio stays where it is: moving an edge in \
         hides the audio there, moving it back out restores it, as far as the audio reaches \
         (errors say where). Works on stretched clips, split pieces and clips that follow the \
         tempo.",
        Safety::Edit,
        set_clip,
    );
    r.add(
        "copy_clips",
        "Copy clips (audio or MIDI, by id) to the clipboard, replacing what it held. The \
         clipboard belongs to the session: it isn't saved and isn't part of undo.",
        Safety::Edit,
        |host, input: ClipList| {
            let count = host.session().copy_clips(&input.clips)?;
            Ok(Count { clips: count })
        },
    );
    r.add(
        "cut_clips",
        "Copy clips (audio or MIDI, by id) to the clipboard and remove them from the timeline \
         (their files stay on disk). All or none.",
        Safety::Destructive,
        |host, input: ClipList| {
            let count = host.session().cut_clips(&input.clips)?;
            Ok(Count { clips: count })
        },
    );
    r.add(
        "delete_clips",
        "Remove several clips (audio or MIDI, by id) in one undo step; their files stay on \
         disk. All or none.",
        Safety::Destructive,
        |host, input: ClipList| {
            let count = input.clips.len();
            host.session().delete_clips(&input.clips)?;
            Ok(Count { clips: count })
        },
    );
    r.add(
        "paste_clips",
        "Paste the clipboard so its earliest clip starts at `position` (samples; default: the \
         playhead), the others keeping their distances. On the tracks they were copied from, \
         or with `track` moved together so the topmost lands on that track (the others keep \
         their rows below it). MIDI clips go on instrument tracks only. Every pasted clip gets \
         new ids; fades, stretch, pitch and tempo following come along. Returns the new ids.",
        Safety::Edit,
        |host, input: Paste| {
            let s = host.session();
            let at = input.position.unwrap_or_else(|| s.position());
            let clips = s.paste_clips(at, input.track)?;
            Ok(NewClips { clips })
        },
    );
    r.add(
        "duplicate_clips",
        "Copy clips (audio or MIDI, by id) to right after the last of them ends, on the same \
         tracks (the clipboard is left alone). Returns the copies' ids, in the order given.",
        Safety::Edit,
        |host, input: ClipList| {
            let clips = host.session().duplicate_clips(&input.clips)?;
            Ok(NewClips { clips })
        },
    );
    r.add(
        "move_clips",
        "Move clips (audio or MIDI, by id) together: `delta` samples later (negative: earlier) \
         and `track_delta` rows down (negative: up). All or none: refused if any would start \
         before 0 or land on a track that can't hold it.",
        Safety::Edit,
        |host, input: MoveClips| {
            let s = host.session();
            s.move_clips(&input.clips, input.delta, input.track_delta.unwrap_or(0))?;
            Ok(Count {
                clips: input.clips.len(),
            })
        },
    );
    r.add(
        "copy_clips_to",
        "Copy clips (audio or MIDI, by id) and move the copies as move_clips would: `delta` samples later (negative: earlier) and `track_delta` rows down (negative: up); the originals stay (an Alt-drag in the arrangement). All or none: refused if any copy would start before 0 or land on a track that can't hold it (MIDI clips go on instrument tracks, audio clips not on folders or buses). Returns the copies' ids, in the order given.",
        Safety::Edit,
        |host, input: MoveClips| {
            let s = host.session();
            let clips = s.copy_clips_to(&input.clips, input.delta, input.track_delta.unwrap_or(0))?;
            Ok(NewClips { clips })
        },
    );
    r.add(
        "crossfade_clips",
        "Crossfade a clip with the clip before it on its track: the earlier one fades out and \
         this one fades in over their overlap. `length` (samples) defaults to the overlap, or \
         20 ms if they only touch; if they overlap less than `length`, both are extended into \
         the audio beyond their ends (half each, as far as there is audio). `shape`: \
         \"equal_power\" (default; different audio, such as two takes) or \"linear\" (two pieces \
         of the same audio, which then sum back to it exactly). Refused if a gap separates \
         them. Returns both clips.",
        Safety::Edit,
        crossfade,
    );
    r.add(
        "set_active_take",
        "Choose which take a clip plays (`take`, an index from 0). Loop recording gives a \
         clip one take per pass, newest last; get_project lists them. Split the clip first to \
         pick different takes for different parts (comping).",
        Safety::Edit,
        |host, input: TakeArg| {
            let s = host.session();
            check_clip(s, input.clip)?;
            s.set_active_take(input.clip, input.take)?;
            view(s, input.clip)
        },
    );
    r.add(
        "delete_take",
        "Remove one take from a clip (the audio file on disk is kept). A clip's last take \
         cannot be removed; delete the clip instead. If the playing take is removed, the one \
         before it plays.",
        Safety::Destructive,
        |host, input: TakeArg| {
            let s = host.session();
            check_clip(s, input.clip)?;
            s.delete_take(input.clip, input.take)?;
            view(s, input.clip)
        },
    );
    r.add(
        "get_clip_waveform",
        "Read the waveform of a clip's playing take as `points` (default 200, max 4000) evenly spaced segments, \
         each the minimum and maximum sample value (-1 to 1) of the clip's channels in that \
         segment, with the segment's timeline start in samples. Use it to see where the audio \
         is loud, quiet or silent.",
        Safety::Read,
        waveform,
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct RangeArg {
    /// Range start, samples.
    start: u64,
    /// Range end, samples (after `start`).
    end: u64,
    /// The tracks; omit for every track that holds clips.
    #[serde(default)]
    tracks: Option<Vec<TrackId>>,
}

#[derive(Serialize, JsonSchema)]
struct Pieces {
    clips: Vec<ClipId>,
}

#[derive(Serialize, JsonSchema)]
struct Removed {
    removed: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClipArg {
    clip: ClipId,
}

#[derive(Serialize, JsonSchema)]
struct ClipDone {
    clip: ClipId,
}

#[derive(Serialize, JsonSchema)]
struct ClipView {
    clip: ClipId,
    track: TrackId,
    name: String,
    start: u64,
    length: u64,
    /// Where in its audio the clip begins, in samples of the audio as heard (for a stretched
    /// clip, its render).
    offset: u64,
    gain_db: f32,
    /// Time-stretch ratio (1 = untouched).
    stretch: f64,
    /// Transposition in semitones (0 = untouched).
    pitch: f64,
    /// The recording's tempo when the clip follows the project tempo (quarter notes per
    /// minute), else absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    source_tempo: Option<f64>,
    /// A stretch keeps the attacks unstretched and on time.
    transients: bool,
    /// Still waiting for its stretched render (it plays unchanged meanwhile).
    rendering: bool,
    /// Fades at each end, in samples.
    fade_in: crate::project::Fade,
    fade_out: crate::project::Fade,
    /// How many takes the clip has (one per pass of a loop recording)...
    takes: usize,
    /// ...and which one plays.
    active: usize,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct TakeArg {
    clip: ClipId,
    /// Take index, from 0 (see get_project's `takes`).
    take: usize,
}

fn view(s: &crate::session::Session, clip: ClipId) -> ApiResult<ClipView> {
    let (track, c) = s
        .project
        .find_clip(clip)
        .ok_or_else(|| error(format!("no clip with id {clip}")))?;
    Ok(ClipView {
        clip,
        track,
        name: c.name.clone(),
        start: c.start,
        length: c.length,
        offset: c.offset(),
        gain_db: c.gain_db,
        stretch: c.stretch,
        pitch: c.pitch,
        source_tempo: c.follow.as_ref().map(|f| f.tempo),
        transients: c.transients,
        rendering: s.clip_rendering(c),
        fade_in: c.fade_in,
        fade_out: c.fade_out,
        takes: c.takes.len(),
        active: c.active,
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ImportAudio {
    track: TrackId,
    /// Path to a WAV, FLAC, MP3 or AIFF file.
    path: PathBuf,
    /// Timeline position in samples; the playhead when omitted.
    #[serde(default)]
    position: Option<u64>,
}

fn import_audio(host: &mut dyn Host, input: ImportAudio) -> ApiResult<ClipView> {
    let s = host.session();
    check_track(s, input.track)?;
    if s.project.track(input.track).map(|t| t.kind) != Some(TrackKind::Audio) {
        return Err(error(format!(
            "track {} is not an audio track",
            input.track
        )));
    }
    let at = input.position.unwrap_or_else(|| s.position());
    let clip = s.add_clip_from_file(input.track, &input.path, at)?;
    view(s, clip)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveClip {
    clip: ClipId,
    /// New start in samples.
    #[serde(default)]
    start: Option<u64>,
    /// Track to move it to.
    #[serde(default)]
    track: Option<TrackId>,
}

fn move_clip(host: &mut dyn Host, input: MoveClip) -> ApiResult<ClipView> {
    let s = host.session();
    check_clip(s, input.clip)?;
    let current = s.project.find_clip(input.clip).expect("checked").1.start;
    s.project
        .move_clip(input.clip, input.start.unwrap_or(current), input.track)
        .map_err(error)?;
    s.mark_dirty();
    view(s, input.clip)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SplitClip {
    clip: ClipId,
    /// Timeline position in samples, strictly between the clip's start and end.
    position: u64,
}

#[derive(Serialize, JsonSchema)]
struct Split {
    first: Piece,
    second: Piece,
}

/// A piece of a split clip, as the clip's own actions show it: `get_clip`'s fields for audio,
/// `get_midi_clip`'s for MIDI.
#[derive(Serialize, JsonSchema)]
#[serde(untagged)]
enum Piece {
    Audio(ClipView),
    Midi(super::midi_clips::MidiClipView),
}

fn piece(s: &crate::session::Session, clip: ClipId) -> ApiResult<Piece> {
    if s.project.find_midi_clip(clip).is_some() {
        return Ok(Piece::Midi(super::midi_clips::view(s, clip)));
    }
    view(s, clip).map(Piece::Audio)
}

fn split_clip(host: &mut dyn Host, input: SplitClip) -> ApiResult<Split> {
    let s = host.session();
    let midi = s.project.find_midi_clip(input.clip).is_some();
    if !midi {
        check_clip(s, input.clip)?;
    }
    let second = s.split_clip(input.clip, input.position)?.ok_or_else(|| {
        let map = s.tempo_map();
        let (start, end) = match s.project.find_midi_clip(input.clip) {
            Some((_, c)) => (
                map.sample_at_tick(c.start as f64).round() as u64,
                map.sample_at_tick(c.end() as f64).round() as u64,
            ),
            None => {
                let c = s.project.find_clip(input.clip).expect("checked").1;
                (c.start, c.end())
            }
        };
        let tick = if midi {
            format!(" (tick {})", s.tick_at_sample(input.position))
        } else {
            String::new()
        };
        error(format!(
            "position {}{tick} is not inside clip {} ({start}..{end})",
            input.position, input.clip,
        ))
    })?;
    s.mark_dirty();
    Ok(Split {
        first: piece(s, input.clip)?,
        second: piece(s, second)?,
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetClip {
    clip: ClipId,
    #[serde(default)]
    gain_db: Option<f32>,
    #[serde(default)]
    name: Option<String>,
    /// Time-stretch ratio, 0.25 to 4 (1 = untouched).
    #[serde(default)]
    stretch: Option<f64>,
    /// Transposition in semitones, -24 to +24 (0 = untouched).
    #[serde(default)]
    pitch: Option<f64>,
    /// Follow the project tempo, recorded at this many quarter notes per minute (20 to 999);
    /// 0 stops following.
    #[serde(default)]
    source_tempo: Option<f64>,
    /// Fade lengths in samples from each end (0 = none).
    #[serde(default)]
    fade_in: Option<u64>,
    #[serde(default)]
    fade_out: Option<u64>,
    #[serde(default)]
    fade_in_shape: Option<FadeShape>,
    #[serde(default)]
    fade_out_shape: Option<FadeShape>,
    /// Keep attacks sharp and on time when stretching.
    #[serde(default)]
    transients: Option<bool>,
    /// Move the clip's start edge to this timeline sample, its audio staying put: later hides
    /// the beginning, earlier brings it back (as far as the audio starts).
    #[serde(default)]
    trim_start: Option<u64>,
    /// Move the clip's end edge to this timeline sample likewise.
    #[serde(default)]
    trim_end: Option<u64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ClipList {
    /// Clip ids, audio or MIDI.
    clips: Vec<ClipId>,
}

#[derive(Serialize, JsonSchema)]
struct Count {
    clips: usize,
}

#[derive(Serialize, JsonSchema)]
struct NewClips {
    clips: Vec<ClipId>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Paste {
    /// Samples; the playhead when omitted.
    #[serde(default)]
    position: Option<u64>,
    /// The track the topmost clip lands on (default: where they came from).
    #[serde(default)]
    track: Option<TrackId>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MoveClips {
    clips: Vec<ClipId>,
    /// Samples later (negative: earlier).
    delta: i64,
    /// Rows down (negative: up); default 0.
    #[serde(default)]
    track_delta: Option<i64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Crossfade {
    /// The later clip; it crossfades with the one before it on its track.
    clip: ClipId,
    /// Samples (default: the overlap, or 20 ms if they only touch).
    #[serde(default)]
    length: Option<u64>,
    /// "equal_power" (default) or "linear".
    #[serde(default)]
    shape: Option<FadeShape>,
}

#[derive(Serialize, JsonSchema)]
struct Crossfaded {
    earlier: ClipView,
    later: ClipView,
}

fn crossfade(host: &mut dyn Host, input: Crossfade) -> ApiResult<Crossfaded> {
    let s = host.session();
    check_clip(s, input.clip)?;
    let earlier = s.crossfade_clips(input.clip, input.length, input.shape.unwrap_or_default())?;
    Ok(Crossfaded {
        earlier: view(s, earlier)?,
        later: view(s, input.clip)?,
    })
}

fn set_clip(host: &mut dyn Host, input: SetClip) -> ApiResult<ClipView> {
    let s = host.session();
    check_clip(s, input.clip)?;
    if let Some(gain) = input.gain_db {
        if !gain.is_finite() {
            return Err(error("gain_db must be a number"));
        }
    }
    if let Some(tempo) = input.source_tempo {
        s.set_clip_follow(input.clip, (tempo != 0.0).then_some(tempo))?;
    }
    if input.stretch.is_some() || input.pitch.is_some() {
        s.set_clip_stretch(input.clip, input.stretch, input.pitch)?;
    }
    if input.trim_start.is_some() || input.trim_end.is_some() {
        s.trim_clip(input.clip, input.trim_start, input.trim_end)
            .map_err(|e| error(format!("{e:#}")))?;
    }
    let clip = s.project.clip_mut(input.clip).expect("checked");
    if let Some(gain) = input.gain_db {
        clip.gain_db = gain.clamp(-60.0, 24.0);
    }
    if let Some(name) = input.name {
        clip.name = name;
    }
    if let Some(keep) = input.transients {
        clip.transients = keep;
    }
    let (fade_in, fade_out) = (clip.fade_in, clip.fade_out);
    clip.set_fades(
        Fade {
            length: input.fade_in.unwrap_or(fade_in.length),
            shape: input.fade_in_shape.unwrap_or(fade_in.shape),
        },
        Fade {
            length: input.fade_out.unwrap_or(fade_out.length),
            shape: input.fade_out_shape.unwrap_or(fade_out.shape),
        },
    );
    s.mark_dirty();
    view(s, input.clip)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Waveform {
    clip: ClipId,
    /// Number of segments (default 200, max 4000).
    #[serde(default)]
    points: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct WaveformView {
    clip: ClipId,
    start: u64,
    length: u64,
    /// Samples per point.
    step: f64,
    /// [timeline position, min, max] per segment.
    points: Vec<(u64, f32, f32)>,
}

fn waveform(host: &mut dyn Host, input: Waveform) -> ApiResult<WaveformView> {
    let s = host.session();
    check_clip(s, input.clip)?;
    let clip = s.project.find_clip(input.clip).expect("checked").1.clone();
    let (data, offset) = s
        .clip_audio(&clip)
        .ok_or_else(|| error(format!("the audio for clip {} is not loaded", clip.id)))?;
    let count = input.points.unwrap_or(200).clamp(1, 4000);
    let step = clip.length as f64 / count as f64;
    let points = (0..count)
        .map(|i| {
            let from = offset as f64 + i as f64 * step;
            let to = (from + step).max(from + 1.0);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for channel in 0..data.channels.len() {
                let (l, h) = data.peak_range(channel, from as usize, to as usize);
                lo = lo.min(l);
                hi = hi.max(h);
            }
            if lo > hi {
                (lo, hi) = (0.0, 0.0);
            }
            (clip.start + (i as f64 * step) as u64, lo, hi)
        })
        .collect();
    Ok(WaveformView {
        clip: clip.id,
        start: clip.start,
        length: clip.length,
        step,
        points,
    })
}
