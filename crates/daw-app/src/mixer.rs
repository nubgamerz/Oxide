//! The mixer: one strip per track, master on the right.

use crate::app::{DawApp, Meter, Selection};
use crate::arrange::draw_vertical_meter;
use crate::automation::{apply_gestures, control, mode_button, mute_toggle, Automated, Gesture};
use crate::inspector::pan_label;
use crate::theme;
use crate::value_edit;
use daw_core::automation::AutomationTarget;
use daw_core::project::{Destination, InsertSlot, Mixer, TrackId, TrackSend};
use daw_core::session::InsertTarget;
use egui::{vec2, Color32, RichText, Stroke, UiBuilder};

const STRIP_WIDTH: f32 = 96.0;
const INNER_WIDTH: f32 = STRIP_WIDTH - 8.0;
const INSERT_ROWS: f32 = 4.0;
const SEND_ROWS: f32 = 2.0;
const ROW: f32 = 18.0;
/// Everything in a strip except the fader: header, inserts, sends, pan, buttons, output label.
const STRIP_OVERHEAD: f32 = 24.0
    + (INSERT_ROWS + 1.0) * ROW
    + 6.0
    + (SEND_ROWS + 1.0) * ROW
    + 4.0
    + 40.0
    + 26.0
    + 20.0
    + 16.0;

pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    let height = ui.available_height();
    let row = egui::Layout::left_to_right(egui::Align::Min);
    ui.with_layout(row, |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        let master_width = STRIP_WIDTH + 12.0;
        let tracks_width = (ui.available_width() - master_width).max(STRIP_WIDTH);
        ui.allocate_ui(vec2(tracks_width, height), |ui| {
            egui::ScrollArea::horizontal()
                .auto_shrink(false)
                .show(ui, |ui| {
                    ui.with_layout(row, |ui| {
                        let ids: Vec<TrackId> =
                            app.session.project.tracks.iter().map(|t| t.id).collect();
                        app.strip_rects.clear();
                        for id in ids {
                            track_strip(app, ui, id, height);
                        }
                        // A strip dragged by its name, now that every strip is placed.
                        let strips = app.strip_rects.clone();
                        crate::track_drag::apply(
                            app,
                            ui,
                            crate::track_drag::Surface::Mixer,
                            &strips,
                        );
                        if app.session.project.tracks.is_empty() {
                            ui.label(RichText::new("No tracks").color(theme::TEXT_DIM));
                        }
                    });
                });
        });
        ui.separator();
        master_strip(app, ui, height);
    });
}

fn strip_frame(
    ui: &mut egui::Ui,
    selected: bool,
    height: f32,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    egui::Frame::new()
        .fill(if selected {
            theme::LANE_SELECTED
        } else {
            theme::PANEL
        })
        .corner_radius(4)
        .inner_margin(4)
        .stroke(if selected {
            Stroke::new(1.0, theme::ACCENT)
        } else {
            Stroke::NONE
        })
        .show(ui, |ui| {
            ui.set_width(INNER_WIDTH);
            ui.set_height(height - 10.0);
            ui.vertical(add);
        })
        .response
}

/// A strip's name bar; `key` makes its id (a track's strip keeps it when dragged to
/// another place, `track_drag`).
fn strip_header(
    ui: &mut egui::Ui,
    name: &str,
    color: Color32,
    key: Option<TrackId>,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(vec2(INNER_WIDTH, 20.0), egui::Sense::hover());
    let response = ui.interact(
        rect,
        egui::Id::unique(("mixer-strip-name", key)),
        egui::Sense::click_and_drag(),
    );
    ui.painter().rect_filled(rect, 3, color);
    let font = egui::FontId::proportional(12.0);
    let shown = crate::arrange::keep_ending(ui, name, rect.width() - 6.0, font.clone());
    ui.painter().with_clip_rect(rect).text(
        rect.left_center() + vec2(4.0, 0.0),
        egui::Align2::LEFT_CENTER,
        shown,
        font,
        Color32::BLACK,
    );
    response.on_hover_text(name)
}

