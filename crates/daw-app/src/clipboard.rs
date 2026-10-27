//! Ctrl+C, Ctrl+X and Ctrl+V on clips, audio and the piano roll's notes (dev plan 24.1).
//!
//! egui-winit 0.37 (`State::on_keyboard_input`, with `clipboard_shortcuts` on, as eframe
//! leaves it) turns Ctrl+C / Ctrl+X / Ctrl+V, and Ctrl+Insert, Shift+Delete and Shift+Insert,
//! into `Event::Copy`, `Event::Cut` and `Event::Paste(text)` (or `Event::PasteImage`) and sends
//! **no** `Event::Key` for them, so `consume_key(Modifiers::COMMAND, Key::C)` never matches.
//! The shortcuts read those events instead ([`take`]), after the text-field gate, so a
//! focused field (a rename, a typed volume) keeps its own copy and paste.
//!
//! Ctrl+V reaches the app only while the system clipboard holds text or an image: egui-winit
//! pushes `Event::Paste` only with non-empty text, and nothing at all otherwise. eframe 0.37
//! gives an app no raw window events and no hook that sees the key before egui-winit
//! swallows it, so instead copying or cutting in Oxide puts a short line on the system
//! clipboard ([`announce`]: "1 clip copied in Oxide"). Ctrl+V then always arrives as a
//! `Paste`, whose text is ignored: it pastes Oxide's own clipboard (the session's clips, or
//! the Waveform view's audio). A copy made in another app afterwards still pastes Oxide's
//! clips here; one that leaves no text on the clipboard (files copied in Explorer) silences
//! Ctrl+V until the next copy in Oxide (TD174; Edit > Paste still works).
//!
//! Where they go ([`Target::of`]): the piano roll's notes while it has the keyboard
//! (`piano_roll::has_keys`: the pane under the pointer, else the one last clicked, as F1 and
//! its Delete and Ctrl+A follow), else the view shown: the Multitrack's clips or the Waveform
//! view's audio. Notes have a clipboard of their own in the session (`Session::copy_notes`),
//! so copying notes never replaces copied clips, and an empty one pastes nothing.

use crate::app::DawApp;
use daw_core::api::ViewMode;
use egui::Event;

/// What has the clipboard's keys this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Multitrack,
    Waveform,
    /// The piano roll's notes (`piano_roll::has_keys`).
    PianoRoll,
}

impl Target {
    /// The piano roll when it has the keyboard, else the view shown.
    pub fn of(app: &DawApp, ctx: &egui::Context) -> Self {
        if crate::piano_roll::has_keys(app, ctx) {
            return Self::PianoRoll;
        }
        match app.view_mode {
            ViewMode::Multitrack => Self::Multitrack,
            ViewMode::Waveform => Self::Waveform,
        }
    }
}

/// What a clipboard event asks of the view it lands in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// The Multitrack's selected clips (`DawApp::copy_selection(false)`).
    CopyClips,
    /// `DawApp::copy_selection(true)`.
    CutClips,
    /// Oxide's clips at the playhead on the selected track (`DawApp::paste`).
    PasteClips,
    /// The Waveform view's selection (`waveform::copy(false)`).
    CopyAudio,
    /// `waveform::copy(true)`.
    CutAudio,
    /// `waveform::paste`.
    PasteAudio,
    /// The piano roll's selected notes (`piano_roll::copy(false)`).
    CopyNotes,
    /// `piano_roll::copy(true)`.
    CutNotes,
    /// Oxide's notes at the playhead in the clip open in the piano roll (`piano_roll::paste`).
    PasteNotes,
}

/// The clipboard events egui-winit sent this frame, taken out of `events` (so no widget acts
/// on them too) and turned into the commands for `target`, in order. While a text field has
/// the keyboard (`typing`) nothing is taken: the field copies and pastes its own text.
pub fn take(events: &mut Vec<Event>, target: Target, typing: bool) -> Vec<Command> {
    if typing {
        return Vec::new();
    }
    let (copy, cut, paste) = match target {
        Target::Multitrack => (Command::CopyClips, Command::CutClips, Command::PasteClips),
        Target::Waveform => (Command::CopyAudio, Command::CutAudio, Command::PasteAudio),
        Target::PianoRoll => (Command::CopyNotes, Command::CutNotes, Command::PasteNotes),
    };
    let mut commands = Vec::new();
    events.retain(|event| {
        let command = match event {
            Event::Copy => copy,
            Event::Cut => cut,
            // The text (or image) is someone else's: Oxide pastes its own clipboard.
            Event::Paste(_) | Event::PasteImage(_) => paste,
            _ => return true,
        };
        commands.push(command);
        false
    });
    commands
}

