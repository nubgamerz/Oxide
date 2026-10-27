//! The inspector: the selected audio clip's stretch, pitch and tempo following, then the
//! routing, monitoring and insert chain of the selected track or master.

use crate::app::{DawApp, Selection};
use crate::automation::{apply_gestures, control, mute_toggle, Automated, Gesture};
use crate::theme;
use crate::value_edit;
use daw_core::automation::{AutomationMode, AutomationTarget};
use daw_core::project::{ClipId, InsertSlot, Monitor, TrackId, TrackInput, TrackKind, TrackOutput};
use daw_core::session::InsertTarget;
use daw_core::stretch::{MAX_SEMITONES, RATIO_RANGE};
use egui::RichText;

pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    settle_fields(app, ui.ctx());
    egui::ScrollArea::vertical()
        .auto_shrink(false)
        .show(ui, |ui| match app.selection {
            Some(Selection::Track(id)) if app.session.project.track(id).is_some() => {
                if let Some((track, clip)) = app.selected_clip {
                    if track == id {
                        clip_inspector(app, ui, clip);
                    }
                }
                track_inspector(app, ui, id)
            }
            Some(Selection::Master) => master_inspector(app, ui),
            _ => {
                ui.add_space(8.0);
                ui.label(
                    RichText::new("Select a track to edit its routing and plug-ins.")
                        .color(theme::TEXT_DIM),
                );
                ui.add_space(8.0);
                if ui.button("Master bus").clicked() {
                    app.selection = Some(Selection::Master);
                }
            }
        });
}

/// The inspector's name and level fields are one each for whatever it shows, and edit what it
/// showed when they opened: a field left open on a track (or the master) it no longer shows
/// (another selected, or none) keeps what was typed there, as clicking away does (22.1, 22.2):
/// an empty name or a value that doesn't read keeps the old one.
fn settle_fields(app: &mut DawApp, ctx: &egui::Context) {
    use value_edit::{level_id, Level, Place};
    let shown = match app.selection {
        Some(Selection::Track(id)) if app.session.project.track(id).is_some() => id,
        Some(Selection::Master) => value_edit::MASTER,
        _ => value_edit::NOTHING,
    };
    let name = value_edit::name_id(Place::Inspector, None);
    if let Some((was, text)) = value_edit::orphaned(ctx, name, shown) {
        let _ = app.session.rename_track(was, &text);
    }
    for level in [Level::Volume, Level::Pan] {
        let id = level_id(Place::Inspector, None, level);
        let Some((was, text)) = value_edit::orphaned(ctx, id, shown) else {
            continue;
        };
        let owner = (was != value_edit::MASTER).then_some(was);
        let typed = match level {
            Level::Volume => value_edit::parse_db(&text).map(|db| (Some(db), None)),
            Level::Pan => value_edit::parse_pan(&text).map(|pan| (None, Some(pan))),
        };
        if let Ok((gain_db, pan)) = typed {
            if let Err(e) = app.session.set_mix_level(owner, gain_db, pan) {
                app.toast(format!("{e:#}"));
            }
        }
    }
}

