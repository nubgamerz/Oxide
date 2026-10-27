//! Help > Manual and F1 (dev plan 27.3): the manual (`manual\` beside the program, built by
//! `crates/manual`) opened in the default browser, at the page for where the user is.
//!
//! - **Where you are** is the pane or window under the pointer, as drawn last frame (each one
//!   notes its rect here with [`Here::note`]); with the pointer on none of them, the window
//!   open on top (settings, an effect's editor, the CPU window, the start screen), else the
//!   pane last clicked, else the view shown (the Multitrack or the Waveform view).
//! - **F1's table** is [`Place::page`]: a place to a page id and heading anchor of the manual.
//!   `every_place_has_its_page_in_the_manual` builds `docs/manual` and fails while a page or
//!   heading it names is missing, so renaming one fails the quick tier until the table
//!   follows. Pages 27.4 and 27.5 will write are stubs there now ("being written").
//! - **Opening** goes through the `webbrowser` crate (eframe's own dependency): it asks
//!   Windows for the default browser's command (`AssocQueryStringW` on `http`) and starts it
//!   with a `file:///` URL, so a `#anchor` survives (`ShellExecute` on the file drops it).
//!   Check runs (`--demo`, a screenshot) never start a browser: they note the request in
//!   `DawApp::manual_requests`, which the driven check (`--show help-manual`) and
//!   `open_manual`'s answer read.

use std::path::{Path, PathBuf};

use crate::settings_window::Section;
use daw_core::manual::Target;
use egui::{LayerId, Pos2, Rect};

/// A part of the app with a page of its own in the manual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Place {
    /// The start screen (no project open).
    Start,
    Multitrack,
    Inspector,
    Mixer,
    PianoRoll,
    /// The Waveform view (an open file's tab, or the place files open).
    Waveform,
    /// The Waveform view's Diagnostics pane.
    Diagnostics,
    /// Options > Settings… on a section.
    Settings(Section),
    /// A built-in effect's editor: its kind's id (`native::Kind::id`).
    Effect(&'static str),
    /// The CPU use window.
    Cpu,
    /// The export dialog.
    Export,
}

impl Place {
    /// Every place, for the table's test: these and an editor per built-in effect.
    #[cfg(test)]
    pub const ALL: [Place; 13] = [
        Place::Start,
        Place::Multitrack,
        Place::Inspector,
        Place::Mixer,
        Place::PianoRoll,
        Place::Waveform,
        Place::Diagnostics,
        Place::Settings(Section::Audio),
        Place::Settings(Section::Midi),
        Place::Settings(Section::Projects),
        Place::Settings(Section::Mcp),
        Place::Cpu,
        Place::Export,
    ];

    /// F1's table: the manual's page (its id: the Markdown file's name) and the heading on
    /// it (its anchor) for this place. Checked against `docs/manual` by a test.
    pub fn page(self) -> (&'static str, Option<&'static str>) {
        match self {
            Place::Start => ("getting-started", None),
            Place::Multitrack => ("multitrack", None),
            Place::Inspector => ("multitrack", Some("the-inspector")),
            Place::Mixer => ("mixer", None),
            Place::PianoRoll => ("piano-roll", None),
            Place::Waveform => ("waveform-view", None),
            Place::Diagnostics => ("waveform-view", Some("diagnostics")),
            Place::Settings(Section::Audio) => ("settings", Some("audio")),
            Place::Settings(Section::Midi) => ("settings", Some("midi")),
            Place::Settings(Section::Projects) => ("settings", Some("projects")),
            Place::Settings(Section::Mcp) => ("settings", Some("ai-control-mcp")),
            // The effect's own section, on its browser group's page (27.5).
            Place::Effect(id) => match daw_core::native::find(id) {
                Some(kind) => (effect_page(kind.category), Some(kind.id)),
                None => ("effects", None),
            },
            Place::Cpu => ("choppy-audio", Some("find-what-is-heavy-the-cpu-panel")),
            Place::Export => ("exporting", None),
        }
    }

    pub fn target(self) -> Target {
        let (page, anchor) = self.page();
        Target::new(page, anchor)
    }

    /// A window rather than a docked pane: it wins over the panes when the pointer is on
    /// neither.
    fn is_window(self) -> bool {
        matches!(
            self,
            Place::Start | Place::Settings(_) | Place::Effect(_) | Place::Cpu | Place::Export
        )
    }
}

/// The manual's page for a built-in effect's browser group; the overview for another.
fn effect_page(category: &str) -> &'static str {
    match category {
        "Dynamics" => "effects-dynamics",
        "EQ" => "effects-eq",
        "Restoration" => "effects-restoration",
        "Reverb" => "effects-reverb",
        "Delay" => "effects-delay",
        "Modulation" => "effects-modulation",
        _ => "effects",
    }
}