/// Run what [`take`] found.
pub fn run(app: &mut DawApp, commands: &[Command]) {
    for command in commands {
        match command {
            Command::CopyClips => app.copy_selection(false),
            Command::CutClips => app.copy_selection(true),
            Command::PasteClips => app.paste(),
            Command::CopyAudio => crate::waveform::copy(app, false),
            Command::CutAudio => crate::waveform::copy(app, true),
            Command::PasteAudio => crate::waveform::paste(app),
            Command::CopyNotes => crate::piano_roll::copy(app, false),
            Command::CutNotes => crate::piano_roll::copy(app, true),
            Command::PasteNotes => crate::piano_roll::paste(app),
        }
    }
}

/// After a copy or cut in Oxide: `line` on the system clipboard, so the next Ctrl+V arrives
/// as an `Event::Paste` even if the clipboard held no text (see the module docs). Check runs
/// leave the user's clipboard alone and only keep the line (`DawApp::clipboard_line`), which
/// the driven check pastes back as egui-winit would.
pub fn announce(app: &mut DawApp, line: String) {
    if !app.is_check_run() {
        app.egui_ctx.copy_text(line.clone());
    }
    app.clipboard_line = Some(line);
}

/// The line for `count` clips.
pub fn clips_line(count: usize) -> String {
    format!(
        "{count} clip{} copied in Oxide",
        if count == 1 { "" } else { "s" }
    )
}

/// The line for `count` notes.
pub fn notes_line(count: usize) -> String {
    format!("{} copied in Oxide", crate::piano_roll::notes_text(count))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, Modifiers};

    fn key(key: Key) -> Event {
        key_with(key, Modifiers::NONE)
    }

    fn key_with(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn clipboard_events() -> Vec<Event> {
        vec![
            key(Key::Space),
            Event::Copy,
            Event::Cut,
            Event::Paste("anything from another app".into()),
            Event::PasteImage(std::sync::Arc::new(egui::ColorImage::filled(
                [1, 1],
                egui::Color32::BLACK,
            ))),
            Event::Text("q".into()),
        ]
    }

    #[test]
    fn the_multitrack_copies_cuts_and_pastes_clips() {
        let mut events = clipboard_events();
        let commands = take(&mut events, Target::Multitrack, false);
        assert_eq!(
            commands,
            [
                Command::CopyClips,
                Command::CutClips,
                Command::PasteClips,
                Command::PasteClips
            ]
        );
        // Taken out, so no widget acts on them too; everything else stays.
        assert_eq!(events, [key(Key::Space), Event::Text("q".into())]);
    }

    #[test]
    fn the_waveform_view_copies_cuts_and_pastes_its_audio() {
        let mut events = clipboard_events();
        let commands = take(&mut events, Target::Waveform, false);
        assert_eq!(
            commands,
            [
                Command::CopyAudio,
                Command::CutAudio,
                Command::PasteAudio,
                Command::PasteAudio
            ]
        );
        assert_eq!(events, [key(Key::Space), Event::Text("q".into())]);
    }

    /// While the piano roll has the keys, Ctrl+C / X / V are its notes', never the clips (the
    /// bug: they copied the open MIDI clip and pasted a copy of it at the playhead).
    #[test]
    fn the_piano_roll_copies_cuts_and_pastes_notes() {
        let mut events = clipboard_events();
        let commands = take(&mut events, Target::PianoRoll, false);
        assert_eq!(
            commands,
            [
                Command::CopyNotes,
                Command::CutNotes,
                Command::PasteNotes,
                Command::PasteNotes
            ]
        );
        assert_eq!(events, [key(Key::Space), Event::Text("q".into())]);
    }

    #[test]
    fn a_focused_text_field_keeps_its_copy_and_paste() {
        for target in [Target::Multitrack, Target::Waveform, Target::PianoRoll] {
            let mut events = clipboard_events();
            assert!(take(&mut events, target, true).is_empty());
            assert_eq!(events, clipboard_events(), "nothing taken in {target:?}");
        }
    }

    #[test]
    fn ctrl_c_as_a_key_is_not_a_clipboard_command() {
        // egui-winit never sends these for Ctrl+C / Ctrl+V; if anything did, they are keys.
        let mut events = vec![
            key_with(Key::C, Modifiers::COMMAND),
            key_with(Key::V, Modifiers::COMMAND),
        ];
        for target in [Target::Multitrack, Target::PianoRoll] {
            assert!(take(&mut events, target, false).is_empty());
        }
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn the_line_says_how_many_clips() {
        assert_eq!(clips_line(1), "1 clip copied in Oxide");
        assert_eq!(clips_line(3), "3 clips copied in Oxide");
        assert_eq!(notes_line(1), "1 note copied in Oxide");
        assert_eq!(notes_line(3), "3 notes copied in Oxide");
    }
}