/// The selected audio clip: time-stretch, pitch and tempo following (dev plan 7.4). The
/// fields call the same session methods as the `set_clip` action and the edge drags.
fn clip_inspector(app: &mut DawApp, ui: &mut egui::Ui, id: ClipId) {
    // Taken before the widgets run: a drag starting this frame stretches from it.
    let Some(before) = app.session.project.find_clip(id).map(|(_, c)| c.clone()) else {
        return;
    };
    section(ui, "CLIP");
    ui.label(RichText::new(&before.name).strong());
    let following = before.follow.is_some();
    let mut error = None;
    egui::Grid::new("clip-stretch")
        .num_columns(2)
        .spacing([8.0, 4.0])
        .show(ui, |ui| {
            ui.label("Stretch");
            let mut ratio = before.stretch;
            let (low, high) = RATIO_RANGE;
            let field = ui
                .add_enabled(
                    !following,
                    egui::DragValue::new(&mut ratio)
                        .range(low..=high)
                        .speed(0.005)
                        .fixed_decimals(3)
                        .prefix("×"),
                )
                .on_hover_text(
                    "Time-stretch: 2 plays twice as long at the same pitch. Alt-drag a clip's \
                     edge to stretch it there.",
                )
                .on_disabled_hover_text("Set by the tempo while the clip follows it");
            if field.drag_started() {
                app.stretch_gesture = Some((id, before.clone()));
            }
            if field.changed() {
                let origin = match &app.stretch_gesture {
                    Some((clip, origin)) if *clip == id => origin.clone(),
                    _ => before.clone(),
                };
                error = app.session.restretch_clip(id, &origin, ratio, false).err();
            }
            if field.drag_stopped() {
                app.stretch_gesture = None;
            }
            ui.end_row();

            ui.label("");
            let mut keep = before.transients;
            if ui
                .add(crate::theme::checkbox(&mut keep, "Keep attacks"))
                .on_hover_text(
                    "Stretching plays each attack unstretched and on time, and stretches the \
                     audio between them instead, so picks and hits stay sharp",
                )
                .changed()
            {
                if let Some(clip) = app.session.project.clip_mut(id) {
                    clip.transients = keep;
                }
                app.session.mark_dirty();
            }
            ui.end_row();

            ui.label("Pitch");
            let mut pitch = before.pitch;
            let field = ui
                .add(
                    egui::DragValue::new(&mut pitch)
                        .range(-MAX_SEMITONES..=MAX_SEMITONES)
                        .speed(0.05)
                        // Signed, as the clip's title shows it.
                        .custom_formatter(|v, _| format!("{v:+.2}"))
                        .suffix(" st"),
                )
                .on_hover_text("Transpose in semitones, at the same length");
            if field.changed() {
                error = app.session.set_clip_stretch(id, None, Some(pitch)).err();
            }
            ui.end_row();

            ui.label("Tempo");
            ui.horizontal(|ui| {
                let mut on = following;
                if ui
                    .add(crate::theme::checkbox(&mut on, "Follow"))
                    .on_hover_text(
                        "Follow the project's tempo and its changes: the clip moves with the \
                         bars and stretches to fit",
                    )
                    .changed()
                {
                    // Recorded in this project, most likely: the tempo where it starts, which
                    // needs no render until the tempo changes.
                    let tempo = on.then(|| app.tempo_map.segment_at(before.start as f64).tempo);
                    error = app.session.set_clip_follow(id, tempo).err();
                }
                if let Some(follow) = &before.follow {
                    let mut tempo = follow.tempo;
                    let field = ui
                        .add(
                            egui::DragValue::new(&mut tempo)
                                .range(20.0..=999.0)
                                .speed(0.1)
                                .fixed_decimals(1)
                                .suffix(" BPM"),
                        )
                        .on_hover_text("The tempo it was recorded at");
                    if field.changed() {
                        error = app.session.set_clip_follow(id, Some(tempo)).err();
                    }
                }
            });
            ui.end_row();
        });
    if let Some(e) = error {
        app.toast(format!("{e:#}"));
    }
    if let Some((_, clip)) = app.session.project.find_clip(id) {
        if app.session.clip_rendering(clip) {
            ui.label(theme::hint("Rendering… it plays unchanged until then."));
        }
    }
}

/// A section's heading, marked `inspector.<title>` for the manual (`inspector.routing`).
fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(8.0);
    let heading = ui.label(RichText::new(title).small().strong().color(theme::TEXT_DIM));
    let line = ui.separator();
    crate::marks::put_with(
        ui,
        || format!("inspector.{}", title.to_lowercase()),
        heading.rect.union(line.rect),
    );
}

fn input_label(input: &TrackInput, midi_ports: &[String]) -> String {
    match input {
        TrackInput::None => "No input".into(),
        TrackInput::Audio {
            channel,
            stereo: false,
        } => format!("In {}", channel + 1),
        TrackInput::Audio {
            channel,
            stereo: true,
        } => format!("In {}+{}", channel + 1, channel + 2),
        TrackInput::Midi { port, channel } => {
            let port = port.clone().unwrap_or_else(|| "All MIDI inputs".into());
            let missing =
                !midi_ports.is_empty() && !port.starts_with("All") && !midi_ports.contains(&port);
            let channel = channel.map_or("omni".into(), |c| format!("ch {}", c + 1));
            format!(
                "{port} ({channel}){}",
                if missing { " — not connected" } else { "" }
            )
        }
        TrackInput::Plugin { .. } => "A plug-in's output".into(),
    }
}

