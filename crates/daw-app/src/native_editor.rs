//! The editors of the built-in effects (dev plan 11.0): a window per open one, drawn by the
//! app, with a slider per parameter in its own units. VST3 plug-ins open their own windows
//! instead; both open and close through `Session::toggle_editor`.

use crate::app::DawApp;
use crate::theme;
use daw_core::native::{
    analog_delay, chorus, click_pop, click_remover, declipper, dehummer, delay, echo, flanger,
    full_reverb, phaser, studio_reverb, ParamSpec,
};
use daw_core::project::InsertId;
use egui::RichText;

/// Draw every open native editor; closing a window closes its editor.
pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    for id in app.session.open_native_editors() {
        let mut open = true;
        let title = editor_title(app, id);
        let shown = egui::Window::new(title)
            .id(egui::Id::unique(("native-editor", id)))
            .open(&mut open)
            .resizable(false)
            .collapsible(true)
            .default_width(340.0)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .show(ctx, |ui| {
                ui.spacing_mut().slider_width = 220.0;
                // A solid scroll bar: the floating kind is invisible until hovered.
                ui.spacing_mut().scroll = egui::style::ScrollStyle::solid();
                body(app, ui, id)
            });
        if let Some(shown) = shown {
            // F1 here: the effect's own section of the manual (27.3, 27.5).
            let (layer, rect) = (shown.response.layer_id, shown.response.rect);
            let kind = app.session.native(id).map_or("", |n| n.kind.id);
            app.help
                .note(ctx, crate::help::Place::Effect(kind), layer, rect);
            crate::marks::put_rect("effect.editor", rect);
        }
        if !open {
            let closed = app.session.toggle_editor(id);
            app.report(closed);
        }
    }
}

/// "Amplify · Guitar L", naming where the insert is.
fn editor_title(app: &DawApp, id: InsertId) -> String {
    let name = app
        .session
        .native(id)
        .map_or("Effect", |n| n.kind.name)
        .to_string();
    let owner = app
        .session
        .project
        .tracks
        .iter()
        .find(|t| t.inserts.iter().any(|s| s.id == id))
        .map(|t| t.name.clone())
        .or_else(|| {
            app.session
                .project
                .master
                .inserts
                .iter()
                .any(|s| s.id == id)
                .then(|| "Master".to_string())
        })
        .unwrap_or_else(|| "Waveform rack".into());
    format!("{name} · {owner}")
}

fn body(app: &mut DawApp, ui: &mut egui::Ui, id: InsertId) {
    // Clip Repair and the Click & Pop Repair scan the Waveform view's selection from here,
    // if there is one.
    let is = |kind: &daw_core::native::Kind| {
        app.session
            .native(id)
            .is_some_and(|n| std::ptr::eq(n.kind, kind))
    };
    let (is_declipper, is_click_pop) = (is(&declipper::KIND), is(&click_pop::KIND));
    let selection = if is_declipper || is_click_pop {
        crate::waveform::current_selection(app)
    } else {
        None
    };
    // A scan asked for: its name, or `None` for the effect's only one.
    let mut scan: Option<Option<&'static str>> = None;
    let Some(native) = app.session.native(id) else {
        ui.label("Not loaded");
        return;
    };
    let specs = native.kind.params;
    let values: Vec<f64> = native.parameters().iter().map(|p| p.value).collect();
    if let Some(response) = native.kind.response {
        let plain = native.plain_values();
        let rate = f64::from(app.session.sample_rate());
        if std::ptr::eq(native.kind, &dehummer::KIND) {
            // Deep, narrow notches: a fixed scale down to -90 dB, sampled finely and at each
            // harmonic's centre, which is marked. The hum alone (its peaks lift one another
            // well above 0 dB) gets room above.
            let marks: Vec<f64> = dehummer::harmonics(rate, &plain)
                .into_iter()
                .map(|(hz, _)| hz)
                .collect();
            let scale = if dehummer::hum_only(&plain) {
                (-80.0, 40.0)
            } else {
                (-90.0, 10.0)
            };
            curve(ui, |hz| response(rate, &plain, hz), Some(scale), &marks);
        } else if std::ptr::eq(native.kind, &echo::KIND) {
            // The echo's EQ for one pass, its eight bands marked where the dialog labels them.
            let marks: Vec<f64> = (0..echo::BANDS)
                .map(|b| echo::band_label_hz(rate, b))
                .collect();
            curve(ui, |hz| response(rate, &plain, hz), None, &marks);
        } else {
            curve(ui, |hz| response(rate, &plain, hz), None, &[]);
        }
        ui.add_space(6.0);
    }
    if is_declipper {
        if declipper_panel(ui, native, app.session.sample_rate(), selection.is_some()) {
            scan = Some(None);
        }
        ui.add_space(6.0);
    } else if is_click_pop {
        scan = click_pop_panel(ui, native, selection.is_some()).map(Some);
        ui.add_space(6.0);
    } else if native.kind.capture.is_some() {
        match native
            .print()
            .and_then(daw_core::native::spectral::Print::read)
        {
            Some(print) => {
                print_curve(ui, |hz| print.level_db(hz));
                ui.label(
                    RichText::new(format!(
                        "Noise print: {} frames of {} samples, {} channel{}",
                        print.frames,
                        print.size,
                        print.channels,
                        if print.channels == 1 { "" } else { "s" }
                    ))
                    .small()
                    .color(theme::TEXT_DIM),
                );
            }
            None => {
                ui.label(
                    RichText::new(
                        "No noise print: select noise alone in the Waveform view, then \
                         Effects > Capture noise print.",
                    )
                    .color(theme::TEXT_DIM),
                );
            }
        }
        ui.add_space(6.0);
    }
    if let Some(transfer) = native.kind.transfer {
        let plain = native.plain_values();
        let rate = f64::from(app.session.sample_rate());
        transfer_curve(ui, |db| transfer(rate, &plain, db));
        ui.add_space(6.0);
    }
    if std::ptr::eq(native.kind, &click_remover::KIND) {
        // What the two percentages set, in the engine's own terms.
        let e = click_remover::engine(&native.plain_values());
        ui.label(
            RichText::new(format!(
                "Engine threshold {:.0}: flags {:.1} times the noise's level.\nSize {}: \
                 clicks up to {} samples, an order-{} model.\n{} samples of delay.",
                e.threshold,
                e.multiple,
                e.size,
                e.longest,
                e.order,
                click_remover::LATENCY
            ))
            .color(theme::TEXT_DIM),
        );
        ui.add_space(6.0);
    }
    if std::ptr::eq(native.kind, &studio_reverb::KIND) {
        let rate = f64::from(app.session.sample_rate());
        studio_reverb_panel(ui, rate, &native.plain_values());
        ui.add_space(6.0);
    }
    let is_analog_delay = std::ptr::eq(native.kind, &analog_delay::KIND);
    let mut mode_picked = None;
    if is_analog_delay {
        let rate = f64::from(app.session.sample_rate());
        let meter = app
            .session
            .tempo_map()
            .meter_at(app.session.position() as i64);
        mode_picked = analog_delay_panel(ui, rate, &native.plain_values(), &meter);
        ui.add_space(6.0);
    }
    let is_echo = std::ptr::eq(native.kind, &echo::KIND);
    if is_echo {
        let rate = f64::from(app.session.sample_rate());
        echo_panel(ui, rate, &native.plain_values());
        ui.add_space(6.0);
    }
    let is_chorus = std::ptr::eq(native.kind, &chorus::KIND);
    if is_chorus {
        let rate = f64::from(app.session.sample_rate());
        chorus_panel(ui, rate, &native.plain_values());
        ui.add_space(6.0);
    }
    let is_flanger = std::ptr::eq(native.kind, &flanger::KIND);
    if is_flanger {
        let rate = f64::from(app.session.sample_rate());
        flanger_panel(ui, rate, &native.plain_values());
        ui.add_space(6.0);
    }
    let is_phaser = std::ptr::eq(native.kind, &phaser::KIND);
    if is_phaser {
        let rate = f64::from(app.session.sample_rate());
        phaser_panel(ui, rate, &native.plain_values());
        ui.add_space(6.0);
    }
    let is_delay = std::ptr::eq(native.kind, &delay::KIND);
    if is_delay {
        let rate = f64::from(app.session.sample_rate());
        // Beats through the song's tempo from the playhead, as the timeline counts them.
        let map = app.session.tempo_map();
        let at = app.session.position() as f64;
        let beats = |samples: f64| map.quarter_notes_at(at + samples) - map.quarter_notes_at(at);
        delay_panel(ui, rate, &native.plain_values(), &beats);
        ui.add_space(6.0);
    }
    // An effect that plays a file (the Convolution Reverb's impulse): what it plays, and a
    // button to choose another.
    let mut load_file = false;
    let is_full_reverb = std::ptr::eq(native.kind, &full_reverb::KIND);
    if let Some(reads) = native.kind.reads {
        let preparing = app.session.impulses_preparing().contains(&id);
        if reads.makes() {
            // An impulse made from the settings (the Space Reverb): what it made.
            full_reverb_panel(ui, native, preparing);
        } else {
            load_file = file_panel(ui, native, reads, preparing);
        }
        ui.add_space(6.0);
    }
    let mut changes: Vec<(usize, f64)> = Vec::new();
    // Many parameters (an EQ's bands) scroll, under the curve.
    let params = egui::ScrollArea::vertical()
        .max_height(420.0)
        .auto_shrink([false, true])
        // Always shown: an EQ's bands are below the fold, and a hidden bar hides them.
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
        .show(ui, |ui| {
            let groups = if is_click_pop {
                // The settings in parts: the detection curve's points, the rejection's,
                // then the advanced settings.
                Some(CLICK_POP_GROUPS)
            } else if is_full_reverb {
                // Its two tabs' groups; the outputs (the room's dimensions) show above.
                Some(FULL_REVERB_GROUPS)
            } else if is_delay {
                Some(DELAY_GROUPS)
            } else if is_echo {
                Some(ECHO_GROUPS)
            } else if is_analog_delay {
                Some(ANALOG_DELAY_GROUPS)
            } else if is_chorus {
                Some(CHORUS_GROUPS)
            } else if is_flanger {
                Some(FLANGER_GROUPS)
            } else if is_phaser {
                Some(PHASER_GROUPS)
            } else {
                None
            };
            if let Some(groups) = groups {
                for (title, open, indices) in groups {
                    egui::CollapsingHeader::new(*title)
                        .id_salt(("groups", id, *title))
                        .default_open(*open)
                        .show(ui, |ui| {
                            param_rows(
                                ui,
                                ("native-params", id, *title),
                                specs,
                                &values,
                                indices,
                                &mut changes,
                            )
                        });
                }
            } else {
                let all: Vec<usize> = (0..specs.len()).collect();
                param_rows(
                    ui,
                    ("native-params", id),
                    specs,
                    &values,
                    &all,
                    &mut changes,
                );
            }
        });
    crate::marks::put(ui, "effect.params", params.inner_rect);
    if let Some(mode) = mode_picked {
        let spec = &specs[analog_delay::MODE];
        changes.push((analog_delay::MODE, spec.normalized(mode as f64)));
    }
    // A linked parameter moves the others on the same link (and group) while its switch is on.
    let linked: Vec<(usize, f64)> = changes
        .iter()
        .flat_map(|&(i, value)| {
            let link = specs[i].link.and_then(|link| {
                let switch = link.split(':').next().unwrap_or(link);
                let on = specs
                    .iter()
                    .position(|s| s.name == switch)
                    .is_some_and(|k| specs[k].plain(values[k]) >= 0.5);
                on.then_some(link)
            });
            specs
                .iter()
                .enumerate()
                .filter(move |(k, s)| *k != i && link.is_some() && s.link == link)
                .map(move |(k, _)| (k, value))
        })
        .collect();
    // A switch that locks groups (`Switch:group`, the Echo's Lock), turned on: each group's
    // first parameter is copied onto the others.
    let mut locked: Vec<(usize, f64)> = Vec::new();
    for &(i, value) in &changes {
        let switch = &specs[i];
        if switch.steps != 1 || switch.plain(value) < 0.5 || switch.plain(values[i]) >= 0.5 {
            continue;
        }
        for (k, p) in specs.iter().enumerate() {
            let Some(link) = p.link else { continue };
            if link
                .split_once(':')
                .is_none_or(|(name, _)| name != switch.name)
            {
                continue;
            }
            if let Some(first) = specs.iter().position(|q| q.link == Some(link)) {
                if first != k {
                    locked.push((k, values[first]));
                }
            }
        }
    }
    for (i, value) in changes.into_iter().chain(linked).chain(locked) {
        if let Err(e) = app.session.set_parameter(id, i as u32, value) {
            app.toast(format!("{e:#}"));
        }
    }
    if let (Some(which), Some((doc, range))) = (scan, selection) {
        crate::waveform::learn_into(app, doc, range, Some(id), which);
    }
    if load_file {
        let dialog = app
            .dialog_in(Some(daw_core::folder::IMPORTED))
            .set_title("Load an impulse response")
            .add_filter(
                "Impulse (WAV, AIFF, FLAC)",
                &["wav", "wave", "aif", "aiff", "aifc", "flac"],
            );
        crate::file_dialog::pick_file(app, dialog, move |app, path| {
            if let Err(e) = app.session.set_impulse_file(id, &path) {
                app.toast(format!("Impulse: {e:#}"));
            }
        });
    }
    ui.add_space(4.0);
    ui.label(theme::hint("Double-click a slider for its default."));
}

