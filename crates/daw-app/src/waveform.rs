//! The Waveform view (dev plan 10): one audio file on its own. The files themselves are the session's documents; this module keeps how each is
//! viewed (zoom, scroll, selection, playhead, all in the file's own samples) and draws it.
//! Playback is the session's preview, through the engine's output and never in an export.

use crate::app::DawApp;
use crate::theme;
use crate::zoom::MIN_SPP;
use daw_core::api::{SpectralMode, TimeDisplay, ViewMode};
use daw_core::audio_edit::Edit;
use daw_core::grid::{Division, Grid};
use daw_core::peaks::{Place, Wave};
use daw_core::project::{FadeShape, InsertId};
use daw_core::session::{DocumentId, InsertTarget, LoadKind};
use daw_core::spectral_edit::{SpectralBox, SpectralOp};
use daw_core::spectrum::{self, Spectrogram};
use daw_core::tempo::{LineKind, TempoMap};
use egui::{pos2, vec2, Align2, Color32, FontId, Rect, Sense, Stroke};
use std::collections::HashMap;
use std::path::Path;

const RULER_HEIGHT: f32 = 22.0;

/// How one open file is viewed. Positions are the file's own samples.
#[derive(Debug, Clone, Default)]
pub struct DocView {
    /// The first sample at the left edge.
    pub start: f64,
    /// Samples per point; 0 until the first frame fits the file to the width.
    pub spp: f64,
    /// The selection, if any, and the one channel it covers (`None`: all of them).
    pub selection: Option<(u64, u64)>,
    pub channel: Option<usize>,
    /// Where playback starts when there's no selection.
    pub playhead: u64,
    /// A box on the spectrogram (time and band) and the one channel it covers (`None`: all),
    /// for the spectral repairs. It and the time selection replace each other.
    pub boxed: Option<(SpectralBox, Option<usize>)>,
}

/// What a drag on the spectrogram selects: a time range across every
/// frequency (T), or a box in time and frequency (E). Drags on the waveform always select time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Time,
    Box,
}

/// The Waveform view's state: which file it shows, and how each open file is viewed.
#[derive(Debug, Default)]
pub struct Editor {
    pub current: Option<DocumentId>,
    pub views: HashMap<DocumentId, DocView>,
    /// How the ruler and the clock count: the file's time, its samples, or bars and beats
    /// (17.4).
    pub time: TimeDisplay,
    /// A selection being dragged: where it began (a sample) and its channel (Alt).
    drag: Option<(u64, Option<usize>)>,
    /// A box being dragged: where it began (a sample and a frequency), the lane's channel, and
    /// whether it covers only that channel (Alt).
    box_drag: Option<(u64, f64, usize, bool)>,
    pub tool: Tool,
    /// The width of the waveform last drawn, for zooming from outside a frame (actions).
    pub width: f32,
    /// An effect's settings or a question, while one is open.
    pub dialog: Option<EditDialog>,
    /// The waveform, a spectrogram, or both.
    pub spectral: SpectralMode,
    /// The spectrogram's colour range, dB below full scale.
    pub range_db: f32,
    spectro: Spectro,
    /// Apply asked while the rack is synced with the file's track (21.2): the file and the
    /// range, until the question is answered.
    pub apply_question: Option<(DocumentId, Option<(u64, u64)>)>,
}

impl Editor {
    pub fn view(&mut self, id: DocumentId) -> &mut DocView {
        self.views.entry(id).or_default()
    }
}