/// Where each place was drawn, for F1.
#[derive(Debug, Default)]
pub struct Here {
    /// This frame's places, in drawing order.
    drawn: Vec<Noted>,
    /// Last frame's: what F1 looks at (keys are read before this frame's panes are drawn).
    last: Vec<Noted>,
    /// The place last clicked.
    clicked: Option<Place>,
}

#[derive(Debug, Clone, Copy)]
struct Noted {
    place: Place,
    layer: LayerId,
    rect: Rect,
}

impl Here {
    /// A new frame: what was drawn becomes what F1 looks at.
    pub fn begin_frame(&mut self) {
        self.last = std::mem::take(&mut self.drawn);
    }

    /// `place` is drawn in `rect` on `layer` this frame; a press inside it, on top, makes it
    /// the place last clicked.
    pub fn note(&mut self, ctx: &egui::Context, place: Place, layer: LayerId, rect: Rect) {
        let noted = Noted { place, layer, rect };
        let pressed = ctx.input(|i| i.pointer.any_pressed().then(|| i.pointer.interact_pos()));
        if let Some(Some(at)) = pressed {
            if on_top(ctx, &noted, at) {
                self.clicked = Some(place);
            }
        }
        self.drawn.push(noted);
    }

    /// Where the user is, as last frame drew it (see the module's docs); `view`: the view
    /// shown, when nothing else says.
    pub fn place(&self, ctx: &egui::Context, view: Place) -> Place {
        if let Some(at) = ctx.pointer_latest_pos() {
            // The last drawn of those under the pointer on its topmost layer.
            if let Some(n) = self.last.iter().rev().find(|n| on_top(ctx, n, at)) {
                return n.place;
            }
        }
        if let Some(n) = self.last.iter().rev().find(|n| n.place.is_window()) {
            return n.place;
        }
        self.clicked.unwrap_or(view)
    }

    /// Where `place` was drawn last frame (the driven check points there).
    pub fn rect_of(&self, place: Place) -> Option<Rect> {
        self.last
            .iter()
            .rev()
            .find(|n| n.place == place)
            .map(|n| n.rect)
    }
}

/// `at` is in `noted`'s rect and its layer is the one on top there (panes are drawn on the
/// background, which has no area of its own).
fn on_top(ctx: &egui::Context, noted: &Noted, at: Pos2) -> bool {
    if !noted.rect.contains(at) {
        return false;
    }
    match ctx.layer_id_at(at) {
        Some(top) => top == noted.layer,
        None => noted.layer.order == egui::Order::Background,
    }
}

/// The manual's folder: `manual\` beside the program (the installer puts it there); in a
/// build that has none, the repository's `out\manual` when it was built there
/// (`cargo run -p manual`, the `manual` tier): a convenience for development; else none.
pub fn manual_dir() -> Option<PathBuf> {
    let beside = expected_dir();
    if beside.join("index.html").is_file() {
        return Some(beside);
    }
    let built = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../out/manual");
    built
        .join("index.html")
        .is_file()
        .then(|| std::path::absolute(&built).unwrap_or(built))
}

/// Where the installer puts the manual: `manual\` beside the program.
pub fn expected_dir() -> PathBuf {
    crate::settings::app_dir().join("manual")
}

/// Why there is no manual to open, and how to get one.
pub fn missing() -> String {
    format!(
        "The manual isn't beside the program: it would be in {}. The installer puts it there; \
         in a development build, `cargo run -p manual` (or `scripts/check.ps1 -Tier manual`) \
         builds it into out\\manual, which Oxide opens instead.",
        expected_dir().display()
    )
}

/// The page's `file:///` URL, the anchor as its fragment.
pub fn url(dir: &Path, target: &Target) -> Result<String, String> {
    let file = target.file(dir);
    let mut url = url::Url::from_file_path(&file)
        .map_err(|()| format!("{} can't be made a URL", file.display()))?;
    url.set_fragment(target.anchor.as_deref());
    Ok(url.to_string())
}