fn track_strip(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, height: f32) {
    let Some(track) = app.session.project.track(id).cloned() else {
        return;
    };
    let selected = app.selection == Some(Selection::Track(id));
    // The manual's `mixer.strip.*` marks are the selected track's strip.
    let _quiet = crate::marks::quiet(!selected);
    let meter = app.meters.get(&id).copied().unwrap_or_default();
    let destination = match app.session.project.destination(&track) {
        Destination::Master => "Master".to_string(),
        Destination::Bus(b) => app
            .session
            .project
            .track(b)
            .map_or("Bus".into(), |t| t.name.clone()),
        Destination::Hardware(c) => format!("Out {}+{}", c + 1, c + 2),
    };
    let fader = (height - STRIP_OVERHEAD).max(60.0);
    let automated = crate::automation::automated(app, Some(id));
    let mut gestures = Vec::new();
    let mut typed = value_edit::Typed::for_owner(Some(id));
    let response = strip_frame(ui, selected, height, |ui| {
        let shade = if track.kind.mixes_tracks() { 0.45 } else { 0.8 };
        let color = theme::track_color(track.color).linear_multiply(shade);
        let header = strip_header(ui, &track.name, color, Some(id));
        crate::marks::put(ui, "mixer.strip.name", header.rect);
        if header.clicked() {
            app.selection = Some(Selection::Track(id));
        }
        crate::track_drag::respond(app, &header, id, crate::track_drag::Surface::Mixer);
        inserts_column(app, ui, InsertTarget::Track(id), &track.inserts);
        ui.add_space(4.0);
        sends_column(app, ui, id, &track.sends);
        if let Some(t) = app.session.project.track_mut(id) {
            gestures = fader_section(ui, &mut t.mixer, &meter, fader, automated, &mut typed);
            ui.horizontal(|ui| {
                let mute = mute_toggle(ui, automated.mute, &mut t.mixer.mute, "mixer.strip.mute");
                gestures.extend(mute.map(|g| (AutomationTarget::Mute, g)));
                let (_, solo) = theme::toggle_at(ui, &mut t.mixer.solo, "S", theme::SOLO, "Solo");
                crate::marks::put(ui, "mixer.strip.solo", solo);
                if t.kind.holds_clips() {
                    let (_, arm) = theme::toggle_at(ui, &mut t.arm, "●", theme::RECORD, "Arm");
                    crate::marks::put(ui, "mixer.strip.arm", arm);
                }
                mode_button(ui, &mut t.automation_mode, "mixer.strip.automation_mode");
            });
        }
        let output = ui.add(
            egui::Label::new(
                RichText::new(format!("→ {destination}"))
                    .small()
                    .color(theme::TEXT_DIM),
            )
            .truncate(),
        );
        crate::marks::put(ui, "mixer.strip.output", output.rect);
        output.on_hover_text("Output (change it in the inspector)");
    });
    crate::marks::put(ui, "mixer.strip", response.rect);
    if response.clicked() {
        app.selection = Some(Selection::Track(id));
    }
    app.strip_rects.push((id, track.kind, response.rect));
    apply_gestures(app, Some(id), &gestures);
    typed.apply(app);
}

fn master_strip(app: &mut DawApp, ui: &mut egui::Ui, height: f32) {
    let selected = app.selection == Some(Selection::Master);
    let meter = app.master_meter;
    let channel = app.session.project.master.output_channel;
    let slots = app.session.project.master.inserts.clone();
    let fader = (height - STRIP_OVERHEAD).max(60.0);
    let automated = crate::automation::automated(app, None);
    let mut gestures = Vec::new();
    let mut typed = value_edit::Typed::for_owner(None);
    let strip = strip_frame(ui, selected, height, |ui| {
        let header = strip_header(ui, "MASTER", theme::ACCENT.linear_multiply(0.8), None);
        crate::marks::put(ui, "mixer.master.name", header.rect);
        if header.clicked() {
            app.selection = Some(Selection::Master);
        }
        inserts_column(app, ui, InsertTarget::Master, &slots);
        let master = &mut app.session.project.master;
        gestures = fader_section(ui, &mut master.mixer, &meter, fader, automated, &mut typed);
        ui.horizontal(|ui| {
            let mute = mute_toggle(
                ui,
                automated.mute,
                &mut master.mixer.mute,
                "mixer.master.mute",
            );
            gestures.extend(mute.map(|g| (AutomationTarget::Mute, g)));
            mode_button(
                ui,
                &mut master.automation_mode,
                "mixer.master.automation_mode",
            );
        });
        crate::loudness::strip_line(app, ui);
        ui.label(
            RichText::new(format!("→ Out {}+{}", channel + 1, channel + 2))
                .small()
                .color(theme::TEXT_DIM),
        );
    });
    crate::marks::put(ui, "mixer.master", strip.rect);
    apply_gestures(app, None, &gestures);
    typed.apply(app);
}