/// A file-playing effect's file (the Convolution Reverb's impulse): its name, a sketch of
/// what it plays (left above right, each scaled to its loudest sample), its length and the
/// file's format, or why there is none. Returns whether "Load impulse..." was clicked.
fn file_panel(
    ui: &mut egui::Ui,
    native: &daw_core::native::Native,
    reads: &daw_core::native::Reads,
    preparing: bool,
) -> bool {
    use egui::{pos2, vec2, Stroke};
    let mut clicked = false;
    ui.horizontal(|ui| {
        let name = native
            .file()
            .and_then(|f| f.file_name())
            .map(|n| n.to_string_lossy().into_owned());
        ui.label(RichText::new(format!("{}:", capitalised(reads.what))).strong());
        match &name {
            Some(name) => ui.label(name),
            None => ui.label(RichText::new("none").color(theme::TEXT_DIM)),
        };
        if preparing {
            ui.spinner();
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            clicked = ui
                .button(format!("Load {}…", reads.what))
                .on_hover_text(
                    "A WAV, AIFF or FLAC file, copied into the project's Imported folder",
                )
                .clicked();
        });
    });
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 96.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    match native.file_overview() {
        Some(overview) if !overview.peaks.is_empty() => {
            let rows = overview.peaks.len() as f32;
            let row_height = rect.height() / rows;
            for (c, peaks) in overview.peaks.iter().enumerate() {
                let top = rect.top() + c as f32 * row_height;
                // On a pixel boundary, so the quiet tail shows as a crisp two-pixel line.
                let mid = (top + row_height / 2.0).round();
                if c > 0 {
                    painter.hline(rect.x_range(), top, Stroke::new(1.0, theme::GRID_BAR));
                }
                let loudest = peaks
                    .iter()
                    .fold(0.0f32, |m, (lo, hi)| m.max(lo.abs()).max(hi.abs()))
                    .max(1e-12);
                let half = row_height / 2.0 - 3.0;
                let step = rect.width() / peaks.len() as f32;
                // Each column a box at least two pixels tall: egui draws a box narrower than
                // two pixels as a line, rounded to pixel centres, which loses one a pixel
                // tall, so the quiet tail would vanish.
                for (k, (lo, hi)) in peaks.iter().enumerate() {
                    let x = rect.left() + k as f32 * step;
                    let y0 = (mid - hi / loudest * half).min(mid - 1.0);
                    let y1 = (mid - lo / loudest * half).max(mid + 1.0);
                    painter.rect_filled(
                        egui::Rect::from_x_y_ranges(x..=x + step.max(1.0), y0..=y1),
                        0,
                        theme::ACCENT,
                    );
                }
                painter.text(
                    pos2(rect.left() + 4.0, top + 2.0),
                    egui::Align2::LEFT_TOP,
                    if c == 0 { "L" } else { "R" },
                    egui::FontId::proportional(10.0),
                    theme::TEXT_DIM,
                );
            }
            ui.label(
                RichText::new(format!(
                    "{:.2} s as played ({} samples at {:.1} kHz). The file: {} channel{}, {:.1} kHz, {:.2} s{}.",
                    overview.samples as f64 / overview.rate,
                    overview.samples,
                    overview.rate / 1000.0,
                    overview.file_channels,
                    if overview.file_channels == 1 { "" } else { "s" },
                    f64::from(overview.file_rate) / 1000.0,
                    overview.file_frames as f64 / f64::from(overview.file_rate),
                    match overview.file_channels {
                        2 => ", left on left and right on right",
                        _ => ", averaged to one impulse on both sides",
                    }
                ))
                .small()
                .color(theme::TEXT_DIM),
            );
        }
        _ => {
            let note = match (native.file_error(), native.file().is_some(), preparing) {
                (Some(e), _, _) => format!("Couldn't use it: {e}"),
                (None, true, _) | (None, false, true) => format!("Preparing the {}…", reads.what),
                (None, false, false) => format!(
                    "No {} loaded: the input passes through unchanged. Load a WAV, AIFF or \
                     FLAC impulse response.",
                    reads.what
                ),
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                note,
                egui::FontId::proportional(12.0),
                theme::TEXT_DIM,
            );
        }
    }
    if let (Some(e), Some(_)) = (native.file_error(), native.file_overview()) {
        ui.label(RichText::new(format!("Couldn't use the new one: {e}")).color(theme::REFUSED));
    }
    clicked
}