/// Open `path` and show it in the Waveform view.
pub fn open_file(app: &mut DawApp, path: &Path) {
    match app.session.open_document(path) {
        Ok(id) => show_document(app, id),
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// Open an audio clip's playing take in the Waveform view (double-click on a clip), which
/// remembers the clip and its track (the rack can sync with it, 21.2).
pub fn open_clip(app: &mut DawApp, clip: daw_core::project::ClipId) {
    if app.session.project.find_clip(clip).is_none() {
        return;
    }
    match app.session.open_clip_document(clip) {
        Ok(id) => show_document(app, id),
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// Switch to the Waveform view on document `id`.
pub fn show_document(app: &mut DawApp, id: DocumentId) {
    app.editor.current = Some(id);
    app.editor.view(id);
    app.view_mode = ViewMode::Waveform;
}

/// Play the current file from the selection (to its end) or from the playhead, or stop it
/// (Space, or the transport's Play in the Waveform view).
pub fn toggle_preview(app: &mut DawApp) {
    if app.session.preview_state().is_some() {
        app.session.stop_preview();
        return;
    }
    let Some(id) = app.editor.current else {
        return;
    };
    let view = app.editor.view(id).clone();
    let (start, end) = match view.selection {
        Some((a, b)) => (a, Some(b)),
        None => (view.playhead, None),
    };
    let played = app.session.preview_document(id, start, end);
    app.report(played);
}

/// The position the transport bar shows in the Waveform view: where the preview plays, or the
/// playhead; with the file's sample rate.
pub fn clock(app: &mut DawApp) -> Option<(u64, u32)> {
    let id = app.editor.current?;
    let rate = app.session.document(id)?.audio.sample_rate;
    let at = match app.session.preview_state() {
        Some((playing, at)) if playing == id => at,
        _ => app.editor.view(id).playhead,
    };
    Some((at, rate))
}

/// Zoom the current file to fit, or by a step around its playhead.
pub fn zoom(app: &mut DawApp, step: Option<bool>) {
    let Some(id) = app.editor.current else {
        return;
    };
    let Some(len) = app.session.document(id).map(|d| d.audio.len() as f64) else {
        return;
    };
    let width = f64::from(app.editor.width.max(100.0));
    let view = app.editor.view(id);
    match step {
        None => {
            view.spp = (len / width).max(MIN_SPP);
            view.start = 0.0;
        }
        Some(zoom_in) => {
            let anchor = view.playhead as f64;
            let x = (anchor - view.start) / view.spp.max(1e-9);
            let factor = if zoom_in { 0.5 } else { 2.0 };
            view.spp = (view.spp * factor).clamp(MIN_SPP, (len / width).max(1.0));
            view.start = (anchor - x * view.spp).clamp(0.0, (len - width * view.spp).max(0.0));
        }
    }
}

pub fn show(app: &mut DawApp, ui: &mut egui::Ui) {
    let Some(id) = app
        .editor
        .current
        .filter(|id| app.session.document(*id).is_some())
    else {
        app.editor.current = None;
        empty(app, ui);
        return;
    };
    // Unset (the derived default): 100 dB.
    if app.editor.range_db <= 0.0 {
        app.editor.range_db = 100.0;
    }
    toolbar(app, ui, id);
    let Some(doc) = app.session.document(id) else {
        return;
    };
    let audio = doc.audio.clone();
    let len = audio.len() as f64;
    let area = ui.available_rect_before_wrap();
    let ruler = Rect::from_min_size(area.min, vec2(area.width(), RULER_HEIGHT));
    let waves = Rect::from_min_max(pos2(area.left(), ruler.bottom()), area.max);
    app.editor.width = waves.width();
    let playing = app
        .session
        .preview_state()
        .filter(|(doc, _)| *doc == id)
        .map(|(_, at)| at);
    // In bars and beats, the song's tempo map from the file's start, at the file's own rate,
    // and the song's grid.
    let bars = (app.editor.time == TimeDisplay::BarsBeats)
        .then(|| TempoMap::of(&app.session.project, audio.sample_rate));
    let grid = app.session.project.grid;
    let samples_ruler = app.editor.time == TimeDisplay::Samples;
    let view = app.editor.view(id);
    if view.spp <= 0.0 {
        view.spp = (len / f64::from(waves.width().max(1.0))).max(MIN_SPP);
        view.start = 0.0;
    }
    // While playing, keep the playback in view.
    if let Some(at) = playing {
        let end = view.start + f64::from(waves.width()) * view.spp;
        if (at as f64) < view.start || at as f64 > end {
            view.start = at as f64;
        }
    }
    let max_spp = (len / f64::from(waves.width().max(1.0))).max(1.0);
    view.spp = view.spp.clamp(MIN_SPP, max_spp);
    view.start = view
        .start
        .clamp(0.0, (len - f64::from(waves.width()) * view.spp).max(0.0));
    let (start, spp) = (view.start, view.spp);
    let x_of = |sample: f64| waves.left() + ((sample - start) / spp) as f32;
    let sample_of = |x: f32| (start + f64::from(x - waves.left()) * spp).clamp(0.0, len);

    let painter = ui.painter_at(area);
    painter.rect_filled(area, 0, theme::LANE);
    // The grid's lines across the view in bars and beats (x and kind), for the ruler and the
    // waveform lanes.
    let lines: Vec<(f32, LineKind)> = bars
        .as_ref()
        .map(|map| {
            let end = start + f64::from(waves.width()) * spp;
            let from = map.tick_at(start).floor().max(0.0) as u64;
            let to = map.tick_at(end).ceil().max(0.0) as u64;
            map.grid_lines(from, to, grid, &grid_x(map, spp))
                .into_iter()
                .map(|l| (x_of(map.sample_at_tick(l.tick as f64)), l.kind))
                .collect()
        })
        .unwrap_or_default();
    match &bars {
        Some(map) => draw_bars_ruler(&painter, ruler, start, spp, map, &lines),
        None => draw_ruler(
            &painter,
            ruler,
            start,
            spp,
            audio.sample_rate,
            samples_ruler,
        ),
    }

    // The lanes: each channel's waveform, spectrogram, or both.
    let channels = audio.channels.len().max(1);
    let lanes = lanes(waves, channels, app.editor.spectral);
    let view = app.editor.view(id).clone();
    if let Some(&(_, lane, _)) = lanes.iter().find(|l| l.2) {
        spectro_update(app, ui.ctx(), &audio, start, spp, lane.size());
    }
    for &(c, lane, spectral) in &lanes {
        // The manual's marks (27.5): the first channel's waveform and spectrogram.
        crate::marks::put(
            ui,
            if spectral {
                "waveform.spectrogram"
            } else {
                "waveform.wave"
            },
            lane,
        );
        if spectral {
            draw_spectrogram(app, &painter, lane, c, start, spp, audio.sample_rate);
        } else {
            painter.rect_filled(
                lane,
                0,
                if c % 2 == 0 {
                    theme::LANE
                } else {
                    theme::LANE_ALT
                },
            );
        }
        // The selection, on every channel or the one it covers: a light fill on the
        // waveform, an outline on the spectrogram (a fill would wash its colours out).
        if let Some((a, b)) = view.selection {
            if view.channel.is_none_or(|ch| ch == c) {
                let band = Rect::from_x_y_ranges(
                    x_of(a as f64).max(lane.left())..=x_of(b as f64).min(lane.right()),
                    lane.y_range(),
                );
                crate::marks::put(ui, "waveform.selection", band);
                if spectral {
                    painter.rect(
                        band,
                        0,
                        Color32::from_white_alpha(18),
                        Stroke::new(1.0, Color32::from_white_alpha(200)),
                        egui::StrokeKind::Inside,
                    );
                } else {
                    painter.rect_filled(band, 0, Color32::from_white_alpha(40));
                }
            }
        }
        if let (true, Some((b, only))) = (spectral, view.boxed) {
            if only.is_none_or(|ch| ch == c) {
                let rate = audio.sample_rate;
                let band = Rect::from_x_y_ranges(
                    x_of(b.start as f64).max(lane.left())..=x_of(b.end as f64).min(lane.right()),
                    y_of_hz(lane, b.high_hz, rate)..=y_of_hz(lane, b.low_hz, rate),
                );
                crate::marks::put(ui, "waveform.box", band);
                painter.rect(
                    band,
                    0,
                    Color32::from_rgba_unmultiplied(120, 200, 255, 28),
                    Stroke::new(1.5, Color32::from_rgb(120, 200, 255)),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if !spectral {
            for &(x, kind) in &lines {
                painter.vline(
                    x,
                    lane.y_range(),
                    Stroke::new(1.0, crate::arrange::line_color(kind)),
                );
            }
            painter.hline(
                lane.x_range(),
                lane.center().y,
                Stroke::new(1.0, theme::GRID_BAR),
            );
            draw_channel(&painter, lane, &audio.wave(c), start, spp);
        }
        painter.hline(
            lane.x_range(),
            lane.bottom() - 0.5,
            Stroke::new(1.0, theme::BG),
        );
    }
    // The playhead, and the playback while it plays.
    let head = x_of(view.playhead as f64);
    painter.vline(head, waves.y_range(), Stroke::new(1.0, theme::TEXT_DIM));
    if let Some(at) = playing {
        painter.vline(
            x_of(at as f64),
            area.y_range(),
            Stroke::new(1.5, theme::PLAYHEAD),
        );
        ui.ctx().request_repaint();
    }

    // Input: drag to select (Alt: one channel), click to place the playhead, wheel to scroll,
    // Ctrl+wheel (or pinch) to zoom around the pointer.
    let response = ui.allocate_rect(waves, Sense::click_and_drag());
    let channel_at = |y: f32| {
        lanes
            .iter()
            .find(|l| l.1.y_range().contains(y))
            .map_or(0, |l| l.0)
    };
    let spectral_lane_at = |y: f32| {
        lanes
            .iter()
            .find(|l| l.2 && l.1.y_range().contains(y))
            .copied()
    };
    let rate = audio.sample_rate;
    if app.editor.tool == Tool::Box {
        if let Some(p) = response.hover_pos() {
            if spectral_lane_at(p.y).is_some() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
    }
    if response.drag_started() {
        if let Some(p) = ui.input(|i| i.pointer.press_origin()) {
            let alt = ui.input(|i| i.modifiers.alt);
            match spectral_lane_at(p.y).filter(|_| app.editor.tool == Tool::Box) {
                Some((c, lane, _)) => {
                    app.editor.box_drag = Some((
                        sample_of(p.x).round() as u64,
                        hz_at(lane, p.y, rate),
                        c,
                        alt,
                    ));
                }
                None => {
                    app.editor.drag =
                        Some((sample_of(p.x).round() as u64, alt.then(|| channel_at(p.y))));
                }
            }
        }
    }
    if let (true, Some((anchor, anchor_hz, c, alt)), Some(p)) = (
        response.dragged(),
        app.editor.box_drag,
        response.interact_pointer_pos(),
    ) {
        if let Some(&(_, lane, _)) = lanes.iter().find(|l| l.2 && l.0 == c) {
            let here = sample_of(p.x).round() as u64;
            let hz = hz_at(lane, p.y, rate);
            let view = app.editor.view(id);
            view.boxed = (here != anchor).then(|| {
                (
                    SpectralBox {
                        start: anchor.min(here),
                        end: anchor.max(here),
                        low_hz: anchor_hz.min(hz),
                        high_hz: anchor_hz.max(hz),
                    },
                    alt.then_some(c),
                )
            });
            view.selection = None;
            view.channel = None;
            view.playhead = anchor.min(here);
        }
    }
    if response.dragged() && app.editor.box_drag.is_none() {
        if let (Some((anchor, channel)), Some(p)) =
            (app.editor.drag, response.interact_pointer_pos())
        {
            // Near or past either edge the view scrolls with the selection's end, as the
            // arrangement does with a dragged playhead; the end stays on screen.
            let speed = crate::zoom::edge_scroll(p.x, waves.left(), waves.right());
            let dt = ui.input(|i| i.stable_dt).min(0.1);
            let view = app.editor.view(id);
            let before = view.start;
            if speed != 0.0 {
                view.start = (view.start + f64::from(speed * dt) * view.spp).max(0.0);
                ui.ctx().request_repaint();
            }
            let shift = view.start - before;
            let x = p.x.clamp(waves.left(), waves.right());
            let here = (sample_of(x) + shift).max(0.0).round() as u64;
            let view = app.editor.view(id);
            view.selection = (here != anchor).then(|| (anchor.min(here), anchor.max(here)));
            view.channel = channel;
            view.playhead = anchor.min(here);
            view.boxed = None;
        }
    }
    if response.drag_stopped() {
        app.editor.drag = None;
        if app.editor.box_drag.take().is_some() {
            // A box as thin as a line selects nothing.
            let view = app.editor.view(id);
            if view.boxed.is_some_and(|(b, _)| b.high_hz <= b.low_hz) {
                view.boxed = None;
            }
        }
    }
    if response.clicked() {
        if let Some(p) = response.interact_pointer_pos() {
            let view = app.editor.view(id);
            view.playhead = sample_of(p.x).round() as u64;
            view.selection = None;
            view.channel = None;
            view.boxed = None;
        }
    }
    if response.hovered() {
        let (scroll, zoom, pointer) =
            ui.input(|i| (i.smooth_scroll_delta, i.zoom_delta(), i.pointer.hover_pos()));
        let view = app.editor.view(id);
        if zoom != 1.0 {
            if let Some(p) = pointer {
                let anchor = sample_of(p.x);
                view.spp = (view.spp / f64::from(zoom)).clamp(MIN_SPP, max_spp);
                view.start = (anchor - f64::from(p.x - waves.left()) * view.spp).max(0.0);
            }
        } else if scroll != egui::Vec2::ZERO {
            let delta = if scroll.x != 0.0 { scroll.x } else { scroll.y };
            view.start = (view.start - f64::from(delta) * view.spp).max(0.0);
        }
    }
}

/// With no file open: how to open one.
fn empty(app: &mut DawApp, ui: &mut egui::Ui) {
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() / 3.0);
            ui.label(
                egui::RichText::new(
                    "No audio file open. Double-click an audio clip in the Multitrack view, \
                     or open a file.",
                )
                .color(theme::TEXT_DIM),
            );
            if ui.button("Open audio file…").clicked() {
                choose_file(app);
            }
        });
    });
}

/// File > Open audio file…
pub fn choose_file(app: &mut DawApp) {
    let dialog = app
        .dialog_in(None)
        .add_filter("Audio", &["wav", "flac", "mp3", "aif", "aiff"]);
    crate::file_dialog::pick_file(app, dialog, |app, path| open_file(app, &path));
}

/// The file switcher, its facts, the selection's length and the zoom buttons.
fn toolbar(app: &mut DawApp, ui: &mut egui::Ui, id: DocumentId) {
    let docs: Vec<(DocumentId, String)> = app
        .session
        .documents()
        .iter()
        .map(|d| {
            let name: String = d.path.file_name().map_or_else(
                || d.path.display().to_string(),
                |n| n.to_string_lossy().into(),
            );
            // An asterisk marks unsaved edits.
            (
                d.id,
                if d.is_dirty() {
                    format!("{name} *")
                } else {
                    name
                },
            )
        })
        .collect();
    let Some(doc) = app.session.document(id) else {
        return;
    };
    let (rate, channels, len) = (
        doc.audio.sample_rate,
        doc.audio.channels.len(),
        doc.audio.len(),
    );
    let path = doc.path.display().to_string();
    let mut close = false;
    // In a narrow pane the view's controls go on a line of their own, under the file's.
    let width = ui.available_width();
    let narrow = width < 900.0;
    let row = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        if narrow {
            ui.horizontal_wrapped(|ui| add(ui));
        } else {
            ui.horizontal(|ui| add(ui));
        }
    };
    row(ui, &mut |ui| {
        let current = docs
            .iter()
            .find(|d| d.0 == id)
            .map_or(String::new(), |d| d.1.clone());
        egui::ComboBox::from_id_salt("waveform-file")
            .selected_text(current)
            .width(220.0)
            .show_ui(ui, |ui| {
                for (doc, name) in &docs {
                    if ui.selectable_label(*doc == id, name).clicked() {
                        app.editor.current = Some(*doc);
                    }
                }
            })
            .response
            .on_hover_text(&path);
        crate::marks::put(ui, "waveform.file", ui.min_rect());
        if ui
            .small_button("✕")
            .on_hover_text("Close this file")
            .clicked()
        {
            close = true;
        }
        ui.separator();
        edit_menus(app, ui, id);
        ui.separator();
        let seconds = |n: u64| n as f64 / f64::from(rate.max(1));
        let shape = match channels {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            n => format!("{n} channels"),
        };
        // Sample counts on hover, so the line fits beside the rack and the display menus.
        // Whole, on the next line if it doesn't fit (a narrow pane).
        let info = ui
            .add(
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{rate} Hz · {shape} · {:.3} s",
                        seconds(len as u64)
                    ))
                    .color(theme::TEXT_DIM),
                )
                .wrap_mode(egui::TextWrapMode::Extend),
            )
            .on_hover_text(format!("{len} samples"));
        crate::marks::put(ui, "waveform.info", info.rect);
        if let Some((b, _)) = app.editor.view(id).boxed {
            ui.label(
                egui::RichText::new(format!(
                    "Box {:.3}–{:.3} s · {}–{} Hz",
                    seconds(b.start),
                    seconds(b.end),
                    b.low_hz.round(),
                    b.high_hz.round()
                ))
                .color(theme::PLAYHEAD),
            )
            .on_hover_text(format!(
                "{}–{}: {} samples",
                b.start,
                b.end,
                b.end - b.start
            ));
        }
        if let Some((a, b)) = app.editor.view(id).selection {
            ui.label(
                egui::RichText::new(format!("Selection {:.3}–{:.3} s", seconds(a), seconds(b)))
                    .color(theme::PLAYHEAD),
            )
            .on_hover_text(format!("{a}–{b}: {} samples", b - a));
        }
        if !narrow {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                view_controls(app, ui)
            });
        }
    });
    if narrow {
        let line = vec2(width, ui.spacing().interact_size.y);
        ui.allocate_ui_with_layout(
            line,
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| view_controls(app, ui),
        );
    }
    if close {
        close_file(app, id);
    }
}

