//! Files dropped on the arrangement from Explorer: audio files become audio clips and MIDI
//! files MIDI clips, on the track under the pointer and the ones after it, at the time under
//! the pointer.
//!
//! - **Into a track, or new ones (16.2):** dropped where a file would land on a track that
//!   can take it, the app asks first: into that track (a template's, with its plug-ins), or
//!   new tracks. Where none can, new tracks are made without asking, one per file track.
//! - **The tempo:** a MIDI file whose tempo or time signature differs from the project's asks
//!   then whether to take them (an audio file carries no tempo, so it never asks).
//! - **Where the pointer is:** winit 0.30 sends no position while files from Explorer hover
//!   over the window, so Windows is asked ([`os_pointer`]); a ghost of each clip follows it
//!   ([`Ghost`], drawn by the arrangement).
//!
//! File > Import audio and Import MIDI go the same way, at the playhead on the selected track.

use crate::app::DawApp;
use daw_core::project::{TrackId, TrackKind};
use daw_core::smf::{self, MidiFile, MidiFileTrack, TempoSettings};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// A dropped file, sorted by what it holds (recognised by content, not the extension).
pub enum Dropped {
    Audio(PathBuf),
    Midi { path: PathBuf, file: MidiFile },
}

impl Dropped {
    /// Read enough of `path` to know what it is: a MIDI file is read whole.
    pub fn open(path: PathBuf) -> Result<Self, String> {
        let head = read_head(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if smf::is_midi_file(&head) {
            let file = smf::read_path(&path).map_err(|e| format!("{e:#}"))?;
            if file.tracks.is_empty() {
                return Err(format!("{} has no notes", path.display()));
            }
            Ok(Self::Midi { path, file })
        } else {
            Ok(Self::Audio(path))
        }
    }

    /// The tracks it needs, in order: one for an audio file, one per MIDI file track.
    fn slots(&self) -> Vec<TrackKind> {
        match self {
            Self::Audio(_) => vec![TrackKind::Audio],
            Self::Midi { file, .. } => vec![TrackKind::Instrument; file.tracks.len()],
        }
    }
}

/// A file's first four bytes (enough to tell a MIDI file), without reading the rest.
fn read_head(path: &Path) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    let mut head = Vec::with_capacity(4);
    std::fs::File::open(path)?.take(4).read_to_end(&mut head)?;
    Ok(head)
}

/// Which existing track each slot lands on: the row under the pointer (`start`) and the rows
/// after it, one slot each, when they're the right kind; `None` makes a new track there.
/// Dropped below the last row (`start` is `None`), every slot gets a new track.
pub fn plan(
    rows: &[(TrackId, TrackKind)],
    start: Option<usize>,
    slots: &[TrackKind],
) -> Vec<Option<TrackId>> {
    slots
        .iter()
        .enumerate()
        .map(|(k, kind)| {
            let row = rows.get(start? + k)?;
            (row.1 == *kind).then_some(row.0)
        })
        .collect()
}

/// A drop waiting for its questions' answers: into the tracks or new ones, then the tempo.
pub struct PendingDrop {
    files: Vec<Dropped>,
    /// Where it was dropped, in samples (snapped).
    at: u64,
    rows: Vec<(TrackId, TrackKind)>,
    start: Option<usize>,
    /// Asked first, while unanswered: land on the existing tracks, or make new ones.
    pub into: Option<IntoQuestion>,
    /// The answer: the existing tracks (true, until answered otherwise).
    existing: bool,
    /// Then, for a MIDI file with a tempo of its own: its settings and the question.
    pub tempo: Option<(TempoSettings, String)>,
}

/// "Import riff.wav into Guitar L?": the question and its two buttons.
#[derive(Debug, Clone, PartialEq)]
pub struct IntoQuestion {
    pub text: String,
    pub into: String,
    pub new: String,
}

