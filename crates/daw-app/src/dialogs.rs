//! Menus, the export dialogs and the export progress overlay.

use crate::app::DawApp;
use crate::theme;
use daw_core::project::TrackKind;
use egui::RichText;
use std::path::Path;

/// The question a dropped MIDI file asks when its tempo or time signature differs from the
/// project's.
pub fn tempo_question(app: &mut DawApp, ctx: &egui::Context) {
    let Some(pending) = app.pending_drop.as_ref() else {
        return;
    };
    // Asked first: into the tracks it was dropped on, or onto new ones (16.2).
    if let Some(into) = pending.into.clone() {
        into_question(app, ctx, &into);
        return;
    }
    let Some((_, question)) = pending.tempo.clone() else {
        return;
    };
    let mut answer = None;
    let modal = crate::theme::modal(ctx, "tempo-question").show(ctx, |ui| {
        ui.set_max_width(420.0);
        ui.heading("Use the MIDI file's tempo?");
        ui.add_space(6.0);
        ui.label(question);
        ui.add_space(4.0);
        ui.label(theme::hint(
            "Either way the notes keep their beats; only how fast the song runs changes.",
        ));
        ui.add_space(12.0);
        // The answer to go ahead on the right; Cancel furthest from it.
        theme::button_row(ui, |ui| {
            if ui
                .add(theme::primary_button("Use the file's tempo"))
                .clicked()
            {
                answer = Some(Some(true));
            }
            if ui.button("Keep the project's tempo").clicked() {
                answer = Some(Some(false));
            }
            if ui.button("Cancel").clicked() {
                answer = Some(None);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(None);
    }
    if let Some(answer) = answer {
        crate::drop::answer(app, answer);
    }
}

/// Files dropped (or imported) onto tracks that can take them: into those tracks (a
/// template's, with their plug-ins), or onto new ones.
fn into_question(app: &mut DawApp, ctx: &egui::Context, into: &crate::drop::IntoQuestion) {
    let mut answer = None;
    let modal = crate::theme::modal(ctx, "into-question").show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading("Import into the track?");
        ui.add_space(6.0);
        ui.label(&into.text);
        ui.add_space(12.0);
        theme::button_row(ui, |ui| {
            if ui.add(theme::primary_button(&into.into)).clicked() {
                answer = Some(Some(true));
            }
            if ui.button(&into.new).clicked() {
                answer = Some(Some(false));
            }
            if ui.button("Cancel").clicked() {
                answer = Some(None);
            }
        });
    });
    if modal.should_close() && answer.is_none() {
        answer = Some(None);
    }
    if let Some(answer) = answer {
        crate::drop::answer_into(app, answer);
    }
}

pub fn menu_bar(app: &mut DawApp, ui: &mut egui::Ui) {
    egui::MenuBar::new().ui(ui, |ui| {
        let file = ui.menu_button("File", |ui| {
            if ui.add(egui::Button::new("New project…").shortcut_text("Ctrl+N")).clicked() {
                app.start.open = true;
                ui.close();
            }
            if ui.add(egui::Button::new("Open…").shortcut_text("Ctrl+O")).clicked() {
                open_dialog(app);
                ui.close();
            }
            let recent = app.settings.recent.clone();
            ui.add_enabled_ui(!recent.is_empty(), |ui| {
                ui.menu_button("Open recent", |ui| {
                    for path in recent {
                        if ui.button(path.display().to_string()).clicked() {
                            app.open_project_asking(&path);
                            ui.close();
                        }
                    }
                });
            });
            if ui
                .add_enabled(
                    app.session.path.is_some(),
                    egui::Button::new("Open backup…"),
                )
                .on_hover_text("The project's autosaved backups, newest first")
                .clicked()
            {
                app.show_backups = true;
                ui.close();
            }
            ui.separator();
            if ui.add(egui::Button::new("Save").shortcut_text("Ctrl+S")).clicked() {
                app.save_project(false);
                ui.close();
            }
            if ui.button("Save as…").clicked() {
                app.save_project(true);
                ui.close();
            }
            if ui
                .button("Save as template…")
                .on_hover_text(
                    "The project's tracks, routing and plug-ins with their settings, without its clips, to start new projects from",
                )
                .clicked()
            {
                let name = app.session.project.name.clone();
                app.save_template = Some(crate::templates::SaveTemplate::new(&name));
                ui.close();
            }
            if ui.button("Templates…").clicked() {
                app.templates_window = Some(Default::default());
                ui.close();
            }
            ui.separator();
            if ui.button("Import audio…").clicked() {
                app.import_audio_dialog();
                ui.close();
            }
            if ui
                .button("Import MIDI…")
                .on_hover_text(
                    "At the playhead, on the selected track and the ones after it: each of \
                     the file's tracks on its own track",
                )
                .clicked()
            {
                app.import_midi_dialog();
                ui.close();
            }
            if ui
                .button("Open audio file…")
                .on_hover_text("Open a file in the Waveform view, on its own")
                .clicked()
            {
                crate::waveform::choose_file(app);
                ui.close();
            }
            let current = app
                .editor
                .current
                .filter(|_| app.view_mode == daw_core::api::ViewMode::Waveform);
            if ui
                .add_enabled(current.is_some(), egui::Button::new("Export file…"))
                .on_hover_text(
                    "The Waveform view's file as WAV, AIFF, FLAC, MP3 or Ogg Vorbis (its own \
                     file stays as it is)",
                )
                .clicked()
            {
                app.export_document = current;
                ui.close();
            }
            if ui.button("Export mixdown…").clicked() {
                app.export_dialog();
                ui.close();
            }
            ui.menu_button("Mixdown to new file", |ui| {
                let hint = "Render the mix as it plays into a new file in the project's Rendered folder, and open it in the Waveform view";
                if ui.button("Whole session").on_hover_text(hint).clicked() {
                    app.mixdown_to_new_file(None);
                    ui.close();
                }
                let selection = app.time_selection.as_ref().map(|r| (r.start, r.end));
                if ui
                    .add_enabled(selection.is_some(), egui::Button::new("Time selection"))
                    .on_hover_text(hint)
                    .clicked()
                {
                    app.mixdown_to_new_file(selection);
                    ui.close();
                }
            });
            ui.separator();
            if ui.add(egui::Button::new("Exit").shortcut_text("Alt+F4")).clicked() {
                crate::unsaved::exit(app, ui.ctx());
                ui.close();
            }
        });
        crate::marks::put(ui, "menu.file", file.response.rect);
        let edit = ui.menu_button("Edit", |ui| {
            let (undo, redo, _) = app.session.history();
            let label = |what: &str, next: Option<&String>| match next {
                Some(name) => format!("{what} {name}"),
                None => what.to_string(),
            };
            let undo_button =
                egui::Button::new(label("Undo", undo.first())).shortcut_text("Ctrl+Z");
            if ui.add_enabled(!undo.is_empty(), undo_button).clicked() {
                app.undo();
                ui.close();
            }
            let redo_button =
                egui::Button::new(label("Redo", redo.first())).shortcut_text("Ctrl+Shift+Z");
            if ui.add_enabled(!redo.is_empty(), redo_button).clicked() {
                app.redo();
                ui.close();
            }
            ui.separator();
            let selected = !app.selected_clip_ids().is_empty();
            let pastable = app.session.clipboard_len() > 0;
            for (label, keys, enabled, what) in [
                ("Cut", "Ctrl+X", selected, 0),
                ("Copy", "Ctrl+C", selected, 1),
                ("Paste at playhead", "Ctrl+V", pastable, 2),
                ("Duplicate", "Ctrl+D", selected, 3),
            ] {
                let button = egui::Button::new(label).shortcut_text(keys);
                if ui.add_enabled(enabled, button).clicked() {
                    match what {
                        0 => app.copy_selection(true),
                        1 => app.copy_selection(false),
                        2 => app.paste(),
                        _ => app.duplicate_selection(),
                    }
                    ui.close();
                }
            }
            if ui
                .add_enabled(selected, egui::Button::new("Quantize…").shortcut_text("Q"))
                .clicked()
            {
                crate::quantize::open_quantize(app);
                ui.close();
            }
            ui.separator();
            if ui
                .add(egui::Button::new("Add marker at playhead").shortcut_text("Ins"))
                .clicked()
            {
                app.add_marker_at_playhead();
                ui.close();
            }
        });
        crate::marks::put(ui, "menu.edit", edit.response.rect);
        let track = ui.menu_button("Track", |ui| {
            if ui.button("Add audio track Ctrl+T").clicked() {
                app.add_track(TrackKind::Audio);
                ui.close();
            }
            if ui.button("Add instrument track").clicked() {
                app.add_track(TrackKind::Instrument);
                ui.close();
            }
            if ui.button("Add folder (submix)").clicked() {
                app.add_track(TrackKind::Folder);
                ui.close();
            }
            if ui
                .button("Add bus (return)")
                .on_hover_text("A return track for shared effects, fed by sends")
                .clicked()
            {
                app.add_track(TrackKind::Bus);
                ui.close();
            }
            if let Some(crate::app::Selection::Track(id)) = app.selection {
                ui.separator();
                ui.menu_button("Add send from selected track", |ui| {
                    crate::sends::add_send_menu(app, ui, id)
                });
                crate::render::menu_items(app, ui, id);
            }
        });
        crate::marks::put(ui, "menu.track", track.response.rect);
        let view = ui.menu_button("View", |ui| {
            let current = app
                .editor
                .current
                .filter(|_| app.view_mode == daw_core::api::ViewMode::Waveform);
            if ui
                .add_enabled(current.is_some(), egui::Button::new("Metadata…"))
                .on_hover_text("The Waveform view's file: its title, artist, album, cover art…")
                .clicked()
            {
                if let Some(doc) = current {
                    app.metadata_panel = crate::metadata_panel::MetadataPanel::open(app, doc);
                }
                ui.close();
            }
            ui.separator();
            panes_menu(app, ui);
            ui.add(crate::theme::checkbox(&mut app.follow_playhead, "Follow playhead"));
            ui.menu_button("Layouts", |ui| layouts_menu(app, ui))
                .response
                .on_hover_text(
                    "Drag a tab onto any side of a pane to split it there, or onto another tab bar to join it. Save the arrangement here to bring it back later.",
                );
            ui.separator();
            let zoom_in = egui::Button::new("Zoom in").shortcut_text("= / Ctrl+wheel");
            if ui.add(zoom_in).clicked() {
                app.zoom_step(true);
            }
            let zoom_out = egui::Button::new("Zoom out").shortcut_text("- / Ctrl+wheel");
            if ui.add(zoom_out).clicked() {
                app.zoom_step(false);
            }
            if ui
                .add(egui::Button::new("Fit project").shortcut_text("F"))
                .clicked()
            {
                app.zoom_to_fit();
                ui.close();
            }
            let clip = app.selected_clip.map(|(_, clip)| clip);
            let zoom_clip = egui::Button::new("Zoom to selected clip");
            if ui.add_enabled(clip.is_some(), zoom_clip).clicked() {
                if let Some(clip) = clip {
                    let _ = app.zoom_to_clip(clip);
                }
                ui.close();
            }
            ui.separator();
            ui.menu_button("Track height", |ui| {
                ui.label(egui::RichText::new("Alt+wheel, or drag a header's bottom edge").weak());
                for (name, height) in crate::arrange::TRACK_HEIGHT_PRESETS {
                    if ui.button(name).clicked() {
                        app.set_all_track_heights(height);
                        ui.close();
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.label("Waveform gain");
                ui.add(
                    egui::Slider::new(
                        &mut app.waveform_scale,
                        1.0..=daw_core::api::MAX_WAVEFORM_SCALE,
                    )
                    .logarithmic(true)
                    .suffix("×"),
                )
                .on_hover_text("Display only (Alt+Shift+wheel). The sound does not change.");
            });
        });
        crate::marks::put(ui, "menu.view", view.response.rect);
        let options = ui.menu_button("Options", |ui| {
            use crate::settings_window::Section;
            if ui
                .button("Settings…")
                .on_hover_text("Audio, MIDI, projects and AI control (MCP)")
                .clicked()
            {
                app.open_settings(Section::Audio);
                ui.close();
            }
            // The same window, on its AI control section (the MCP port, the token, the status).
            if ui
                .button("AI control (MCP)…")
                .on_hover_text("The MCP server's port, its token and how to connect Claude Code")
                .clicked()
            {
                app.open_settings(Section::Mcp);
                ui.close();
            }
            if ui.button("Rescan plug-ins").clicked() {
                app.session.rescan_plugins();
                let n = app.session.installed_plugins().len();
                app.toast(format!("Found {n} VST3 plug-ins"));
                ui.close();
            }
            if ui.button("Reconnect MIDI devices").clicked() {
                app.session.restart_midi();
                ui.close();
            }
        });
        crate::marks::put(ui, "menu.options", options.response.rect);
        let help = ui.menu_button("Help", |ui| {
            // Open this pass (the driven check, `help_menu_open`).
            let pass = ui.ctx().cumulative_pass_nr();
            ui.ctx().data_mut(|d| d.insert_temp(help_menu_id().with("open"), pass));
            let manual = ui
                .add(egui::Button::new("Manual").shortcut_text("F1"))
                .on_hover_text(
                    "Oxide's manual in your web browser. F1 anywhere opens the page for the \
                     pane or window under the pointer.",
                );
            crate::marks::put(ui, "menu.help.manual", manual.rect);
            crate::value_edit::remember(ui, manual_item_id(), manual.rect, "Manual");
            if manual.clicked() {
                // Its first page (F1 opens the page for where the pointer is).
                let first = daw_core::manual::Target::new(daw_core::manual::INDEX_PAGE, None);
                if let Err(e) = app.open_manual(first) {
                    app.toast(e);
                }
                ui.close();
            }
            if ui
                .button("About Oxide…")
                .on_hover_text("The version, the commit it was built from and its licences")
                .clicked()
            {
                app.show_about = true;
                ui.close();
            }
        });
        crate::marks::put(ui, "menu.help", help.response.rect);
        crate::value_edit::remember(ui, help_menu_id(), help.response.rect, "Help");
        // The two views (Multitrack and Waveform), after the menus (the transport bar has no room to spare).
        ui.separator();
        for (mode, label, tip) in [
            (
                daw_core::api::ViewMode::Waveform,
                "Waveform",
                "One audio file, to edit (Ctrl+Tab switches)",
            ),
            (
                daw_core::api::ViewMode::Multitrack,
                "Multitrack",
                "The arrangement (Ctrl+Tab switches)",
            ),
        ] {
            let view = ui.selectable_label(app.view_mode == mode, label);
            let name = match mode {
                daw_core::api::ViewMode::Waveform => "menu.waveform",
                daw_core::api::ViewMode::Multitrack => "menu.multitrack",
            };
            crate::marks::put(ui, name, view.rect);
            if view.on_hover_text(tip).clicked() {
                app.view_mode = mode;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let name = match &app.session.path {
                // The manual's shots name no machine's folders (`--neutral`).
                Some(_) if app.neutral => crate::neutral::project_path(&app.session.project.name),
                Some(p) => p.display().to_string(),
                None => format!("{} (not saved)", app.session.project.name),
            };
            ui.label(RichText::new(name).small().color(theme::TEXT_DIM));
        });
    });
}

/// The licences of what Oxide bundles and links, beside the program (the installer puts it
/// there; `scripts/installer.ps1` writes it).
pub const LICENCES_FILE: &str = "THIRD-PARTY.txt";

/// Help > About Oxide…: the version and commit (the installer's and `get_app_info`'s), where
/// the program and its own files are, and the licences.
pub fn about(app: &mut DawApp, ctx: &egui::Context) {
    if !app.show_about {
        return;
    }
    let folder = crate::settings::app_dir();
    let licences = folder.join(LICENCES_FILE);
    let mut close = false;
    let modal = theme::modal(ctx, "about").show(ctx, |ui| {
        ui.set_max_width(440.0);
        ui.heading("Oxide");
        ui.add_space(6.0);
        ui.label(format!(
            "Version {}, built from commit {}.",
            crate::version::VERSION,
            crate::version::COMMIT
        ));
        ui.add_space(4.0);
        ui.label(theme::hint(format!(
            "Its settings, templates and rack presets are kept beside the program, in {}. \
             Installing a newer version keeps them.",
            folder.display()
        )));
        ui.label(theme::hint(
            "The libraries it bundles and their licences are listed in THIRD-PARTY.txt in \
             the same folder.",
        ));
        ui.add_space(12.0);
        theme::button_row(ui, |ui| {
            if ui.add(theme::primary_button("Close")).clicked() {
                close = true;
            }
            let found = licences.is_file();
            if ui
                .add_enabled(found, egui::Button::new("Show the licences"))
                .on_disabled_hover_text("Builds that weren't installed have no THIRD-PARTY.txt")
                .clicked()
            {
                if let Err(e) = crate::recent::show_in_explorer(&licences, false) {
                    app.toast(format!("{e:#}"));
                }
            }
        });
    });
    if close || modal.should_close() {
        app.show_about = false;
    }
}

pub fn open_dialog(app: &mut DawApp) {
    let dir = app
        .session
        .project_dir()
        .and_then(|d| d.parent().map(Path::to_path_buf))
        .or_else(|| app.settings.projects_parent.clone());
    let dialog = match dir {
        Some(dir) => rfd::FileDialog::new().set_directory(dir),
        None => rfd::FileDialog::new(),
    }
    .add_filter(
        "Oxide project",
        &[daw_core::folder::EXTENSION, daw_core::folder::OLD_EXTENSION],
    );
    // The file first, then "Save the changes?" (`unsaved`) before the project is replaced.
    crate::file_dialog::pick_file(app, dialog, |app, path| app.open_project_asking(&path));
}

pub fn export_progress(app: &mut DawApp, ctx: &egui::Context) {
    // Read now, not from housekeeping's copy: a render started this frame (the render dialog's
    // Render) shows its bar in this frame already.
    let daw_core::session::ExportStatus::Running {
        progress,
        kind,
        what,
    } = app.session.export_status()
    else {
        return;
    };
    crate::theme::modal(ctx, "export").show(ctx, |ui| {
        ui.set_width(360.0);
        ui.label(RichText::new(format!("{what}…")).strong());
        theme::progress(ui, progress, true);
        let note = match kind {
            daw_core::session::ExportKind::File => "Writing the file in its format.",
            _ => {
                "Rendered through the live engine with the same plug-ins and block size as playback, then written in its format."
            }
        };
        ui.add(egui::Label::new(theme::hint(note)).wrap());
        theme::button_row(ui, |ui| {
            if ui.button("Cancel").clicked() {
                app.session.cancel_export();
            }
        });
    });
    crate::frame_check::bar_painted(app);
}

/// Export options, before the file picker: normalising to the project's loudness target, and
/// the tail.
pub fn export_options(app: &mut DawApp, ctx: &egui::Context) {
    if !app.export_options {
        return;
    }
    let target = app.session.project.loudness_target;
    let mut go = false;
    let mut close = false;
    let modal = crate::theme::modal(ctx, "export-options").show(ctx, |ui| {
        ui.set_max_width(500.0);
        ui.heading("Export mixdown");
        ui.add_space(6.0);
        let rate = app.session.sample_rate();
        let problem = crate::export_format::controls(ui, &mut app.settings.export_encoding, rate);
        ui.add_space(6.0);
        let normalise = ui.add(crate::theme::checkbox(
            &mut app.settings.normalize_exports,
            format!("Normalise to the {target:.1} LUFS target, peaks at most {EXPORT_CEILING:.1} dBTP"),
        ));
        crate::marks::put(ui, "export.normalise", normalise.rect);
        ui.add(
            egui::Label::new(theme::hint(
                "One gain for the whole file, no limiter: the sound doesn't change, only its level. \
                 If the target would push the true peak past the ceiling, the gain stops there.",
            ))
            .wrap(),
        );
        if let Some(range) = &app.time_selection {
            ui.add_space(6.0);
            let rate = f64::from(app.session.sample_rate());
            let seconds = |s: u64| s as f64 / rate;
            ui.add(crate::theme::checkbox(
                &mut app.export_selection,
                format!(
                    "Only the time selection ({:.2} to {:.2} s)",
                    seconds(range.start),
                    seconds(range.end)
                ),
            ))
            .on_hover_text("Untick for the whole project");
        }
        ui.add_space(6.0);
        let tail = ui.horizontal(|ui| {
            ui.label("Tail after the end");
            ui.add(
                egui::DragValue::new(&mut app.export_tail)
                    .range(0.0..=60.0)
                    .speed(0.1)
                    .fixed_decimals(1)
                    .suffix(" s"),
            )
            .on_hover_text("Extra time for reverb and delay tails");
        });
        crate::marks::put(ui, "export.tail", tail.response.rect);
        // The bounce must hold the stretched audio, so it waits for the renders.
        let rendering = app.session.clips_rendering().len();
        if rendering > 0 {
            ui.add_space(6.0);
            ui.label(theme::hint(format!(
                "{rendering} stretched clip{} still rendering…",
                if rendering == 1 { " is" } else { "s are" }
            )));
        }
        ui.add_space(12.0);
        theme::button_row(ui, |ui| {
            if ui
                .add_enabled(
                    rendering == 0 && problem.is_none(),
                    theme::primary_button("Export…"),
                )
                .clicked()
            {
                go = true;
            }
            if ui.button("Cancel").clicked() {
                close = true;
            }
        });
    });
    let (layer, rect) = (modal.response.layer_id, modal.response.rect);
    app.help.note(ctx, crate::help::Place::Export, layer, rect);
    crate::marks::put_rect("export", rect);
    if go || close || modal.should_close() {
        app.export_options = false;
        app.save_settings();
    }
    if go {
        app.choose_export_file();
    }
}

/// The true-peak ceiling the app normalises exports under (R 128 and the streaming services).
pub const EXPORT_CEILING: f64 = -1.0;

/// What the last export wrote: its loudness against the target, and how it was normalised.
pub fn export_report(app: &mut DawApp, ctx: &egui::Context) {
    use crate::loudness::{format_level, format_peak, format_range};
    let Some(daw_core::session::ExportStatus::Finished {
        path,
        loudness: l,
        normalized,
        ..
    }) = app.export_report.clone()
    else {
        return;
    };
    let target = app.session.project.loudness_target;
    let mut close = false;
    let modal = crate::theme::modal(ctx, "export-report").show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading("Exported");
        // The manual's shots (`--neutral`) show it in the neutral projects folder.
        let shown = if app.neutral {
            let folder = app.session.path.as_deref().and_then(std::path::Path::parent);
            crate::neutral::file_path(&app.session.project.name, folder, &path)
        } else {
            path.display().to_string()
        };
        ui.add(egui::Label::new(theme::hint(shown)).truncate());
        ui.add_space(8.0);
        egui::Grid::new("export-report")
            .num_columns(2)
            .spacing([14.0, 4.0])
            .show(ui, |ui| {
                let row = |ui: &mut egui::Ui, name: &str, value: String, color: egui::Color32| {
                    ui.label(RichText::new(name).color(theme::TEXT_DIM));
                    ui.label(RichText::new(value).monospace().color(color));
                    ui.end_row();
                };
                let text = egui::Color32::from_gray(0xdd);
                row(ui, "Integrated", format_level(l.integrated, false, target), text);
                row(ui, "Target", format!("{target:.1} LUFS"), text);
                let offset = l.integrated.map_or("–".into(), |i| {
                    let lu = i - target;
                    if lu.abs() < 0.05 {
                        "on target".to_string()
                    } else if lu > 0.0 {
                        format!("{lu:+.1} LU (louder)")
                    } else {
                        format!("{lu:+.1} LU (quieter)")
                    }
                });
                row(ui, "From the target", offset, text);
                row(ui, "Loudness range", format_range(l.range), text);
                let hot = l.true_peak.is_some_and(|p| p > EXPORT_CEILING);
                row(
                    ui,
                    "True peak",
                    format_peak(l.true_peak),
                    if hot { theme::METER_HIGH } else { text },
                );
                row(ui, "Max short-term", format_level(l.max_short_term, false, target), text);
                row(ui, "Max momentary", format_level(l.max_momentary, false, target), text);
            });
        ui.add_space(8.0);
        let normalised = match &normalized {
            None => "Not normalised: the file is exactly what playback plays.".to_string(),
            Some(n) => match n.gain_db {
                Some(db) if n.held_by_peak => format!(
                    "Normalised: {db:+.1} dB applied, held by the {:.1} dBTP ceiling (the target wasn't reached).",
                    n.ceiling
                ),
                Some(db) => format!("Normalised: {db:+.1} dB applied, to the {:.1} LUFS target.", n.target),
                None => "Nothing loud enough to normalise: the file was left as rendered.".into(),
            },
        };
        ui.label(normalised);
        ui.add_space(10.0);
        theme::button_row(ui, |ui| {
            if ui.add(theme::primary_button("OK")).clicked() {
                close = true;
            }
        });
    });
    crate::marks::put_rect("export.report", modal.response.rect);
    if close || modal.should_close() {
        app.export_report = None;
    }
}

/// View's panes: every one a tab can close comes back from here, where it was.
fn panes_menu(app: &mut DawApp, ui: &mut egui::Ui) {
    use daw_core::api::ViewMode;
    if ui
        .selectable_label(app.view_mode == ViewMode::Multitrack, "Multitrack")
        .on_hover_text("Bring the arrangement's tab to the front")
        .clicked()
    {
        app.view_mode = ViewMode::Multitrack;
        ui.close();
    }
    if ui
        .selectable_label(app.view_mode == ViewMode::Waveform, "Waveform")
        .on_hover_text("Bring the open file's tab to the front (or the Waveform tab, with none)")
        .clicked()
    {
        app.view_mode = ViewMode::Waveform;
        if app.editor.current.is_none() {
            app.editor.current = app.session.documents().first().map(|d| d.id);
        }
        ui.close();
    }
    ui.add(crate::theme::checkbox(&mut app.show_mixer, "Mixer (M)"));
    ui.add(crate::theme::checkbox(&mut app.show_inspector, "Inspector"));
    ui.add(crate::theme::checkbox(
        &mut app.show_diagnostics,
        "Diagnostics",
    ))
    .on_hover_text("Find clicks or clipped runs in the Waveform view's file and repair them");
    // The piano roll edits a clip: the selected MIDI clip, else the one it last showed, else
    // the project's first.
    let clip = piano_roll_clip(app);
    let mut open = app.piano_roll.is_some();
    let toggle = ui
        .add_enabled(
            open || clip.is_some(),
            crate::theme::checkbox(&mut open, "Piano roll"),
        )
        .on_hover_text("Edit a MIDI clip's notes")
        .on_disabled_hover_text("The project has no MIDI clips to edit");
    if toggle.changed() {
        match (open, clip) {
            (true, Some(clip)) => {
                if let Err(e) = crate::piano_roll::open(app, clip) {
                    app.toast(e);
                }
            }
            _ => app.piano_roll = None,
        }
    }
}

/// The MIDI clip View > Piano roll opens.
fn piano_roll_clip(app: &DawApp) -> Option<daw_core::project::ClipId> {
    let selected = app.selected_clip.map(|(_, c)| c);
    clip_to_edit(&app.session.project, selected, app.last_piano_clip)
}

/// The selected clip if it is a MIDI clip, else `last` if it still is one, else the
/// project's first MIDI clip.
fn clip_to_edit(
    project: &daw_core::project::Project,
    selected: Option<daw_core::project::ClipId>,
    last: Option<daw_core::project::ClipId>,
) -> Option<daw_core::project::ClipId> {
    let midi = |clip: &daw_core::project::ClipId| project.find_midi_clip(*clip).is_some();
    selected.filter(midi).or(last.filter(midi)).or_else(|| {
        project
            .tracks
            .iter()
            .find_map(|t| t.midi_clips.first().map(|c| c.id))
    })
}

/// View > Layouts: save the window's arrangement by name, bring one back, or the default.
fn layouts_menu(app: &mut DawApp, ui: &mut egui::Ui) {
    if ui.button("Save layout as…").clicked() {
        app.layout_name = Some(String::new());
        ui.close();
    }
    ui.separator();
    let names: Vec<String> = std::iter::once(crate::dock::DEFAULT.to_string())
        .chain(app.settings.layouts.keys().cloned())
        .collect();
    let mut delete = None;
    for name in names {
        ui.horizontal(|ui| {
            if ui.button(&name).clicked() {
                let used = app.use_layout(&name);
                if let Err(e) = used {
                    app.toast(e);
                }
                ui.close();
            }
            if name != crate::dock::DEFAULT
                && ui
                    .small_button("🗑")
                    .on_hover_text(format!("Delete the layout \"{name}\""))
                    .clicked()
            {
                delete = Some(name.clone());
            }
        });
    }
    if let Some(name) = delete {
        app.settings.layouts.remove(&name);
        app.save_settings();
    }
}

/// View > Layouts > Save layout as…: the name to keep the window's arrangement under.
pub fn save_layout_dialog(app: &mut DawApp, ctx: &egui::Context) {
    let Some(name) = app.layout_name.as_mut() else {
        return;
    };
    let mut save = false;
    let mut cancel = false;
    crate::theme::modal(ctx, "save-layout").show(ctx, |ui| {
        ui.set_width(360.0);
        ui.heading("Save layout");
        ui.add(
            egui::Label::new(theme::hint(
                "Where each pane is and how big. Open files aren't kept: their place is, so \
                 the files you open later go there.",
            ))
            .wrap(),
        );
        ui.add_space(8.0);
        let edit = theme::setting_row(ui, 50.0, "Name", "", |ui| {
            ui.add(egui::TextEdit::singleline(name).desired_width(280.0))
        });
        edit.request_focus();
        let enter = edit.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let ready = !name.trim().is_empty();
        theme::button_row(ui, |ui| {
            save = ui
                .add_enabled(ready, theme::primary_button("Save"))
                .clicked()
                || (ready && enter);
            cancel = ui.button("Cancel").clicked();
        });
    });
    if save {
        let name = app.layout_name.take().unwrap_or_default();
        match app.save_layout(&name) {
            Ok(()) => app.toast(format!("Saved the layout \"{}\"", name.trim())),
            Err(e) => app.toast(e),
        }
    } else if cancel {
        app.layout_name = None;
    }
}

/// Where the menu bar's Help was drawn (`value_edit::remember`, the driven checks).
pub fn help_menu_id() -> egui::Id {
    egui::Id::unique("menu-help")
}

/// Where Help > Manual was drawn.
pub fn manual_item_id() -> egui::Id {
    egui::Id::unique("menu-help-manual")
}

/// The Help menu was drawn this frame or the one before (the driven check).
pub fn help_menu_open(ctx: &egui::Context) -> bool {
    let drawn = ctx.data(|d| d.get_temp::<u64>(help_menu_id().with("open")));
    drawn.is_some_and(|pass| pass + 1 >= ctx.cumulative_pass_nr())
}

#[cfg(test)]
mod tests {
    use super::*;
    use daw_core::project::{Project, TrackKind};

    #[test]
    fn the_piano_roll_reopens_on_a_midi_clip() {
        let mut project = Project::default();
        assert_eq!(clip_to_edit(&project, None, None), None, "nothing to edit");
        let keys = project.add_track(TrackKind::Instrument, None);
        let first = project.add_midi_clip(keys, 0, 960).unwrap();
        let second = project.add_midi_clip(keys, 3840, 960).unwrap();
        // Nothing chosen: the first; the one it last showed; the selected one wins.
        assert_eq!(clip_to_edit(&project, None, None), Some(first));
        assert_eq!(clip_to_edit(&project, None, Some(second)), Some(second));
        assert_eq!(
            clip_to_edit(&project, Some(first), Some(second)),
            Some(first)
        );
        // An audio clip selected, or a clip deleted since: passed over.
        assert_eq!(clip_to_edit(&project, Some(9999), Some(9998)), Some(first));
    }
}
