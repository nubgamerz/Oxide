//! The Waveform view's audio files (dev plan 10): opening one, reading what is open, playing
//! it on its own, editing it in its own history and saving it over the file. Positions here
//! are the file's own samples, at its own rate, not the project's.

use super::{check_clip, check_insert, error};
use crate::api::{ApiResult, Host, Registry, Safety};
use crate::audio_edit::Edit;
use crate::project::{ClipId, FadeShape, InsertId};
use crate::session::{DocumentId, Session};
use crate::spectral_edit::SpectralOp;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

pub(super) fn register(r: &mut Registry) {
    r.add(
        "open_audio_editor",
        "Open an audio file in the Waveform view's editor: `path` (absolute; WAV, FLAC, MP3 or \
         AIFF) or `clip` (an audio clip id: its playing take's file). A file already open \
         gives its existing document. Opened from a clip, the document remembers the clip and \
         its track (`source`), and the effects rack can then sync with that track \
         (set_rack_sync). Returns the document (see get_audio_editor). To show it, set_view \
         `view_mode` \"waveform\" with `document`. The project remembers its open files: \
         saving it (and its backups) writes the list, which file shows and whether the Waveform \
         view is in front, and opening the project reopens them (a file that is gone is \
         skipped with a notice); opening another project, or a new one, closes them. Opening \
         or closing a file is neither an undo step nor a change to save on its own.",
        Safety::Edit,
        open_audio_editor,
    );
    r.add(
        "get_audio_editor",
        "The audio files open in the Waveform view, which one is playing and where, the effects \
         rack (its plug-ins, bypass and latency; `loaded` false for a slot whose plug-in is \
         still loading or failed to, with `error` saying why it failed: such a slot keeps its \
         settings and is skipped) and an Apply's progress. The rack is kept between runs, and \
         saved and loaded as presets (list_rack_presets). `sync` (null when off) says the \
         track the rack is synced with (set_rack_sync), its document and clip, and each rack \
         slot's `track_insert` is the track's insert it mirrors. Each \
         document: id, path, sample rate, channels, length in the file's own samples, its WAV \
         format (`null` for FLAC, MP3 or AIFF: those save only with save_audio_as), `dirty` \
         (unsaved edits), its own history (step names, the showing step, the saved one), the \
         clips that use the file (a save changes what they play) and `source` (the clip it was \
         opened from and that clip's track, or null). The open files are saved with the \
         project and reopen with it.",
        Safety::Read,
        |host, _: super::NoArgs| Ok(editor_state(host.session())),
    );
    r.add(
        "close_audio_editor",
        "Close an audio file in the Waveform view (`document` id), stopping it if it plays. A \
         file with unsaved edits is refused unless `discard` is true.",
        Safety::Edit,
        |host, input: CloseDocument| {
            host.session()
                .close_document(input.document, input.discard)?;
            Ok(editor_state(host.session()))
        },
    );
    r.add(
        "play_audio_editor",
        "Play an open audio file on its own (`document`), from `start` to `end` (the file's own \
         samples; the whole file by default), on the master's outputs with no track \
         processing, as the Waveform view's Play does; `play` false stops it. Playing the \
         project stops it, and it stops the project. It is never in an export. Returns the \
         editor's state.",
        Safety::Transport,
        |host, input: PlayDocument| {
            let s = host.session();
            if input.play {
                s.preview_document(input.document, input.start.unwrap_or(0), input.end)?;
            } else {
                s.stop_preview();
            }
            Ok(editor_state(s))
        },
    );
    r.add(
        "get_spectrum",
        "The spectrum of an open audio file (`document`) over `start`..`end` (the file's own \
         samples; the whole file by default), of one `channel` (from 0) or all channels mixed: \
         `bands` (default 32, up to 512) log-spaced bands from 20 Hz to the Nyquist frequency, \
         each its low and high edge in Hz and its level in dB, where a full-scale sine reads 0 \
         dB (Hann-windowed FFTs of `fft` points, default 4096, half overlapping, power averaged). \
         Returns the bands, the FFT size and how many frames were averaged.",
        Safety::Read,
        get_spectrum,
    );
    r.add(
        "edit_audio",
        "Edit an open audio file (`document`) as the Waveform view does, as a step in the \
         file's own history (undo_audio_edit), not the project's; nothing is written until \
         save_audio. `op`: delete, trim (keep only the range), insert_silence (`at`, `length`), \
         silence, amplify (`db`), normalize_peak (`peak_db` dBFS), normalize_loudness (`lufs`, \
         integrated, EBU R 128; at least 400 ms), fade_in / fade_out (`shape`: linear or \
         equal_power), reverse, invert, paste (copy_audio's audio at `at`, or over `start`..`end`; \
         another rate is converted, a mono copy fills every channel), and the spectral view's \
         repairs of a box from `start` to `end` and `low_hz` to `high_hz` (Hz): \
         spectral_attenuate (`db`, negative to turn it down), spectral_remove, and \
         spectral_heal (fills the box from the sound just before and after it, as \
         spot healing does; the bins just below and above it when it spans the whole file). Audio \
         outside the box is untouched beyond one analysis frame (about 43 ms at 48 kHz). \
         The range is `start`..`end` (the file's own samples; the whole file when left out, \
         except delete, trim, fades and the spectral ops, which need one). `channels` (from 0) \
         limits silence, amplify, normalize, fades, reverse, invert and the spectral ops; ops \
         that change the length apply to every channel. Returns the document.",
        Safety::Edit,
        edit_audio,
    );
    r.add(
        "apply_audio_effects",
        "Render `start`..`end` of an open audio file (`document`; the file's own samples, the \
         whole file by default) through the Waveform view's effects rack (add_insert with \
         `editor` true) and write the result into the file as one step of its own history. \
         It is exactly what play_audio_editor plays through the rack: \
         the same plug-ins, settled the same way, in the same blocks, with their latency \
         compensated. It renders in the background: get_audio_editor's `applying` shows its \
         progress, and the document's history gains the step when it is done. Stops the \
         file's playback first. The slots' bypass is taken as it is when it starts: bypassing \
         or removing a slot while it renders changes the preview and the next Apply, not this \
         one. While the rack is synced with this file's track (set_rack_sync) the track would \
         play the printed effects again, so `turn_off_track` is required: true switches off \
         the track's inserts that were applied once the Apply lands (an undo step of the \
         project's own; the rack's copies follow), false applies anyway.",
        Safety::Edit,
        |host, input: ApplyEffects| {
            let s = host.session();
            let range = match (input.start, input.end) {
                (Some(a), Some(b)) => Some((a, b)),
                (None, None) => None,
                _ => return Err(error("give both `start` and `end`, or neither")),
            };
            let synced = s.rack_sync().is_some_and(|i| i.document == input.document);
            match (input.turn_off_track, synced) {
                (Some(true), _) => s.apply_effects_turning_off(input.document, range)?,
                (Some(false), _) => s.apply_effects(input.document, range)?,
                (None, false) => s.apply_effects(input.document, range)?,
                (None, true) => {
                    let track = s
                        .rack_sync()
                        .and_then(|i| s.project.track(i.track))
                        .map_or_else(|| "its track".to_string(), |t| t.name.clone());
                    return Err(error(format!(
                        "the effects rack is synced with {track}, which plays this file's clip \
                         through the same effects: give `turn_off_track` true to apply them \
                         into the file and switch them off on {track} (so they aren't heard \
                         twice), or false to apply anyway"
                    )));
                }
            }
            Ok(editor_state(s))
        },
    );
    r.add(
        "capture_noise_print",
        "Have a built-in effect learn from `start`..`end` (the file's own samples) of an open \
         audio file (`document`) what it learns from audio: a noise print for Noise Reduction or \
         Hiss Reduction (select the noise alone), or the clip \
         levels for Clip Repair (its Scan: select the clipped audio; each channel's highest \
         and lowest sample become the levels its bands sit under, and the runs found are \
         counted), or for the Click & Pop Repair one of its scans (`scan`: \"Scan For \
         Threshold Levels\", the default, sets Max Level to the highest level measured, Avg \
         Level to the lowest and Min Level to the mean; \"Scan For All \
         Levels\" also sets the six detection and rejection thresholds from a trial detection; \
         they are its parameters, to change after). It goes to `insert`, any such insert (a track's, the master's or the Waveform \
         rack's), or without one to the Waveform view's effects rack: its first effect that \
         learns (whichever kind), or a Noise Reduction added to it. Measured on the audio the \
         rack plays (the engine's sample rate; a noise print at the effect's FFT Size at the \
         time). The effect uses it from its next block, even while playing, and saves it with \
         its settings. Returns the insert and the effect.",
        Safety::Edit,
        |host, input: CaptureNoisePrint| {
            let s = host.session();
            if let Some(insert) = input.insert {
                check_insert(s, insert)?;
            }
            let insert = s.learn_from(
                input.document,
                (input.start, input.end),
                input.insert,
                input.scan.as_deref(),
            )?;
            let effect = s.native(insert).map_or("", |n| n.kind.name).to_string();
            Ok(Captured { insert, effect })
        },
    );
    r.add(
        "undo_audio_edit",
        "Take back the last edit to an open audio file (`document`), or with `redo` true bring \
         one back: its own history, not the project's. Returns the document; `done` names the \
         step, or is null at the history's end.",
        Safety::Edit,
        |host, input: StepDocument| {
            let s = host.session();
            let done = s.step_document(input.document, !input.redo)?;
            Ok(Stepped {
                done,
                document: document_view(s, input.document).expect("checked"),
            })
        },
    );
    r.add(
        "copy_audio",
        "Copy `start`..`end` (the file's own samples) of an open audio file (`document`) for \
         edit_audio's paste, into this file or another. Returns how many samples were copied.",
        Safety::Read,
        |host, input: CopyAudio| {
            let copied = host
                .session()
                .copy_document(input.document, (input.start, input.end))?;
            Ok(Copied { copied })
        },
    );
    r.add(
        "save_audio",
        "Write an open audio file's edits over the file itself, in its own WAV format (16- or \
         24-bit stays so). It can't be undone on disk except by undoing \
         the edits and saving again. Every clip on the file plays the new audio. FLAC, MP3 and \
         AIFF files are refused: use save_audio_as. Returns the document.",
        Safety::Destructive,
        |host, input: DocumentArg| {
            let s = host.session();
            s.save_document(input.document)?;
            Ok(document_view(s, input.document).expect("checked"))
        },
    );
    r.add(
        "save_audio_as",
        "Write an open audio file (`document`) to `path` (absolute, .wav; replaced if it \
         exists; its folder must exist) in the file's WAV format (32-bit float for a FLAC, MP3 \
         or AIFF), and carry on editing that file. Returns the document.",
        Safety::Destructive,
        |host, input: SaveAs| {
            let s = host.session();
            s.save_document_as(input.document, &input.path)?;
            Ok(document_view(s, input.document).expect("checked"))
        },
    );
    r.add(
        "get_metadata",
        "An open audio file's tags (`document`): title, artist, album, album_artist, year, \
         track (\"3\" or \"3/12\"), genre, composer, comment, and its cover art's type and \
         size (null without one). Read from the file when it opened; set_metadata changes them.",
        Safety::Read,
        |host, input: DocumentArg| {
            let s = host.session();
            let doc = s
                .document(input.document)
                .ok_or_else(|| error(format!("no open document with id {}", input.document)))?;
            Ok(metadata_view(&doc.metadata))
        },
    );
    r.add(
        "set_metadata",
        "Change an open audio file's tags (`document`): only the fields given change, and an \
         empty string clears one. `cover` is an absolute path to a JPEG or PNG image for the \
         front cover, or \"\" to remove it. The change is a step of the file's own history \
         (undo_audio_edit undoes it) and is written by save_audio and export_audio: ID3v2.3 \
         in an MP3, Vorbis comments in FLAC and Ogg, LIST INFO and ID3 chunks in a WAV, an ID3 \
         chunk in an AIFF. Returns the tags.",
        Safety::Edit,
        set_metadata,
    );
    r.add(
        "export_audio",
        "Export an open audio file (`document`) as it is now, unsaved edits included, to \
         `path` (absolute; overwritten) in another format, leaving the document and its file \
         as they are. Runs in the background: poll get_export_status. `format` (default a 32-bit float WAV): {\"type\":\"wav\",\"bits\":16|24|32}, {\"type\":\"aiff\",\"bits\":16|24}, {\"type\":\"flac\",\"bits\":16|24,\"level\":0-8}, {\"type\":\"mp3\",\"bitrate\":128|160|192|224|256|320} or {\"type\":\"mp3\",\"vbr\":0-9} (V0 best), {\"type\":\"ogg\",\"quality\":0-10}; `sample_rate` (Hz) converts first (an MP3 must be 32000, 44100 or 48000); `dither` (default true) adds TPDF dither of one bit when the format holds integers. A path without an extension gets the format's; one with another extension is refused.",
        Safety::Destructive,
        |host, input: ExportAudio| {
            let s = host.session();
            let encoding = crate::encode::Encoding {
                format: input.format.unwrap_or_default(),
                sample_rate: input.sample_rate,
                dither: input.dither.unwrap_or(true),
            };
            let path = crate::encode::with_extension(&input.path, &encoding.format)?;
            s.export_document(input.document, &path, encoding)?;
            Ok(ExportedTo { path })
        },
    );
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SetMetadata {
    document: DocumentId,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    artist: Option<String>,
    #[serde(default)]
    album: Option<String>,
    #[serde(default)]
    album_artist: Option<String>,
    #[serde(default)]
    year: Option<String>,
    /// "3", or "3/12".
    #[serde(default)]
    track: Option<String>,
    #[serde(default)]
    genre: Option<String>,
    #[serde(default)]
    composer: Option<String>,
    #[serde(default)]
    comment: Option<String>,
    /// Absolute path of a JPEG or PNG for the front cover; "" removes it.
    #[serde(default)]
    cover: Option<PathBuf>,
}

#[derive(Serialize, JsonSchema)]
struct CoverView {
    mime: String,
    bytes: usize,
}

#[derive(Serialize, JsonSchema)]
struct MetadataView {
    #[serde(flatten)]
    tags: crate::metadata::Metadata,
    cover: Option<CoverView>,
}

fn metadata_view(m: &crate::metadata::Metadata) -> MetadataView {
    MetadataView {
        tags: m.clone(),
        cover: m.cover.as_ref().map(|c| CoverView {
            mime: c.mime.clone(),
            bytes: c.data.len(),
        }),
    }
}

fn set_metadata(host: &mut dyn Host, input: SetMetadata) -> ApiResult<MetadataView> {
    let s = host.session();
    let doc = s
        .document(input.document)
        .ok_or_else(|| error(format!("no open document with id {}", input.document)))?;
    let mut m = (*doc.metadata).clone();
    for (field, value) in [
        (&mut m.title, input.title),
        (&mut m.artist, input.artist),
        (&mut m.album, input.album),
        (&mut m.album_artist, input.album_artist),
        (&mut m.year, input.year),
        (&mut m.track, input.track),
        (&mut m.genre, input.genre),
        (&mut m.composer, input.composer),
        (&mut m.comment, input.comment),
    ] {
        if let Some(value) = value {
            *field = value.trim().to_string();
        }
    }
    match input.cover {
        Some(path) if path.as_os_str().is_empty() => m.cover = None,
        Some(path) => m.cover = Some(crate::metadata::Cover::from_path(&path)?),
        None => {}
    }
    s.set_document_metadata(input.document, m)?;
    let doc = s.document(input.document).expect("checked");
    Ok(metadata_view(&doc.metadata))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ExportAudio {
    document: DocumentId,
    /// Absolute path of the file to write.
    path: PathBuf,
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

#[derive(Serialize, JsonSchema)]
struct ExportedTo {
    path: PathBuf,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct OpenEditor {
    /// An audio file, absolute path.
    #[serde(default)]
    path: Option<PathBuf>,
    /// An audio clip: its playing take's file.
    #[serde(default)]
    clip: Option<ClipId>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DocumentArg {
    document: DocumentId,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloseDocument {
    document: DocumentId,
    /// Close even with unsaved edits, losing them.
    #[serde(default)]
    discard: bool,
}

fn default_play() -> bool {
    true
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PlayDocument {
    document: DocumentId,
    /// Play (true, the default) or stop (false).
    #[serde(default = "default_play")]
    play: bool,
    /// From this sample of the file (default 0).
    #[serde(default)]
    start: Option<u64>,
    /// To this sample (default: the end).
    #[serde(default)]
    end: Option<u64>,
}

#[derive(Deserialize, JsonSchema, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum Op {
    Delete,
    Trim,
    InsertSilence,
    Silence,
    Amplify,
    NormalizePeak,
    NormalizeLoudness,
    FadeIn,
    FadeOut,
    Reverse,
    Invert,
    Paste,
    SpectralAttenuate,
    SpectralRemove,
    SpectralHeal,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct EditAudio {
    document: DocumentId,
    op: Op,
    /// The range's start, the file's own samples.
    #[serde(default)]
    start: Option<u64>,
    /// The range's end.
    #[serde(default)]
    end: Option<u64>,
    /// Only these channels (from 0); every channel when left out.
    #[serde(default)]
    channels: Option<Vec<usize>>,
    /// amplify, spectral_attenuate: the gain in dB.
    #[serde(default)]
    db: Option<f64>,
    /// normalize_peak: the loudest sample's level, dBFS.
    #[serde(default)]
    peak_db: Option<f64>,
    /// normalize_loudness: the target, LUFS.
    #[serde(default)]
    lufs: Option<f64>,
    /// fade_in / fade_out: the curve (default equal_power).
    #[serde(default)]
    shape: Option<FadeShape>,
    /// insert_silence / paste: where, the file's own samples.
    #[serde(default)]
    at: Option<u64>,
    /// insert_silence: how many samples.
    #[serde(default)]
    length: Option<u64>,
    /// spectral_*: the box's lowest frequency, Hz.
    #[serde(default)]
    low_hz: Option<f64>,
    /// spectral_*: the box's highest frequency, Hz (at most half the sample rate).
    #[serde(default)]
    high_hz: Option<f64>,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CaptureNoisePrint {
    document: DocumentId,
    /// The first sample to learn from (the noise alone, or the clipped audio), the file's own
    /// samples.
    start: u64,
    /// Its end.
    end: u64,
    /// The effect to learn it (one that learns from audio); the Waveform rack's when left out.
    #[serde(default)]
    insert: Option<InsertId>,
    /// For the Click & Pop Repair: which scan, "Scan For Threshold Levels" (the default) or
    /// "Scan For All Levels" (or a unique part of the name, "all levels").
    #[serde(default)]
    scan: Option<String>,
}

#[derive(Serialize, JsonSchema)]
struct Captured {
    insert: InsertId,
    effect: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetSpectrum {
    document: DocumentId,
    /// From this sample of the file (default 0).
    #[serde(default)]
    start: Option<u64>,
    /// To this sample (default: the end).
    #[serde(default)]
    end: Option<u64>,
    /// One channel (from 0); all mixed when left out.
    #[serde(default)]
    channel: Option<usize>,
    /// How many bands (default 32).
    #[serde(default)]
    bands: Option<usize>,
    /// FFT size, a power of two from 256 to 32768 (default 4096).
    #[serde(default)]
    fft: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
struct Spectrum {
    bands: Vec<SpectrumBand>,
    fft: usize,
    frames: usize,
}

#[derive(Serialize, JsonSchema)]
struct SpectrumBand {
    low_hz: f64,
    high_hz: f64,
    db: f32,
}

fn get_spectrum(host: &mut dyn Host, input: GetSpectrum) -> ApiResult<Spectrum> {
    let s = host.session();
    let doc = s
        .document(input.document)
        .ok_or_else(|| error(format!("no open audio file with id {}", input.document)))?;
    let audio = &doc.audio;
    let len = audio.len() as u64;
    let (start, end) = (input.start.unwrap_or(0), input.end.unwrap_or(len).min(len));
    if end <= start {
        return Err(error(format!(
            "nothing to measure: {start}..{end} (the file has {len} samples)"
        )));
    }
    let fft = input.fft.unwrap_or(4096);
    if !fft.is_power_of_two() || !(256..=32_768).contains(&fft) {
        return Err(error(format!(
            "fft must be a power of two from 256 to 32768, not {fft}"
        )));
    }
    let count = input.bands.unwrap_or(32);
    if !(1..=512).contains(&count) {
        return Err(error(format!("bands must be 1 to 512, not {count}")));
    }
    let samples = match input.channel {
        Some(c) => audio.channels.get(c).cloned().ok_or_else(|| {
            error(format!(
                "the file has channels 0 to {}; there is no channel {c}",
                audio.channels.len().saturating_sub(1)
            ))
        })?,
        None => crate::analysis::mono(&audio.channels, 0, audio.len()),
    };
    let (bands, frames) = crate::spectrum::bands(
        &samples,
        audio.sample_rate,
        start as usize,
        end as usize,
        count,
        fft,
    );
    Ok(Spectrum {
        bands: bands
            .into_iter()
            .map(|b| SpectrumBand {
                low_hz: b.low_hz,
                high_hz: b.high_hz,
                db: b.db,
            })
            .collect(),
        fft,
        frames,
    })
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ApplyEffects {
    document: DocumentId,
    /// From this sample of the file.
    #[serde(default)]
    start: Option<u64>,
    /// To this sample.
    #[serde(default)]
    end: Option<u64>,
    /// While the rack is synced with this file's track: switch the applied effects off on
    /// the track once the Apply lands (true), or apply anyway (false). Required then.
    #[serde(default)]
    turn_off_track: Option<bool>,
}

#[derive(Serialize, JsonSchema)]
struct RackSlotView {
    insert: crate::project::InsertId,
    name: String,
    bypass: bool,
    latency: Option<usize>,
    /// Its plug-in is in (false: still loading, or it failed to: `error`).
    loaded: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
    /// While synced: the track's insert this slot mirrors.
    #[serde(skip_serializing_if = "Option::is_none")]
    track_insert: Option<crate::project::InsertId>,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct SourceView {
    track: crate::project::TrackId,
    clip: ClipId,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct SyncView {
    document: DocumentId,
    track: crate::project::TrackId,
    track_name: String,
    clip: ClipId,
}

pub(super) fn sync_view(s: &Session) -> Option<SyncView> {
    let info = s.rack_sync()?;
    Some(SyncView {
        document: info.document,
        track: info.track,
        track_name: s
            .project
            .track(info.track)
            .map(|t| t.name.clone())
            .unwrap_or_default(),
        clip: info.clip,
    })
}

#[derive(Serialize, JsonSchema)]
struct Applying {
    document: DocumentId,
    progress: f32,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct StepDocument {
    document: DocumentId,
    /// Bring an undone edit back instead.
    #[serde(default)]
    redo: bool,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CopyAudio {
    document: DocumentId,
    start: u64,
    end: u64,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SaveAs {
    document: DocumentId,
    /// Where to write it: absolute path to a .wav.
    path: PathBuf,
}

#[derive(Serialize, JsonSchema)]
struct Format {
    bits: u16,
    float: bool,
}

#[derive(Serialize, JsonSchema)]
struct History {
    steps: Vec<String>,
    showing: usize,
    saved: Option<usize>,
}

#[derive(Serialize, JsonSchema)]
pub(super) struct DocumentView {
    document: DocumentId,
    path: PathBuf,
    sample_rate: u32,
    channels: usize,
    length: u64,
    seconds: f64,
    format: Option<Format>,
    dirty: bool,
    history: History,
    clips: Vec<ClipId>,
    /// The clip it was opened from and that clip's track.
    source: Option<SourceView>,
}

#[derive(Serialize, JsonSchema)]
struct Playing {
    document: DocumentId,
    position: u64,
}

#[derive(Serialize, JsonSchema)]
struct EditorState {
    documents: Vec<DocumentView>,
    playing: Option<Playing>,
    /// The effects rack the preview plays through and Apply renders.
    rack: Vec<RackSlotView>,
    /// The track the rack is synced with, if it is.
    sync: Option<SyncView>,
    applying: Option<Applying>,
}

#[derive(Serialize, JsonSchema)]
struct Stepped {
    done: Option<String>,
    document: DocumentView,
}

#[derive(Serialize, JsonSchema)]
struct Copied {
    copied: u64,
}

pub(super) fn document_view(s: &Session, id: DocumentId) -> Option<DocumentView> {
    let doc = s.document(id)?;
    let length = doc.audio.len() as u64;
    let (steps, showing, saved) = doc.history();
    Some(DocumentView {
        document: doc.id,
        path: doc.path.clone(),
        sample_rate: doc.audio.sample_rate,
        channels: doc.audio.channels.len(),
        length,
        seconds: length as f64 / f64::from(doc.audio.sample_rate.max(1)),
        format: doc.format.map(|f| Format {
            bits: f.bits,
            float: f.float,
        }),
        dirty: doc.is_dirty(),
        history: History {
            steps,
            showing,
            saved,
        },
        clips: s.clips_using(&doc.path),
        source: doc.source.map(|(track, clip)| SourceView { track, clip }),
    })
}

fn editor_state(s: &Session) -> EditorState {
    EditorState {
        documents: s
            .documents()
            .iter()
            .filter_map(|d| document_view(s, d.id))
            .collect(),
        playing: s
            .preview_state()
            .map(|(document, position)| Playing { document, position }),
        rack: s
            .editor_rack()
            .iter()
            .map(|slot| RackSlotView {
                insert: slot.id,
                name: slot.name.clone(),
                bypass: slot.bypass,
                latency: s.insert_info(slot.id).map(|(_, latency, _)| latency),
                loaded: s.is_loaded(slot.id),
                error: s.load_failure(slot.id).map(str::to_string),
                track_insert: s.rack_counterpart(slot.id),
            })
            .collect(),
        sync: sync_view(s),
        applying: s
            .apply_state()
            .map(|(document, progress)| Applying { document, progress }),
    }
}

fn open_audio_editor(host: &mut dyn Host, input: OpenEditor) -> ApiResult<DocumentView> {
    let s = host.session();
    let id = match (input.path, input.clip) {
        (Some(path), None) => s.open_document(&path)?,
        (None, Some(clip)) => {
            check_clip(s, clip)?;
            s.open_clip_document(clip)?
        }
        _ => return Err(error("give either `path` or `clip`")),
    };
    Ok(document_view(s, id).expect("just opened"))
}

/// The action's fields as an [`Edit`], refusing what the op doesn't take or lacks.
fn to_edit(s: &Session, input: &EditAudio) -> ApiResult<Edit> {
    let name = op_name;
    let op = input.op;
    let range = match (input.start, input.end) {
        (Some(a), Some(b)) => Some((a, b)),
        (None, None) => None,
        _ => return Err(error("give both `start` and `end`, or neither")),
    };
    let needs_range =
        || range.ok_or_else(|| error(format!("{} needs `start` and `end`", name(op))));
    let channels = input.channels.clone();
    let no_channels = |op: Op| -> ApiResult<()> {
        if channels.is_some() {
            return Err(error(format!(
                "{} changes the file's length, so it applies to every channel: leave out \
                 `channels`",
                name(op)
            )));
        }
        Ok(())
    };
    let shape = input.shape.unwrap_or_default();
    Ok(match op {
        Op::Delete => {
            no_channels(op)?;
            Edit::Delete {
                range: needs_range()?,
            }
        }
        Op::Trim => {
            no_channels(op)?;
            Edit::Trim {
                range: needs_range()?,
            }
        }
        Op::InsertSilence => {
            no_channels(op)?;
            Edit::InsertSilence {
                at: input.at.ok_or_else(|| error("insert_silence needs `at`"))?,
                length: input
                    .length
                    .filter(|&l| l > 0)
                    .ok_or_else(|| error("insert_silence needs a `length` above 0"))?,
            }
        }
        Op::Silence => Edit::Silence { range, channels },
        Op::Amplify => Edit::Amplify {
            range,
            channels,
            db: input.db.ok_or_else(|| error("amplify needs `db`"))?,
        },
        Op::NormalizePeak => Edit::NormalizePeak {
            range,
            channels,
            peak_db: input.peak_db.unwrap_or(0.0),
        },
        Op::NormalizeLoudness => Edit::NormalizeLoudness {
            range,
            channels,
            lufs: input
                .lufs
                .ok_or_else(|| error("normalize_loudness needs `lufs`"))?,
        },
        Op::FadeIn => Edit::FadeIn {
            range: needs_range()?,
            channels,
            shape,
        },
        Op::FadeOut => Edit::FadeOut {
            range: needs_range()?,
            channels,
            shape,
        },
        Op::Reverse => Edit::Reverse { range, channels },
        Op::Invert => Edit::Invert { range, channels },
        Op::SpectralAttenuate | Op::SpectralRemove | Op::SpectralHeal => {
            let (low_hz, high_hz) = match (input.low_hz, input.high_hz) {
                (Some(l), Some(h)) => (l, h),
                _ => {
                    return Err(error(format!(
                        "{} needs the band: `low_hz` and `high_hz`",
                        name(op)
                    )))
                }
            };
            Edit::Spectral {
                range: needs_range()?,
                channels,
                low_hz,
                high_hz,
                op: match op {
                    Op::SpectralAttenuate => SpectralOp::Attenuate {
                        db: input
                            .db
                            .ok_or_else(|| error("spectral_attenuate needs `db`"))?,
                    },
                    Op::SpectralRemove => SpectralOp::Remove,
                    _ => SpectralOp::Heal,
                },
            }
        }
        Op::Paste => {
            no_channels(op)?;
            let clip = s
                .audio_clipboard()
                .cloned()
                .ok_or_else(|| error("nothing has been copied (copy_audio)"))?;
            Edit::Paste {
                at: input.at.or(range.map(|r| r.0)).unwrap_or(0),
                replace: range,
                clip,
            }
        }
    })
}

/// An [`Op`] as its JSON name, for messages.
fn op_name(op: Op) -> &'static str {
    match op {
        Op::Delete => "delete",
        Op::Trim => "trim",
        Op::InsertSilence => "insert_silence",
        Op::Silence => "silence",
        Op::Amplify => "amplify",
        Op::NormalizePeak => "normalize_peak",
        Op::NormalizeLoudness => "normalize_loudness",
        Op::FadeIn => "fade_in",
        Op::FadeOut => "fade_out",
        Op::Reverse => "reverse",
        Op::Invert => "invert",
        Op::Paste => "paste",
        Op::SpectralAttenuate => "spectral_attenuate",
        Op::SpectralRemove => "spectral_remove",
        Op::SpectralHeal => "spectral_heal",
    }
}

fn edit_audio(host: &mut dyn Host, input: EditAudio) -> ApiResult<DocumentView> {
    let s = host.session();
    if s.document(input.document).is_none() {
        return Err(error(format!(
            "no open audio file with id {}",
            input.document
        )));
    }
    let edit = to_edit(s, &input)?;
    s.edit_document(input.document, &edit)?;
    Ok(document_view(s, input.document).expect("checked"))
}