/// The question to ask when `files` (their names) dropped as planned (`targets`, one per file
/// track) would land on existing tracks (named by `name`): None when every one gets a new
/// track, which needs no asking.
pub fn into_question(
    files: &[String],
    targets: &[Option<TrackId>],
    name: &dyn Fn(TrackId) -> String,
) -> Option<IntoQuestion> {
    let existing: Vec<TrackId> = targets.iter().flatten().copied().collect();
    let first = *existing.first()?;
    let what = match files {
        [one] => one.clone(),
        many => format!("{} files", many.len()),
    };
    let new = if targets.len() == 1 {
        "New track"
    } else {
        "New tracks"
    };
    Some(if targets.len() == 1 {
        let track = name(first);
        IntoQuestion {
            text: format!("Import {what} into {track}, or onto a new track?"),
            into: format!("Into {track}"),
            new: new.into(),
        }
    } else {
        let names: Vec<String> = existing.iter().map(|&t| name(t)).collect();
        let more = targets.len() - existing.len();
        let mut text = format!("Import {what} into {}", list(&names));
        if more > 0 {
            text.push_str(&format!(
                " (and {more} new track{} for the rest)",
                if more == 1 { "" } else { "s" }
            ));
        }
        text.push_str(", or onto new tracks?");
        IntoQuestion {
            text,
            into: "Into these tracks".into(),
            new: new.into(),
        }
    })
}