/// The toolbar's display controls, right to left: zoom, the ruler's units, the spectral tools
/// and the display.
fn view_controls(app: &mut DawApp, ui: &mut egui::Ui) {
    let fit = ui.small_button("Fit").on_hover_text("Whole file (F)");
    if fit.clicked() {
        zoom(app, None);
    }
    let zoom_in = ui.small_button("+").on_hover_text("Zoom in (=)");
    if zoom_in.clicked() {
        zoom(app, Some(true));
    }
    let zoom_out = ui.small_button("−").on_hover_text("Zoom out (-)");
    if zoom_out.clicked() {
        zoom(app, Some(false));
    }
    // The manual's marks (27.5); the controls run right to left.
    crate::marks::put(ui, "waveform.zoom", fit.rect.union(zoom_out.rect));
    let shown = match app.editor.time {
        TimeDisplay::Time => "Time",
        TimeDisplay::Samples => "Samples",
        TimeDisplay::BarsBeats => "Bars and beats",
    };
    let time = egui::ComboBox::from_id_salt("waveform-time")
        .selected_text(shown)
        .width(110.0)
        .show_ui(ui, |ui| {
            let time = &mut app.editor.time;
            ui.selectable_value(time, TimeDisplay::Time, "Time");
            ui.selectable_value(time, TimeDisplay::Samples, "Samples");
            ui.selectable_value(time, TimeDisplay::BarsBeats, "Bars and beats");
        })
        .response;
    crate::marks::put(ui, "waveform.time", time.rect);
    time.on_hover_text(
        "Time display: the file's time, its samples, or bars and beats from its start \
             (the song's tempo, meter and grid)",
    );
    if app.editor.spectral != SpectralMode::Off {
        let boxed = ui
            .selectable_value(&mut app.editor.tool, Tool::Box, "Box")
            .on_hover_text(
                "Drag a box in time and frequency on the spectrogram (E), to attenuate, \
                 remove or heal it (Effects)",
            );
        let time = ui
            .selectable_value(&mut app.editor.tool, Tool::Time, "Time")
            .on_hover_text("Drag a time range on the spectrogram, as on the waveform (T)");
        crate::marks::put(ui, "waveform.tool", boxed.rect.union(time.rect));
        let range = ui
            .add(
                egui::DragValue::new(&mut app.editor.range_db)
                    .range(20.0..=200.0)
                    .speed(1.0)
                    .suffix(" dB"),
            )
            .on_hover_text("The spectrogram's range: this far below full scale is black");
        crate::marks::put(ui, "waveform.range", range.rect);
    }
    let shown = match app.editor.spectral {
        SpectralMode::Off => "Waveform",
        SpectralMode::Spectral => "Spectral",
        SpectralMode::Both => "Both",
    };
    let display = egui::ComboBox::from_id_salt("waveform-display")
        .selected_text(shown)
        .width(90.0)
        .show_ui(ui, |ui| {
            ui.selectable_value(&mut app.editor.spectral, SpectralMode::Off, "Waveform");
            ui.selectable_value(&mut app.editor.spectral, SpectralMode::Spectral, "Spectral");
            ui.selectable_value(&mut app.editor.spectral, SpectralMode::Both, "Both");
        })
        .response;
    crate::marks::put(ui, "waveform.display", display.rect);
    display.on_hover_text("Show the waveform, the spectral frequency display, or both");
}

/// Close file `id` (its toolbar's button or its tab's): at once, or after asking whether to
/// save its edits.
pub fn close_file(app: &mut DawApp, id: DocumentId) {
    let dirty = app.session.document(id).is_some_and(|d| d.is_dirty());
    if dirty {
        // The question is about the current file.
        app.editor.current = Some(id);
        app.view_mode = ViewMode::Waveform;
        app.editor.dialog = Some(EditDialog::ConfirmClose);
    } else {
        let closed = app.session.close_document(id, false);
        app.report(closed);
        app.editor.views.remove(&id);
        if app.editor.current == Some(id) {
            app.editor.current = app.session.documents().first().map(|d| d.id);
        }
    }
}

/// Where song tick `tick` is in a file shown at `spp` samples a point, `map` at the file's
/// rate: the zoom the grid's lines are thinned by and Auto picks its division by, as the
/// Multitrack's (`arrange::grid_x`).
fn grid_x(map: &TempoMap, spp: f64) -> impl Fn(f64) -> f64 + '_ {
    move |tick| map.sample_at_tick(tick) / spp
}