fn capitalised(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// The parameters `indices` of `specs` as rows of a grid (a checkbox for a switch, a slider
/// for the rest), the changes made pushed to `changes` (index, normalized value).
fn param_rows(
    ui: &mut egui::Ui,
    grid: impl std::hash::Hash + std::fmt::Debug,
    specs: &[ParamSpec],
    values: &[f64],
    indices: &[usize],
    changes: &mut Vec<(usize, f64)>,
) {
    egui::Grid::new(grid)
        .num_columns(2)
        .spacing([12.0, 8.0])
        .show(ui, |ui| {
            for &i in indices {
                let spec = &specs[i];
                let mut plain = spec.plain(values[i]);
                ui.label(spec.name);
                if spec.steps == 1 && spec.min == 0.0 && spec.max == 1.0 {
                    let on = plain >= 0.5;
                    if let Some(format) = spec.format {
                        // A switch between two named choices (the Phaser's Shape): both shown.
                        ui.horizontal(|ui| {
                            for choice in [false, true] {
                                let label = format(f64::from(u8::from(choice)));
                                if ui.selectable_label(on == choice, label).clicked()
                                    && on != choice
                                {
                                    changes.push((i, spec.normalized(f64::from(u8::from(choice)))));
                                }
                            }
                        });
                    } else {
                        let mut on = on;
                        if ui.add(crate::theme::checkbox(&mut on, "")).changed() {
                            changes.push((i, spec.normalized(f64::from(u8::from(on)))));
                        }
                    }
                } else {
                    let slider = egui::Slider::new(&mut plain, spec.min..=spec.max)
                        .fixed_decimals(spec.decimals)
                        .suffix(if spec.unit.is_empty() || spec.format.is_some() {
                            String::new()
                        } else {
                            format!(" {}", spec.unit)
                        });
                    let slider = slider.logarithmic(spec.log_slider);
                    let slider = if spec.steps > 0 {
                        slider.step_by((spec.max - spec.min) / f64::from(spec.steps))
                    } else {
                        slider
                    };
                    let slider = if let Some(format) = spec.format {
                        slider.custom_formatter(move |v, _| format(v))
                    } else {
                        slider
                    };
                    let response = ui.add(slider);
                    // Double-click: back to the default.
                    if response.double_clicked() {
                        changes.push((i, spec.normalized(spec.default)));
                    } else if response.changed() {
                        changes.push((i, spec.normalized(plain)));
                    }
                }
                ui.end_row();
            }
        });
}

/// The Space Reverb's parameters in groups (the reverb's settings, then its coloration); the outputs (the room's dimensions, the progress) are not
/// controls.
const FULL_REVERB_GROUPS: &[(&str, bool, &[usize])] = &[
    (
        "Reverberation",
        true,
        &[
            full_reverb::DECAY,
            full_reverb::PRE_DELAY,
            full_reverb::DIFFUSION,
            full_reverb::PERCEPTION,
        ],
    ),
    (
        "Early Reflections",
        true,
        &[
            full_reverb::ROOM_SIZE,
            full_reverb::DIMENSION,
            full_reverb::LOCATION,
            full_reverb::ER_HIGH_PASS,
            full_reverb::BASED_ON_ROOM,
        ],
    ),
    (
        "Output Level",
        true,
        &[
            full_reverb::DRY,
            full_reverb::REVERB,
            full_reverb::EARLY,
            full_reverb::INCLUDE_DIRECT,
            full_reverb::SUM_INPUTS,
        ],
    ),
    (
        "Coloration",
        false,
        &[
            full_reverb::LOW_GAIN,
            full_reverb::LOW_FREQ,
            full_reverb::MID_GAIN,
            full_reverb::MID_FREQ,
            full_reverb::MID_Q,
            full_reverb::HIGH_GAIN,
            full_reverb::HIGH_FREQ,
            full_reverb::COLORATION_DECAY,
        ],
    ),
    ("Advanced", false, &[full_reverb::ORIGINAL_RATE]),
];

/// The Space Reverb's impulse as made: the late reverb's envelope (filled) and the early
/// reflections (lines), in dB below the loudest, each at its output level now (the levels act
/// at the output, so moving them redraws without making it again), with its length, the room
/// and how the input is taken.
fn full_reverb_panel(ui: &mut egui::Ui, native: &daw_core::native::Native, preparing: bool) {
    use egui::{pos2, vec2, Color32, Stroke};
    let plain = native.plain_values();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Impulse:").strong());
        ui.label("made from the settings");
        if preparing {
            ui.spinner();
        }
    });
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 110.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let Some(overview) = native.file_overview().filter(|o| o.peaks.len() >= 4) else {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "Making the impulse…",
            egui::FontId::proportional(12.0),
            theme::TEXT_DIM,
        );
        return;
    };
    let early = (plain[full_reverb::EARLY] * 0.005) as f32;
    let late = (plain[full_reverb::REVERB] * 0.01) as f32;
    // Per column: the louder ear's peak, early and late at their levels.
    let column = |rows: [usize; 2], gain: f32| -> Vec<f32> {
        (0..overview.peaks[0].len())
            .map(|k| {
                rows.iter()
                    .map(|&r| {
                        let (lo, hi) = overview.peaks[r][k];
                        lo.abs().max(hi.abs()) * gain
                    })
                    .fold(0.0f32, f32::max)
            })
            .collect()
    };
    let (e, r) = (column([0, 1], early), column([2, 3], late));
    let loudest = e.iter().chain(&r).fold(1e-12f32, |m, v| m.max(*v));
    const FLOOR: f32 = -90.0;
    let db = |v: f32| (20.0 * (v / loudest).max(1e-9).log10()).max(FLOOR);
    let y_of = |d: f32| rect.top() + 4.0 + d / FLOOR * (rect.height() - 8.0);
    let step = rect.width() / r.len() as f32;
    let fill = Color32::from_rgba_unmultiplied(0x4a, 0xa8, 0xff, 110);
    for (k, v) in r.iter().enumerate() {
        if *v > 0.0 {
            let x = rect.left() + k as f32 * step;
            painter.rect_filled(
                egui::Rect::from_x_y_ranges(x..=x + step.max(1.0), y_of(db(*v))..=rect.bottom()),
                0,
                fill,
            );
        }
    }
    let reflections = Color32::from_rgb(230, 190, 90);
    for (k, v) in e.iter().enumerate() {
        if *v > 0.0 {
            let x = rect.left() + (k as f32 + 0.5) * step;
            painter.vline(
                x,
                y_of(db(*v))..=rect.bottom(),
                Stroke::new(1.0, reflections),
            );
        }
    }
    // The scale over the impulse, so it reads on the bars.
    let grid = Stroke::new(1.0, Color32::from_rgba_unmultiplied(0xd0, 0xd4, 0xdc, 70));
    let label = Color32::from_gray(215);
    for d in [-30.0f32, -60.0] {
        painter.hline(rect.x_range(), y_of(d), grid);
        painter.text(
            pos2(rect.right() - 3.0, y_of(d) - 1.0),
            egui::Align2::RIGHT_BOTTOM,
            format!("{d:.0} dB"),
            egui::FontId::proportional(10.0),
            label,
        );
    }
    let seconds = overview.samples as f64 / overview.rate;
    let tick = if seconds > 2.0 { 1.0 } else { 0.1 };
    let mut t = tick;
    while t < seconds {
        let x = rect.left() + (t / seconds) as f32 * rect.width();
        painter.vline(x, rect.y_range(), grid);
        painter.text(
            pos2(x + 3.0, rect.top() + 2.0),
            egui::Align2::LEFT_TOP,
            format!("{t:.*} s", usize::from(tick < 1.0)),
            egui::FontId::proportional(10.0),
            label,
        );
        t += tick;
    }
    let rows_end = |row: usize| {
        overview.peaks[row]
            .iter()
            .rposition(|(lo, hi)| lo.abs().max(hi.abs()) > 0.0)
            .map_or(0.0, |k| {
                (k + 1) as f64 / overview.peaks[row].len() as f64 * seconds
            })
    };
    let early_end = rows_end(0).max(rows_end(1));
    let [w, d, h] = [
        full_reverb::ROOM_WIDTH,
        full_reverb::ROOM_HEIGHT,
        full_reverb::ROOM_DEPTH,
    ]
    .map(|i| plain[i]);
    let true_stereo = overview.file_channels == 4;
    ui.label(
        RichText::new(format!(
            "{seconds:.2} s as played ({} samples at {:.1} kHz; {:.2} s before the trim), the \
             early reflections (amber) to {early_end:.2} s.\nRoom {w:.1} × {d:.1} × {h:.1} m \
             (width × depth × height). {}. 256 samples of delay.",
            overview.samples,
            overview.rate / 1000.0,
            overview.file_frames as f64 / overview.rate,
            if true_stereo {
                "True stereo: two speakers, four paths"
            } else {
                "The inputs summed into one source"
            },
        ))
        .small()
        .color(theme::TEXT_DIM),
    );
    if let Some(note) = &overview.note {
        ui.label(RichText::new(note).color(theme::REFUSED));
    }
}

/// The Click & Pop Repair's parameters in groups: its curve's points (the levels in the
/// effect's own order: "Avg" is the lowest), the verification's, then
/// the advanced settings; whether each starts open.
const CLICK_POP_GROUPS: &[(&str, bool, &[usize])] = &[
    (
        "Detection",
        true,
        &[
            click_pop::AVG_LEVEL,
            click_pop::AVG_THRESHOLD,
            click_pop::MIN_LEVEL,
            click_pop::MIN_THRESHOLD,
            click_pop::MAX_LEVEL,
            click_pop::MAX_THRESHOLD,
        ],
    ),
    (
        "Second Level Verification",
        false,
        &[
            click_pop::SECOND_LEVEL,
            click_pop::AVG_REJECT,
            click_pop::MIN_REJECT,
            click_pop::MAX_REJECT,
        ],
    ),
    (
        "Passes and sizes",
        false,
        &[
            click_pop::MULTIPLE_PASS,
            click_pop::PASSES,
            click_pop::RUN_SIZE,
            click_pop::POP_SIZE,
            click_pop::AUTO_FFT,
            click_pop::FFT_SIZE,
        ],
    ),
    (
        "Pops, crackle and channels",
        false,
        &[
            click_pop::BIG_POPS,
            click_pop::BIG_POP_LEVEL,
            click_pop::LIGHT_CRACKLE,
            click_pop::PULSE_TRAIN,
            click_pop::LINK,
        ],
    ),
    (
        "Scan For All Levels",
        false,
        &[click_pop::SENSITIVITY, click_pop::DISCRIMINATION],
    ),
];