/// "A", "A and B", "A, B and C".
fn list(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// Handle files dropped at sample `at` over row `start` of `rows` (the visible tracks):
/// place them now, or ask about the tempo first.
pub fn drop_files(
    app: &mut DawApp,
    paths: Vec<PathBuf>,
    at: u64,
    rows: Vec<(TrackId, TrackKind)>,
    start: Option<usize>,
) {
    let mut files = Vec::new();
    for path in paths {
        match Dropped::open(path) {
            Ok(file) => files.push(file),
            Err(e) => app.toast(e),
        }
    }
    if files.is_empty() {
        return;
    }
    // Onto existing tracks: ask first.
    let slots: Vec<TrackKind> = files.iter().flat_map(Dropped::slots).collect();
    let targets = plan(&rows, start, &slots);
    let names: Vec<String> = files
        .iter()
        .map(|f| {
            let path = match f {
                Dropped::Audio(p) | Dropped::Midi { path: p, .. } => p,
            };
            path.file_name()
                .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
        })
        .collect();
    let project = &app.session.project;
    let track_name = |t: TrackId| {
        project
            .track(t)
            .map_or_else(String::new, |t| t.name.clone())
    };
    let into = into_question(&names, &targets, &track_name);
    let midi: Vec<(&PathBuf, TempoSettings)> = files
        .iter()
        .filter_map(|f| match f {
            Dropped::Midi { path, file } => file.tempo_settings().map(|s| (path, s)),
            Dropped::Audio(_) => None,
        })
        .collect();
    let project = &app.session.project;
    // The first MIDI file that sets a tempo map decides; only ask if it differs.
    let ask = midi.first().filter(|(_, s)| {
        s.tempo != project.tempo
            || s.time_signature != project.time_signature
            || !s.changes.is_empty()
    });
    let mut tempo = None;
    if let Some((path, settings)) = ask {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let (n, d) = settings.time_signature;
        let mut question = format!(
            "{name} is at {} BPM in {n}/{d}. Set the project's tempo and time signature from it?",
            trim(settings.tempo)
        );
        if !settings.changes.is_empty() {
            question.push_str(&format!(
                " It also changes {} time{} later on.",
                settings.changes.len(),
                if settings.changes.len() == 1 { "" } else { "s" }
            ));
        }
        if midi.len() > 1 {
            question.push_str(" (The first of the dropped MIDI files decides.)");
        }
        tempo = Some((settings.clone(), question));
    }
    if into.is_none() && tempo.is_none() {
        place(app, files, at, &rows, start, true, None);
        return;
    }
    app.pending_drop = Some(PendingDrop {
        files,
        at,
        rows,
        start,
        into,
        existing: true,
        tempo,
    });
}

/// The answer to the first question: into the existing tracks (true) or new ones (false);
/// `None` cancels the drop. The tempo question comes next if there is one.
pub fn answer_into(app: &mut DawApp, existing: Option<bool>) {
    let Some(existing) = existing else {
        app.pending_drop = None;
        return;
    };
    let Some(pending) = app.pending_drop.as_mut() else {
        return;
    };
    pending.into = None;
    pending.existing = existing;
    if pending.tempo.is_none() {
        let pending = app.pending_drop.take().expect("checked");
        place(
            app,
            pending.files,
            pending.at,
            &pending.rows,
            pending.start,
            pending.existing,
            None,
        );
    }
}

/// "140" for 140.0, "92.5" for 92.5.
fn trim(bpm: f64) -> String {
    let text = format!("{bpm:.3}");
    text.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// The answer: place the pending drop, with the file's tempo map or without. `None` cancels.
pub fn answer(app: &mut DawApp, use_file_tempo: Option<bool>) {
    let Some(pending) = app.pending_drop.take() else {
        return;
    };
    let Some(use_tempo) = use_file_tempo else {
        return;
    };
    let settings = pending.tempo.filter(|_| use_tempo).map(|(s, _)| s);
    place(
        app,
        pending.files,
        pending.at,
        &pending.rows,
        pending.start,
        pending.existing,
        settings,
    );
}

fn place(
    app: &mut DawApp,
    files: Vec<Dropped>,
    at: u64,
    rows: &[(TrackId, TrackKind)],
    start: Option<usize>,
    existing: bool,
    tempo: Option<TempoSettings>,
) {
    let slots: Vec<TrackKind> = files.iter().flat_map(Dropped::slots).collect();
    // New tracks asked for: none of the existing ones (they go after the one dropped on).
    let targets: Vec<Option<TrackId>> = if existing {
        plan(rows, start, &slots)
    } else {
        vec![None; slots.len()]
    };
    app.session.label_edit("Drop files", None);
    // MIDI lands on the bar and beat it was dropped on, through a tempo change too.
    let before = app.session.tempo_map();
    let mut tick = before.tick_at(at as f64).round().max(0.0) as u64;
    if let Some(settings) = tempo {
        let bar = before.bar_of_tick(tick);
        let into = tick - before.tick_of_bar(bar, 0);
        match app
            .session
            .apply_tempo_settings(&settings, bar.max(0) as u32)
        {
            Ok(()) => tick = app.session.tempo_map().tick_of_bar(bar, 0) + into,
            Err(e) => app.toast(format!("{e:#}")),
        }
        if settings.moved > 0 {
            app.toast(format!(
                "{} tempo or meter changes were moved onto a beat or bar line",
                settings.moved
            ));
        }
    }
    let mut after = start.and_then(|i| rows.get(i)).map(|r| r.0);
    let mut targets = targets.into_iter();
    for file in &files {
        match file {
            Dropped::Audio(path) => {
                let track = match targets.next().flatten() {
                    Some(t) => t,
                    None => app
                        .session
                        .project
                        .insert_track_after(TrackKind::Audio, after),
                };
                if let Err(e) = app.session.add_clip_from_file(track, path, at) {
                    app.toast(format!("{e:#}"));
                }
                after = Some(track);
            }
            Dropped::Midi { path, file } => {
                let stem = path
                    .file_stem()
                    .map_or_else(|| "MIDI".into(), |s| s.to_string_lossy().into_owned());
                // New tracks play the instrument of the track the file landed on, each
                // with its own copy (a piano's two staves both on the piano).
                let mut instrument = None;
                for (file_track, name) in file.tracks.iter().zip(file.track_names(&stem)) {
                    let track = match targets.next().flatten() {
                        Some(t) => {
                            instrument = instrument.or_else(|| app.session.instrument_of(t));
                            t
                        }
                        None => {
                            let new = app
                                .session
                                .project
                                .insert_track_after(TrackKind::Instrument, after);
                            if let Some(t) = app.session.project.track_mut(new) {
                                t.name = name.clone();
                                t.arm = true;
                            }
                            if let Some(id) = instrument {
                                let copied = app.session.copy_insert(
                                    id,
                                    daw_core::session::InsertTarget::Track(new),
                                    0,
                                );
                                app.report(copied.map(|_| ()));
                            }
                            new
                        }
                    };
                    if let Err(e) = app
                        .session
                        .import_midi_track(track, file_track, tick, &name)
                    {
                        app.toast(format!("{e:#}"));
                    }
                    after = Some(track);
                }
            }
        }
    }
    app.session.mark_dirty();
}

/// Where the pointer is, from Windows, while files from Explorer hover over the window (or
/// drop): in the window's points, `hwnd` being its handle and `ppp` egui's pixels per point.
/// None where it can't be read.
pub fn os_pointer(hwnd: Option<isize>, ppp: f32) -> Option<egui::Pos2> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::Graphics::Gdi::ScreenToClient;
        use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let hwnd = hwnd?;
        let mut p = POINT { x: 0, y: 0 };
        // SAFETY: `p` is ours to fill; `hwnd` is the app's own window, alive while it runs
        // (and a stale handle only makes ScreenToClient fail).
        let ok = unsafe { GetCursorPos(&mut p) != 0 && ScreenToClient(hwnd as _, &mut p) != 0 };
        ok.then(|| egui::pos2(p.x as f32 / ppp, p.y as f32 / ppp))
    }
    #[cfg(not(windows))]
    {
        let _ = (hwnd, ppp);
        None
    }
}