fn inserts_column(app: &mut DawApp, ui: &mut egui::Ui, target: InsertTarget, slots: &[InsertSlot]) {
    // A fixed-height box so every strip's fader lines up, scrolling when there are many inserts.
    let (rect, _) = ui.allocate_exact_size(
        vec2(INNER_WIDTH, (INSERT_ROWS + 1.0) * ROW),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 2, theme::BG);
    let mark = match target {
        InsertTarget::Master => "mixer.master.inserts",
        _ => "mixer.strip.inserts",
    };
    crate::marks::put(ui, mark, rect);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink(1.0)));
    egui::ScrollArea::vertical()
        .id_salt(("inserts", format!("{target:?}")))
        .auto_shrink(false)
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for slot in slots {
                let info = app.session.insert_info(slot.id);
                let open = info.is_some_and(|(_, _, o)| o);
                let text = RichText::new(&slot.name).small();
                let text = if slot.bypass {
                    text.color(theme::TEXT_DIM).strikethrough()
                } else {
                    text
                };
                let button = egui::Button::selectable(open, text).truncate();
                let response = ui
                    .add_sized(vec2(ui.available_width(), ROW - 2.0), button)
                    .on_hover_text(format!(
                        "{}\nClick: editor · Right-click: options",
                        slot.name
                    ));
                if response.clicked() {
                    if let Err(e) = app.session.toggle_editor(slot.id) {
                        app.toast(format!("{}: {e:#}", slot.name));
                    }
                }
                response.context_menu(|ui| {
                    let label = if slot.bypass { "Enable" } else { "Bypass" };
                    if ui.button(label).clicked() {
                        for s in app.session.project.all_inserts_mut() {
                            if s.id == slot.id {
                                s.bypass = !s.bypass;
                            }
                        }
                        ui.close();
                    }
                    if ui.button("Remove").clicked() {
                        app.session.remove_insert(slot.id);
                        ui.close();
                    }
                    crate::plugin_clipboard::menu_items(app, ui, target, slot.id);
                    crate::multi_out::menu_items(app, ui, target, slot.id);
                });
            }
            let add = ui
                .add_sized(
                    vec2(ui.available_width(), ROW - 2.0),
                    egui::Button::new(RichText::new("+").small()),
                )
                .on_hover_text("Add plug-in (right-click: paste a copied one)");
            if add.clicked() {
                app.browser.open_for(target);
            }
            add.context_menu(|ui| crate::plugin_clipboard::paste_menu(app, ui, target));
        });
}

/// The strip's sends, in a fixed-height box like the inserts so faders line up.
fn sends_column(app: &mut DawApp, ui: &mut egui::Ui, id: TrackId, sends: &[TrackSend]) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(INNER_WIDTH, (SEND_ROWS + 1.0) * ROW),
        egui::Sense::hover(),
    );
    ui.painter().rect_filled(rect, 2, theme::BG);
    crate::marks::put(ui, "mixer.strip.sends", rect);
    let mut child = ui.new_child(UiBuilder::new().max_rect(rect.shrink(1.0)));
    egui::ScrollArea::vertical()
        .id_salt(("sends", id))
        .auto_shrink(false)
        .show(&mut child, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for send in sends {
                let size = vec2(ui.available_width(), ROW - 2.0);
                crate::sends::strip_bar(app, ui, send, size);
            }
            let add = ui
                .add_sized(
                    vec2(ui.available_width(), ROW - 2.0),
                    egui::Button::new(RichText::new("+ send").small()),
                )
                .on_hover_text("Add a send to a folder or bus");
            egui::Popup::menu(&add).show(|ui| crate::sends::add_send_menu(app, ui, id));
        });
}