/// The Click & Pop Repair's curves with their points, what its last scan found, its counters
/// and delay, and its two scan buttons (for the Waveform view's selection, `can_scan`). The
/// scan clicked, if one was.
fn click_pop_panel(
    ui: &mut egui::Ui,
    native: &daw_core::native::Native,
    can_scan: bool,
) -> Option<&'static str> {
    let plain = native.plain_values();
    threshold_curves(ui, &plain);
    let mut text = match native.print().and_then(click_pop::scanned) {
        Some(s) => format!(
            "{}: the levels {:.1} dB (max), {:.1} (mean), {:.1} (min){}",
            if s.all {
                "Scan For All Levels"
            } else {
                "Scan For Threshold Levels"
            },
            s.max,
            s.mean,
            s.min,
            if s.all {
                format!(", thresholds from {} trial clicks", s.clicks)
            } else {
                String::new()
            }
        ),
        None => "Not scanned: the levels are as set.".to_string(),
    };
    let counts = native.readouts();
    if let [corrected, big, rejected] = counts[..] {
        text.push_str(&format!(
            "\nCorrected {corrected} (big pops {big}), rejected {rejected} since playback \
             started.\n{} samples of delay (its blocks).",
            click_pop::LATENCY
        ));
    }
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
    let mut clicked = None;
    ui.horizontal(|ui| {
        for &name in click_pop::KIND.learns.map_or(&[][..], |l| l.scans) {
            let hint = if name.ends_with("All Levels") {
                "Learn the levels and the six thresholds from the Waveform view's selection (a \
                 trial detection with Sensitivity and Discrimination)"
            } else {
                "Learn the levels from the Waveform view's selection: its highest to Max Level, \
                 its lowest to Avg Level, its mean to Min Level, as the levels are labelled"
            };
            if ui
                .add_enabled(can_scan, egui::Button::new(name))
                .on_hover_text(hint)
                .on_disabled_hover_text("Select the audio to scan in the Waveform view")
                .clicked()
            {
                clicked = Some(name);
            }
        }
    });
    clicked
}

/// The Chorus's parameters in groups.
const CHORUS_GROUPS: &[(&str, bool, &[usize])] = &[
    (
        "Characteristics",
        true,
        &[
            chorus::VOICES,
            chorus::DELAY_TIME,
            chorus::DELAY_RATE,
            chorus::FEEDBACK,
            chorus::SPREAD,
            chorus::MOD_DEPTH,
            chorus::MOD_RATE,
            chorus::HIGHEST_QUALITY,
        ],
    ),
    (
        "Stereo Field",
        true,
        &[chorus::AVERAGE, chorus::BINAURAL, chorus::FIELD],
    ),
    ("Output Levels", true, &[chorus::DRY, chorus::WET]),
];

/// The Chorus's voices as a start draws them, on a delay axis (ms): each a bar over where its
/// delay wanders (its offset plus 0 to the Delay Time: the noise's ±1), a dot at the middle,
/// placed from left (top) to right (bottom) by its gains, blue on the left and amber on the
/// right, its binaural delay a tick past the bar; then what the settings make of the wander and
/// the level.
fn chorus_panel(ui: &mut egui::Ui, rate: f64, plain: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let voices = chorus::layout(rate, plain);
    let d = plain[chorus::DELAY_TIME];
    let reach = voices
        .iter()
        .map(|v| v.offset_ms + d + v.binaural_ms[0].max(v.binaural_ms[1]))
        .fold(1.0f64, f64::max);
    // A round span: 10, 20, 50, 100, 200, 300 ms.
    let span = [10.0, 20.0, 50.0, 100.0, 200.0, 300.0]
        .into_iter()
        .find(|s| *s >= reach * 1.05)
        .unwrap_or(300.0);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 120.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let plot = rect.shrink2(vec2(10.0, 8.0));
    let plot = egui::Rect::from_min_max(plot.min, pos2(plot.max.x, plot.max.y - 10.0));
    let x_of = |ms: f64| plot.left() + (ms / span) as f32 * plot.width();
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    let small = egui::FontId::proportional(10.0);
    for k in 0..=5 {
        let ms = span * f64::from(k) / 5.0;
        painter.vline(x_of(ms), rect.y_range(), grid);
        let (x, align) = if k == 5 {
            (x_of(ms) - 2.0, egui::Align2::RIGHT_BOTTOM)
        } else {
            (x_of(ms) + 2.0, egui::Align2::LEFT_BOTTOM)
        };
        painter.text(
            pos2(x, rect.bottom() - 1.0),
            align,
            format!("{ms:.0} ms"),
            small.clone(),
            theme::TEXT_DIM,
        );
    }
    painter.text(
        pos2(rect.right() - 3.0, rect.top() + 2.0),
        egui::Align2::RIGHT_TOP,
        "L",
        small.clone(),
        theme::TEXT_DIM,
    );
    painter.text(
        pos2(rect.right() - 3.0, plot.bottom()),
        egui::Align2::RIGHT_BOTTOM,
        "R",
        small.clone(),
        theme::TEXT_DIM,
    );
    let amber = Color32::from_rgb(230, 190, 90);
    for v in &voices {
        let [l, r] = v.gains;
        let right = (r / (l + r).max(1e-9)) as f32;
        let y = plot.top() + right * plot.height();
        let colour = if right > 0.55 {
            amber
        } else if right < 0.45 {
            theme::ACCENT
        } else {
            Color32::from_gray(200)
        };
        let (a, b) = (x_of(v.offset_ms), x_of(v.offset_ms + d));
        painter.line_segment(
            [pos2(a, y), pos2(b.max(a + 2.0), y)],
            Stroke::new(3.0, colour.gamma_multiply(0.6)),
        );
        painter.circle_filled(pos2(x_of(v.offset_ms + d / 2.0), y), 3.0, colour);
        let late = v.binaural_ms[0].max(v.binaural_ms[1]);
        if late > 0.0 {
            let x = x_of(v.offset_ms + d / 2.0 + late);
            painter.line_segment(
                [pos2(x, y - 4.0), pos2(x, y + 4.0)],
                Stroke::new(1.5, colour),
            );
        }
    }
    let depth = plain[chorus::MOD_DEPTH];
    let hq = plain[chorus::HIGHEST_QUALITY] >= 0.5;
    ui.label(
        RichText::new(format!(
            "{} voice{}: each delay wanders under noise filtered at {:.2} Hz, mostly within its \
             bar (offset to offset + {d:.1} ms){}; {}. {} interpolation. The same start \
             always plays the same: the noise is reseeded from the settings.",
            voices.len(),
            if voices.len() == 1 { "" } else { "s" },
            plain[chorus::DELAY_RATE],
            if plain[chorus::BINAURAL] >= 0.5 {
                ", the far ear's binaural delay ticked"
            } else {
                ""
            },
            if depth > 0.0 {
                format!(
                    "levels wobble near {:.2} Hz, at most ±{:.1} dB",
                    plain[chorus::MOD_RATE],
                    3.0 * depth
                )
            } else {
                "levels steady".to_string()
            },
            if hq { "32-tap sinc" } else { "Cubic" }
        ))
        .color(theme::TEXT_DIM),
    );
}

/// The Flanger's parameters in groups.
const FLANGER_GROUPS: &[(&str, bool, &[usize])] = &[
    (
        "Characteristics",
        true,
        &[
            flanger::INITIAL,
            flanger::FINAL,
            flanger::PHASING,
            flanger::FEEDBACK,
            flanger::RATE,
        ],
    ),
    ("Output", true, &[flanger::MIX]),
    (
        "Modes",
        true,
        &[flanger::INVERTED, flanger::SPECIAL, flanger::SINUSOIDAL],
    ),
];