/// The song grid's division the Waveform view shows for file `id` where its view starts, in
/// bars and beats (none in the other time displays). Auto's follows this view's own zoom.
pub fn grid_division(app: &DawApp, id: DocumentId) -> Option<Division> {
    if app.editor.time != TimeDisplay::BarsBeats {
        return None;
    }
    let rate = app.session.document(id)?.audio.sample_rate;
    let view = app.editor.views.get(&id)?;
    if view.spp <= 0.0 {
        return None;
    }
    Some(bars_division(
        &app.session.project,
        rate,
        view.start,
        view.spp,
    ))
}

/// The division the bars-and-beats display shows for a file at `rate` viewed from `start`
/// at `spp` samples a point: the song's grid in the bar it starts in.
pub(crate) fn bars_division(
    project: &daw_core::project::Project,
    rate: u32,
    start: f64,
    spp: f64,
) -> Division {
    let map = TempoMap::of(project, rate);
    let bar = map.bar_at(start.max(0.0)).0;
    let grid: Grid = project.grid;
    let division = map.division_in(grid, bar, &grid_x(&map, spp));
    division
}

/// The ruler in bars and beats: the grid's lines (`lines`, drawn taller for beats and bars),
/// bar numbers kept apart, and bar.beat where beats have room.
fn draw_bars_ruler(
    painter: &egui::Painter,
    rect: Rect,
    start: f64,
    spp: f64,
    map: &TempoMap,
    lines: &[(f32, LineKind)],
) {
    painter.rect_filled(rect, 0, theme::BG);
    let (first, _) = map.bar_at(start.max(0.0));
    let bar_px = (map.bar_start(first + 1) - map.bar_start(first)) / spp;
    let mut every = 1i64;
    while bar_px * (every as f64) < 48.0 && every < 1 << 20 {
        every *= 2;
    }
    let beat_px = {
        let at = map.beats_at(start.max(0.0)).floor();
        (map.sample_of_beat(at + 1.0) - map.sample_of_beat(at)) / spp
    };
    let sample_at = |x: f32| start + f64::from(x - rect.left()) * spp;
    for &(x, kind) in lines {
        if x < rect.left() || x > rect.right() {
            continue;
        }
        let height = match kind {
            LineKind::Bar => rect.height(),
            LineKind::Beat => 6.0,
            LineKind::Division => 3.0,
        };
        painter.vline(
            x,
            (rect.bottom() - height)..=rect.bottom(),
            Stroke::new(1.0, theme::GRID_BAR),
        );
        let (bar, beats) = map.bar_at(sample_at(x) + spp * 0.5);
        let label = match kind {
            LineKind::Bar if bar.rem_euclid(every) == 0 => Some(((bar + 1).to_string(), 10.0)),
            LineKind::Beat if beat_px >= 50.0 => {
                Some((format!("{}.{}", bar + 1, beats.floor() as i64 + 1), 9.0))
            }
            _ => None,
        };
        if let Some((text, size)) = label {
            painter.text(
                pos2(x + 3.0, rect.top() + 3.0),
                Align2::LEFT_TOP,
                text,
                FontId::monospace(size),
                theme::TEXT_DIM,
            );
        }
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::GRID_BAR),
    );
}

/// The ruler: time (or samples) at a spacing that keeps labels apart.
fn draw_ruler(painter: &egui::Painter, rect: Rect, start: f64, spp: f64, rate: u32, samples: bool) {
    painter.rect_filled(rect, 0, theme::BG);
    let rate = f64::from(rate.max(1));
    // A step (in samples) of 1, 2 or 5 times a power of ten (seconds, or samples), at least
    // 70 points apart.
    let unit = if samples { 1.0 } else { rate };
    let min_step = 70.0 * spp / unit;
    let mut step = 1e-4;
    'find: for exp in -4..=6 {
        for m in [1.0, 2.0, 5.0] {
            let s = m * 10f64.powi(exp);
            if s >= min_step && (!samples || s >= 1.0) {
                step = s;
                break 'find;
            }
        }
    }
    let step_samples = step * unit;
    let end = start + f64::from(rect.width()) * spp;
    let mut k = (start / step_samples).floor();
    while k * step_samples <= end {
        let at = k * step_samples;
        let x = rect.left() + ((at - start) / spp) as f32;
        if x >= rect.left() {
            painter.vline(
                x,
                (rect.bottom() - 6.0)..=rect.bottom(),
                Stroke::new(1.0, theme::GRID_BAR),
            );
            let label = if samples {
                format!("{}", at.round() as u64)
            } else {
                let decimals = (-step.log10()).ceil().max(0.0) as usize;
                format!("{:.*}", decimals, at / rate)
            };
            painter.text(
                pos2(x + 3.0, rect.top() + 3.0),
                Align2::LEFT_TOP,
                label,
                FontId::monospace(10.0),
                theme::TEXT_DIM,
            );
        }
        k += 1.0;
    }
    painter.hline(
        rect.x_range(),
        rect.bottom() - 0.5,
        Stroke::new(1.0, theme::GRID_BAR),
    );
}

/// One channel's waveform in `lane` from sample `start` at `spp` samples a point, as the
/// Multitrack draws clips ([`crate::wave`]): columns from the peaks, or the samples themselves
/// (a line, dots once they are far apart) zoomed in past one a point.
fn draw_channel(painter: &egui::Painter, lane: Rect, wave: &Wave, start: f64, spp: f64) {
    let place = Place {
        origin: f64::from(lane.left()) - start / spp,
        spp,
        from: 0,
        to: wave.len() as u64,
        left: lane.left(),
        right: lane.right(),
    };
    let look = crate::wave::Look {
        mid: lane.center().y,
        half: lane.height() / 2.0 - 2.0,
        color: theme::ACCENT,
    };
    crate::wave::paint(painter, wave, &place, look, |_| 1.0);
}

// ------------------------------------------------------------------------------ editing

/// A dialog the Waveform view has open: an effect's settings, or a question before saving
/// over a file or closing one with unsaved edits.
#[derive(Debug, Clone)]
pub enum EditDialog {
    Amplify {
        db: f64,
    },
    Normalize {
        loudness: bool,
        peak_db: f64,
        lufs: f64,
    },
    InsertSilence {
        seconds: f64,
    },
    SpectralAttenuate {
        db: f64,
    },
    ConfirmSave,
    ConfirmClose,
}

/// The selection as an edit's range and channels, or `None` for the whole file.
fn selection(app: &mut DawApp, id: DocumentId) -> (Option<(u64, u64)>, Option<Vec<usize>>) {
    let view = app.editor.view(id);
    (view.selection, view.channel.map(|c| vec![c]))
}

/// Repair what is inside the spectral box; the box stays, to compare with undo.
fn spectral(app: &mut DawApp, id: DocumentId, op: SpectralOp) {
    if let Some((b, only)) = app.editor.view(id).boxed {
        apply(app, spectral_edit(b, only, op));
    }
}

fn spectral_edit(b: SpectralBox, only: Option<usize>, op: SpectralOp) -> Edit {
    Edit::Spectral {
        range: (b.start, b.end),
        channels: only.map(|c| vec![c]),
        low_hz: b.low_hz,
        high_hz: b.high_hz,
        op,
    }
}

/// The frequency at height `y` in a spectral lane: the inverse of [`y_of_hz`], the same log
/// scale the spectrogram is drawn on (its bottom edge is 0 Hz, its top the Nyquist frequency).
fn hz_at(lane: Rect, y: f32, rate: u32) -> f64 {
    let nyquist = f64::from(rate) / 2.0;
    let t = 1.0 - f64::from(((y - lane.top()) / lane.height().max(1.0)).clamp(0.0, 1.0));
    if t <= 0.0 {
        return 0.0;
    }
    (spectrum::MIN_HZ * (nyquist / spectrum::MIN_HZ).powf(t)).min(nyquist)
}

/// The height of `hz` in a spectral lane, as the spectrogram's labels are placed.
fn y_of_hz(lane: Rect, hz: f64, rate: u32) -> f32 {
    let height = lane.height().max(1.0) as usize;
    lane.top() + spectrum::hz_row(hz, height, rate) * lane.height() / height as f32
}

/// Have the rack's first effect that learns from audio learn the selection (the noise print
/// for Noise and Hiss Reduction, Clip Repair's scan) and show it.
pub fn capture_noise_print(app: &mut DawApp, id: DocumentId) {
    let (Some(range), _) = selection(app, id) else {
        return;
    };
    learn_into(app, id, range, None, None);
}