fn output_label(app: &DawApp, output: TrackOutput, parent: Option<TrackId>) -> String {
    match output {
        TrackOutput::Parent => match parent.and_then(|p| app.session.project.track(p)) {
            Some(folder) => format!("Folder: {}", folder.name),
            None => "Master".into(),
        },
        TrackOutput::Master => "Master".into(),
        TrackOutput::Track(id) => app
            .session
            .project
            .track(id)
            .map_or("Master".into(), |t| format!("Bus: {}", t.name)),
        TrackOutput::Hardware { channel } => {
            format!("Out {}+{} (direct)", channel + 1, channel + 2)
        }
    }
}

fn track_inspector(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId) {
    let mut dirty = false;
    let inputs = app.session.input_channels();
    let outputs = app.session.output_channels();
    let midi_ports = app.session.midi.ports.clone();
    let buses: Vec<(TrackId, String)> = app
        .session
        .project
        .valid_buses(id)
        .iter()
        .map(|t| (t.id, t.name.clone()))
        .collect();
    let track = app
        .session
        .project
        .track(id)
        .cloned()
        .expect("checked by caller");

    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(6.0, 22.0), egui::Sense::hover());
        ui.painter()
            .rect_filled(rect, 2, theme::track_color(track.color));
        name_line(app, ui, id, &track.name);
    });
    let kind = match track.kind {
        TrackKind::Audio => "Audio track",
        TrackKind::Instrument => "Instrument track",
        TrackKind::Folder => "Folder: a submix bus for its sub-tracks",
        TrackKind::Bus => "Bus: a return track, fed by other tracks' sends and outputs",
    };
    let kind = ui.label(RichText::new(kind).color(theme::TEXT_DIM));
    // A plain spot the rename check clicks away to (`edit_check`).
    value_edit::remember(ui, kind_id(), kind.rect, "");
    ui.horizontal(|ui| {
        ui.label("Colour");
        let mut color = theme::track_color(track.color);
        if ui.color_edit_button_srgba(&mut color).changed() {
            if let Some(t) = app.session.project.track_mut(id) {
                t.color = [color.r(), color.g(), color.b()];
            }
        }
    });

    section(ui, "ROUTING");
    egui::Grid::new("routing")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            if let TrackInput::Plugin { insert, output } = track.input {
                // A multi-out rack's track: it hears that output, set by the rack.
                ui.label("Input");
                ui.label(app.session.plugin_output_label(insert, output))
                    .on_hover_text(
                        "One of a plug-in's outputs (its multi-out rack). Remove the rack from the plug-in's menu to undo it",
                    );
                ui.end_row();
            } else if track.kind.holds_clips() {
                let input_label_rect = ui.label("Input").rect;
                let mut input = track.input.clone();
                let input_combo = egui::ComboBox::from_id_salt("input")
                    .width(180.0)
                    .selected_text(input_label(&input, &midi_ports))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut input, TrackInput::None, "No input");
                        if track.kind == TrackKind::Audio {
                            if inputs == 0 {
                                ui.label(
                                    RichText::new("The audio device has no inputs")
                                        .color(theme::TEXT_DIM),
                                );
                            }
                            for c in 0..inputs {
                                let mono = TrackInput::Audio {
                                    channel: c as u16,
                                    stereo: false,
                                };
                                let label = input_label(&mono, &midi_ports);
                                ui.selectable_value(&mut input, mono, label);
                            }
                            for c in (0..inputs.saturating_sub(1)).step_by(2) {
                                let stereo = TrackInput::Audio {
                                    channel: c as u16,
                                    stereo: true,
                                };
                                let label = input_label(&stereo, &midi_ports);
                                ui.selectable_value(&mut input, stereo, label);
                            }
                        } else {
                            let all = TrackInput::Midi {
                                port: None,
                                channel: None,
                            };
                            ui.selectable_value(&mut input, all, "All MIDI inputs");
                            for port in &midi_ports {
                                let one = TrackInput::Midi {
                                    port: Some(port.clone()),
                                    channel: None,
                                };
                                ui.selectable_value(&mut input, one, port);
                            }
                        }
                    });
                crate::marks::put(
                    ui,
                    "inspector.input",
                    input_label_rect.union(input_combo.response.rect),
                );
                if let TrackInput::Midi { channel, .. } = &mut input {
                    ui.end_row();
                    ui.label("MIDI channel");
                    egui::ComboBox::from_id_salt("midi-channel")
                        .width(180.0)
                        .selected_text(channel.map_or("Omni".into(), |c| (c + 1).to_string()))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(channel, None, "Omni");
                            for c in 0..16u8 {
                                ui.selectable_value(channel, Some(c), (c + 1).to_string());
                            }
                        });
                }
                if input != track.input {
                    if let Some(t) = app.session.project.track_mut(id) {
                        t.input = input;
                    }
                    dirty = true;
                }
                ui.end_row();
            }

            let output_row = ui.label("Output").rect;
            let mut output = track.output;
            let output_list = egui::ComboBox::from_id_salt("output")
                .width(180.0)
                .selected_text(output_label(app, output, track.parent))
                .show_ui(ui, |ui| {
                    let parent_label = output_label(app, TrackOutput::Parent, track.parent);
                    ui.selectable_value(
                        &mut output,
                        TrackOutput::Parent,
                        format!("Parent ({parent_label})"),
                    );
                    ui.selectable_value(&mut output, TrackOutput::Master, "Master");
                    for (bus, name) in &buses {
                        ui.selectable_value(
                            &mut output,
                            TrackOutput::Track(*bus),
                            format!("Bus: {name}"),
                        );
                    }
                    for c in (0..outputs.saturating_sub(1)).step_by(2) {
                        let hw = TrackOutput::Hardware { channel: c as u16 };
                        ui.selectable_value(&mut output, hw, output_label(app, hw, None));
                    }
                })
                .response;
            crate::marks::put(ui, "inspector.output", output_row.union(output_list.rect));
            if output != track.output {
                if let Some(t) = app.session.project.track_mut(id) {
                    t.output = output;
                }
                dirty = true;
            }
            ui.end_row();

            if track.kind.holds_clips() {
                let monitor_label = ui.label("Monitor").rect;
                let mut monitor = track.monitor;
                let monitor_combo = egui::ComboBox::from_id_salt("monitor")
                    .width(180.0)
                    .selected_text(match monitor {
                        Monitor::Auto => "Auto (when armed)",
                        Monitor::On => "Always on",
                        Monitor::Off => "Off",
                    })
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut monitor, Monitor::Auto, "Auto (when armed)");
                        ui.selectable_value(&mut monitor, Monitor::On, "Always on");
                        ui.selectable_value(&mut monitor, Monitor::Off, "Off");
                    });
                crate::marks::put(
                    ui,
                    "inspector.monitor",
                    monitor_label.union(monitor_combo.response.rect),
                );
                if let Some(t) = app.session.project.track_mut(id) {
                    t.monitor = monitor;
                }
                ui.end_row();
            }
        });

    section(ui, "MIXER");
    let automated = crate::automation::automated(app, Some(id));
    let mut typed = value_edit::Typed::for_owner(Some(id));
    if let Some(t) = app.session.project.track_mut(id) {
        let gestures = mixer_controls(
            ui,
            &mut t.mixer.gain_db,
            &mut t.mixer.pan,
            &mut t.automation_mode,
            automated,
            &mut typed,
        );
        apply_gestures(app, Some(id), &gestures);
    }
    typed.apply(app);

    section(ui, "SENDS");
    crate::sends::inspector_rows(app, ui, id);

    section(ui, "INSERTS");
    inserts(app, ui, InsertTarget::Track(id), &track.inserts);

    if !track.clips.is_empty() {
        section(ui, "CLIPS");
        for clip in &track.clips {
            ui.label(
                RichText::new(format!(
                    "{}  @ {}",
                    clip.name,
                    crate::transport::format_time(clip.start, app.session.sample_rate())
                ))
                .size(theme::HINT_SIZE),
            );
        }
    }
    if dirty {
        app.session.mark_dirty();
    }
}