/// The Flanger's sweep over two cycles: each tap's distance from the centre (ms) against time,
/// the left channel in blue and the right (leading by Stereo Phasing) in amber, as the DSP
/// computes it; then the sweep in words, the taps' mode and the latency.
fn flanger_panel(ui: &mut egui::Ui, rate: f64, plain: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let hz = plain[flanger::RATE].max(1e-6);
    let ms_of = |p: f64| p * 1000.0 / rate;
    let top = [flanger::INITIAL, flanger::FINAL]
        .map(|i| plain[i])
        .into_iter()
        .fold(0.5f64, f64::max);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 110.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let plot = egui::Rect::from_min_max(rect.min + vec2(40.0, 8.0), rect.max - vec2(8.0, 16.0));
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    let small = egui::FontId::proportional(10.0);
    let y_of = |ms: f64| plot.bottom() - (ms / top) as f32 * plot.height();
    for ms in [0.0, top / 2.0, top] {
        painter.hline(plot.x_range(), y_of(ms), grid);
        painter.text(
            pos2(rect.left() + 3.0, y_of(ms)),
            egui::Align2::LEFT_CENTER,
            format!("{ms:.1} ms"),
            small.clone(),
            theme::TEXT_DIM,
        );
    }
    let period = 1.0 / hz;
    let span = 2.0 * period;
    for k in 0..=4 {
        let x = plot.left() + plot.width() * k as f32 / 4.0;
        painter.vline(x, plot.y_range(), grid);
    }
    painter.text(
        pos2(plot.right(), rect.bottom() - 1.0),
        egui::Align2::RIGHT_BOTTOM,
        if span >= 1.0 {
            format!("{span:.1} s")
        } else {
            format!("{:.0} ms", span * 1000.0)
        },
        small.clone(),
        theme::TEXT_DIM,
    );
    painter.text(
        pos2(plot.left(), rect.bottom() - 1.0),
        egui::Align2::LEFT_BOTTOM,
        "0",
        small,
        theme::TEXT_DIM,
    );
    let amber = Color32::from_rgb(230, 190, 90);
    for (channel, colour) in [(1, amber), (0, theme::ACCENT)] {
        let points: Vec<_> = (0..=400)
            .map(|k| {
                let u = f64::from(k) / 400.0;
                let phi = -std::f64::consts::FRAC_PI_2 + 2.0 * std::f64::consts::PI * 2.0 * u;
                let p = flanger::position(rate, plain, phi, channel);
                pos2(plot.left() + u as f32 * plot.width(), y_of(ms_of(p)))
            })
            .collect();
        painter.add(egui::Shape::line(points, Stroke::new(2.0, colour)));
    }
    let sine = plain[flanger::SINUSOIDAL] >= 0.5;
    let special = plain[flanger::SPECIAL] >= 0.5;
    ui.label(
        RichText::new(format!(
            "A {} sweep from {:.2} to {:.2} ms every {} ({:.3} beats, counted \
             at 120 BPM); the right (amber) leads by {:.0}°. {} Latency {} samples \
             ({:.1} ms), compensated.",
            if sine { "sine" } else { "triangle" },
            plain[flanger::INITIAL],
            plain[flanger::FINAL],
            if period >= 1.0 {
                format!("{period:.2} s")
            } else {
                format!("{:.1} ms", period * 1000.0)
            },
            2.0 / hz,
            plain[flanger::PHASING],
            if special {
                "One tap behind the centre."
            } else {
                "Two taps either side of the centre, nulling it through zero where they meet."
            },
            flanger::LATENCY,
            flanger::LATENCY as f64 * 1000.0 / rate,
        ))
        .color(theme::TEXT_DIM),
    );
}

/// The Phaser's parameters in its dialog's order, then the engine's switches the dialog
/// doesn't show.
const PHASER_GROUPS: &[(&str, bool, &[usize])] = &[
    (
        "Phaser",
        true,
        &[
            phaser::STAGES,
            phaser::INTENSITY,
            phaser::DEPTH,
            phaser::RATE,
            phaser::PHASE,
            phaser::UPPER,
            phaser::FEEDBACK,
        ],
    ),
    ("Output", true, &[phaser::MIX, phaser::GAIN]),
    ("Sweep", true, &[phaser::SHAPE, phaser::STEP, phaser::SYNC]),
];

/// The Phaser's response with the sweep held at its bottom (amber) and its top (blue), 20 Hz to
/// 20 kHz, the sweep's ends marked; then the sweep and the pole radius in words.
fn phaser_panel(ui: &mut egui::Ui, rate: f64, plain: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let (lo, hi) = (20.0f64, 20_000.0f64.min(rate / 2.0));
    let freq = |k: usize| lo * (hi / lo).powf(k as f64 / 400.0);
    let curves: Vec<Vec<f64>> = [0.0, 1.0]
        .iter()
        .map(|l| {
            (0..=400)
                .map(|k| phaser::response_db(rate, plain, *l, freq(k)))
                .collect()
        })
        .collect();
    let peak = curves
        .iter()
        .flatten()
        .copied()
        .fold(f64::MIN, f64::max)
        .max(6.0);
    let top = (peak / 6.0).ceil() * 6.0;
    // Down to the deepest point drawn (a notch at Mix 50 % goes on down: cut at -48 dB), at
    // least -12 dB.
    let deepest = curves.iter().flatten().copied().fold(f64::MAX, f64::min);
    let bottom = ((deepest - 3.0) / 6.0).floor().clamp(-8.0, -2.0) * 6.0;
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 120.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    let small = egui::FontId::proportional(10.0);
    let x_of = |hz: f64| rect.left() + ((hz / lo).ln() / (hi / lo).ln()) as f32 * rect.width();
    let y_of = |db: f64| {
        rect.top()
            + 4.0
            + ((top - db.clamp(bottom, top)) / (top - bottom)) as f32 * (rect.height() - 8.0)
    };
    for db in [0.0, (bottom / 2.0 / 6.0).round() * 6.0] {
        painter.hline(rect.x_range(), y_of(db), grid);
        painter.text(
            pos2(rect.left() + 3.0, y_of(db) - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:.0} dB"),
            small.clone(),
            theme::TEXT_DIM,
        );
    }
    for hz in [100.0, 1000.0, 10_000.0] {
        painter.vline(x_of(hz), rect.y_range(), grid);
        painter.text(
            pos2(x_of(hz) + 2.0, rect.bottom() - 1.0),
            egui::Align2::LEFT_BOTTOM,
            if hz >= 1000.0 {
                format!("{:.0} kHz", hz / 1000.0)
            } else {
                format!("{hz:.0} Hz")
            },
            small.clone(),
            theme::TEXT_DIM,
        );
    }
    let (from, to) = phaser::sweep_range(rate, plain);
    let amber = Color32::from_rgb(230, 190, 90);
    for (hz, colour) in [(from, amber), (to, theme::ACCENT)] {
        painter.vline(
            x_of(hz),
            rect.y_range(),
            Stroke::new(1.0, colour.gamma_multiply(0.5)),
        );
    }
    for (curve, colour) in curves.iter().zip([amber, theme::ACCENT]) {
        let line: Vec<_> = curve
            .iter()
            .enumerate()
            .map(|(k, db)| pos2(x_of(freq(k)), y_of(*db)))
            .collect();
        painter.add(egui::Shape::line(line, Stroke::new(2.0, colour)));
    }
    let stages = phaser::stages(plain);
    let sync = plain[phaser::SYNC] >= 0.5;
    ui.label(
        RichText::new(format!(
            "{stages} notch{} swept from {to:.0} Hz (blue) down to {from:.0} Hz (amber) and back, \
             {} on a {} scale, at {:.2} Hz; {}. Pole radius {:.4} (Intensity). Mix 50 % nulls \
             the notches; feedback raises peaks between them.",
            if stages == 1 { "" } else { "es" },
            if plain[phaser::SHAPE] >= 0.5 {
                "a sine"
            } else {
                "a triangle"
            },
            if plain[phaser::STEP] >= 0.5 {
                "linear"
            } else {
                "logarithmic"
            },
            plain[phaser::RATE],
            if sync {
                "both channels on one LFO".to_string()
            } else {
                format!("the right {:+.0}° ahead", plain[phaser::PHASE])
            },
            phaser::radius(rate, plain[phaser::INTENSITY]),
        ))
        .color(theme::TEXT_DIM),
    );
}

/// The Vintage Delay's parameters as its dialog groups them (Mode is the buttons above).
const ANALOG_DELAY_GROUPS: &[(&str, bool, &[usize])] = &[
    ("Output", true, &[analog_delay::DRY, analog_delay::WET]),
    (
        "Delay",
        true,
        &[
            analog_delay::DELAY,
            analog_delay::FEEDBACK,
            analog_delay::SYNC,
            analog_delay::NOTE,
        ],
    ),
    (
        "Character",
        true,
        &[analog_delay::TRASH, analog_delay::SPREAD],
    ),
];