/// Have `insert` (or, without one, the rack's first effect that learns from audio) learn
/// `range` of document `id`, by `scan` for an effect with several (the Click & Pop Repair
/// Eliminator's), open its editor and say what it learned.
pub fn learn_into(
    app: &mut DawApp,
    id: DocumentId,
    range: (u64, u64),
    insert: Option<InsertId>,
    scan: Option<&str>,
) {
    match app.session.learn_from(id, range, insert, scan) {
        Ok(insert) => {
            let open = app.session.insert_info(insert).is_some_and(|i| i.2);
            if !open {
                let shown = app.session.toggle_editor(insert);
                app.report(shown);
            }
            let kind = app.session.native(insert).map(|n| n.kind.id);
            app.toast(match kind {
                Some("clip-repair") => "Clip levels scanned".to_string(),
                Some("click-pop-repair") => format!(
                    "{} done: the levels (and thresholds) are set",
                    scan.unwrap_or("Scan For Threshold Levels")
                ),
                _ => "Noise print captured".to_string(),
            });
        }
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// The current file's selection in the Waveform view, if there is one: what an effect's
/// editor can learn from.
pub fn current_selection(app: &mut DawApp) -> Option<(DocumentId, (u64, u64))> {
    let id = app.editor.current?;
    app.session.document(id)?;
    let (range, _) = selection(app, id);
    Some((id, range?))
}

/// Apply an edit to the current file and move the selection and playhead to what it made.
pub fn apply(app: &mut DawApp, edit: Edit) {
    let Some(id) = app.editor.current else {
        return;
    };
    let after = match &edit {
        Edit::Delete { range } => Some((range.0, None)),
        Edit::Trim { .. } => Some((0, None)),
        Edit::InsertSilence { at, length } => Some((*at, Some((*at, at + length)))),
        Edit::Paste { at, replace, clip } => {
            let start = replace.map_or(*at, |r| r.0);
            let len = clip.channels.first().map_or(0, Vec::len) as u64;
            Some((start, Some((start, start + len))))
        }
        _ => None,
    };
    match app.session.edit_document(id, &edit) {
        Ok(()) => {
            if let Some((playhead, selection)) = after {
                let view = app.editor.view(id);
                view.playhead = playhead;
                view.selection = selection;
                view.channel = None;
            }
        }
        Err(e) => app.toast(format!("{}: {e:#}", edit.name())),
    }
}

/// Undo (or redo) the current file's last edit: its own history, not the project's.
pub fn step(app: &mut DawApp, undo: bool) {
    let Some(id) = app.editor.current else {
        return;
    };
    match app.session.step_document(id, undo) {
        Ok(Some(name)) => app.toast(format!("{} {name}", if undo { "Undid" } else { "Redid" })),
        Ok(None) => {}
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// Copy the selection (Ctrl+C); with `cut`, delete it too (Ctrl+X).
pub fn copy(app: &mut DawApp, cut: bool) {
    let Some(id) = app.editor.current else {
        return;
    };
    let (Some(range), _) = selection(app, id) else {
        app.toast("Select some audio to copy".to_string());
        return;
    };
    match app.session.copy_document(id, range) {
        Ok(_) => {
            // So the next Ctrl+V arrives even if the system clipboard held no text.
            crate::clipboard::announce(app, "Audio copied in Oxide".to_string());
            if cut {
                apply(app, Edit::Delete { range });
            }
        }
        Err(e) => app.toast(format!("{e:#}")),
    }
}

/// Paste over the selection, or at the playhead (Ctrl+V).
pub fn paste(app: &mut DawApp) {
    let Some(id) = app.editor.current else {
        return;
    };
    let Some(clip) = app.session.audio_clipboard().cloned() else {
        app.toast("Nothing has been copied".to_string());
        return;
    };
    let view = app.editor.view(id).clone();
    apply(
        app,
        Edit::Paste {
            at: view.playhead,
            replace: view.selection,
            clip,
        },
    );
}

/// Delete the selection (Delete), or remove what is inside the spectral box.
pub fn delete(app: &mut DawApp) {
    let Some(id) = app.editor.current else {
        return;
    };
    if app.editor.view(id).boxed.is_some() {
        spectral(app, id, SpectralOp::Remove);
        return;
    }
    if let (Some(range), _) = selection(app, id) {
        apply(app, Edit::Delete { range });
    }
}

/// Save (Ctrl+S): asks first, naming the clips that will change.
pub fn ask_save(app: &mut DawApp) {
    if app.editor.current.is_some() {
        app.editor.dialog = Some(EditDialog::ConfirmSave);
    }
}

/// Save As… (Ctrl+Shift+S): a new WAV, which the view then edits.
pub fn save_as(app: &mut DawApp) {
    let Some(id) = app.editor.current else {
        return;
    };
    let Some(doc) = app.session.document(id) else {
        return;
    };
    let stem = doc
        .path
        .file_stem()
        .map_or("audio".into(), |s| s.to_string_lossy().to_string());
    let dir = doc.path.parent().map(Path::to_path_buf);
    let mut dialog = rfd::FileDialog::new()
        .add_filter("WAV audio", &["wav"])
        .set_file_name(format!("{stem} edit.wav"));
    if let Some(dir) = dir {
        dialog = dialog.set_directory(dir);
    }
    crate::file_dialog::save_file(app, dialog, move |app, path| {
        let saved = app.session.save_document_as(id, &path).map(|_| ());
        app.report(saved);
    });
}

/// The Edit, Effects and History menus.
fn edit_menus(app: &mut DawApp, ui: &mut egui::Ui, id: DocumentId) {
    let has_selection = app.editor.view(id).selection.is_some();
    let has_clipboard = app.session.audio_clipboard().is_some();
    let menu = ui.menu_button("Edit", |ui| {
        if ui
            .add_enabled(has_selection, egui::Button::new("Cut  (Ctrl+X)"))
            .clicked()
        {
            copy(app, true);
            ui.close();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("Copy  (Ctrl+C)"))
            .clicked()
        {
            copy(app, false);
            ui.close();
        }
        if ui
            .add_enabled(has_clipboard, egui::Button::new("Paste  (Ctrl+V)"))
            .clicked()
        {
            paste(app);
            ui.close();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("Delete  (Del)"))
            .clicked()
        {
            delete(app);
            ui.close();
        }
        ui.separator();
        if ui
            .add_enabled(has_selection, egui::Button::new("Trim to selection"))
            .clicked()
        {
            if let (Some(range), _) = selection(app, id) {
                apply(app, Edit::Trim { range });
            }
            ui.close();
        }
        if ui.button("Insert silence…").clicked() {
            app.editor.dialog = Some(EditDialog::InsertSilence { seconds: 1.0 });
            ui.close();
        }
        if ui
            .button("Silence")
            .on_hover_text("The selection (or the whole file) to silence")
            .clicked()
        {
            let (range, channels) = selection(app, id);
            apply(app, Edit::Silence { range, channels });
            ui.close();
        }
        ui.separator();
        if ui.button("Save  (Ctrl+S)").clicked() {
            ask_save(app);
            ui.close();
        }
        if ui.button("Save As…  (Ctrl+Shift+S)").clicked() {
            save_as(app);
            ui.close();
        }
    });
    crate::marks::put(ui, "waveform.menu.edit", menu.response.rect);
    let menu = ui.menu_button("Effects", |ui| {
        if ui.button("Amplify…").clicked() {
            app.editor.dialog = Some(EditDialog::Amplify { db: 3.0 });
            ui.close();
        }
        if ui.button("Normalize…").clicked() {
            app.editor.dialog = Some(EditDialog::Normalize {
                loudness: false,
                peak_db: -1.0,
                lufs: -16.0,
            });
            ui.close();
        }
        ui.separator();
        for (label, fade_in, shape) in [
            ("Fade in (linear)", true, FadeShape::Linear),
            ("Fade in (equal power)", true, FadeShape::EqualPower),
            ("Fade out (linear)", false, FadeShape::Linear),
            ("Fade out (equal power)", false, FadeShape::EqualPower),
        ] {
            if ui
                .add_enabled(has_selection, egui::Button::new(label))
                .on_disabled_hover_text("Select the part to fade")
                .clicked()
            {
                if let (Some(range), channels) = selection(app, id) {
                    apply(
                        app,
                        if fade_in {
                            Edit::FadeIn {
                                range,
                                channels,
                                shape,
                            }
                        } else {
                            Edit::FadeOut {
                                range,
                                channels,
                                shape,
                            }
                        },
                    );
                }
                ui.close();
            }
        }
        ui.separator();
        if ui
            .add_enabled(
                has_selection,
                egui::Button::new("Capture noise print / clip levels"),
            )
            .on_hover_text(
                "Teach the selection to the rack's first effect that learns from audio: Noise \
                 or Hiss Reduction take the noise (select noise alone), Clip Repair the clip \
                 levels (its Scan: select the clipped audio), the Click & Pop Repair its \
                 threshold levels (Scan For All Levels is in its editor). A Noise Reduction is \
                 added when the rack has none.",
            )
            .on_disabled_hover_text("Select the noise alone, or the clipped audio")
            .clicked()
        {
            capture_noise_print(app, id);
            ui.close();
        }
        ui.separator();
        if ui.button("Reverse").clicked() {
            let (range, channels) = selection(app, id);
            apply(app, Edit::Reverse { range, channels });
            ui.close();
        }
        if ui.button("Invert").clicked() {
            let (range, channels) = selection(app, id);
            apply(app, Edit::Invert { range, channels });
            ui.close();
        }
        ui.separator();
        let boxed = app.editor.view(id).boxed.is_some();
        let hint = "Drag a box on the spectrogram first (Box, or E)";
        if ui
            .add_enabled(boxed, egui::Button::new("Attenuate box…"))
            .on_disabled_hover_text(hint)
            .clicked()
        {
            app.editor.dialog = Some(EditDialog::SpectralAttenuate { db: -12.0 });
            ui.close();
        }
        if ui
            .add_enabled(boxed, egui::Button::new("Remove box  (Del)"))
            .on_disabled_hover_text(hint)
            .clicked()
        {
            spectral(app, id, SpectralOp::Remove);
            ui.close();
        }
        if ui
            .add_enabled(boxed, egui::Button::new("Heal box"))
            .on_hover_text("Fill the box from the sound just before and after it")
            .on_disabled_hover_text(hint)
            .clicked()
        {
            spectral(app, id, SpectralOp::Heal);
            ui.close();
        }
    });
    let Some(doc) = app.session.document(id) else {
        return;
    };
    let (steps, showing, saved) = doc.history();
    crate::marks::put(ui, "waveform.menu.effects", menu.response.rect);
    let menu = ui.menu_button("History", |ui| {
        for (k, name) in steps.iter().enumerate() {
            let mark = if Some(k) == saved { "  (saved)" } else { "" };
            let text = egui::RichText::new(format!("{name}{mark}"));
            let text = if k > showing {
                text.color(theme::TEXT_DIM).italics()
            } else {
                text
            };
            if ui.selectable_label(k == showing, text).clicked() {
                // Step to it one edit at a time, as undo and redo do.
                let (from, to) = (showing, k);
                for _ in to..from {
                    step(app, true);
                }
                for _ in from..to {
                    step(app, false);
                }
                ui.close();
            }
        }
    });
    crate::marks::put(ui, "waveform.menu.history", menu.response.rect);
}

/// The open dialog, if any.
pub fn dialogs(app: &mut DawApp, ctx: &egui::Context) {
    let Some(id) = app.editor.current else {
        app.editor.dialog = None;
        return;
    };
    let Some(mut dialog) = app.editor.dialog.clone() else {
        return;
    };
    let Some(doc) = app.session.document(id) else {
        app.editor.dialog = None;
        return;
    };
    let rate = doc.audio.sample_rate;
    let name = doc
        .path
        .file_name()
        .map_or(String::new(), |n| n.to_string_lossy().into());
    let path = doc.path.clone();
    let wav = doc.format.is_some();
    let clips: Vec<String> = app
        .session
        .clips_using(&path)
        .iter()
        .filter_map(|&c| {
            let (track, clip) = app.session.project.find_clip(c)?;
            let track = app.session.project.track(track)?;
            Some(format!("{} on {}", clip.name, track.name))
        })
        .collect();
    let (range, channels) = selection(app, id);
    let mut done: Option<Option<Edit>> = None;
    let mut save = false;
    let mut close = false;
    let modal = crate::theme::modal(ctx, "waveform-dialog").show(ctx, |ui| {
        ui.set_max_width(420.0);
        match &mut dialog {
            EditDialog::Amplify { db } => {
                ui.heading("Amplify");
                ui.add(egui::Slider::new(db, -24.0..=24.0).suffix(" dB"));
                if ok_cancel(ui, &mut done) {
                    done = Some(Some(Edit::Amplify {
                        range,
                        channels: channels.clone(),
                        db: *db,
                    }));
                }
            }
            EditDialog::Normalize {
                loudness,
                peak_db,
                lufs,
            } => {
                ui.heading("Normalize");
                ui.radio_value(loudness, false, "Peak level");
                ui.add_enabled(
                    !*loudness,
                    egui::DragValue::new(peak_db)
                        .range(-60.0..=0.0)
                        .speed(0.1)
                        .suffix(" dBFS"),
                );
                ui.radio_value(loudness, true, "Loudness (integrated, EBU R 128)");
                ui.add_enabled(
                    *loudness,
                    egui::DragValue::new(lufs)
                        .range(-60.0..=0.0)
                        .speed(0.1)
                        .suffix(" LUFS"),
                );
                if ok_cancel(ui, &mut done) {
                    done = Some(Some(if *loudness {
                        Edit::NormalizeLoudness {
                            range,
                            channels: channels.clone(),
                            lufs: *lufs,
                        }
                    } else {
                        Edit::NormalizePeak {
                            range,
                            channels: channels.clone(),
                            peak_db: *peak_db,
                        }
                    }));
                }
            }
            EditDialog::InsertSilence { seconds } => {
                ui.heading("Insert silence");
                ui.label("At the playhead:");
                ui.add(
                    egui::DragValue::new(seconds)
                        .range(0.001..=600.0)
                        .speed(0.01)
                        .suffix(" s"),
                );
                if ok_cancel(ui, &mut done) {
                    let at = app.editor.views.get(&id).map_or(0, |v| v.playhead);
                    done = Some(Some(Edit::InsertSilence {
                        at,
                        length: (*seconds * f64::from(rate)).round().max(1.0) as u64,
                    }));
                }
            }
            EditDialog::SpectralAttenuate { db } => {
                ui.heading("Attenuate box");
                ui.add(egui::Slider::new(db, -60.0..=0.0).suffix(" dB"));
                ui.label(theme::hint(
                    "Turns down what is inside the box: its time range and its band.",
                ));
                if ok_cancel(ui, &mut done) {
                    let boxed = app.editor.views.get(&id).and_then(|v| v.boxed);
                    done = Some(boxed.map(|(b, only)| {
                        spectral_edit(b, only, SpectralOp::Attenuate { db: *db })
                    }));
                }
            }
            EditDialog::ConfirmSave => {
                ui.heading(format!("Save over {name}?"));
                if !wav {
                    ui.label(
                        "This isn't a WAV file, so it can't be written back. Use Save As to \
                         write a WAV.",
                    );
                } else {
                    ui.label(
                        "The file on disk is replaced, in its own format. Undo here, then save \
                         again, puts it back.",
                    );
                    if clips.is_empty() {
                        ui.label(theme::hint("No clip in the project uses it."));
                    } else {
                        ui.label(format!(
                            "{} clip{} will play the new audio:",
                            clips.len(),
                            if clips.len() == 1 { "" } else { "s" }
                        ));
                        for c in clips.iter().take(8) {
                            ui.label(egui::RichText::new(format!("• {c}")).color(theme::TEXT_DIM));
                        }
                    }
                }
                ui.add_space(8.0);
                theme::button_row(ui, |ui| {
                    if ui.add_enabled(wav, theme::primary_button("Save")).clicked() {
                        save = true;
                        done = Some(None);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(None);
                    }
                });
            }
            EditDialog::ConfirmClose => {
                ui.heading(format!("{name} has unsaved edits"));
                ui.label("Close it and lose them?");
                ui.add_space(8.0);
                theme::button_row(ui, |ui| {
                    if ui.button("Close without saving").clicked() {
                        close = true;
                        done = Some(None);
                    }
                    if ui.button("Cancel").clicked() {
                        done = Some(None);
                    }
                });
            }
        }
    });
    crate::marks::put_rect("waveform.dialog", modal.response.rect);
    if modal.should_close() {
        done = Some(None);
    }
    match done {
        None => app.editor.dialog = Some(dialog),
        Some(edit) => {
            app.editor.dialog = None;
            if let Some(edit) = edit {
                apply(app, edit);
            }
            if save {
                let saved = app.session.save_document(id).map(|_| ());
                app.report(saved);
            }
            if close {
                let closed = app.session.close_document(id, true);
                app.report(closed);
                app.editor.views.remove(&id);
                app.editor.current = app.session.documents().first().map(|d| d.id);
            }
        }
    }
}

/// OK and Cancel buttons: returns whether OK was pressed; Cancel ends the dialog in `done`.
fn ok_cancel(ui: &mut egui::Ui, done: &mut Option<Option<Edit>>) -> bool {
    ui.add_space(8.0);
    let mut ok = false;
    theme::button_row(ui, |ui| {
        ok = ui.add(theme::primary_button("OK")).clicked();
        if ui.button("Cancel").clicked() {
            *done = Some(None);
        }
    });
    ok
}

// ------------------------------------------------------------------------------ spectral

/// A spectrogram render: what it was made from and for.
#[derive(Clone)]
struct SpectroKey {
    audio: std::sync::Arc<daw_core::media::AudioData>,
    start: f64,
    spp: f64,
    width: usize,
    height: usize,
}

impl SpectroKey {
    /// The same audio (the same `Arc`: an edit makes a new one) and the same view.
    fn same(&self, other: &SpectroKey) -> bool {
        std::sync::Arc::ptr_eq(&self.audio, &other.audio)
            && self.start == other.start
            && self.spp == other.spp
            && self.width == other.width
            && self.height == other.height
    }
}

/// The spectral display's renders: the one showing (its spectrograms and textures, coloured
/// for a range) and one being made on a worker thread. One job at a time; when it lands and
/// the view has moved, the next starts.
#[derive(Default)]
pub struct Spectro {
    shown: Option<(SpectroKey, Vec<Spectrogram>, Vec<egui::TextureHandle>, f32)>,
    pending: Option<(SpectroKey, std::sync::mpsc::Receiver<Vec<Spectrogram>>)>,
}

impl std::fmt::Debug for Spectro {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Spectro")
            .field("shown", &self.shown.is_some())
            .field("pending", &self.pending.is_some())
            .finish()
    }
}

/// Where each channel's waveform or spectrogram goes: `(channel, rect, spectral)`, stacked by
/// channel; with both, the waveforms above (40%) and the spectrograms below. One list drives the drawing, the selection's shading and the pointer.
fn lanes(area: Rect, channels: usize, mode: SpectralMode) -> Vec<(usize, Rect, bool)> {
    let stack = |rect: Rect, spectral: bool| {
        let h = rect.height() / channels as f32;
        (0..channels).map(move |c| {
            (
                c,
                Rect::from_min_size(
                    pos2(rect.left(), rect.top() + c as f32 * h),
                    vec2(rect.width(), h),
                ),
                spectral,
            )
        })
    };
    match mode {
        SpectralMode::Off => stack(area, false).collect(),
        SpectralMode::Spectral => stack(area, true).collect(),
        SpectralMode::Both => {
            let split = area.top() + area.height() * 0.4;
            let top = Rect::from_min_max(area.min, pos2(area.right(), split));
            let bottom = Rect::from_min_max(pos2(area.left(), split), area.max);
            stack(top, false).chain(stack(bottom, true)).collect()
        }
    }
}

/// Keep the spectrogram in step with the view: take a finished render, and start the next one
/// when the view (or the audio) has changed and none is running. The worker asks for a repaint
/// when it is done, so the result shows without waiting for input.
fn spectro_update(
    app: &mut DawApp,
    ctx: &egui::Context,
    audio: &std::sync::Arc<daw_core::media::AudioData>,
    start: f64,
    spp: f64,
    lane: egui::Vec2,
) {
    let want = SpectroKey {
        audio: audio.clone(),
        start,
        spp,
        width: lane.x.max(1.0) as usize,
        height: lane.y.max(1.0) as usize,
    };
    let range = app.editor.range_db;
    let spectro = &mut app.editor.spectro;
    if let Some((key, rx)) = &spectro.pending {
        if let Ok(renders) = rx.try_recv() {
            let textures = textures(ctx, &renders, range);
            spectro.shown = Some((key.clone(), renders, textures, range));
            spectro.pending = None;
        }
    }
    // A new colour range only re-colours what is there.
    if let Some((_, renders, textures_now, used)) = &mut spectro.shown {
        if *used != range {
            *textures_now = textures(ctx, renders, range);
            *used = range;
        }
    }
    let current = spectro.shown.as_ref().is_some_and(|s| s.0.same(&want));
    if spectro.pending.is_none() && !current {
        let (tx, rx) = std::sync::mpsc::channel();
        let job = want.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let fft = spectrum::fft_size_for(job.spp);
            let renders: Vec<Spectrogram> = job
                .audio
                .channels
                .iter()
                .map(|c| {
                    spectrum::render(
                        c,
                        job.audio.sample_rate,
                        job.start,
                        job.spp,
                        job.width,
                        job.height,
                        fft,
                    )
                })
                .collect();
            let _ = tx.send(renders);
            ctx.request_repaint();
        });
        spectro.pending = Some((want, rx));
    }
}

/// The spectrograms as textures, coloured over `range` dB below full scale.
fn textures(ctx: &egui::Context, renders: &[Spectrogram], range: f32) -> Vec<egui::TextureHandle> {
    renders
        .iter()
        .enumerate()
        .map(|(c, s)| {
            let pixels = s.db.iter().map(|&db| colour(db, range)).collect();
            ctx.load_texture(
                format!("spectrogram-{c}"),
                egui::ColorImage::new([s.width, s.height], pixels),
                egui::TextureOptions::LINEAR,
            )
        })
        .collect()
}

/// A level's colour, from black at `-range` dB through blue, purple, red and orange to
/// pale yellow at 0 dB (a full-scale sine).
fn colour(db: f32, range: f32) -> Color32 {
    const STOPS: [(f32, [u8; 3]); 6] = [
        (0.0, [0, 0, 0]),
        (0.2, [20, 10, 80]),
        (0.45, [120, 20, 140]),
        (0.65, [220, 40, 40]),
        (0.85, [250, 160, 30]),
        (1.0, [255, 255, 210]),
    ];
    let t = ((db + range) / range).clamp(0.0, 1.0);
    let k = STOPS
        .iter()
        .position(|s| s.0 >= t)
        .unwrap_or(STOPS.len() - 1)
        .max(1);
    let (a, b) = (STOPS[k - 1], STOPS[k]);
    let f = (t - a.0) / (b.0 - a.0).max(1e-6);
    let mix = |i: usize| (f32::from(a.1[i]) + (f32::from(b.1[i]) - f32::from(a.1[i])) * f) as u8;
    Color32::from_rgb(mix(0), mix(1), mix(2))
}

/// One channel's spectrogram in `lane`: the latest render, placed where its view falls in this
/// one (so scrolling or zooming moves the old image until the new one lands), then the
/// frequency scale along the right.
fn draw_spectrogram(
    app: &DawApp,
    painter: &egui::Painter,
    lane: Rect,
    channel: usize,
    start: f64,
    spp: f64,
    rate: u32,
) {
    painter.rect_filled(lane, 0, Color32::BLACK);
    if let Some((key, _, textures, _)) = &app.editor.spectro.shown {
        if let Some(texture) = textures.get(channel) {
            let x0 = lane.left() + ((key.start - start) / spp) as f32;
            let width = (key.width as f64 * key.spp / spp) as f32;
            let image = Rect::from_min_size(pos2(x0, lane.top()), vec2(width, lane.height()));
            painter.with_clip_rect(lane).image(
                texture.id(),
                image,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }
    }
    let nyquist = f64::from(rate) / 2.0;
    let height = lane.height().max(1.0) as usize;
    for (hz, label) in [
        (50.0, "50"),
        (100.0, "100"),
        (200.0, "200"),
        (500.0, "500"),
        (1000.0, "1k"),
        (2000.0, "2k"),
        (5000.0, "5k"),
        (10000.0, "10k"),
        (20000.0, "20k"),
    ] {
        if hz >= nyquist {
            continue;
        }
        let y = lane.top() + spectrum::hz_row(hz, height, rate) * lane.height() / height as f32;
        if y < lane.top() + 8.0 || y > lane.bottom() - 4.0 {
            continue;
        }
        painter.hline(
            (lane.right() - 6.0)..=lane.right(),
            y,
            Stroke::new(1.0, Color32::from_white_alpha(120)),
        );
        painter.text(
            pos2(lane.right() - 8.0, y),
            Align2::RIGHT_CENTER,
            label,
            FontId::monospace(9.0),
            Color32::from_white_alpha(160),
        );
    }
}

/// The effects rack beside the file: the plug-ins the preview
/// plays through, and Apply, which writes them into the selection (or the whole file) as one
/// step of its history. Apply renders exactly what the preview plays.
pub fn rack_panel(app: &mut DawApp, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new("Effects Rack").strong())
            .on_hover_text("Kept as it is when Oxide closes, plug-in settings and all");
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            crate::rack_presets::menu(app, ui);
        });
    });
    sync_row(app, ui);
    ui.add_space(4.0);
    let footer = 92.0;
    let height = (ui.available_height() - footer).max(60.0);
    ui.allocate_ui(vec2(ui.available_width(), height), |ui| {
        crate::arrange::fx_rack(app, ui, InsertTarget::Editor, height);
    });
    ui.separator();
    let current = app.editor.current;
    let selection = current.and_then(|id| app.editor.view(id).selection);
    let rack = app.session.editor_rack();
    let latency: usize = rack
        .iter()
        .filter_map(|s| app.session.insert_info(s.id).map(|(_, latency, _)| latency))
        .sum();
    let empty = rack.is_empty();
    let dim = |s: String| egui::RichText::new(s).small().color(theme::TEXT_DIM);
    ui.label(dim(if empty {
        "Add plug-ins above to hear them on the file".into()
    } else {
        format!("Latency {latency} samples, compensated")
    }));
    ui.label(dim(match (current, selection) {
        (None, _) => "No file open".into(),
        (Some(_), Some(_)) => "Process: the selection".into(),
        (Some(_), None) => "Process: the entire file".into(),
    }));
    let busy = app.session.apply_state().is_some();
    let apply = ui.add_enabled(
        !empty && current.is_some() && !busy,
        egui::Button::new("Apply").min_size(vec2(90.0, 0.0)),
    );
    crate::marks::put(ui, "waveform.rack.apply", apply.rect);
    let apply = apply
        .on_hover_text(
            "Write the rack into the selection (every channel), or the whole file without one: \
             exactly what the preview plays. One step of the file's history; Save writes it.",
        )
        .on_disabled_hover_text(if empty {
            "The rack is empty"
        } else if busy {
            "Applying…"
        } else {
            "No file open"
        });
    if apply.clicked() {
        if let Some(id) = current {
            // Synced with this file's track, the track would play what Apply prints: ask.
            if app.session.rack_sync().is_some_and(|i| i.document == id) {
                app.editor.apply_question = Some((id, selection));
            } else {
                let applied = app.session.apply_effects(id, selection);
                app.report(applied);
            }
        }
    }
}

