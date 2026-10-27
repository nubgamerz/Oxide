//! The Waveform view's rack presets (dev plan 18.2): a
//! Presets menu in the rack's header loads one (replacing the rack, its plug-ins loading a frame
//! at a time behind the busy overlay), saves the rack as one (a name asked), or deletes one
//! (asked first). The work is the session's (`Session::save_rack_preset`, …), shared with the
//! actions (`list_rack_presets`, …); this is only the menu and its two questions.
//!
//! The check runs `--show rack-presets` (the menu opened by a click the app makes itself) and
//! `--show rack-restored` (a kept rack, in `out/check/rack`, coming back) set up here too.

use crate::app::DawApp;
use crate::theme;
use daw_core::rack::{RackFile, RackSlot};
use daw_core::session::InsertTarget;
use egui::Rect;

/// The menu's and the questions' state.
#[derive(Default)]
pub struct State {
    question: Option<Question>,
    /// Where the menu's button was drawn (the check run clicks it).
    button: Option<Rect>,
    /// The check run's click on the menu's button: frames since it began.
    pub demo_click: Option<u32>,
    /// The check run waits for a restored rack's plug-ins before its shot.
    pub demo_restored: bool,
}

#[derive(Clone)]
enum Question {
    /// Save the rack as… (the name typed, why the last try was refused).
    Save { name: String, error: Option<String> },
    /// Delete this preset?
    Delete(String),
}

