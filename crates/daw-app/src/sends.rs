//! Send controls shared by the inspector, the mixer strips and the track menu. Level and mute
//! reach the engine without a rebuild; anything that changes the routing (adding, removing,
//! retargeting, pre/post) marks the graph dirty.

use crate::app::DawApp;
use crate::theme;
use daw_core::project::{SendId, TrackId, TrackKind, TrackSend, SEND_GAIN_RANGE};
use egui::{vec2, Align2, FontId, RichText, Sense, Stroke};

/// The "add a send" menu: every folder or bus `from` can feed without a loop (and isn't
/// already sending to), then a new bus.
pub fn add_send_menu(app: &mut DawApp, ui: &mut egui::Ui, from: TrackId) {
    let project = &app.session.project;
    let taken: Vec<TrackId> = project
        .track(from)
        .map(|t| t.sends.iter().map(|s| s.target).collect())
        .unwrap_or_default();
    let targets: Vec<(TrackId, String)> = project
        .valid_buses(from)
        .iter()
        .filter(|t| !taken.contains(&t.id))
        .map(|t| (t.id, label(t.kind, &t.name)))
        .collect();
    for (target, name) in &targets {
        if ui.button(name).clicked() {
            match app.session.project.add_send(from, *target, false) {
                Ok(_) => app.session.mark_dirty(),
                Err(e) => app.toast(e),
            }
            ui.close();
        }
    }
    if targets.is_empty() {
        ui.label(RichText::new("No folders or buses to send to").color(theme::TEXT_DIM));
    }
    ui.separator();
    if ui
        .button("New bus")
        .on_hover_text("Add a bus (return track) and send to it")
        .clicked()
    {
        if let Err(e) = app.session.add_send_to_new_bus(from) {
            app.toast(format!("{e:#}"));
        }
        ui.close();
    }
}

/// A send target as menus show it.
fn label(kind: TrackKind, name: &str) -> String {
    match kind {
        TrackKind::Folder => format!("📁 {name}"),
        _ => format!("↩ {name}"),
    }
}

fn target_name(app: &DawApp, target: TrackId) -> String {
    app.session
        .project
        .track(target)
        .map_or_else(|| "?".into(), |t| t.name.clone())
}

/// The inspector's send rows: target, level, pre/post, mute and remove for each send, then an
/// add button.
pub fn inspector_rows(app: &mut DawApp, ui: &mut egui::Ui, from: TrackId) {
    let sends: Vec<TrackSend> = app
        .session
        .project
        .track(from)
        .map(|t| t.sends.clone())
        .unwrap_or_default();
    if sends.is_empty() {
        ui.label(RichText::new("No sends").color(theme::TEXT_DIM));
    }
    let buses: Vec<(TrackId, String)> = app
        .session
        .project
        .valid_buses(from)
        .iter()
        .map(|t| (t.id, label(t.kind, &t.name)))
        .collect();
    for send in &sends {
        ui.push_id(("send", send.id), |ui| {
            ui.horizontal(|ui| {
                let mut target = send.target;
                egui::ComboBox::from_id_salt("target")
                    .width(130.0)
                    .selected_text(target_name(app, target))
                    .show_ui(ui, |ui| {
                        for (bus, name) in &buses {
                            ui.selectable_value(&mut target, *bus, name);
                        }
                    });
                if target != send.target {
                    match app.session.project.set_send_target(send.id, target) {
                        Ok(()) => app.session.mark_dirty(),
                        Err(e) => app.toast(e),
                    }
                }
                let mut pre = send.pre_fader;
                if theme::toggle(
                    ui,
                    &mut pre,
                    "Pre",
                    theme::ACCENT,
                    "Pre-fader: the track's fader and pan don't change this send",
                ) {
                    update(app, send.id, |s| s.pre_fader = pre);
                    app.session.mark_dirty();
                }
                let mut mute = send.mute;
                if theme::toggle(ui, &mut mute, "M", theme::MUTE, "Mute this send") {
                    update(app, send.id, |s| s.mute = mute);
                }
                if ui
                    .small_button("✕")
                    .on_hover_text("Remove the send")
                    .clicked()
                {
                    remove(app, send.id);
                }
            });
            let (low, high) = SEND_GAIN_RANGE;
            let mut gain = send.gain_db;
            let slider = ui.add(
                egui::Slider::new(&mut gain, low..=high)
                    .suffix(" dB")
                    .fixed_decimals(1),
            );
            if slider.double_clicked() {
                gain = 0.0;
            }
            if gain != send.gain_db {
                update(app, send.id, |s| s.gain_db = gain);
            }
        });
        ui.add_space(4.0);
    }
    let add = ui.button("+ Add send");
    egui::Popup::menu(&add).show(|ui| add_send_menu(app, ui, from));
}