/// "Sync with <track>" above the rack (21.2): for a file opened from a clip, the rack shows
/// that clip's track's effects and changes reach both views; off, the user's own rack is back.
/// Disabled, with why, for a file opened on its own.
fn sync_row(app: &mut DawApp, ui: &mut egui::Ui) {
    let current = app.editor.current;
    let target = current.and_then(|id| Some((id, app.session.sync_track_of(id)?)));
    let track_name = |app: &DawApp, track| {
        app.session
            .project
            .track(track)
            .map_or_else(|| "its track".to_string(), |t| t.name.clone())
    };
    let busy = app.session.apply_state().is_some()
        || app
            .session
            .loading()
            .is_some_and(|l| l.kind != LoadKind::Rack);
    match target {
        Some((doc, (track, on))) => {
            let name = track_name(app, track);
            let mut checked = on;
            let toggled = ui.add_enabled(
                !busy,
                theme::checkbox(&mut checked, format!("Sync with {name}")),
            );
            crate::marks::put(ui, "waveform.rack.sync", toggled.rect);
            let toggled = toggled
                .on_hover_text(format!(
                    "The rack shows {name}'s effects, so the file plays as its clip does in the \
                     Multitrack. Adding, removing, moving, bypassing or turning an effect here \
                     does the same on {name}, and the other way. Off: your own rack comes back."
                ))
                .on_disabled_hover_text("Wait for the plug-ins loading, or the Apply, to finish");
            if toggled.changed() {
                let done = app.session.set_rack_sync(doc, checked).map(|_| ());
                app.report(done);
            }
            if on {
                ui.label(theme::hint(format!(
                    "Changes here reach {name} in the Multitrack, and back"
                )));
            }
        }
        None => {
            let mut off = false;
            ui.add_enabled(
                false,
                theme::checkbox(&mut off, "Sync with the clip's track"),
            )
            .on_disabled_hover_text(
                "Only for a file opened from a clip: double-click the clip in the Multitrack",
            );
        }
    }
    // Synced through another file than the one showing.
    if let Some(info) = app
        .session
        .rack_sync()
        .filter(|i| Some(i.document) != current)
    {
        let file = app
            .session
            .document(info.document)
            .and_then(|d| d.path.file_name())
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let name = track_name(app, info.track);
        ui.label(theme::hint(format!(
            "The rack is synced with {name} (through {file})"
        )));
    }
}