/// The Presets menu, at the right of the rack's title line.
pub fn menu(app: &mut DawApp, ui: &mut egui::Ui) {
    // Read from the folder only while the menu is open (a click), not every frame.
    let session = &app.session;
    let names = || -> Vec<String> { session.rack_presets().into_iter().map(|p| p.name).collect() };
    let empty = app.session.editor_rack().is_empty();
    let busy = app.session.apply_state().is_some();
    let mut load = None;
    let mut ask = None;
    let response = egui::containers::menu::MenuButton::new("Presets ▾")
        .ui(ui, |ui| {
            let presets = names();
            ui.set_min_width(190.0);
            if presets.is_empty() {
                ui.label(theme::hint("No presets saved yet"));
            }
            for name in &presets {
                let button = ui
                    .add_enabled(!busy, egui::Button::new(name))
                    .on_hover_text(format!(
                        "Load \"{name}\": its plug-ins and settings replace the rack's"
                    ))
                    .on_disabled_hover_text("Applying…");
                if button.clicked() {
                    load = Some(name.clone());
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .add_enabled(!empty, egui::Button::new("Save rack as…"))
                .on_hover_text(
                    "Keep the rack (its plug-ins in order, their settings and bypass) as a \
                     preset, for every project",
                )
                .on_disabled_hover_text("The rack is empty")
                .clicked()
            {
                ask = Some(Question::Save {
                    name: String::new(),
                    error: None,
                });
                ui.close();
            }
            ui.add_enabled_ui(!presets.is_empty(), |ui| {
                ui.menu_button("Delete", |ui| {
                    for name in &presets {
                        if ui.button(name).clicked() {
                            ask = Some(Question::Delete(name.clone()));
                            ui.close();
                        }
                    }
                });
            });
        })
        .0
        .on_hover_text("Rack presets: load, save the rack as one, delete");
    app.rack_presets.button = Some(response.rect);
    crate::marks::put(ui, "waveform.rack.presets", response.rect);
    if let Some(question) = ask {
        app.rack_presets.question = Some(question);
    }
    if let Some(name) = load {
        // The old rack's plug-ins go now; the preset's load a frame at a time (busy overlay).
        let loaded = app.session.load_rack_preset(&name).map(|_| ());
        app.report(loaded);
    }
}

/// The open question, if any (every frame, after the panes).
pub fn dialogs(app: &mut DawApp, ctx: &egui::Context) {
    let Some(mut question) = app.rack_presets.question.clone() else {
        return;
    };
    let existing: Vec<String> = app
        .session
        .rack_presets()
        .into_iter()
        .map(|p| p.name)
        .collect();
    let mut done = false;
    let mut save: Option<(String, bool)> = None;
    let mut delete = None;
    let modal = theme::modal(ctx, "rack-preset").show(ctx, |ui| {
        ui.set_width(380.0);
        match &mut question {
            Question::Save { name, error } => {
                ui.heading("Save rack preset");
                ui.label(theme::hint(
                    "The rack's plug-ins in order, with their settings and bypass. Kept beside \
                     the program, for every project; Presets loads it again.",
                ));
                ui.add_space(6.0);
                let field = theme::setting_row(ui, 60.0, "Name", "", |ui| {
                    ui.add(egui::TextEdit::singleline(name).desired_width(260.0))
                });
                if field.changed() {
                    *error = None;
                }
                if ui.memory(|m| m.focused().is_none()) {
                    field.request_focus();
                }
                let taken = existing
                    .iter()
                    .find(|e| e.eq_ignore_ascii_case(name.trim()))
                    .cloned();
                if let Some(taken) = &taken {
                    ui.label(theme::hint(format!(
                        "A preset called \"{taken}\" exists: Replace overwrites it."
                    )));
                }
                if let Some(error) = error {
                    ui.label(egui::RichText::new(error.as_str()).color(theme::RECORD));
                }
                ui.add_space(8.0);
                let enter = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                theme::button_row(ui, |ui| {
                    let label = if taken.is_some() { "Replace" } else { "Save" };
                    let ok = ui
                        .add_enabled(!name.trim().is_empty(), theme::primary_button(label))
                        .clicked();
                    if (ok || enter) && !name.trim().is_empty() {
                        save = Some((name.clone(), taken.is_some()));
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            }
            Question::Delete(name) => {
                ui.heading(format!("Delete \"{name}\"?"));
                ui.label("The preset is deleted for good. The rack as it is now stays.");
                ui.add_space(8.0);
                theme::button_row(ui, |ui| {
                    if ui.add(theme::primary_button("Delete")).clicked() {
                        delete = Some(name.clone());
                    }
                    if ui.button("Cancel").clicked() {
                        done = true;
                    }
                });
            }
        }
    });
    if modal.should_close() {
        done = true;
    }
    if let Some((name, replace)) = save {
        match app.session.save_rack_preset(&name, replace) {
            Ok(_) => {
                app.toast(format!("Saved the rack as \"{}\"", name.trim()));
                done = true;
            }
            Err(e) => {
                // Said in the question, which stays open to fix the name.
                if let Question::Save { error, .. } = &mut question {
                    *error = Some(format!("{e:#}"));
                }
            }
        }
    }
    if let Some(name) = delete {
        let deleted = app.session.delete_rack_preset(&name);
        app.report(deleted);
        done = true;
    }
    app.rack_presets.question = (!done).then_some(question);
}

/// The check run's click on the Presets button (`raw_input_hook`), once the rack has been drawn
/// a few frames; the pointer then rests on the menu's first preset.
pub fn demo_input(app: &mut DawApp, raw_input: &mut egui::RawInput) {
    let (Some(step), Some(button)) = (
        app.rack_presets.demo_click.as_mut(),
        app.rack_presets.button,
    ) else {
        return;
    };
    *step += 1;
    let at = button.center();
    if *step == 5 {
        raw_input.events.push(egui::Event::PointerMoved(at));
        for pressed in [true, false] {
            raw_input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
    } else if *step > 10 {
        // Down onto the menu's first entry.
        raw_input
            .events
            .push(egui::Event::PointerMoved(at + egui::vec2(0.0, 30.0)));
    }
}

/// The first clip's file in the Waveform view with a selection (the shots' setting).
fn show_first_clip(app: &mut DawApp) {
    let clip = app
        .session
        .project
        .tracks
        .iter()
        .find_map(|t| t.clips.first().map(|c| c.id));
    if let Some(clip) = clip {
        crate::waveform::open_clip(app, clip);
    }
    if let Some(id) = app.editor.current {
        let rate = app
            .session
            .document(id)
            .map_or(48_000, |d| u64::from(d.audio.sample_rate));
        let view = app.editor.view(id);
        view.selection = Some((rate, rate * 3));
        view.playhead = rate;
    }
}

/// A rack for the shots: Amplify set off its defaults, a Parametric EQ bypassed, and
/// the `eq` VST3 if one is given and installed (`--vst3`; else, and in a neutral run, the
/// Limiter).
fn demo_rack(app: &mut DawApp) -> anyhow::Result<()> {
    let amp = app.session.add_insert(
        InsertTarget::Editor,
        &daw_core::native::path_of(&daw_core::native::amplify::KIND),
    )?;
    app.session.set_parameter(amp, 0, (3.0 + 96.0) / 144.0)?;
    app.session.set_parameter(amp, 1, (-2.0 + 96.0) / 144.0)?;
    let eq = app.session.add_insert(
        InsertTarget::Editor,
        &daw_core::native::path_of(&daw_core::native::parametric_eq::KIND),
    )?;
    if let Some(slot) = app.session.insert_mut(eq) {
        slot.bypass = true;
    }
    if let Some(path) = app.demo_effect("eq", "Limiter") {
        app.session.add_insert(InsertTarget::Editor, &path)?;
    }
    Ok(())
}

/// `--show rack-restored`: a rack kept in `out/check/rack/rack.json` (the demo rack, and a
/// plug-in that is no longer installed, with its settings) brought back as a new run brings
/// the user's: its slots at once, its plug-ins behind the busy overlay; the shot once they are
/// in, the missing one greyed with why.
pub fn show_restored(app: &mut DawApp) -> anyhow::Result<()> {
    show_first_clip(app);
    demo_rack(app)?;
    let mut rack = app.session.rack_as_file();
    let folder = std::path::absolute("out/check/rack")?;
    // Where a plug-in would be on any Windows machine, so the shot's notice names no
    // machine's folders; only looked for (not found), never written.
    let missing = std::path::Path::new(r"C:\Program Files\Common Files\VST3");
    rack.slots.push(RackSlot {
        name: "Missing Plug-in".into(),
        path: missing.join("Missing Plug-in.vst3"),
        bypass: false,
        state: Some(std::sync::Arc::new(b"its settings, kept".to_vec())),
    });
    std::fs::create_dir_all(&folder)?;
    let file = folder.join("rack.json");
    std::fs::write(&file, serde_json::to_string(&RackFile::new(rack.slots))?)?;
    app.session.rack_file = Some(file);
    app.session.restore_rack()?;
    app.rack_presets.demo_restored = true;
    Ok(())
}

/// `--show rack-presets`: the demo rack, two presets saved from it (in the check run's own
/// `out/check/RackPresets`), and the Presets menu opened.
pub fn show_presets(app: &mut DawApp) -> anyhow::Result<()> {
    show_first_clip(app);
    demo_rack(app)?;
    const SHOWN: [&str; 2] = ["Clean-up", "Mastering chain"];
    // Only these two: an earlier run's presets go, from the check run's own folder only
    // (never the user's, as a `--show` run without a screenshot would use).
    let own = std::path::absolute("out/check/RackPresets").ok();
    if own.is_some() && app.session.rack_presets_dir == own {
        for old in app.session.rack_presets() {
            if !SHOWN.contains(&old.name.as_str()) {
                app.session.delete_rack_preset(&old.name)?;
            }
        }
    }
    for name in SHOWN {
        app.session.save_rack_preset(name, true)?;
    }
    app.rack_presets.demo_click = Some(0);
    Ok(())
}