/// Pan, fader, meter and level, as their automation stands (see `automation::control`).
/// Returns what the pan and fader did for the automation writer; the pan's and the level's
/// readouts can be typed into (22.2), and what was typed comes back in `typed`.
fn fader_section(
    ui: &mut egui::Ui,
    mixer: &mut Mixer,
    meter: &Meter,
    fader_height: f32,
    automated: Automated,
    typed: &mut value_edit::Typed,
) -> Vec<(AutomationTarget, Gesture)> {
    use value_edit::{level_id, Level, Place};
    let mut gestures = Vec::new();
    let owner = typed.owner;
    let target = value_edit::target_of(owner);
    // The manual's names: the first track's strip, or the master's.
    let prefix = if owner.is_some() {
        "mixer.strip"
    } else {
        "mixer.master"
    };
    ui.add_space(4.0);
    ui.spacing_mut().slider_width = INNER_WIDTH;
    let (pan, gesture) = control(ui, automated.pan, &mut mixer.pan, |ui, pan| {
        ui.add(
            egui::Slider::new(pan, -1.0..=1.0)
                .show_value(false)
                .trailing_fill(false),
        )
        .on_hover_text(format!("Pan {} (double-click to centre)", pan_label(*pan)))
    });
    crate::marks::put_with(ui, || format!("{prefix}.pan"), pan.rect);
    gestures.extend(gesture.map(|g| (AutomationTarget::Pan, g)));
    let fixed = automated.pan.lane.is_none() && !automated.pan.writable;
    if pan.double_clicked() && fixed {
        mixer.pan = 0.0;
    }
    let shown = automated
        .pan
        .written
        .or(automated.pan.lane)
        .unwrap_or(mixer.pan);
    let id = level_id(Place::Mixer, owner, Level::Pan);
    let font = egui::TextStyle::Small.resolve(ui.style());
    let size = vec2(40.0, 16.0);
    typed.pan = value_edit::level(ui, id, Level::Pan, shown, size, font, fixed, target);
    ui.horizontal(|ui| {
        ui.spacing_mut().slider_width = fader_height;
        let (fader, gesture) = control(ui, automated.volume, &mut mixer.gain_db, |ui, gain| {
            ui.add(
                egui::Slider::new(gain, -60.0..=12.0)
                    .vertical()
                    .show_value(false)
                    .trailing_fill(true),
            )
        });
        gestures.extend(gesture.map(|g| (AutomationTarget::Volume, g)));
        crate::marks::put_with(ui, || format!("{prefix}.fader"), fader.rect);
        let fixed = automated.volume.lane.is_none() && !automated.volume.writable;
        if fader.double_clicked() && fixed {
            mixer.gain_db = 0.0;
        }
        let (rect, _) = ui.allocate_exact_size(vec2(12.0, fader_height), egui::Sense::hover());
        draw_vertical_meter(ui.painter(), rect, meter);
        crate::marks::put_with(ui, || format!("{prefix}.meter"), rect);
        ui.vertical(|ui| {
            let db = meter.db();
            let peak = db[0].max(db[1]);
            ui.label(
                RichText::new(if peak <= -99.0 {
                    "-inf".into()
                } else {
                    format!("{peak:.1}")
                })
                .small()
                .monospace()
                .color(if peak > -0.1 {
                    theme::METER_HIGH
                } else {
                    theme::TEXT_DIM
                }),
            );
            // The fader's level, typed (the fader drags it; double-click it for 0 dB).
            let shown = automated
                .volume
                .written
                .or(automated.volume.lane)
                .unwrap_or(mixer.gain_db);
            let id = level_id(Place::Mixer, owner, Level::Volume);
            let font = egui::FontId::monospace(egui::TextStyle::Small.resolve(ui.style()).size);
            let size = vec2(40.0, 18.0);
            typed.gain_db =
                value_edit::level(ui, id, Level::Volume, shown, size, font, fixed, target);
        });
    });
    gestures
}
