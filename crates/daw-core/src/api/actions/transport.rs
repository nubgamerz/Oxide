//! Playback, recording and mixdown export.

use super::{error, NoArgs};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::project::{MetronomeSettings, Region};
use crate::session::{ExportStatus, Session};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "get_transport",
        "Transport state: playing, recording, playhead position (samples; in beats of the \
         time signature's lower number; as bar.beat from 1; and in quarter notes, which is \
         what plug-ins are told), sample rate, the tempo (quarter notes per minute) and time \
         signature at the playhead (see get_tempo_map for changes), the loop region (`loop`: \
         start, end in samples, enabled), the punch range (`punch`, the same shape) and the \
         metronome settings. A plug-in behind latency or on a bus sees a position that much \
         earlier. While recording, `recording_takes` lists each audio take so far: `track`, \
         `start` and `length` in samples, where its clip will be when recording stops (a loop \
         recording: the pass being recorded); left out when not recording and during a \
         count-in. During a count-in (recording started from a stop with set_metronome's \
         `count_in_bars` at 1 or 2), `count_in` says so: `bars` (its length), `beats_left` \
         (beats of the time signature's lower number before the take starts, counting the \
         one playing: 4, 3, 2, 1 through a bar of 4/4) and `record_point` (samples, where the \
         take starts); left out otherwise. `position` stays at 0 while a count-in plays \
         before the timeline's start.",
        Safety::Read,
        |host, _: NoArgs| Ok(transport(host.session())),
    );
    r.add(
        "play",
        "Start playback from the playhead.",
        Safety::Transport,
        |host, _: NoArgs| {
            host.session().play();
            Ok(transport(host.session()))
        },
    );
    r.add(
        "stop",
        "Stop playback or recording. The playhead stays where playback stopped. Recorded \
         takes appear as clips shortly after.",
        Safety::Transport,
        |host, _: NoArgs| {
            host.session().stop();
            Ok(transport(host.session()))
        },
    );
    r.add(
        "locate",
        "Move the playhead to `position` (samples). Refused while recording.",
        Safety::Transport,
        |host, input: Locate| {
            host.session().locate(input.position)?;
            // With a device running the engine applies the locate on its next block; report
            // the position asked for rather than the one it is leaving.
            Ok(transport_at(host.session(), input.position))
        },
    );
    r.add(
        "set_loop",
        "Set the loop region and/or turn looping on or off. `start` and `end` are in samples \
         and go together (end after start); `enabled` turns looping on or off. A region given \
         without `enabled` leaves looping on or off as it was (as drawing one in the ruler \
         does). Turning it on with no region set loops the 4 bars from the bar the playhead is \
         in. While looping, \
         playback wraps from `end` to `start` sample-accurately; playback that starts past \
         `end` plays on. Recording and exports ignore the loop. The region is saved with the \
         project.",
        Safety::Edit,
        set_loop,
    );
    r.add(
        "set_punch",
        "Set the punch range and/or turn punch recording on or off, like set_loop: `start` \
         and `end` in samples go together, `enabled` turns it on or off (a range alone leaves \
         it as it was). Turning it on with no \
         range set punches the loop region if there is one, or else the 4 bars from the \
         playhead's bar. While on, `record` records only inside the range, to the sample \
         (playback runs on before and after), so takes are exactly the range or the part of \
         it after the playhead. Recording fails if the playhead is already past the range's \
         end. Saved with the project.",
        Safety::Edit,
        set_punch,
    );
    r.add(
        "set_metronome",
        "Change the metronome: `enabled`, `volume_db` (-60 to +6), `only_while_recording`, \
         `count_in_bars` (0 = off, 1 or 2: when recording starts from a stop, the transport \
         starts that many bars early with the click, even with the metronome off, and the \
         take starts at the playhead; in the app, the Record button's right-click menu and the \
         metronome's menu set it too); fields left out stay as they are. The click accents the first beat of each bar, \
         follows the project tempo and time signature, and is aligned with what you hear \
         (delay compensation and loop wraps included). It plays on the master's output pair \
         after the master, is silenced when the master is muted, and is never in exports. \
         Saved with the project.",
        Safety::Edit,
        set_metronome,
    );
    r.add(
        "record",
        "Start recording every armed audio track that has an audio input, and play. Takes are \
         written to the project's Audio folder and become clips when stop is called. From a \
         stop with a count-in set (set_metronome count_in_bars), playback starts that many \
         bars early with the click and the takes start at the playhead. With the loop on, \
         this is loop recording: recording starts at the loop start (a playhead inside the \
         loop moves there; one before it plays a run-up), and each pass becomes a take of one \
         clip spanning the loop (trimmed to the punch range if punch is on). The last \
         complete pass plays; set_active_take picks another. Recording the same loop again \
         adds takes to that clip. While recording, the loop, punch range and playhead are \
         fixed, and earlier passes are not heard until stop.",
        Safety::Transport,
        |host, _: NoArgs| {
            host.session().record()?;
            Ok(transport(host.session()))
        },
    );
    r.add(
        "export_mixdown",
        "Render the master mix to a 32-bit float WAV at `path` (absolute), through the live engine and \
         plug-ins (bit-faithful to playback). Range is `start`..`end` in samples (default: the \
         whole project) plus `tail_seconds` (default 2) for reverb tails. Runs in the \
         background; poll get_export_status, which reports the file's loudness (EBU R 128) \
         when it's done. `normalize` (default false) brings the file to the project's \
         loudness target (see set_loudness_target) with one constant gain and no limiter, held \
         back so the true peak stays at or under `ceiling_dbtp` (default -1): the file then \
         differs from playback by exactly that gain, which get_export_status reports, with \
         whether the ceiling held it. Overwrites `path`. `format` (default a 32-bit float WAV): {\"type\":\"wav\",\"bits\":16|24|32}, {\"type\":\"aiff\",\"bits\":16|24}, {\"type\":\"flac\",\"bits\":16|24,\"level\":0-8}, {\"type\":\"mp3\",\"bitrate\":128|160|192|224|256|320} or {\"type\":\"mp3\",\"vbr\":0-9} (V0 best), {\"type\":\"ogg\",\"quality\":0-10}; `sample_rate` (Hz) converts first (an MP3 must be 32000, 44100 or 48000); `dither` (default true) adds TPDF dither of one bit when the format holds integers. A path without an extension gets the format's; one with another extension is refused.",
        Safety::Destructive,
        export,
    );
    r.add(
        "mixdown_to_new_file",
        "Mixdown to a new file: render the mix as it plays (every track, \
         level, plug-in, automation and send, and the master) into a new 32-bit float WAV in \
         the project's Rendered folder, then open it in the Waveform view for final edits or \
         an export. The same as export_mixdown without normalising, over `start`..`end` in \
         samples (default: the whole project; give a time selection's range for just that) \
         plus `tail_seconds` (default 2). Runs in the background: poll get_export_status; \
         get_audio_editor then lists the file. Returns the file's path.",
        Safety::Edit,
        |host, input: Mixdown| {
            let s = host.session();
            let range = match (input.start, input.end) {
                (None, None) => None,
                (start, end) => Some((start.unwrap_or(0), end.unwrap_or(s.content_end()))),
            };
            let tail = input.tail_seconds.unwrap_or(2.0);
            if !(0.0..=60.0).contains(&tail) {
                return Err(error("tail_seconds must be between 0 and 60"));
            }
            let path = s.mixdown_to_new_file(range, tail)?;
            Ok(MixdownFile { path })
        },
    );
    r.add(
        "get_export_status",
        "Progress of the running export, render, mixdown or file export (`progress` 0 to 1 \
         through rendering and writing the file; `kind`: export, render, mixdown or file; \
         `what` it is making, as the app's progress dialog says), or how the last one ended: \
         finished with its \
         path, its `loudness` (as get_loudness reports it: integrated, range, true peak, maxima) \
         and, if it was normalised, `normalized` (target, ceiling, `gain_db` applied, null \
         when nothing was above the -70 LUFS gate, and `held_by_peak`), and for a track's \
         render the `track` its file went on; or failed with the reason (a cancel: \
         \"export cancelled\").",
        Safety::Read,
        |host, _: NoArgs| Ok(host.session().export_status()),
    );
    r.add(
        "cancel_export",
        "Stop the running export; no file is written.",
        Safety::Transport,
        |host, _: NoArgs| {
            host.session().cancel_export();
            Ok(host.session().export_status())
        },
    );
}

