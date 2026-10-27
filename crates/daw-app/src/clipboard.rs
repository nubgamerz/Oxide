//! Ctrl+C, Ctrl+X and Ctrl+V on clips and audio (dev plan 24.1).
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

use crate::app::DawApp;
use daw_core::api::ViewMode;
use egui::Event;

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
}

/// The clipboard events egui-winit sent this frame, taken out of `events` (so no widget acts
/// on them too) and turned into the commands for `view`, in order. While a text field has
/// the keyboard (`typing`) nothing is taken: the field copies and pastes its own text.
pub fn take(events: &mut Vec<Event>, view: ViewMode, typing: bool) -> Vec<Command> {
    if typing {
        return Vec::new();
    }
    let multitrack = view == ViewMode::Multitrack;
    let mut commands = Vec::new();
    events.retain(|event| {
        let command = match event {
            Event::Copy if multitrack => Command::CopyClips,
            Event::Copy => Command::CopyAudio,
            Event::Cut if multitrack => Command::CutClips,
            Event::Cut => Command::CutAudio,
            // The text (or image) is someone else's: Oxide pastes its own clipboard.
            Event::Paste(_) | Event::PasteImage(_) if multitrack => Command::PasteClips,
            Event::Paste(_) | Event::PasteImage(_) => Command::PasteAudio,
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
        let commands = take(&mut events, ViewMode::Multitrack, false);
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
        let commands = take(&mut events, ViewMode::Waveform, false);
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

    #[test]
    fn a_focused_text_field_keeps_its_copy_and_paste() {
        for view in [ViewMode::Multitrack, ViewMode::Waveform] {
            let mut events = clipboard_events();
            assert!(take(&mut events, view, true).is_empty());
            assert_eq!(events, clipboard_events(), "nothing taken in {view:?}");
        }
    }

    #[test]
    fn ctrl_c_as_a_key_is_not_a_clipboard_command() {
        // egui-winit never sends these for Ctrl+C / Ctrl+V; if anything did, they are keys.
        let mut events = vec![
            key_with(Key::C, Modifiers::COMMAND),
            key_with(Key::V, Modifiers::COMMAND),
        ];
        assert!(take(&mut events, ViewMode::Multitrack, false).is_empty());
        assert_eq!(events.len(), 2);
    }

    #[test]
    fn the_line_says_how_many_clips() {
        assert_eq!(clips_line(1), "1 clip copied in Oxide");
        assert_eq!(clips_line(3), "3 clips copied in Oxide");
    }
}
