//! The start screen's recent projects (dev plan 17.7): whether each is still there and when it
//! was last saved, found on workers so a slow or disconnected drive never holds up a frame,
//! and the words and shortened paths the list shows.
//!
//! [`Probe`] stats each path on a thread of its own when the start screen shows (or the list
//! changes), polled in `logic`; the screen only reads what has come back. An entry not yet
//! answered shows as "Checking…".

use chrono::{DateTime, Local};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, SystemTime};

/// What a look at a recent project's file found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Found {
    /// There, last written then (if the file system says).
    Present(Option<SystemTime>),
    /// Gone, or its drive isn't there.
    Missing,
}

/// Look at `path` (on a worker: this can block on a network drive).
pub fn look(path: &Path) -> Found {
    match std::fs::metadata(path) {
        Ok(meta) if meta.is_file() => Found::Present(meta.modified().ok()),
        _ => Found::Missing,
    }
}

/// The recent projects' files, looked at on workers.
pub struct Probe {
    wake: Option<egui::Context>,
    sender: Sender<(u64, PathBuf, Found)>,
    receiver: Receiver<(u64, PathBuf, Found)>,
    /// Bumped by each look; older answers are dropped.
    serial: u64,
    /// The list the last look was for (`None`: look again when next wanted).
    asked: Option<Vec<PathBuf>>,
    found: HashMap<PathBuf, Found>,
    /// How many of the last look's answers have come.
    answered: usize,
}