#[derive(Serialize, JsonSchema)]
pub(super) struct TransportView {
    playing: bool,
    recording: bool,
    position: u64,
    /// Position in beats (of the time signature's lower number) from the start.
    position_beats: f64,
    /// Position as bar.beat (1-based), e.g. "3.2".
    bar_beat: String,
    sample_rate: u32,
    /// At the playhead, in quarter notes per minute.
    tempo: f64,
    /// At the playhead.
    time_signature: (u32, u32),
    /// Position as plug-ins get it, in quarter notes from the start.
    position_quarter_notes: f64,
    #[serde(rename = "loop")]
    loop_region: Region,
    punch: Region,
    metronome: MetronomeSettings,
    /// While recording: each audio take as far as it has come. Left out otherwise.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    recording_takes: Vec<RecordingTakeView>,
    /// While a count-in plays before a take. Left out otherwise.
    #[serde(skip_serializing_if = "Option::is_none")]
    count_in: Option<CountInView>,
}

#[derive(Serialize, JsonSchema)]
struct CountInView {
    /// Its length in bars.
    bars: u32,
    /// Beats before the take starts, counting the one playing.
    beats_left: u32,
    /// Where the take starts, in samples.
    record_point: u64,
}

#[derive(Serialize, JsonSchema)]
struct RecordingTakeView {
    track: crate::project::TrackId,
    /// Timeline position of the clip it becomes, in samples.
    start: u64,
    /// Samples recorded so far (of the pass, in a loop recording).
    length: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMetronome {
    #[serde(default)]
    enabled: Option<bool>,
    /// Click level in dB, -60 to +6.
    #[serde(default)]
    volume_db: Option<f32>,
    #[serde(default)]
    only_while_recording: Option<bool>,
    /// Bars of click before recording starts: 0 (off), 1 or 2.
    #[serde(default)]
    count_in_bars: Option<u32>,
}

fn set_metronome(host: &mut dyn Host, input: SetMetronome) -> ApiResult<TransportView> {
    let s = host.session();
    let mut m = s.project.metronome;
    m.enabled = input.enabled.unwrap_or(m.enabled);
    m.volume_db = input.volume_db.unwrap_or(m.volume_db);
    m.only_while_recording = input.only_while_recording.unwrap_or(m.only_while_recording);
    m.count_in_bars = input.count_in_bars.unwrap_or(m.count_in_bars);
    s.set_metronome(m)?;
    Ok(transport(s))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetRegion {
    /// Start in samples; give `end` too.
    #[serde(default)]
    start: Option<u64>,
    /// End in samples, after `start`.
    #[serde(default)]
    end: Option<u64>,
    #[serde(default)]
    enabled: Option<bool>,
}

impl SetRegion {
    /// The range to set, if any; an error for half a range or nothing at all.
    fn range(&self) -> ApiResult<Option<(u64, u64)>> {
        match (self.start, self.end, self.enabled) {
            (Some(start), Some(end), _) => Ok(Some((start, end))),
            (None, None, Some(_)) => Ok(None),
            (None, None, None) => Err(error(
                "nothing to change: give start and end, enabled, or both",
            )),
            _ => Err(error("give both start and end, or neither")),
        }
    }
}

fn set_loop(host: &mut dyn Host, input: SetRegion) -> ApiResult<TransportView> {
    let s = host.session();
    if let Some((start, end)) = input.range()? {
        s.set_loop_region(start, end)?;
    }
    if let Some(enabled) = input.enabled {
        s.set_looping(enabled)?;
    }
    Ok(transport(s))
}

fn set_punch(host: &mut dyn Host, input: SetRegion) -> ApiResult<TransportView> {
    let s = host.session();
    if let Some((start, end)) = input.range()? {
        s.set_punch_range(start, end)?;
    }
    if let Some(enabled) = input.enabled {
        s.set_punching(enabled)?;
    }
    Ok(transport(s))
}

pub(super) fn transport(s: &Session) -> TransportView {
    transport_at(s, s.position())
}

fn transport_at(s: &Session, position: u64) -> TransportView {
    let map = s.tempo_map();
    let at = position as f64;
    let (bar, beat) = map.bar_at(at);
    let meter = map.meter_at(position as i64);
    TransportView {
        loop_region: s.project.loop_region,
        punch: s.project.punch,
        metronome: s.project.metronome,
        position_quarter_notes: map.quarter_notes_at(at),
        playing: s.is_playing(),
        recording: s.is_recording(),
        position,
        position_beats: map.beats_at(at),
        bar_beat: format!("{}.{}", bar + 1, beat.floor() as i64 + 1),
        sample_rate: s.sample_rate(),
        tempo: meter.tempo,
        time_signature: (meter.numerator, meter.denominator),
        recording_takes: s
            .recording_takes()
            .iter()
            .map(|t| RecordingTakeView {
                track: t.track,
                start: t.start,
                length: t.length,
            })
            .collect(),
        count_in: s.count_in_progress().map(|c| CountInView {
            bars: c.bars,
            beats_left: c.beats_left,
            record_point: c.record_point,
        }),
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Locate {
    /// Playhead position in samples.
    position: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Export {
    /// Output .wav path.
    path: PathBuf,
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    /// Extra seconds after the end (default 2).
    #[serde(default)]
    tail_seconds: Option<f64>,
    /// Normalise to the project's loudness target.
    #[serde(default)]
    normalize: bool,
    /// The true peak normalising may not pass, dBTP (default -1).
    #[serde(default)]
    ceiling_dbtp: Option<f64>,
    /// The file's format (default a 32-bit float WAV).
    #[serde(default)]
    format: Option<crate::encode::Format>,
    /// Convert to this rate (Hz) before writing.
    #[serde(default)]
    sample_rate: Option<u32>,
    /// TPDF dither on integer formats (default true).
    #[serde(default)]
    dither: Option<bool>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Mixdown {
    #[serde(default)]
    start: Option<u64>,
    #[serde(default)]
    end: Option<u64>,
    /// Extra seconds after the end (default 2).
    #[serde(default)]
    tail_seconds: Option<f64>,
}

#[derive(Serialize, JsonSchema)]
struct MixdownFile {
    path: PathBuf,
}

fn export(host: &mut dyn Host, input: Export) -> ApiResult<ExportStatus> {
    let s = host.session();
    let range = match (input.start, input.end) {
        (None, None) => None,
        (start, end) => Some((start.unwrap_or(0), end.unwrap_or(s.content_end()))),
    };
    let tail = input.tail_seconds.unwrap_or(2.0);
    if !(0.0..=60.0).contains(&tail) {
        return Err(error("tail_seconds must be between 0 and 60"));
    }
    let ceiling = input.ceiling_dbtp.unwrap_or(-1.0);
    if !(-20.0..=0.0).contains(&ceiling) {
        return Err(error(format!("ceiling_dbtp {ceiling} is outside -20..0")));
    }
    let normalize = input.normalize.then_some(crate::export::Normalize {
        target: s.project.loudness_target,
        ceiling,
    });
    let encoding = crate::encode::Encoding {
        format: input.format.unwrap_or_default(),
        sample_rate: input.sample_rate,
        dither: input.dither.unwrap_or(true),
    };
    let path = crate::encode::with_extension(&input.path, &encoding.format)?;
    s.export_as(&path, range, tail, normalize, encoding)?;
    Ok(s.export_status())
}
