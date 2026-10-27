//! The transport bar: play/stop/record, time display, tempo, load.

use crate::app::DawApp;
use crate::theme;
use daw_core::tempo::TempoMap;
use egui::{Color32, RichText};

pub fn toggle_play(app: &mut DawApp) {
    // In the Waveform view, Play plays the file on its own.
    if app.view_mode == daw_core::api::ViewMode::Waveform {
        crate::waveform::toggle_preview(app);
        return;
    }
    if app.session.is_playing() {
        app.session.stop();
    } else {
        app.session.play();
    }
}

pub fn toggle_record(app: &mut DawApp) {
    if app.session.is_recording() {
        app.session.stop();
    } else if let Err(e) = app.session.record() {
        app.toast(format!("Can't record: {e:#}"));
    }
}

/// Where the Record button was drawn (for the driven `--show count-in`, `value_edit::remember`).
pub fn record_button_id() -> egui::Id {
    egui::Id::unique("transport-record")
}

/// Where a count-in choice (0 = Off) was drawn in a menu (`value_edit::remember`).
pub fn count_in_item_id(bars: u32) -> egui::Id {
    egui::Id::unique(("count-in-choice", bars))
}

/// The transport's time display: its big line as drawn (`value_edit::remember`).
pub fn clock_id() -> egui::Id {
    egui::Id::unique("transport-clock")
}

fn record_menu_id() -> egui::Id {
    egui::Id::unique("transport-record-menu")
}

/// The Record button's menu was drawn this frame or the one before (the driven check).
pub fn record_menu_open(ctx: &egui::Context) -> bool {
    let drawn = ctx.data(|d| d.get_temp::<u64>(record_menu_id()));
    drawn.is_some_and(|pass| pass + 1 >= ctx.cumulative_pass_nr())
}

/// "1 bar" or "2 bars".
fn bars_text(bars: u32) -> String {
    if bars == 1 {
        "1 bar".to_string()
    } else {
        format!("{bars} bars")
    }
}

/// The count-in choices (22.3): Off, 1 bar, 2 bars, the current one lit. The Record button's
/// right-click menu and the metronome's menu both show them, and both set what is picked
/// through `Session::set_metronome` (the Record menu by `set_count_in`), the `set_metronome`
/// action's path. Returns the choice clicked, if any.
pub fn count_in_choices(ui: &mut egui::Ui, current: u32) -> Option<u32> {
    let mut picked = None;
    for bars in [0, 1, 2] {
        let label = if bars == 0 {
            "Off".to_string()
        } else {
            bars_text(bars)
        };
        let response = ui.add(egui::Button::selectable(current == bars, &label));
        crate::value_edit::remember(ui, count_in_item_id(bars), response.rect, &label);
        if response.clicked() {
            picked = Some(bars);
        }
    }
    picked
}

/// The time display's two lines during a count-in (22.3): the beats left before the take
/// starts, counting down as the click plays them ("Count-in · 3"), over its length ("1 bar
/// count-in").
pub fn count_in_text(count_in: &daw_core::session::CountInProgress) -> (String, String) {
    (
        format!("Count-in · {}", count_in.beats_left),
        format!("{} bar count-in", count_in.bars),
    )
}

/// `m:ss.mmm` for a sample position.
pub fn format_time(samples: u64, rate: u32) -> String {
    let ms = samples * 1000 / u64::from(rate.max(1));
    format!("{}:{:02}.{:03}", ms / 60_000, (ms / 1000) % 60, ms % 1000)
}

/// `bar.beat.tick` (1-based, 960 ticks per beat) for a sample position, along the tempo map.
pub fn format_bars(map: &TempoMap, samples: u64) -> String {
    let (bar, beats) = map.bar_at(samples as f64);
    let tick = (beats.fract() * 960.0).floor() as u64;
    format!("{}.{}.{tick:03}", bar + 1, beats.floor() as u64 + 1)
}

pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    let waveform = app.view_mode == daw_core::api::ViewMode::Waveform;
    let playing = if waveform {
        app.session.preview_state().is_some()
    } else {
        app.session.is_playing()
    };
    let recording = app.session.is_recording();
    let rate = app.session.sample_rate();
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        let home = icon_button(ui, Icon::Rewind, false, Color32::WHITE);
        crate::marks::put(ui, "transport.home", home.rect);
        if home.on_hover_text("Go to start (Home)").clicked() {
            let located = app.session.locate(0);
            app.report(located);
            app.view_start = 0.0;
        }
        let play_icon = if playing { Icon::Pause } else { Icon::Play };
        let play = icon_button(ui, play_icon, playing, theme::SOLO);
        crate::marks::put(ui, "transport.play", play.rect);
        if play.on_hover_text("Play / stop (Space)").clicked() {
            toggle_play(app);
        }
        let stop = icon_button(ui, Icon::Stop, false, Color32::WHITE);
        crate::marks::put(ui, "transport.stop", stop.rect);
        if stop.on_hover_text("Stop").clicked() {
            if waveform {
                app.session.stop_preview();
            } else {
                app.session.stop();
            }
        }
        let count_in = app.session.project.metronome.count_in_bars;
        let record = icon_button(ui, Icon::Record, recording, theme::RECORD);
        crate::value_edit::remember(ui, record_button_id(), record.rect, "Record");
        crate::marks::put(ui, "transport.record", record.rect);
        if count_in > 0 {
            count_in_badge(ui, record.rect, count_in);
        }
        let tip = if count_in > 0 {
            format!(
                "Record armed audio tracks (R)\nCount-in: {}. Right-click to change.",
                bars_text(count_in)
            )
        } else {
            "Record armed audio tracks (R)\nRight-click for a count-in.".to_string()
        };
        let record = record.on_hover_text(tip);
        if record.clicked() {
            toggle_record(app);
        }
        record.context_menu(|ui| {
            let pass = ui.ctx().cumulative_pass_nr();
            ui.ctx().data_mut(|d| d.insert_temp(record_menu_id(), pass));
            ui.label(RichText::new("Count-in").strong());
            if let Some(bars) = count_in_choices(ui, count_in) {
                let done = app.session.set_count_in(bars);
                app.report(done);
                ui.close();
            }
            ui.label(theme::hint(
                "Clicks before recording from a stop, even with the\nmetronome off. The take \
                 starts at the playhead.",
            ));
            crate::marks::put(ui, "transport.record_menu", ui.min_rect());
        });

        ui.add_space(12.0);
        // The Waveform view's clock: the file's time and sample, not the project's bars.
        if waveform {
            let (at, file_rate) = crate::waveform::clock(app).unwrap_or((0, rate));
            // In bars and beats (17.4), the song's bars from the file's start, the time under.
            let (big, small) = if app.editor.time == daw_core::api::TimeDisplay::BarsBeats {
                let map = TempoMap::of(&app.session.project, file_rate);
                (format_bars(&map, at), format_time(at, file_rate))
            } else {
                (format_time(at, file_rate), format!("sample {at}"))
            };
            let clock = egui::Frame::new()
                .fill(Color32::from_rgb(0x12, 0x14, 0x18))
                .corner_radius(4)
                .inner_margin(egui::Margin::symmetric(10, 2))
                .show(ui, |ui| {
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        ui.label(
                            RichText::new(big)
                                .monospace()
                                .size(18.0)
                                .color(theme::PLAYHEAD),
                        );
                        ui.label(
                            RichText::new(small)
                                .monospace()
                                .small()
                                .color(theme::TEXT_DIM),
                        );
                    });
                });
            crate::marks::put(ui, "transport.time", clock.response.rect);
            return;
        }
        let position = app.session.position();
        let map = app.session.tempo_map();
        // During a count-in the beats left before the take, then the position from its start.
        let (big, small, color) = match app.session.count_in_progress() {
            Some(count_in) => {
                let (big, small) = count_in_text(&count_in);
                (big, small, theme::RECORD)
            }
            None => (
                format_bars(&map, position),
                format_time(position, rate),
                theme::PLAYHEAD,
            ),
        };
        let clock = egui::Frame::new()
            .fill(Color32::from_rgb(0x12, 0x14, 0x18))
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(10, 2))
            .show(ui, |ui| {
                // As wide as the count-in's text, so the bar doesn't shift when it shows.
                let wide = ui
                    .painter()
                    .layout_no_wrap(
                        "Count-in · 8".to_string(),
                        egui::FontId::monospace(18.0),
                        color,
                    )
                    .size()
                    .x;
                ui.set_min_width(wide);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let shown = ui.label(RichText::new(&big).monospace().size(18.0).color(color));
                    crate::value_edit::remember(ui, clock_id(), shown.rect, &big);
                    ui.label(
                        RichText::new(small)
                            .monospace()
                            .small()
                            .color(theme::TEXT_DIM),
                    );
                });
            });
        crate::marks::put(ui, "transport.time", clock.response.rect);

        ui.add_space(12.0);
        // Tempo and meter in force at the playhead; with changes in the project, editing
        // them edits that stretch only.
        let changes = !app.session.project.tempo_changes.is_empty();
        let hint = if changes {
            "At the playhead. Right-click the ruler to add or edit tempo and meter changes."
        } else {
            "The project's tempo (quarter notes per minute) and time signature. Right-click the \
             ruler to change them from a bar on."
        };
        let tempo_label = ui
            .label(RichText::new("Tempo").color(theme::TEXT_DIM))
            .on_hover_text(hint);
        let here = map.meter_at(position as i64);
        let mut tempo = here.tempo;
        let tempo_field = ui.add(
            egui::DragValue::new(&mut tempo)
                .range(20.0..=300.0)
                .speed(0.2)
                .fixed_decimals(1)
                .suffix(" BPM"),
        );
        crate::marks::put(
            ui,
            "transport.tempo",
            tempo_label.rect.union(tempo_field.rect),
        );
        if tempo_field.on_hover_text(hint).changed() {
            let done = app.session.set_tempo_at(position, tempo);
            app.report(done);
        }
        let (mut n, mut d) = (here.numerator, here.denominator);
        let numerator = ui.add(egui::DragValue::new(&mut n).range(1..=16));
        ui.label("/");
        let denominator = egui::ComboBox::from_id_salt("den")
            .width(40.0)
            .selected_text(d.to_string())
            .show_ui(ui, |ui| {
                for v in [2u32, 4, 8, 16] {
                    ui.selectable_value(&mut d, v, v.to_string());
                }
            });
        crate::marks::put(
            ui,
            "transport.meter",
            numerator.rect.union(denominator.response.rect),
        );
        if (n, d) != (here.numerator, here.denominator) {
            let done = app.session.set_time_signature_at(position, (n, d));
            app.report(done);
        }
        if changes {
            ui.label(
                RichText::new("(at playhead)")
                    .small()
                    .color(theme::TEXT_DIM),
            );
        }

        ui.add_space(12.0);
        let (snapped, snap) = theme::toggle_at(
            ui,
            &mut app.settings.snap,
            "Snap",
            theme::ACCENT,
            "Snap the playhead, clips, selections and markers to the grid (right-click the ruler for its division)",
        );
        crate::marks::put(ui, "transport.snap", snap);
        if snapped {
            app.save_settings();
        }
        let mut click = app.session.project.metronome;
        let before = click;
        let (_, click_rect) = theme::toggle_at(
            ui,
            &mut click.enabled,
            "Click",
            theme::PLAYHEAD,
            "Metronome (C). Plays after the master and is never exported.",
        );
        crate::marks::put(ui, "transport.click", click_rect);
        let click_menu = ui.menu_button("▾", |ui| {
            ui.label(RichText::new("Metronome").strong());
            ui.add(
                egui::Slider::new(&mut click.volume_db, -60.0..=6.0)
                    .suffix(" dB")
                    .text("Volume"),
            );
            ui.add(crate::theme::checkbox(&mut click.only_while_recording, "Only while recording"));
            ui.horizontal(|ui| {
                ui.label("Count-in");
                // The Record button's right-click menu offers the same (22.3).
                if let Some(bars) = count_in_choices(ui, click.count_in_bars) {
                    click.count_in_bars = bars;
                }
            });
        });
        crate::marks::put(ui, "transport.click_menu", click_menu.response.rect);
        click_menu.response.on_hover_text("Metronome options");
        if click != before {
            let _ = app.session.set_metronome(click);
        }
        let mut looping = app.session.project.loop_region.enabled;
        let (looped, loop_rect) = theme::toggle_at(
            ui,
            &mut looping,
            "Loop",
            theme::LOOP,
            "Loop playback (L). Drag on the ruler to set the loop. Recording with the loop on \
             makes a take per pass.",
        );
        crate::marks::put(ui, "transport.loop", loop_rect);
        if looped {
            let done = app.session.set_looping(looping);
            app.report(done);
        }
        let mut punching = app.session.project.punch.enabled;
        let (punched, punch_rect) = theme::toggle_at(
            ui,
            &mut punching,
            "Punch",
            theme::RECORD,
            "Record only inside the punch range (P). Shift+drag on the ruler to set it.",
        );
        crate::marks::put(ui, "transport.punch", punch_rect);
        if punched {
            let done = app.session.set_punching(punching);
            app.report(done);
        }
        let (_, follow) = theme::toggle_at(
            ui,
            &mut app.follow_playhead,
            "Follow",
            theme::ACCENT,
            "Scroll the arrangement with the playhead",
        );
        crate::marks::put(ui, "transport.follow", follow);

        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let load = app.session.stats().dsp_load;
            let color = if load > 0.9 {
                theme::METER_HIGH
            } else if load > 0.6 {
                theme::METER_MID
            } else {
                theme::METER_LOW
            };
            let peak = app.dsp_peak.max(load);
            let (rect, bar) = ui.allocate_exact_size(egui::vec2(80.0, 10.0), egui::Sense::click());
            ui.painter().rect_filled(rect, 2, Color32::from_gray(0x30));
            let mut fill = rect;
            fill.set_width(rect.width() * load.clamp(0.0, 1.0));
            ui.painter().rect_filled(fill, 2, color);
            let x = rect.left() + rect.width() * peak.clamp(0.0, 1.0);
            ui.painter()
                .vline(x, rect.y_range(), egui::Stroke::new(1.5, Color32::WHITE));
            let peak_color = if peak > 1.0 {
                theme::METER_HIGH
            } else {
                theme::TEXT_DIM
            };
            let readout = ui
                .add(
                    egui::Label::new(
                        RichText::new(format!(
                            "DSP {:>3.0}% · peak {:>3.0}%",
                            load * 100.0,
                            peak * 100.0
                        ))
                        .monospace()
                        .color(peak_color),
                    )
                    .sense(egui::Sense::click()),
                )
                .union(bar)
                .on_hover_text(crate::cpu::DSP_TIP);
            crate::marks::put(ui, "transport.dsp", readout.rect);
            if readout.clicked() {
                app.show_cpu = !app.show_cpu;
            }
            ui.add_space(8.0);
            let mixer_label = if app.show_mixer {
                "Hide mixer"
            } else {
                "Mixer"
            };
            let mixer = ui.button(mixer_label);
            crate::marks::put(ui, "transport.mixer", mixer.rect);
            if mixer.on_hover_text("Toggle mixer (M)").clicked() {
                app.show_mixer = !app.show_mixer;
            }
            let inspector = ui.button(if app.show_inspector {
                "Hide inspector"
            } else {
                "Inspector"
            });
            crate::marks::put(ui, "transport.inspector", inspector.rect);
            if inspector.clicked() {
                app.show_inspector = !app.show_inspector;
            }
            ui.add_space(8.0);
            crate::loudness::transport_readout(app, ui);
        });
    });
}