/// `--show rack-sync` / `rack-sync-off`: the demo's first clip (Guitar L) opened in the
/// Waveform view with a selection, Guitar L given an Amplify, a Parametric EQ
/// (bypassed) and the `compressor` VST3 if one is given and installed (`--vst3`; else, and
/// in a neutral run, the built-in Compressor), and the user's own rack a Room Reverb; with `on`, the rack synced with Guitar L.
pub fn show_rack_sync(app: &mut DawApp, on: bool) -> anyhow::Result<()> {
    let (track, clip) = app
        .session
        .project
        .tracks
        .iter()
        .find_map(|t| t.clips.first().map(|c| (t.id, c.id)))
        .ok_or_else(|| anyhow::anyhow!("the demo has no audio clip"))?;
    let mut chain = vec![
        daw_core::native::path_of(daw_core::native::find("Amplify").expect("built in")),
        daw_core::native::path_of(daw_core::native::find("parametric-eq").expect("built in")),
    ];
    if let Some(path) = app.demo_effect("compressor", "Compressor") {
        chain.push(path);
    }
    for (k, path) in chain.iter().enumerate() {
        let id = app.session.add_insert(InsertTarget::Track(track), path)?;
        if k == 1 {
            if let Some(slot) = app.session.insert_mut(id) {
                slot.bypass = true;
            }
        }
    }
    let reverb =
        daw_core::native::path_of(daw_core::native::find("room-reverb").expect("built in"));
    app.session.add_insert(InsertTarget::Editor, &reverb)?;
    open_clip(app, clip);
    let id = app
        .editor
        .current
        .ok_or_else(|| anyhow::anyhow!("the clip didn't open"))?;
    let rate = app
        .session
        .document(id)
        .map_or(48_000, |d| u64::from(d.audio.sample_rate));
    let view = app.editor.view(id);
    view.selection = Some((rate, rate * 3));
    view.playhead = rate;
    if on {
        app.session.set_rack_sync(id, true)?;
    }
    Ok(())
}