fn master_inspector(app: &mut DawApp, ui: &mut egui::Ui) {
    ui.heading("Master");
    section(ui, "OUTPUT");
    let outputs = app.session.output_channels();
    let mut channel = app.session.project.master.output_channel;
    egui::ComboBox::from_id_salt("master-out")
        .width(180.0)
        .selected_text(format!("Out {}+{}", channel + 1, channel + 2))
        .show_ui(ui, |ui| {
            for c in (0..outputs.saturating_sub(1).max(1)).step_by(2) {
                ui.selectable_value(&mut channel, c as u16, format!("Out {}+{}", c + 1, c + 2));
            }
        });
    if channel != app.session.project.master.output_channel {
        app.session.project.master.output_channel = channel;
        app.session.mark_dirty();
    }
    section(ui, "LOUDNESS");
    crate::loudness::panel(app, ui);
    section(ui, "MIXER");
    let automated = crate::automation::automated(app, None);
    let mut typed = value_edit::Typed::for_owner(None);
    let master = &mut app.session.project.master;
    let mut gestures = mixer_controls(
        ui,
        &mut master.mixer.gain_db,
        &mut master.mixer.pan,
        &mut master.automation_mode,
        automated,
        &mut typed,
    );
    ui.horizontal(|ui| {
        ui.label("Mute");
        let mute = mute_toggle(
            ui,
            automated.mute,
            &mut master.mixer.mute,
            "inspector.master.mute",
        );
        gestures.extend(mute.map(|g| (AutomationTarget::Mute, g)));
    });
    apply_gestures(app, None, &gestures);
    typed.apply(app);
    section(ui, "INSERTS");
    let slots = app.session.project.master.inserts.clone();
    inserts(app, ui, InsertTarget::Master, &slots);
}