/// Show `url` in the default browser.
pub fn launch(url: &str) -> Result<(), String> {
    webbrowser::open(url).map_err(|e| format!("The manual couldn't be opened in a browser: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// F1's table names pages and headings the manual has: `docs/manual` built in memory, as
    /// `cargo run -p manual` builds it. A renamed page or heading fails here (the quick tier).
    #[test]
    fn every_place_has_its_page_in_the_manual() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/manual");
        let source = manual::Source::read(&src, &src.join("shots")).unwrap();
        let built = manual::build(
            &source,
            &manual::Options::default(),
            &manual::mcp::Reference::load(),
        );
        assert_eq!(built.errors(), 0, "{:#?}", built.problems);
        let editors = daw_core::native::KINDS
            .iter()
            .map(|k| Place::Effect(k.id))
            .chain([Place::Effect("no-such-effect")]);
        for place in Place::ALL.into_iter().chain(editors) {
            let (page, anchor) = place.page();
            let checked = daw_core::manual::check(&built.index, page, anchor);
            assert!(
                checked.is_ok(),
                "F1 in {place:?} names {page}#{}: {:#}",
                anchor.unwrap_or(""),
                checked.unwrap_err()
            );
        }
        // Every settings section is in the table.
        for section in Section::ALL {
            assert!(
                Place::ALL.contains(&Place::Settings(section)),
                "{section:?}"
            );
        }
    }

    /// The keyboard reference (27.4) is written by hand, as the keys live in the code that
    /// handles them (`handle_shortcuts`, the piano roll's, the fields'), not in one list: every
    /// `Key::` this crate's code reads must appear as a key cap (`[[Ctrl+S]]`) in
    /// `docs/manual/reference/keyboard.md`. The driven checks (`edit_check`, `frame_check`)
    /// and the test modules, which press keys rather than handle them, are left out.
    #[test]
    fn every_key_the_app_handles_is_in_the_keyboard_reference() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let page = std::fs::read_to_string(src.join("../../../docs/manual/reference/keyboard.md"))
            .expect("the keyboard reference");
        // The key caps the page draws: each `[[...]]` that isn't a link, split at `+`.
        let mut caps = std::collections::BTreeSet::new();
        for (_, rest) in page.match_indices("[[").map(|(i, _)| ((), &page[i + 2..])) {
            let Some(end) = rest.get(1..).and_then(|r| r.find("]]")).map(|e| e + 1) else {
                continue;
            };
            let inner = &rest[..end];
            if inner.contains(':') {
                continue;
            }
            caps.extend(
                inner
                    .split('+')
                    .filter(|k| !k.is_empty())
                    .map(str::to_string),
            );
            if inner.ends_with('+') {
                caps.insert("+".to_string());
            }
        }
        let mut missing = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&src).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.ends_with(".rs") || name == "edit_check.rs" || name == "frame_check.rs" {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let code = text
                .find("\nmod tests")
                .map_or(text.as_str(), |end| &text[..end]);
            for (at, _) in code.match_indices("Key::") {
                let key: String = code[at + 5..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric())
                    .collect();
                // Comments name keys egui never sends as keys (Ctrl+C arrives as a copy).
                let line = code[..at].rsplit('\n').next().unwrap_or("");
                if line.trim_start().starts_with("//") {
                    continue;
                }
                let shown = match key.as_str() {
                    "Equals" => "=".to_string(),
                    "Plus" => "+".to_string(),
                    "Minus" => "-".to_string(),
                    "OpenBracket" => "[".to_string(),
                    "CloseBracket" => "]".to_string(),
                    "Escape" => "Esc".to_string(),
                    // [[1]]–[[9]]: the row covers the digits between.
                    digit if digit.starts_with("Num") => {
                        if caps.contains("1") && caps.contains("9") {
                            continue;
                        }
                        digit[3..].to_string()
                    }
                    other => other.to_string(),
                };
                if !caps.contains(&shown) {
                    missing.insert(format!("{key} ({name}), shown as [[{shown}]]"));
                }
            }
        }
        assert!(
            missing.is_empty(),
            "keys handled but not in docs/manual/reference/keyboard.md: {missing:#?}"
        );
    }

    #[test]
    fn a_url_keeps_its_anchor_and_spaces() {
        let dir = Path::new(r"C:\Program Files\Oxide Check\manual");
        let url = url(dir, &Target::new("settings", Some("audio"))).unwrap();
        assert_eq!(
            url,
            "file:///C:/Program%20Files/Oxide%20Check/manual/settings.html#audio"
        );
        let top = super::url(dir, &Target::new("index", None)).unwrap();
        assert!(top.ends_with("/manual/index.html"), "{top}");
    }

    #[test]
    fn f1_finds_the_window_on_top_then_the_pane_then_the_view() {
        let ctx = egui::Context::default();
        let mut here = Here::default();
        let bg = LayerId::background();
        let window = LayerId::new(egui::Order::Middle, egui::Id::unique("w"));
        // No pointer: the view, or an open window.
        here.drawn.push(Noted {
            place: Place::PianoRoll,
            layer: bg,
            rect: Rect::from_min_size(Pos2::ZERO, egui::vec2(100.0, 100.0)),
        });
        here.begin_frame();
        assert_eq!(here.place(&ctx, Place::Multitrack), Place::Multitrack);
        here.drawn.push(Noted {
            place: Place::Settings(Section::Audio),
            layer: window,
            rect: Rect::from_min_size(Pos2::new(200.0, 0.0), egui::vec2(100.0, 100.0)),
        });
        here.begin_frame();
        assert_eq!(
            here.place(&ctx, Place::Multitrack),
            Place::Settings(Section::Audio)
        );
        assert_eq!(
            here.rect_of(Place::Settings(Section::Audio))
                .map(|r| r.left()),
            Some(200.0)
        );
        // Clicked last: wins over the view when nothing is open or under the pointer.
        here.clicked = Some(Place::Mixer);
        here.begin_frame();
        assert_eq!(here.place(&ctx, Place::Multitrack), Place::Mixer);
    }
}