/// The clips files hovering over the window would make, for the ghost the arrangement draws
/// under the pointer: one per file track, its kind, name and length. An audio file's length
/// is read on a thread of its own (decoding takes a moment); until then it is `None`.
pub struct Ghost {
    pub paths: Vec<PathBuf>,
    pub clips: Vec<GhostClip>,
    lengths: Arc<Mutex<Vec<Option<u64>>>>,
}

pub struct GhostClip {
    pub kind: TrackKind,
    pub name: String,
    length: GhostLength,
}

enum GhostLength {
    /// Index into the audio lengths, at the project's rate.
    Audio(usize),
    /// Ticks.
    Midi(u64),
}

impl Ghost {
    /// The ghost of `paths`, its audio files' lengths read at `rate` meanwhile.
    pub fn new(paths: Vec<PathBuf>, rate: u32) -> Self {
        let mut clips = Vec::new();
        let mut audio = Vec::new();
        for path in &paths {
            let stem = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            let midi = read_head(path).is_ok_and(|h| smf::is_midi_file(&h));
            let file = midi.then(|| smf::read_path(path).ok()).flatten();
            match file {
                Some(file) => {
                    for (track, name) in file.tracks.iter().zip(file.track_names(&stem)) {
                        clips.push(GhostClip {
                            kind: TrackKind::Instrument,
                            name,
                            length: GhostLength::Midi(midi_length(track)),
                        });
                    }
                }
                None if midi => {}
                None => {
                    clips.push(GhostClip {
                        kind: TrackKind::Audio,
                        name: stem,
                        length: GhostLength::Audio(audio.len()),
                    });
                    audio.push(path.clone());
                }
            }
        }
        let lengths = Arc::new(Mutex::new(vec![None; audio.len()]));
        if !audio.is_empty() {
            let out = lengths.clone();
            std::thread::spawn(move || {
                for (k, path) in audio.iter().enumerate() {
                    let length = daw_core::media::read_audio(path).ok().map(|a| {
                        let frames = a.channels.first().map_or(0, Vec::len) as u64;
                        frames * u64::from(rate) / u64::from(a.sample_rate.max(1))
                    });
                    if let Ok(mut lengths) = out.lock() {
                        lengths[k] = length;
                    }
                }
            });
        }
        Self {
            paths,
            clips,
            lengths,
        }
    }