/// Apply asked while the rack is synced with the file's track (21.2): printing the effects into
/// the file, which the track then plays through them again, so the question first.
pub fn apply_question(app: &mut DawApp, ctx: &egui::Context) {
    let Some((id, selection)) = app.editor.apply_question else {
        return;
    };
    let Some(track) = app
        .session
        .rack_sync()
        .filter(|i| i.document == id)
        .and_then(|i| app.session.project.track(i.track))
        .map(|t| t.name.clone())
    else {
        app.editor.apply_question = None;
        return;
    };
    let mut answer = None;
    let modal = theme::modal(ctx, "apply-synced").show(ctx, |ui| {
        ui.set_width(440.0);
        ui.heading(format!("Apply {track}'s effects into the file?"));
        ui.add_space(4.0);
        ui.label(format!(
            "The rack is synced with {track}, which plays this clip through the same effects: \
             once they are in the file, {track} would play them a second time."
        ));
        ui.label(theme::hint(format!(
            "Turning them off on {track} is one undo step; the Apply is a step of the file's \
             own history, and Save writes it."
        )));
        ui.add_space(8.0);
        theme::button_row(ui, |ui| {
            let primary = theme::primary_button(&format!("Apply and turn off on {track}"));
            if ui.add(primary).clicked() {
                answer = Some(true);
            }
            if ui.button("Apply anyway").clicked() {
                answer = Some(false);
            }
            if ui.button("Cancel").clicked() {
                app.editor.apply_question = None;
            }
        });
    });
    if modal.should_close() {
        app.editor.apply_question = None;
    }
    if let Some(turn_off) = answer {
        app.editor.apply_question = None;
        let applied = if turn_off {
            app.session.apply_effects_turning_off(id, selection)
        } else {
            app.session.apply_effects(id, selection)
        };
        app.report(applied);
    }
}

/// While an Apply renders: its progress, and Cancel.
pub fn apply_progress(app: &mut DawApp, ctx: &egui::Context) {
    let Some((_, progress)) = app.session.apply_state() else {
        return;
    };
    crate::theme::modal(ctx, "apply-effects").show(ctx, |ui| {
        ui.set_width(320.0);
        ui.label("Applying the effects rack…");
        theme::progress(ui, progress, false);
        ui.label(theme::hint(
            "Rendered through the same plug-ins, settle and block size as the preview.",
        ));
        theme::button_row(ui, |ui| {
            if ui.button("Cancel").clicked() {
                app.session.cancel_apply();
            }
        });
    });
    ctx.request_repaint();
}