/// The Vintage Delay's Mode (three buttons), the loop's shaper at
/// the Trash set (input -1.5..1.5 against its output) beside its low-pass (20 Hz to 20 kHz,
/// +6..-36 dB), and what the settings make of the delay: its length in samples, the wow, and the
/// note at the song's tempo when synced (`meter`: the tempo at the playhead). Returns a mode
/// clicked.
fn analog_delay_panel(
    ui: &mut egui::Ui,
    rate: f64,
    plain: &[f64],
    meter: &daw_core::graph::Meter,
) -> Option<usize> {
    use egui::{pos2, vec2, Color32, Rect, Stroke};
    let mode = (plain[analog_delay::MODE].round().max(0.0) as usize).min(2);
    let mut picked = None;
    ui.horizontal(|ui| {
        ui.label(RichText::new("Mode:").strong());
        for (m, name) in analog_delay::MODES.iter().enumerate() {
            if ui.selectable_label(m == mode, *name).clicked() && m != mode {
                picked = Some(m);
            }
        }
    });
    let t = (plain[analog_delay::TRASH] / 100.0) as f32;
    let shaper = analog_delay::Shaper::new(mode, t);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 120.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    let gap = 8.0;
    let left = Rect::from_min_max(rect.min, pos2(rect.left() + 120.0, rect.bottom()));
    let right = Rect::from_min_max(pos2(left.right() + gap, rect.top()), rect.max);
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    let small = egui::FontId::proportional(10.0);
    // The shaper: -1.5..1.5 both ways, with the drive into the loop (Tape).
    painter.rect_filled(left, 3, theme::LANE);
    let to_screen = |u: f32, v: f32| {
        pos2(
            left.center().x + u / 1.5 * (left.width() / 2.0 - 4.0),
            left.center().y - v.clamp(-1.5, 1.5) / 1.5 * (left.height() / 2.0 - 4.0),
        )
    };
    painter.hline(left.x_range(), left.center().y, grid);
    painter.vline(left.center().x, left.y_range(), grid);
    painter.line_segment([to_screen(-1.5, -1.5), to_screen(1.5, 1.5)], grid);
    let shape: Vec<_> = (0..=120)
        .map(|k| {
            let u = -1.5 + 3.0 * k as f32 / 120.0;
            to_screen(u, shaper.apply(u * shaper.drive))
        })
        .collect();
    painter.add(egui::Shape::line(shape, Stroke::new(2.0, theme::ACCENT)));
    painter.text(
        left.left_top() + vec2(3.0, 2.0),
        egui::Align2::LEFT_TOP,
        "shaper",
        small.clone(),
        theme::TEXT_DIM,
    );
    // The low-pass in the loop.
    painter.rect_filled(right, 3, theme::LANE);
    let c = analog_delay::filter(rate, mode);
    let (lo, hi) = (20.0f64, 20_000.0f64);
    let x_of = |hz: f64| right.left() + ((hz / lo).ln() / (hi / lo).ln()) as f32 * right.width();
    let y_of = |db: f64| {
        right.top() + 4.0 + ((6.0 - db.clamp(-36.0, 6.0)) / 42.0) as f32 * (right.height() - 8.0)
    };
    for db in [0.0, -12.0, -24.0] {
        painter.hline(right.x_range(), y_of(db), grid);
        painter.text(
            pos2(right.left() + 3.0, y_of(db) - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:.0} dB"),
            small.clone(),
            theme::TEXT_DIM,
        );
    }
    for hz in [100.0, 1000.0, 10_000.0] {
        painter.vline(x_of(hz), right.y_range(), grid);
    }
    let line: Vec<_> = (0..=200)
        .map(|k| {
            let hz = lo * (hi / lo).powf(f64::from(k) / 200.0);
            pos2(
                x_of(hz),
                y_of(20.0 * c.magnitude(hz, rate).max(1e-9).log10()),
            )
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(2.0, Color32::from_rgb(120, 200, 255)),
    ));
    let (f0, q) = analog_delay::FILTERS[mode];
    painter.text(
        right.right_top() + vec2(-4.0, 2.0),
        egui::Align2::RIGHT_TOP,
        format!("low-pass {:.1} kHz, Q {q}", f0 / 1000.0),
        small,
        theme::TEXT_DIM,
    );
    let synced = plain[analog_delay::SYNC] >= 0.5;
    let quarter = analog_delay::quarter_ms(meter);
    let note = plain[analog_delay::NOTE].round().max(0.0) as usize;
    let ms = if synced {
        analog_delay::note_ms(quarter, note)
    } else {
        plain[analog_delay::DELAY]
    };
    let feedback = plain[analog_delay::FEEDBACK];
    ui.label(
        RichText::new(format!(
            "Delay {ms:.1} ms = {} samples{}; the wow sways it ±0.1 ms every {:.0} ms.\n{}{}",
            analog_delay::delay_samples(rate, ms),
            if synced {
                format!(
                    " ({} at {:.1} BPM at the playhead)",
                    analog_delay::NOTE_NAMES[note.min(analog_delay::NOTE_NAMES.len() - 1)],
                    meter.tempo
                )
            } else {
                String::new()
            },
            analog_delay::WOW_PERIOD_MS[mode],
            if mode == analog_delay::TAPE {
                format!(
                    "Tape drives the loop {:+.1} dB and makes up {:.1} dB on the wet. ",
                    20.0 * shaper.drive.log10(),
                    20.0 * shaper.comp.log10()
                )
            } else {
                String::new()
            },
            if feedback > 100.0 {
                "Above 100 % Feedback the wet output is limited at 0.975."
            } else {
                "Changes of Delay glide at up to a sample a sample, like tape."
            }
        ))
        .color(theme::TEXT_DIM),
    );
    picked
}

/// The Echo's parameters in groups.
const ECHO_GROUPS: &[(&str, bool, &[usize])] = &[
    ("Delay Time", true, &[echo::LEFT_DELAY, echo::RIGHT_DELAY]),
    (
        "Feedback",
        true,
        &[echo::LEFT_FEEDBACK, echo::RIGHT_FEEDBACK],
    ),
    ("Echo Level", true, &[echo::LEFT_LEVEL, echo::RIGHT_LEVEL]),
    ("Options", true, &[echo::LOCK, echo::BOUNCE, echo::UNITS]),
    (
        "Successive Echo Equalization",
        false,
        &[6, 7, 8, 9, 10, 11, 12, 13],
    ),
];

