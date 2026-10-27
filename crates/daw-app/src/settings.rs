//! App preferences, stored as JSON beside the program (`settings.json`), so the app is
//! portable and writes nothing to the system drive: copy its folder and the settings go with
//! it. A first run reads the old `%APPDATA%\daw\settings.json` once, if there is one.

use daw_core::device::AudioSettings;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    pub audio: AudioSettings,
    /// Projects opened or made, the latest first (the start screen and File > Open recent).
    pub recent: Vec<PathBuf>,
    /// The template each recent project was started from, when the start screen made it from
    /// one (a project file doesn't say). Kept beside `recent` rather than in it, so a settings
    /// file from before still reads.
    pub project_templates: std::collections::BTreeMap<PathBuf, String>,
    /// Instruments last loaded on new MIDI tracks, newest first: "Insert MIDI track" offers
    /// them, since most plug-ins only say they're instruments once loaded.
    pub recent_instruments: Vec<PathBuf>,
    pub snap: bool,
    pub mcp: McpSettings,
    /// The loudness meter's scale and units (EBU Tech 3341 §2.7).
    pub loudness_scale: daw_core::api::LoudnessScale,
    /// Loudness relative to the target in LU, rather than LUFS.
    pub loudness_relative: bool,
    /// Normalise exports to the project's loudness target.
    pub normalize_exports: bool,
    /// The folder new projects were last made in: the start screen offers it again.
    pub projects_parent: Option<PathBuf>,
    /// Back the project up this often while it has unsaved changes (0: never).
    pub autosave_minutes: u32,
    /// How many backups each project keeps, the newest.
    pub keep_backups: usize,
    /// The format exports were last written in: the export dialogs start from it.
    pub export_encoding: daw_core::encode::Encoding,
    /// The window's layout when the app last closed (`dock::Layout`, as JSON so a layout
    /// from another version just doesn't load).
    pub layout: Option<serde_json::Value>,
    /// Layouts saved by name (View > Layouts).
    pub layouts: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            audio: AudioSettings::default(),
            recent: Vec::new(),
            project_templates: Default::default(),
            recent_instruments: Vec::new(),
            snap: true,
            mcp: McpSettings::default(),
            loudness_scale: Default::default(),
            loudness_relative: false,
            normalize_exports: false,
            projects_parent: None,
            autosave_minutes: 5,
            keep_backups: 20,
            export_encoding: Default::default(),
            layout: None,
            layouts: Default::default(),
        }
    }
}

impl AppSettings {
    /// The session's autosave, as these settings ask.
    pub fn apply_backups(&self, session: &mut daw_core::session::Session) {
        session.autosave = (self.autosave_minutes > 0)
            .then(|| std::time::Duration::from_secs(u64::from(self.autosave_minutes) * 60));
        session.keep_backups = self.keep_backups.max(1);
    }
}

/// The in-app MCP server that lets AI agents control the DAW (localhost only).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpSettings {
    pub enabled: bool,
    pub port: u16,
    /// A bearer token clients must send (`Authorization: Bearer …`); none: any local client.
    pub token: Option<String>,
}

impl Default for McpSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            port: crate::mcp::DEFAULT_PORT,
            token: None,
        }
    }
}

/// How many recent projects are kept.
pub const RECENT_KEPT: usize = 10;

/// The program's folder (the working directory if that can't be found).
pub fn app_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(PathBuf::from))
        .unwrap_or_default()
}

fn path() -> PathBuf {
    app_dir().join("settings.json")
}

/// Where settings were kept before they moved beside the program: read once, never written.
fn old_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join("daw").join("settings.json"))
}

impl AppSettings {
    pub fn load() -> Self {
        let read = |path: PathBuf| {
            std::fs::read_to_string(path)
                .ok()
                .and_then(|s| serde_json::from_str(&s).ok())
        };
        read(path())
            .or_else(|| old_path().and_then(read))
            .unwrap_or_default()
    }

    /// Write the settings beside the program; the error says why not (a read-only folder).
    pub fn save(&self) -> anyhow::Result<()> {
        use anyhow::Context;
        let path = path();
        let json = serde_json::to_string_pretty(self)?;
        std::fs::write(&path, json)
            .with_context(|| format!("saving settings to {}", path.display()))
    }

    pub fn add_recent(&mut self, project: PathBuf) {
        self.recent.retain(|p| p != &project);
        self.recent.insert(0, project);
        self.recent.truncate(RECENT_KEPT);
        self.prune_templates();
    }

    /// Take `project` off the recent projects (its files stay); false if it wasn't on them.
    pub fn remove_recent(&mut self, project: &std::path::Path) -> bool {
        let before = self.recent.len();
        self.recent.retain(|p| p != project);
        self.prune_templates();
        self.recent.len() != before
    }

    /// Templates are only remembered for projects still on the list.
    fn prune_templates(&mut self) {
        let recent = &self.recent;
        self.project_templates.retain(|p, _| recent.contains(p));
    }
}

/// The registry entries that make Windows open `.oxide` files with the program at `exe`, show
/// them with its icon and call them "Oxide project": `(key, default value)` under the
/// current user's classes only (`HKCU\Software\Classes`), so no other account and no system
/// setting changes.
pub fn file_type_entries(exe: &std::path::Path) -> Vec<(String, String)> {
    let classes = r"HKCU\Software\Classes";
    let exe = exe.display();
    vec![
        (
            format!(r"{classes}\.{}", daw_core::folder::EXTENSION),
            "Oxide.Project".into(),
        ),
        (format!(r"{classes}\Oxide.Project"), "Oxide project".into()),
        (
            format!(r"{classes}\Oxide.Project\DefaultIcon"),
            format!("\"{exe}\",0"),
        ),
        (
            format!(r"{classes}\Oxide.Project\shell\open\command"),
            format!("\"{exe}\" \"%1\""),
        ),
    ]
}