    /// Clip `k`'s length in samples, starting at sample `at` (MIDI through `map`); None while
    /// an audio file's is still being read.
    pub fn length(&self, k: usize, at: u64, map: &daw_core::tempo::TempoMap) -> Option<u64> {
        match self.clips.get(k)?.length {
            GhostLength::Audio(i) => self.lengths.lock().ok()?.get(i).copied().flatten(),
            GhostLength::Midi(ticks) => {
                let from = map.tick_at(at as f64);
                Some((map.sample_at_tick(from + ticks as f64) - at as f64).max(0.0) as u64)
            }
        }
    }
}

/// How long a MIDI file track runs, in ticks: its last note's end or last event.
fn midi_length(track: &MidiFileTrack) -> u64 {
    track
        .notes
        .iter()
        .map(|n| n.tick + n.length)
        .chain(track.controllers.iter().map(|e| e.tick + 1))
        .chain(std::iter::once(track.end))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrackKind::{Audio, Folder, Instrument};

    #[test]
    fn dropping_onto_existing_tracks_asks_and_new_tracks_dont() {
        let name = |t: TrackId| format!("Track {t}");
        let files = |n: &[&str]| n.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // One file onto a track that takes it.
        let one = into_question(&files(&["mix.wav"]), &[Some(4)], &name).unwrap();
        assert_eq!(
            one.text,
            "Import mix.wav into Track 4, or onto a new track?"
        );
        assert_eq!(
            (one.into.as_str(), one.new.as_str()),
            ("Into Track 4", "New track")
        );
        // Two staves, one landing on an existing track and one needing a new one.
        let piano = into_question(&files(&["piano.mid"]), &[Some(2), None], &name).unwrap();
        assert_eq!(
            piano.text,
            "Import piano.mid into Track 2 (and 1 new track for the rest), or onto new tracks?"
        );
        assert_eq!(piano.into, "Into these tracks");
        let two = into_question(&files(&["a.wav", "b.wav"]), &[Some(1), Some(4)], &name);
        assert_eq!(
            two.unwrap().text,
            "Import 2 files into Track 1 and Track 4, or onto new tracks?"
        );
        // Nothing would land on an existing track (the wrong kind, below the rows): no
        // question, new tracks.
        assert_eq!(into_question(&files(&["mix.wav"]), &[None], &name), None);
        assert_eq!(
            into_question(&files(&["piano.mid"]), &[None, None], &name),
            None
        );
        // As `plan` decides it: a WAV over an instrument track needs a new one.
        let rows = [(1, Audio), (2, Instrument)];
        assert_eq!(
            into_question(&files(&["mix.wav"]), &plan(&rows, Some(1), &[Audio]), &name),
            None
        );
    }

    #[test]
    fn files_land_on_the_track_under_the_pointer_and_the_ones_after_it() {
        let rows = [(1, Audio), (2, Instrument), (3, Folder), (4, Audio)];
        // An audio file and a two-track MIDI file dropped on row 0: the audio file on track
        // 1, the MIDI tracks on 2 (an instrument) and a new one (3 is a folder).
        assert_eq!(
            plan(&rows, Some(0), &[Audio, Instrument, Instrument]),
            [Some(1), Some(2), None]
        );
        // On the last row: the first fits; the rest run off the end.
        assert_eq!(plan(&rows, Some(3), &[Audio, Audio]), [Some(4), None]);
        // On a row of the wrong kind: a new track.
        assert_eq!(plan(&rows, Some(1), &[Audio]), [None]);
        // Below the last row: all new.
        assert_eq!(plan(&rows, None, &[Audio, Instrument]), [None, None]);
    }

    #[test]
    fn tempos_read_as_people_write_them() {
        assert_eq!(trim(140.0), "140");
        assert_eq!(trim(92.5), "92.5");
    }
}