/// Volume, pan and the automation mode. Volume and pan stand as their automation does (see
/// `automation::control`); returns what they did for the automation writer. Their values can
/// be typed (22.2): what was typed comes back in `typed`.
fn mixer_controls(
    ui: &mut egui::Ui,
    gain_db: &mut f32,
    pan: &mut f32,
    mode: &mut AutomationMode,
    automated: Automated,
    typed: &mut value_edit::Typed,
) -> Vec<(AutomationTarget, Gesture)> {
    use value_edit::{level_id, Level, Place};
    let mut gestures = Vec::new();
    let owner = typed.owner;
    let target = value_edit::target_of(owner);
    let readout = |ui: &mut egui::Ui, level: Level, shown: f32, editable: bool| {
        let id = level_id(Place::Inspector, owner, level);
        let font = egui::TextStyle::Body.resolve(ui.style());
        let size = egui::vec2(52.0, ui.spacing().interact_size.y);
        value_edit::level(ui, id, level, shown, size, font, editable, target)
    };
    egui::Grid::new("mixer-controls")
        .num_columns(2)
        .spacing([8.0, 6.0])
        .show(ui, |ui| {
            ui.label("Volume");
            ui.horizontal(|ui| {
                let (_, gesture) = control(ui, automated.volume, gain_db, |ui, gain| {
                    ui.add(egui::Slider::new(gain, -60.0..=12.0).show_value(false))
                });
                gestures.extend(gesture.map(|g| (AutomationTarget::Volume, g)));
                let drive = automated.volume;
                let shown = drive.written.or(drive.lane).unwrap_or(*gain_db);
                let editable = drive.lane.is_none() && !drive.writable;
                typed.gain_db = readout(ui, Level::Volume, shown, editable);
                ui.label(RichText::new("dB").color(theme::TEXT_DIM));
            });
            ui.end_row();
            ui.label("Pan");
            ui.horizontal(|ui| {
                let (_, gesture) = control(ui, automated.pan, pan, |ui, pan| {
                    ui.add(egui::Slider::new(pan, -1.0..=1.0).show_value(false))
                });
                gestures.extend(gesture.map(|g| (AutomationTarget::Pan, g)));
                let drive = automated.pan;
                let shown = drive.written.or(drive.lane).unwrap_or(*pan);
                let editable = drive.lane.is_none() && !drive.writable;
                typed.pan = readout(ui, Level::Pan, shown, editable);
            });
            ui.end_row();
            ui.label("Automation").on_hover_text(
                "What moving the controls while playing does to the lanes: Read plays them; \
                 Touch writes while a control is held; Latch keeps writing after it is let go; \
                 Write writes every lane from the start of playback.",
            );
            egui::ComboBox::from_id_salt("automation-mode")
                .selected_text(format!("{mode:?}"))
                .show_ui(ui, |ui| {
                    for m in [
                        AutomationMode::Read,
                        AutomationMode::Touch,
                        AutomationMode::Latch,
                        AutomationMode::Write,
                    ] {
                        ui.selectable_value(mode, m, format!("{m:?}"));
                    }
                });
            ui.end_row();
        });
    gestures
}