/// The count-in's mark on the Record button: its bars in a small tag at the lower right, in
/// the click's colour.
fn count_in_badge(ui: &egui::Ui, button: egui::Rect, bars: u32) {
    let tag = egui::Rect::from_min_size(
        button.right_bottom() - egui::vec2(12.0, 12.0),
        egui::vec2(10.0, 10.0),
    );
    let painter = ui.painter();
    painter.rect_filled(tag, 2, theme::PLAYHEAD);
    painter.text(
        tag.center(),
        egui::Align2::CENTER_CENTER,
        bars.to_string(),
        egui::FontId::proportional(9.0),
        Color32::BLACK,
    );
}

#[derive(Clone, Copy)]
enum Icon {
    Rewind,
    Play,
    Pause,
    Stop,
    Record,
}

/// A transport button with a painted icon (the emoji font draws these glyphs as pictures).
fn icon_button(ui: &mut egui::Ui, icon: Icon, active: bool, color: Color32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(36.0, 28.0), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    let fill = if active { color } else { visuals.bg_fill };
    ui.painter().rect_filled(rect, 4, fill);
    let ink = match (active, icon) {
        (true, _) => Color32::BLACK,
        (false, Icon::Record) => theme::RECORD,
        (false, _) => color,
    };
    let c = rect.center();
    let p = ui.painter();
    use egui::pos2;
    match icon {
        Icon::Play => {
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(c.x - 5.0, c.y - 7.0),
                    pos2(c.x + 7.0, c.y),
                    pos2(c.x - 5.0, c.y + 7.0),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
        Icon::Pause => {
            for dx in [-4.0, 4.0] {
                p.rect_filled(
                    egui::Rect::from_center_size(pos2(c.x + dx, c.y), egui::vec2(4.0, 13.0)),
                    1,
                    ink,
                );
            }
        }
        Icon::Stop => {
            p.rect_filled(
                egui::Rect::from_center_size(c, egui::vec2(12.0, 12.0)),
                2,
                ink,
            );
        }
        Icon::Record => {
            p.circle_filled(c, 7.0, ink);
        }
        Icon::Rewind => {
            p.rect_filled(
                egui::Rect::from_center_size(pos2(c.x - 6.0, c.y), egui::vec2(2.5, 12.0)),
                0,
                ink,
            );
            p.add(egui::Shape::convex_polygon(
                vec![
                    pos2(c.x + 6.0, c.y - 6.0),
                    pos2(c.x + 6.0, c.y + 6.0),
                    pos2(c.x - 4.0, c.y),
                ],
                ink,
                egui::Stroke::NONE,
            ));
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_positions() {
        assert_eq!(format_time(48_000 * 75 + 24_000, 48_000), "1:15.500");
        // 120 BPM: a beat is half a second.
        let map = TempoMap::constant(48_000, 120.0, 4, 4);
        assert_eq!(format_bars(&map, 0), "1.1.000");
        assert_eq!(format_bars(&map, 48_000 * 2 + 12_000), "2.1.480");
    }

    #[test]
    fn a_count_in_shows_its_beats_left_and_length() {
        let count_in = daw_core::session::CountInProgress {
            bars: 2,
            beats_left: 7,
            record_point: 96_000,
        };
        assert_eq!(
            count_in_text(&count_in),
            ("Count-in · 7".to_string(), "2 bar count-in".to_string())
        );
    }

    /// The choices as a menu draws them: clicking "1 bar" picks 1, nothing is picked without
    /// a click. (Picked, it goes through `Session::set_count_in`, which
    /// `the_record_menus_count_in_is_set_metronomes` holds equal to `set_metronome`.)
    #[test]
    fn a_count_in_choice_clicked_is_picked() {
        crate::value_edit::remember_places();
        let ctx = egui::Context::default();
        let run = |events: Vec<egui::Event>| {
            let mut picked = None;
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                events,
                ..Default::default()
            };
            let mut output = ctx.run_ui(input, |ui| {
                picked = count_in_choices(ui, 0);
            });
            output.textures_delta.clear();
            picked
        };
        assert_eq!(run(Vec::new()), None);
        let one = crate::value_edit::drawn(&ctx, count_in_item_id(1)).expect("drawn");
        assert_eq!(one.text, "1 bar");
        let at = one.rect.center();
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        assert_eq!(run(vec![egui::Event::PointerMoved(at), press(true)]), None);
        assert_eq!(run(vec![press(false)]), Some(1));
    }
}