impl Probe {
    /// `wake`: repainted when an answer comes (none in tests).
    pub fn new(wake: Option<egui::Context>) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            wake,
            sender,
            receiver,
            serial: 0,
            asked: None,
            found: HashMap::new(),
            answered: 0,
        }
    }

    /// Look at `paths` unless the last look was at this same list. Answers from before stay
    /// shown until the new ones come, so the list doesn't flicker.
    pub fn want(&mut self, paths: &[PathBuf]) {
        if self.asked.as_deref() == Some(paths) {
            return;
        }
        self.serial += 1;
        self.answered = 0;
        self.asked = Some(paths.to_vec());
        for path in paths {
            let (serial, path, sender) = (self.serial, path.clone(), self.sender.clone());
            let wake = self.wake.clone();
            // A thread each: a drive that hangs holds up only its own projects.
            let _ = std::thread::Builder::new()
                .name("recent-probe".into())
                .spawn(move || {
                    let found = look(&path);
                    let _ = sender.send((serial, path, found));
                    if let Some(wake) = wake {
                        wake.request_repaint();
                    }
                });
        }
    }

    /// The next [`want`](Self::want) looks again, even at the same list (the start screen
    /// closed: files may change before it shows again).
    pub fn forget(&mut self) {
        self.asked = None;
    }

    /// Take the workers' answers (each frame, in `logic`).
    pub fn poll(&mut self) {
        while let Ok((serial, path, found)) = self.receiver.try_recv() {
            if serial == self.serial {
                self.found.insert(path, found);
                self.answered += 1;
            }
        }
    }

    /// Wait up to `limit` for the answers (an agent's list, and checks).
    pub fn settle(&mut self, limit: Duration) {
        let start = std::time::Instant::now();
        loop {
            self.poll();
            // This look's answers, not the ones shown from before it.
            let done = self
                .asked
                .as_ref()
                .is_none_or(|paths| self.answered >= paths.len());
            if done || start.elapsed() >= limit {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// What was found at `path` (`None`: not answered yet).
    pub fn found(&self, path: &Path) -> Option<Found> {
        self.found.get(path).copied()
    }
}

/// When a project was last saved, as the list says it: "today 14:32", "yesterday",
/// "3 days ago", then the date. Days are calendar days in local time.
pub fn saved_when(modified: DateTime<Local>, now: DateTime<Local>) -> String {
    let days = (now.date_naive() - modified.date_naive()).num_days();
    match days {
        0 => format!("today {}", modified.format("%H:%M")),
        1 => "yesterday".into(),
        2..=6 => format!("{days} days ago"),
        // Older, or in the future (another computer's clock): the date.
        _ => modified.format("%-d %b %Y").to_string(),
    }
}

/// [`saved_when`] for a file time, now.
pub fn saved(modified: SystemTime) -> String {
    saved_when(DateTime::<Local>::from(modified), Local::now())
}

/// `text` (a path) in at most `max` characters, cut in the middle with "…": whole folders
/// are left out between the drive and the end when that fits ("D:\…\Songs\Morning Run"),
/// otherwise characters, a third from the start and the rest from the end.
pub fn shorten_middle(text: &str, max: usize) -> String {
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let sep = if text.contains('\\') { '\\' } else { '/' };
    let parts: Vec<&str> = text.split(sep).collect();
    if parts.len() >= 3 {
        let mut best = None;
        for keep in 1..parts.len() - 1 {
            let tail = parts[parts.len() - keep..].join(&sep.to_string());
            let short = format!("{}{sep}…{sep}{tail}", parts[0]);
            if short.chars().count() > max {
                break;
            }
            best = Some(short);
        }
        if let Some(short) = best {
            return short;
        }
    }
    let keep = max - 1;
    let front = keep / 3;
    let back = keep - front;
    let mut short: String = chars[..front].iter().collect();
    short.push('…');
    short.extend(&chars[chars.len() - back..]);
    short
}

/// [`shorten_middle`] to fit `width` points as `measure` lays text out.
pub fn shorten_to_width(text: &str, width: f32, measure: impl Fn(&str) -> f32) -> String {
    let full = measure(text);
    if full <= width {
        return text.to_string();
    }
    let count = text.chars().count();
    // A first guess from the average character, then down until it fits.
    let mut max = ((count as f32 * width / full.max(1.0)) as usize + 1).min(count);
    loop {
        let short = shorten_middle(text, max);
        if max == 0 || measure(&short) <= width {
            return short;
        }
        max -= 1;
    }
}

/// A recent project as the list shows it.
#[derive(Debug, Clone)]
pub struct Entry {
    pub path: PathBuf,
    /// The file's name without `.oxide`.
    pub name: String,
    /// The folder the project's folder is in.
    pub folder: String,
    /// `None` until a worker has looked.
    pub found: Option<Found>,
    pub template: Option<String>,
}

impl Entry {
    pub fn missing(&self) -> bool {
        self.found == Some(Found::Missing)
    }
}

/// The recent projects in the settings' order (the latest opened first), with what the
/// probe found.
pub fn entries(settings: &crate::settings::AppSettings, probe: &Probe) -> Vec<Entry> {
    settings
        .recent
        .iter()
        .map(|path| {
            let name = path
                .file_stem()
                .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
            // A project lives in a folder of its name: show where that folder is.
            let folder = path
                .parent()
                .map(|dir| match dir.file_name() {
                    Some(n) if n.to_string_lossy() == name => dir.parent().unwrap_or(dir),
                    _ => dir,
                })
                .map_or_else(String::new, |d| d.display().to_string());
            Entry {
                path: path.clone(),
                name,
                folder,
                found: probe.found(path),
                template: settings.project_templates.get(path).cloned(),
            }
        })
        .collect()
}

/// Open Explorer on `path`, selected, or on its folder when the probe found it `missing`.
/// Nothing is looked at here (a click on the UI thread): Explorer itself says if a folder
/// isn't there.
pub fn show_in_explorer(path: &Path, missing: bool) -> anyhow::Result<()> {
    use anyhow::{bail, Context};
    let mut command = std::process::Command::new("explorer");
    if !missing {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // `/select,"path"` as one argument, quoted as Explorer wants it.
            command.raw_arg(format!("/select,\"{}\"", path.display()));
        }
        #[cfg(not(windows))]
        command.arg(path);
    } else {
        match path.parent() {
            Some(dir) => {
                command.arg(dir);
            }
            None => bail!("{} has no folder to show", path.display()),
        }
    }
    command
        .spawn()
        .with_context(|| format!("opening Explorer on {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn at(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(y, mo, d, h, mi, 0).single().unwrap()
    }

    #[test]
    fn saved_times_read_as_today_yesterday_days_ago_then_a_date() {
        let now = at(2026, 10, 23, 16, 5);
        assert_eq!(saved_when(at(2026, 10, 23, 14, 32), now), "today 14:32");
        assert_eq!(saved_when(at(2026, 10, 23, 0, 1), now), "today 00:01");
        // Calendar days, not 24-hour spans: late last night is yesterday.
        assert_eq!(saved_when(at(2026, 10, 22, 23, 50), now), "yesterday");
        assert_eq!(saved_when(at(2026, 10, 22, 0, 10), now), "yesterday");
        assert_eq!(saved_when(at(2026, 10, 20, 9, 0), now), "3 days ago");
        assert_eq!(saved_when(at(2026, 10, 17, 9, 0), now), "6 days ago");
        assert_eq!(saved_when(at(2026, 10, 16, 9, 0), now), "16 Oct 2026");
        assert_eq!(saved_when(at(2025, 3, 2, 9, 0), now), "2 Mar 2025");
        // Across a month's end.
        assert_eq!(
            saved_when(at(2026, 10, 31, 22, 0), at(2026, 11, 1, 8, 0)),
            "yesterday"
        );
        // A clock ahead of this one: the date, never "-1 days ago".
        assert_eq!(saved_when(at(2026, 10, 25, 9, 0), now), "25 Oct 2026");
    }

    #[test]
    fn long_paths_lose_their_middle() {
        let path = r"D:\Music\Oxide Projects\2026\Band\Morning Run";
        assert_eq!(shorten_middle(path, 200), path, "short enough: as it is");
        assert_eq!(shorten_middle(path, path.len()), path);
        // Whole folders out, the drive and the end kept.
        assert_eq!(shorten_middle(path, 30), r"D:\…\2026\Band\Morning Run");
        assert_eq!(shorten_middle(path, 20), r"D:\…\Morning Run");
        // Not even that fits: characters, a third from the start.
        let short = shorten_middle(path, 12);
        assert_eq!(short.chars().count(), 12);
        assert_eq!(short, "D:\\…ning Run");
        for max in 0..path.len() {
            assert!(shorten_middle(path, max).chars().count() <= max, "{max}");
        }
        assert_eq!(shorten_middle("/home/me/music/songs/a", 14), "/…/songs/a");
        assert_eq!(shorten_middle("abcdefghij", 7), "ab…ghij");
    }

    #[test]
    fn shortening_to_a_width_stops_at_the_first_that_fits() {
        // Every character 7 points wide.
        let measure = |s: &str| s.chars().count() as f32 * 7.0;
        let path = r"D:\Music\Oxide Projects\2026\Band\Morning Run";
        assert_eq!(shorten_to_width(path, 1000.0, measure), path);
        let short = shorten_to_width(path, 140.0, measure);
        assert!(measure(&short) <= 140.0, "{short}");
        assert_eq!(short, shorten_middle(path, 20));
        assert_eq!(shorten_to_width(path, 3.0, measure), "");
    }

    #[test]
    fn missing_projects_are_found_on_workers_and_kept_off_the_list_when_removed() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/check/test-recent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Song")).unwrap();
        let there = dir.join("Song").join("Song.oxide");
        std::fs::write(&there, "{}").unwrap();
        let gone = dir.join("Gone").join("Gone.oxide");
        let folder_only = dir.join("Song");
        assert!(matches!(look(&there), Found::Present(Some(_))));
        assert_eq!(look(&gone), Found::Missing);
        assert_eq!(
            look(&folder_only),
            Found::Missing,
            "a folder isn't a project"
        );

        let mut settings = crate::settings::AppSettings::default();
        for p in [&gone, &there] {
            settings.add_recent(p.clone());
        }
        settings
            .project_templates
            .insert(there.clone(), "Guitar rig".into());
        let mut probe = Probe::new(None);
        // Nothing answered yet: neither present nor missing.
        let before = entries(&settings, &probe);
        assert!(before.iter().all(|e| e.found.is_none() && !e.missing()));
        probe.want(&settings.recent);
        probe.settle(Duration::from_secs(5));
        let listed = entries(&settings, &probe);
        assert_eq!(listed[0].path, there, "the latest first");
        assert_eq!(listed[0].name, "Song");
        // The project's own folder is left out of where it is.
        assert_eq!(listed[0].folder, dir.display().to_string());
        assert_eq!(listed[0].template.as_deref(), Some("Guitar rig"));
        assert!(!listed[0].missing());
        assert!(listed[1].missing());

        // Removed: off the list, and the file is untouched.
        assert!(settings.remove_recent(&gone));
        assert_eq!(entries(&settings, &probe).len(), 1);
        assert!(there.is_file());

        // The same list isn't looked at twice; forgetting looks again (the file gone now).
        probe.want(&settings.recent);
        let serial = probe.serial;
        probe.want(&settings.recent);
        assert_eq!(probe.serial, serial);
        std::fs::remove_file(&there).unwrap();
        probe.forget();
        probe.want(&settings.recent);
        probe.settle(Duration::from_secs(5));
        assert_eq!(probe.found(&there), Some(Found::Missing));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