/// The inspector's track name, a heading: clicked, it becomes a field to rename the track
/// (22.1, `value_edit`), committed through the session as `set_track` renames.
fn name_line(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, name: &str) {
    let field = value_edit::name_id(value_edit::Place::Inspector, None);
    let font = egui::TextStyle::Heading.resolve(ui.style());
    if value_edit::is_open(ui.ctx(), field) {
        let size = egui::vec2(220.0, font.size + 8.0);
        if let Some(name) = value_edit::name_field(ui, field, size, font) {
            if let Err(e) = app.session.rename_track(id, &name) {
                app.toast(format!("{e:#}"));
            }
        }
        return;
    }
    let label = ui
        .add(
            egui::Label::new(RichText::new(name).font(font).strong())
                .truncate()
                .sense(egui::Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::Text)
        .on_hover_text("Click to rename");
    value_edit::remember(ui, field, label.rect, name);
    if label.clicked() {
        value_edit::open(ui.ctx(), field, name, id);
    }
}

/// Where the inspector's "Audio track" line is remembered (a plain spot for checks).
pub fn kind_id() -> egui::Id {
    egui::Id::unique("inspector-track-kind")
}

pub fn pan_label(pan: f32) -> String {
    let p = (pan * 100.0).round() as i32;
    match p {
        0 => "C".into(),
        p if p < 0 => format!("L{}", -p),
        p => format!("R{p}"),
    }
}

fn inserts(app: &mut DawApp, ui: &mut egui::Ui, target: InsertTarget, slots: &[InsertSlot]) {
    if slots.is_empty() {
        ui.label(RichText::new("No plug-ins").color(theme::TEXT_DIM));
    }
    let count = slots.len();
    for (i, slot) in slots.iter().enumerate() {
        let info = app.session.insert_info(slot.id);
        ui.horizontal(|ui| {
            let mut enabled = !slot.bypass;
            if ui
                .add(crate::theme::checkbox(&mut enabled, ""))
                .on_hover_text("Enable / bypass")
                .changed()
            {
                set_bypass(app, slot.id, !enabled);
            }
            let name = RichText::new(&slot.name);
            // Not loaded: still coming in (an undo's plug-ins, a new sample rate's: dim, as the
            // rack shows it), or failed (struck through).
            let coming = info.is_none()
                && app.session.loading().is_some()
                && app.session.load_failure(slot.id).is_none();
            let name = if coming {
                name.color(theme::TEXT_DIM).italics()
            } else if info.is_none() {
                name.color(theme::RECORD).strikethrough()
            } else {
                name
            };
            let open = info.is_some_and(|(_, _, open)| open);
            let has_editor = info.is_some_and(|(editor, _, _)| editor);
            let button = ui
                .add_enabled(has_editor, egui::Button::selectable(open, name))
                .on_hover_text(match info {
                    Some((_, latency, _)) => {
                        format!("Click to open the editor\nLatency {latency} samples")
                    }
                    None if coming => "Loading…".into(),
                    None => "Failed to load".into(),
                });
            if button.clicked() {
                if let Err(e) = app.session.toggle_editor(slot.id) {
                    app.toast(format!("{}: {e:#}", slot.name));
                }
            }
            // Right-click: copy and paste, and the multi-out rack of a plug-in with outputs.
            let id = slot.id;
            button.context_menu(|ui| {
                crate::plugin_clipboard::menu_items(app, ui, target, id);
                crate::multi_out::menu_items(app, ui, target, id);
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("✕").on_hover_text("Remove").clicked() {
                    app.session.remove_insert(slot.id);
                }
                if ui
                    .add_enabled(i + 1 < count, egui::Button::new("▼").small())
                    .clicked()
                {
                    app.session.move_insert(slot.id, false);
                }
                if ui
                    .add_enabled(i > 0, egui::Button::new("▲").small())
                    .clicked()
                {
                    app.session.move_insert(slot.id, true);
                }
            });
        });
    }
    ui.add_space(4.0);
    let add = ui
        .button("+ Add plug-in…")
        .on_hover_text("Right-click to paste a copied plug-in");
    if add.clicked() {
        app.browser.open_for(target);
    }
    add.context_menu(|ui| crate::plugin_clipboard::paste_menu(app, ui, target));
}

fn set_bypass(app: &mut DawApp, insert: u32, bypass: bool) {
    let project = &mut app.session.project;
    for slot in project.all_inserts_mut() {
        if slot.id == insert {
            slot.bypass = bypass;
        }
    }
}