/// The Echo's repeats of an impulse on each input, a row per output side, each a bar at its
/// time with its level in dB (0 to -60, before the EQ and the clip): Echo Level, then Feedback
/// per pass. The left input's repeats in blue, the right's in amber beside them, so Bounce
/// shows as each colour alternating sides.
fn echo_panel(ui: &mut egui::Ui, rate: f64, plain: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let ages = [echo::LEFT_DELAY, echo::RIGHT_DELAY]
        .map(|i| echo::age_of(rate, echo::delay_samples(rate, plain[i])) as f64 / rate);
    let f = [echo::LEFT_FEEDBACK, echo::RIGHT_FEEDBACK].map(|i| plain[i] / 100.0);
    let e = [echo::LEFT_LEVEL, echo::RIGHT_LEVEL].map(|i| plain[i] / 100.0);
    let bounce = plain[echo::BOUNCE] >= 0.5;
    // What enters each line (time, line, level, which input); each is heard on that line's
    // side a delay later, and goes round again (into the other line with Bounce).
    let span = (ages[0].max(ages[1]) * 8.0).clamp(0.5, 8.0);
    let mut pending: Vec<(f64, usize, f64, usize)> = (0..2)
        .map(|c| (0.0, if bounce { 1 - c } else { c }, e[c], c))
        .collect();
    let mut heard: Vec<(f64, usize, f64, usize)> = Vec::new();
    while let Some((t, line, level, input)) = pending.pop() {
        let (t, level) = (t + ages[line], level * f[line]);
        if t > span || level < 0.001 || heard.len() > 400 {
            continue;
        }
        heard.push((t, line, level, input));
        pending.push((t, if bounce { 1 - line } else { line }, level, input));
    }
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 96.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let x_of = |t: f64| rect.left() + 12.0 + (t / span) as f32 * (rect.width() - 16.0);
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    let step = if span > 2.0 { 1.0 } else { 0.25 };
    let mut t = 0.0;
    while t <= span {
        painter.vline(x_of(t), rect.y_range(), grid);
        painter.text(
            pos2(x_of(t) + 2.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{t:.2} s"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        t += step;
    }
    let row = (rect.height() - 14.0) / 2.0;
    for (c, name) in ["L", "R"].iter().enumerate() {
        let base = rect.top() + row * (c as f32 + 1.0);
        painter.hline(rect.x_range(), base, grid);
        painter.text(
            pos2(rect.left() + 2.0, base - row + 2.0),
            egui::Align2::LEFT_TOP,
            *name,
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        for &(t, _, level, input) in heard.iter().filter(|h| h.1 == c) {
            let db = 20.0 * level.max(1e-6).log10();
            let height = ((db + 60.0) / 60.0).clamp(0.0, 1.0) as f32 * (row - 4.0);
            let (dx, colour) = if input == 0 {
                (-1.5, theme::ACCENT)
            } else {
                (1.5, Color32::from_rgb(230, 190, 90))
            };
            let x = x_of(t) + dx;
            painter.line_segment(
                [pos2(x, base), pos2(x, base - height)],
                Stroke::new(2.0, colour),
            );
        }
    }
    let ms = |i: usize| echo::delay_samples(rate, plain[i]) as f64 * 1000.0 / rate;
    let zero = plain[echo::LEFT_DELAY] == 0.0 || plain[echo::RIGHT_DELAY] == 0.0;
    ui.label(
        RichText::new(format!(
            "Repeats every {:.1} ms (L) and {:.1} ms (R), {} (blue: the left input's, amber: \
             the right's); levels before the EQ, which filters every pass again. The line clips \
             at ±1.{}",
            ms(echo::LEFT_DELAY),
            ms(echo::RIGHT_DELAY),
            if bounce {
                "alternating sides"
            } else {
                "each side on its own"
            },
            if zero {
                " A 0 ms delay reads a whole line back (2 s + 200 samples)."
            } else {
                ""
            }
        ))
        .color(theme::TEXT_DIM),
    );
}

/// The Delay's parameters in groups.
const DELAY_GROUPS: &[(&str, bool, &[usize])] = &[
    (
        "Left Channel",
        true,
        &[delay::LEFT_DELAY, delay::LEFT_MIX, delay::LEFT_INVERT],
    ),
    (
        "Right Channel",
        true,
        &[delay::RIGHT_DELAY, delay::RIGHT_MIX, delay::RIGHT_INVERT],
    ),
    ("Delay Time Units", true, &[delay::UNITS]),
];

/// The Delay's taps on a time axis from -500 to +500 ms, a row per channel: the dry sound at 0
/// (grey, its level 1 - Mix) and the copy at its delay (its level Mix, below the line when
/// inverted); then each delay in the chosen unit first (beats counted through the song's tempo
/// from the playhead by `beats`, from samples), and the latency the negative times cost.
fn delay_panel(ui: &mut egui::Ui, rate: f64, plain: &[f64], beats: &dyn Fn(f64) -> f64) {
    use egui::{pos2, vec2, Color32, Stroke};
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 104.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let x_of =
        |ms: f64| rect.left() + 10.0 + ((ms + 500.0) / 1000.0) as f32 * (rect.width() - 20.0);
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    for ms in [-500.0, -250.0, 0.0, 250.0, 500.0] {
        painter.vline(x_of(ms), rect.y_range(), grid);
        // The last label inside the frame, to the left of its line.
        let (x, align) = if ms > 400.0 {
            (x_of(ms) - 2.0, egui::Align2::RIGHT_BOTTOM)
        } else {
            (x_of(ms) + 2.0, egui::Align2::LEFT_BOTTOM)
        };
        painter.text(
            pos2(x, rect.bottom() - 2.0),
            align,
            format!("{ms:+.0} ms").replace("+0 ms", "0"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    let rows = [
        ("L", delay::LEFT_DELAY, delay::LEFT_MIX, delay::LEFT_INVERT),
        (
            "R",
            delay::RIGHT_DELAY,
            delay::RIGHT_MIX,
            delay::RIGHT_INVERT,
        ),
    ];
    let row_height = (rect.height() - 14.0) / 2.0;
    let half = row_height / 2.0 - 4.0;
    for (k, (name, d, m, inv)) in rows.iter().enumerate() {
        let mid = rect.top() + row_height * (k as f32 + 0.5);
        painter.hline(rect.x_range(), mid, grid);
        painter.text(
            pos2(rect.left() + 3.0, mid - half),
            egui::Align2::LEFT_TOP,
            *name,
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        let mix = (plain[*m] / 100.0) as f32;
        let samples = delay::delay_samples(rate, plain[*d]);
        let ms = samples as f64 * 1000.0 / rate;
        let sign = if plain[*inv] >= 0.5 { -1.0 } else { 1.0 };
        painter.line_segment(
            [
                pos2(x_of(0.0), mid),
                pos2(x_of(0.0), mid - (1.0 - mix) * half),
            ],
            Stroke::new(3.0, Color32::from_gray(170)),
        );
        painter.line_segment(
            [pos2(x_of(ms), mid), pos2(x_of(ms), mid - sign * mix * half)],
            Stroke::new(3.0, theme::ACCENT),
        );
    }
    let unit = plain[delay::UNITS].round() as i64;
    let line = |name: &str, d: usize| {
        let samples = delay::delay_samples(rate, plain[d]);
        let parts = [
            format!("{:.1} ms", plain[d]),
            format!("{samples} samples"),
            format!("{:.3} beats", beats(samples as f64)),
        ];
        let first = match unit {
            1 => 2,
            2 => 1,
            _ => 0,
        };
        let rest: Vec<&str> = (0..3)
            .filter(|&i| i != first)
            .map(|i| parts[i].as_str())
            .collect();
        format!("{name}: {} ({})", parts[first], rest.join(", "))
    };
    let latency = delay::latency(rate, plain);
    ui.label(
        RichText::new(format!(
            "{}\n{}\n{}",
            line("Left", delay::LEFT_DELAY),
            line("Right", delay::RIGHT_DELAY),
            if latency > 0 {
                format!(
                    "Latency {latency} samples ({:.1} ms): the copy ahead of the dry sound \
                     needs it; the track is compensated.",
                    latency as f64 * 1000.0 / rate
                )
            } else {
                "No latency: no copy ahead of the dry sound.".to_string()
            }
        ))
        .color(theme::TEXT_DIM),
    );
}

/// The Room Reverb's decay: the combs' energy falling after an impulse, low (200 Hz) and high
/// (8 kHz, where Damping shortens it), over 0 to -60 dB, with what the settings make of the
/// network (its feedback, RT60s and delay lengths).
fn studio_reverb_panel(ui: &mut egui::Ui, rate: f64, plain: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let (low, high) = (200.0, 8000.0);
    let (short, long) = studio_reverb::rt60(rate, plain, low);
    let (hf_short, hf_long) = studio_reverb::rt60(rate, plain, high);
    let span = (long * 1.15).max(0.2);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 110.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let x_of = |t: f64| rect.left() + (t / span) as f32 * rect.width();
    let y_of =
        |db: f64| rect.top() + 4.0 + (db.clamp(-60.0, 0.0) / -60.0) as f32 * (rect.height() - 8.0);
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    for db in [-20.0, -40.0] {
        painter.hline(rect.x_range(), y_of(db), grid);
        painter.text(
            pos2(rect.left() + 3.0, y_of(db) - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:.0} dB"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    // A second line per second (or per 100 ms on short tails).
    let step = if span > 2.0 { 1.0 } else { 0.1 };
    let mut t = step;
    while t < span {
        painter.vline(x_of(t), rect.y_range(), grid);
        painter.text(
            pos2(x_of(t) + 3.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{t:.1} s"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
        t += step;
    }
    for (hz, color) in [
        (low, Color32::from_rgb(120, 200, 255)),
        (high, Color32::from_rgb(230, 190, 90)),
    ] {
        let line: Vec<_> = (0..=200)
            .map(|k| {
                let t = span * f64::from(k) / 200.0;
                pos2(x_of(t), y_of(studio_reverb::decay_db(rate, plain, hz, t)))
            })
            .collect();
        painter.add(egui::Shape::line(line, Stroke::new(2.0, color)));
    }
    let combs = studio_reverb::comb_lengths(rate, plain[studio_reverb::ROOM], false);
    let allpasses = studio_reverb::allpass_lengths(rate, plain[studio_reverb::EARLY_DELAY], false);
    let ms = |n: usize| n as f64 * 1000.0 / rate;
    ui.label(
        RichText::new(format!(
            "RT60 {short:.2} to {long:.2} s (blue, 200 Hz), {hf_short:.2} to {hf_long:.2} s \
             (amber, 8 kHz).\nComb feedback {:.3}; combs {:.1} to {:.1} ms, allpasses {:.1} to \
             {:.1} ms (+0.53 ms on the right).\nRoom Size and Early Reflections Delay empty the \
             reverb when changed.",
            studio_reverb::feedback(plain[studio_reverb::DECAY], plain[studio_reverb::ROOM]),
            ms(combs[0]),
            ms(combs[7]),
            ms(allpasses[0]),
            ms(allpasses[3]),
        ))
        .color(theme::TEXT_DIM),
    );
}

/// The detection and rejection curves over the level (-100 to 0 dB), as the effect reads them
/// (clamped to 6..150 and 2..150), with the three points of each: "Avg" (the lowest level),
/// "Min" and "Max".
fn threshold_curves(ui: &mut egui::Ui, plain: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let detect: Vec<f64> = click_pop::curve(plain, false)
        .into_iter()
        .map(|v| v.clamp(6.0, 150.0))
        .collect();
    let reject: Vec<f64> = click_pop::curve(plain, true)
        .into_iter()
        .map(|v| v.clamp(2.0, 150.0))
        .collect();
    let points = |reject: bool| click_pop::points(plain, reject);
    let highest = detect
        .iter()
        .chain(&reject)
        .chain(points(false)[2..5].iter().map(|p| &p.1))
        .chain(points(true)[2..5].iter().map(|p| &p.1))
        .fold(0.0f64, |m, v| m.max(*v));
    let top = [40.0, 80.0, 160.0]
        .into_iter()
        .find(|t| highest <= *t)
        .unwrap_or(160.0);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 150.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let x_of = |db: f64| rect.left() + ((db + 100.0) / 100.0) as f32 * rect.width();
    let y_of =
        |t: f64| rect.bottom() - 6.0 - (t.clamp(0.0, top) / top) as f32 * (rect.height() - 12.0);
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    for db in [-80.0, -60.0, -40.0, -20.0] {
        painter.vline(x_of(db), rect.y_range(), grid);
        painter.text(
            pos2(x_of(db) + 3.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:.0} dB"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    for t in [top / 4.0, top / 2.0, 3.0 * top / 4.0] {
        painter.hline(rect.x_range(), y_of(t), grid);
        painter.text(
            pos2(rect.left() + 3.0, y_of(t) - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{t:.0}"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    let blue = Color32::from_rgb(120, 200, 255);
    let orange = Color32::from_rgb(230, 170, 80);
    for (table, color) in [(&reject, orange), (&detect, blue)] {
        let line: Vec<_> = table
            .iter()
            .enumerate()
            .map(|(k, t)| pos2(x_of(-100.0 + k as f64 / 10.0), y_of(*t)))
            .collect();
        painter.add(egui::Shape::line(line, Stroke::new(2.0, color)));
    }
    for (reject, color) in [(true, orange), (false, blue)] {
        for (p, name) in points(reject)[2..5].iter().zip(["Avg", "Min", "Max"]) {
            if !(-100.0..=0.0).contains(&p.0) {
                continue;
            }
            let at = pos2(x_of(p.0), y_of(p.1));
            painter.circle_filled(at, 3.5, color);
            if !reject {
                painter.text(
                    at + vec2(4.0, -3.0),
                    egui::Align2::LEFT_BOTTOM,
                    name,
                    egui::FontId::proportional(10.0),
                    color,
                );
            }
        }
    }
    painter.text(
        pos2(rect.right() - 4.0, rect.top() + 3.0),
        egui::Align2::RIGHT_TOP,
        "detect",
        egui::FontId::proportional(10.0),
        blue,
    );
    painter.text(
        pos2(rect.right() - 4.0, rect.top() + 15.0),
        egui::Align2::RIGHT_TOP,
        "reject",
        egui::FontId::proportional(10.0),
        orange,
    );
}

/// Clip Repair's scan (or full scale, not scanned), the bands each channel's runs are
/// found in, the gain and the delay, and a button to scan the Waveform view's selection
/// (`can_scan`). Whether it was clicked.
fn declipper_panel(
    ui: &mut egui::Ui,
    native: &daw_core::native::Native,
    rate: u32,
    can_scan: bool,
) -> bool {
    let plain = native.plain_values();
    let scanned = native.print().and_then(declipper::learned);
    let levels = scanned.unwrap_or([declipper::FULL_SCALE; 2]);
    let dbfs = |v: f64| 20.0 * v.abs().max(1e-9).log10();
    let mut text = match scanned {
        Some(_) => "Scanned clip levels:".to_string(),
        None => "Not scanned: clipping taken at full scale (±1.0).".to_string(),
    };
    for (name, l) in ["Left", "Right"].iter().zip(&levels) {
        let bands = declipper::Bands::new(l, plain[declipper::TOLERANCE]);
        let band = |v: f64| {
            if v.is_finite() {
                format!("{v:+.3}")
            } else {
                "off".to_string()
            }
        };
        text.push_str(&format!(
            "\n{name}: {:+.3} / {:+.3} ({:.1} / {:.1} dBFS){}\n    clipping looked for from {} up and {} down",
            l.top,
            l.bottom,
            dbfs(l.top),
            dbfs(l.bottom),
            if scanned.is_some() {
                format!(", {} runs in the scan", l.runs)
            } else {
                String::new()
            },
            band(bands.high),
            band(bands.low),
        ));
    }
    let auto = plain[declipper::AUTO] >= 0.5;
    let gain = if auto {
        declipper::auto_gain_db(&levels)
    } else {
        plain[declipper::GAIN]
    };
    text.push_str(&format!(
        "\nGain {gain:+.1} dB{}, then a limiter at -0.1 dBFS.\n{} samples of delay (the \
         repair's {}, the limiter's 20 ms).",
        if auto { " (Auto)" } else { "" },
        declipper::latency(f64::from(rate)),
        declipper::DELAY,
    ));
    ui.label(RichText::new(text).color(theme::TEXT_DIM));
    ui.add_enabled(
        can_scan,
        egui::Button::new("Scan the Waveform view's selection"),
    )
    .on_hover_text("Learn the clip levels from it (a scan)")
    .on_disabled_hover_text(
        "Select the clipped audio in the Waveform view (or use Effects > Capture noise \
             print / clip levels there)",
    )
    .clicked()
}

/// The effect's frequency response, 20 Hz to 20 kHz on a log scale, in dB: the scale grows in
/// steps (±6, 12, 24, 48 dB) to fit the boosts and cuts, so small ones read clearly; a pass
/// filter's slope runs off the bottom. A `fixed` scale (bottom, top) is for deep notches
/// instead, drawn a point per half pixel and exactly at each of `marks` (dotted there).
fn curve(ui: &mut egui::Ui, db_at: impl Fn(f64) -> f64, fixed: Option<(f64, f64)>, marks: &[f64]) {
    use egui::{pos2, vec2, Color32, Stroke};
    let (lo, hi) = (20.0f64, 20_000.0f64);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 150.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let points = if fixed.is_some() {
        (width * 2.0) as usize
    } else {
        240
    };
    let mut samples: Vec<(f64, f64)> = (0..=points)
        .map(|k| {
            let hz = lo * (hi / lo).powf(k as f64 / points as f64);
            (hz, db_at(hz))
        })
        .collect();
    samples.extend(
        marks
            .iter()
            .filter(|hz| (lo..=hi).contains(*hz))
            .map(|&hz| (hz, db_at(hz))),
    );
    samples.sort_by(|a, b| a.0.total_cmp(&b.0));
    // Scaled to the boosts and cuts, not to a high- or low-pass falling away below −18 dB.
    let peak = samples
        .iter()
        .map(|s| s.1)
        .filter(|db| *db > -18.0)
        .map(f64::abs)
        .fold(0.0, f64::max);
    let range = [6.0, 12.0, 24.0, 48.0]
        .into_iter()
        .find(|r| peak <= *r)
        .unwrap_or(96.0);
    let (bottom, top) = fixed.unwrap_or((-range, range));
    let x_of = |hz: f64| rect.left() + ((hz / lo).ln() / (hi / lo).ln()) as f32 * rect.width();
    let y_of = |db: f64| {
        let unit = ((db.clamp(bottom, top) - bottom) / (top - bottom)) as f32;
        rect.bottom() - 6.0 - unit * (rect.height() - 12.0)
    };
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    for hz in [100.0, 1000.0, 10_000.0] {
        painter.vline(x_of(hz), rect.y_range(), grid);
        let label = if hz >= 1000.0 {
            format!("{}k", hz / 1000.0)
        } else {
            format!("{hz}")
        };
        painter.text(
            pos2(x_of(hz) + 3.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    let lines: Vec<f64> = if fixed.is_some() {
        (0..)
            .map(|k| top - top.rem_euclid(20.0) - 20.0 * f64::from(k))
            .take_while(|db| *db > bottom)
            .collect()
    } else {
        vec![-range / 2.0, 0.0, range / 2.0]
    };
    for db in lines {
        painter.hline(rect.x_range(), y_of(db), grid);
        painter.text(
            pos2(rect.left() + 3.0, y_of(db) - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:+.0} dB"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    let line: Vec<_> = samples
        .iter()
        .map(|&(hz, db)| pos2(x_of(hz), y_of(db)))
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(2.0, Color32::from_rgb(120, 200, 255)),
    ));
    for &hz in marks.iter().filter(|hz| (lo..=hi).contains(*hz)) {
        painter.circle_filled(
            pos2(x_of(hz), y_of(db_at(hz))),
            3.0,
            Color32::from_rgb(230, 190, 90),
        );
    }
}

/// A noise print's level by frequency, 20 Hz to 20 kHz on a log scale, −120 to 0 dB (a
/// full-scale sine reads 0 dB in its bin), drawn under the effect's curve.
fn print_curve(ui: &mut egui::Ui, db_at: impl Fn(f64) -> f64) {
    use egui::{pos2, vec2, Color32, Stroke};
    let (lo, hi) = (20.0f64, 20_000.0f64);
    let (bottom, top) = (-120.0f64, 0.0f64);
    let width = ui.available_width().max(300.0);
    let (rect, _) = ui.allocate_exact_size(vec2(width, 120.0), egui::Sense::hover());
    crate::marks::put(ui, "effect.curve", rect);
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 3, theme::LANE);
    let x_of = |hz: f64| rect.left() + ((hz / lo).ln() / (hi / lo).ln()) as f32 * rect.width();
    let y_of = |db: f64| {
        rect.bottom() - ((db.clamp(bottom, top) - bottom) / (top - bottom)) as f32 * rect.height()
    };
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    for (hz, label) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        painter.vline(x_of(hz), rect.y_range(), grid);
        painter.text(
            pos2(x_of(hz) + 3.0, rect.bottom() - 2.0),
            egui::Align2::LEFT_BOTTOM,
            label,
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    for db in [-90.0, -60.0, -30.0] {
        painter.hline(rect.x_range(), y_of(db), grid);
        painter.text(
            pos2(rect.left() + 3.0, y_of(db) - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{db:.0} dB"),
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    let points = 240;
    let line: Vec<_> = (0..=points)
        .map(|k| {
            let hz = lo * (hi / lo).powf(f64::from(k) / f64::from(points));
            pos2(x_of(hz), y_of(db_at(hz)))
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(1.5, Color32::from_rgb(230, 190, 90)),
    ));
}

/// A dynamics effect's static curve: output level against a steady input level, both −60 to
/// 0 dBFS, with the unity line for comparison.
fn transfer_curve(ui: &mut egui::Ui, out_at: impl Fn(f64) -> f64) {
    use egui::{pos2, vec2, Color32, Stroke};
    let (lo, hi) = (-60.0f64, 0.0f64);
    let side = 170.0;
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width().max(side), side),
        egui::Sense::hover(),
    );
    crate::marks::put(ui, "effect.curve", rect);
    let plot = egui::Rect::from_min_size(rect.min, vec2(side, side));
    let painter = ui.painter_at(rect);
    painter.rect_filled(plot, 3, theme::LANE);
    let x_of = |db: f64| plot.left() + ((db - lo) / (hi - lo)) as f32 * plot.width();
    let y_of =
        |db: f64| plot.bottom() - ((db.clamp(lo, hi) - lo) / (hi - lo)) as f32 * plot.height();
    let grid = Stroke::new(1.0, theme::GRID_BAR);
    for db in [-48.0, -36.0, -24.0, -12.0] {
        painter.vline(x_of(db), plot.y_range(), grid);
        painter.hline(plot.x_range(), y_of(db), grid);
    }
    painter.line_segment([pos2(x_of(lo), y_of(lo)), pos2(x_of(hi), y_of(hi))], grid);
    let line: Vec<_> = (0..=120)
        .map(|k| {
            let input = lo + (hi - lo) * f64::from(k) / 120.0;
            pos2(x_of(input), y_of(out_at(input)))
        })
        .collect();
    painter.add(egui::Shape::line(
        line,
        Stroke::new(2.0, Color32::from_rgb(120, 200, 255)),
    ));
    for (text, at) in [("-48", -48.0), ("-24", -24.0), ("0 dBFS in", 0.0)] {
        painter.text(
            pos2(x_of(at) - 2.0, plot.bottom() - 2.0),
            egui::Align2::RIGHT_BOTTOM,
            text,
            egui::FontId::proportional(10.0),
            theme::TEXT_DIM,
        );
    }
    painter.text(
        pos2(plot.left() + 3.0, plot.top() + 2.0),
        egui::Align2::LEFT_TOP,
        "out",
        egui::FontId::proportional(10.0),
        theme::TEXT_DIM,
    );
}