/// Register `.oxide` with this program for the current user (Windows' own `reg.exe`). Only
/// ever run from the settings button: it changes the user's file associations.
pub fn register_file_type() -> anyhow::Result<()> {
    use anyhow::{bail, Context};
    let exe = std::env::current_exe().context("finding the program")?;
    for (key, value) in file_type_entries(&exe) {
        let out = std::process::Command::new("reg")
            .args(["add", &key, "/ve", "/d", &value, "/f"])
            .output()
            .context("running reg.exe")?;
        if !out.status.success() {
            bail!(
                "reg add {key}: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_projects_are_added_first_and_removed_with_their_template() {
        let mut s = AppSettings::default();
        let song = |n: usize| PathBuf::from(format!(r"D:\Music\Song {n}\Song {n}.oxide"));
        for n in 0..12 {
            s.add_recent(song(n));
        }
        assert_eq!(s.recent.len(), RECENT_KEPT);
        assert_eq!(s.recent[0], song(11), "the latest first");
        // Opened again: back to the top, not twice.
        s.add_recent(song(5));
        assert_eq!(s.recent[0], song(5));
        assert_eq!(s.recent.iter().filter(|p| **p == song(5)).count(), 1);
        s.project_templates.insert(song(5), "Guitar rig".into());
        s.project_templates.insert(song(9), "Mixing".into());
        assert!(s.remove_recent(&song(5)));
        assert!(!s.recent.contains(&song(5)));
        assert!(!s.project_templates.contains_key(&song(5)));
        assert_eq!(
            s.project_templates.get(&song(9)).map(String::as_str),
            Some("Mixing")
        );
        assert!(!s.remove_recent(&song(5)), "gone already");
        // The templates' map (paths as keys) goes through the settings file and back.
        let json = serde_json::to_string(&s).unwrap();
        let back: AppSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.recent, s.recent);
        assert_eq!(back.project_templates, s.project_templates);
        assert_eq!(s.recent.len(), RECENT_KEPT - 1);
        // Pushed off the end: its template goes too.
        for n in 20..30 {
            s.add_recent(song(n));
        }
        assert!(s.project_templates.is_empty());
    }

    #[test]
    fn a_settings_file_from_before_the_templates_still_reads() {
        let old = r#"{"recent": ["D:\\Music\\A\\A.oxide"], "snap": false}"#;
        let s: AppSettings = serde_json::from_str(old).unwrap();
        assert_eq!(s.recent, vec![PathBuf::from(r"D:\Music\A\A.oxide")]);
        assert!(s.project_templates.is_empty());
        assert!(!s.snap);
    }

    #[test]
    fn oxide_files_open_with_the_program() {
        let entries = file_type_entries(std::path::Path::new(r"D:\Apps\Oxide\daw.exe"));
        let value = |key: &str| {
            entries
                .iter()
                .find(|(k, _)| k.ends_with(key))
                .map(|(_, v)| v.as_str())
        };
        assert!(entries
            .iter()
            .all(|(k, _)| k.starts_with(r"HKCU\Software\Classes\")));
        assert_eq!(value(r"\.oxide"), Some("Oxide.Project"));
        assert_eq!(
            value(r"\shell\open\command"),
            Some(r#""D:\Apps\Oxide\daw.exe" "%1""#)
        );
        assert_eq!(value(r"\DefaultIcon"), Some(r#""D:\Apps\Oxide\daw.exe",0"#));
    }

    /// The installer's "Open .oxide projects with Oxide" task writes what the Settings button
    /// writes, so either can make the association and the uninstaller can take either away:
    /// `installer/oxide.iss`'s [Registry] lines, read here, against [`file_type_entries`] for
    /// the installed program (`{app}\Oxide.exe`).
    #[test]
    fn the_installer_registers_oxide_files_as_the_settings_button_does() {
        let script = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../installer/oxide.iss"),
        )
        .unwrap();
        // `"…"` fields of a line, with Inno's doubled quotes made single.
        let field = |line: &str, name: &str| -> Option<String> {
            let start = line.find(&format!("{name}: \""))? + name.len() + 3;
            let rest = &line[start..];
            let mut value = String::new();
            let mut chars = rest.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                    } else {
                        return Some(value);
                    }
                }
                value.push(c);
            }
            None
        };
        let installer: Vec<(String, String)> = script
            .lines()
            .filter(|l| l.starts_with("Root: HKCU; Subkey: \"Software\\Classes\\"))
            .map(|l| {
                assert!(l.contains("Tasks: fileassoc"), "{l}");
                (
                    format!(r"HKCU\{}", field(l, "Subkey").unwrap()),
                    field(l, "ValueData").unwrap(),
                )
            })
            .collect();
        let button = file_type_entries(std::path::Path::new(r"{app}\Oxide.exe"));
        assert_eq!(installer, button);
        // The uninstaller takes it away only while it opens this install's program.
        assert!(script.contains(r#"'"' + ExpandConstant('{app}') + '\Oxide.exe" "%1"'"#));
    }
}