/// A send on a mixer strip: a bar whose fill is the level, with the target's name on it.
/// Drag for the level, double-click for 0 dB, right-click for pre/post, mute and remove.
pub fn strip_bar(app: &mut DawApp, ui: &mut egui::Ui, send: &TrackSend, size: egui::Vec2) {
    let name = target_name(app, send.target);
    let (rect, response) = ui.allocate_exact_size(size, Sense::click_and_drag());
    let (low, high) = SEND_GAIN_RANGE;
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2, theme::PANEL);
    let fraction = (send.gain_db - low) / (high - low);
    let fill = egui::Rect::from_min_size(rect.min, vec2(rect.width() * fraction, rect.height()));
    let colour = if send.mute {
        theme::TEXT_DIM.gamma_multiply(0.4)
    } else {
        theme::ACCENT.gamma_multiply(0.45)
    };
    painter.rect_filled(fill, 2, colour);
    // 0 dB tick.
    let zero = rect.left() + rect.width() * (-low / (high - low));
    painter.vline(zero, rect.y_range(), Stroke::new(1.0, theme::BG));
    painter.text(
        rect.left_center() + vec2(3.0, 0.0),
        Align2::LEFT_CENTER,
        &name,
        FontId::proportional(10.0),
        if send.mute {
            theme::TEXT_DIM
        } else {
            egui::Color32::WHITE
        },
    );
    if send.pre_fader {
        painter.text(
            rect.right_center() - vec2(3.0, 0.0),
            Align2::RIGHT_CENTER,
            "PRE",
            FontId::monospace(8.0),
            theme::MUTE,
        );
    }
    let tap = if send.pre_fader { "pre" } else { "post" };
    let response = response.on_hover_text(format!(
        "Send to {name}: {:.1} dB, {tap}-fader{}\nDrag: level · Double-click: 0 dB · \
         Right-click: options",
        send.gain_db,
        if send.mute { ", muted" } else { "" }
    ));
    if response.dragged() {
        let dx = response.drag_delta().x;
        let gain = (send.gain_db + dx * (high - low) / rect.width()).clamp(low, high);
        update(app, send.id, |s| s.gain_db = gain);
    }
    if response.double_clicked() {
        update(app, send.id, |s| s.gain_db = 0.0);
    }
    response.context_menu(|ui| {
        let mut pre = send.pre_fader;
        if ui
            .add(crate::theme::checkbox(&mut pre, "Pre-fader"))
            .changed()
        {
            update(app, send.id, |s| s.pre_fader = pre);
            app.session.mark_dirty();
        }
        let mut mute = send.mute;
        if ui.add(crate::theme::checkbox(&mut mute, "Mute")).changed() {
            update(app, send.id, |s| s.mute = mute);
        }
        if ui.button("Remove").clicked() {
            remove(app, send.id);
            ui.close();
        }
    });
}

fn update(app: &mut DawApp, id: SendId, change: impl FnOnce(&mut TrackSend)) {
    if let Some(send) = app.session.project.find_send_mut(id) {
        change(send);
    }
}

fn remove(app: &mut DawApp, id: SendId) {
    app.session.project.remove_send(id);
    app.session.mark_dirty();
}
