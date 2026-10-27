//! The CPU panel: each track's and insert's share of the audio deadline, opened by clicking
//! the DSP readout. Its numbers are the session's reading (`Session::cpu_usage`), the same
//! as `get_cpu_usage` returns.

use crate::app::{DawApp, Selection};
use crate::theme;
use daw_core::usage::Load;
use egui::{Color32, RichText};

/// What the DSP readout's hover says.
pub const DSP_TIP: &str = "DSP: the time spent processing each audio buffer, as a share of \
    the time the driver allows for it (the buffer's duration). The bar shows it; the white \
    line and \"peak\" are the worst buffer of the last two seconds, and over 100 % that buffer \
    was late: a dropout.\n\nThere is load while stopped: every track and plug-in keeps running \
    on silence, so effects' tails ring out and input monitoring works.\n\nClick for each \
    track's and plug-in's share.";

pub fn window(app: &mut DawApp, ctx: &egui::Context) {
    if !app.show_cpu {
        return;
    }
    let stats = app.session.stats();
    let rate = app.session.sample_rate();
    let usage = app.session.cpu_usage().clone();
    let mut open = true;
    let mut pick = None;
    let shown = egui::Window::new("CPU use")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(420.0)
        .default_pos(ctx.content_rect().right_top() + egui::vec2(-460.0, 80.0))
        // A tool window: kept under the menu bar like the others.
        .constrain_to(crate::theme::below_menu(ctx))
        .show(ctx, |ui| {
            ui.label(
                RichText::new(format!(
                    "{} {} samples ({:.2} ms) · {} worker thread{} · averaged over {:.1} s",
                    // Gathered from smaller device buffers (23.1): the figures are per block.
                    if stats.device_block > 0 && stats.device_block < stats.block_size {
                        "Block"
                    } else {
                        "Buffer"
                    },
                    stats.block_size,
                    stats.block_size as f64 / f64::from(rate.max(1)) * 1e3,
                    stats.workers,
                    if stats.workers == 1 { "" } else { "s" },
                    usage.window,
                ))
                .size(theme::HINT_SIZE)
                .color(theme::TEXT_DIM),
            );
            let whole = ui.label(
                RichText::new(format!(
                    "Whole app: {:.0} % of one core, every thread",
                    usage.process * 100.0
                ))
                .size(theme::HINT_SIZE),
            );
            crate::marks::put(ui, "cpu.whole_app", whole.rect);
            whole.on_hover_text(
                "All of Oxide's CPU time, every thread: the audio threads below, the window, \
                 and what plug-ins do on threads of their own (a sampler's disk streaming, \
                 an analyser's metering), which the figures below can't see. A bypassed plug-in \
                 isn't called, so its figure below is nothing; its own threads usually go \
                 quiet too, and that shows here.",
            );
            ui.separator();
            let list = egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    egui::Grid::new("cpu-use")
                        .num_columns(4)
                        .spacing([10.0, 3.0])
                        .striped(true)
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new("Track / plug-in")
                                    .size(theme::HINT_SIZE)
                                    .strong(),
                            );
                            ui.label(RichText::new("per buffer").size(theme::HINT_SIZE).strong());
                            ui.label(RichText::new("of deadline").size(theme::HINT_SIZE).strong());
                            ui.label("");
                            ui.end_row();
                            for track in &usage.tracks {
                                let name = ui
                                    .add(
                                        egui::Label::new(RichText::new(&track.name).strong())
                                            .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text(
                                        "Everything the track does: its clips, plug-ins, \
                                         fader, sends and delay compensation. Click to select \
                                         it.",
                                    );
                                if name.clicked() {
                                    pick = Some(track.track);
                                }
                                row(ui, &track.load, theme::ACCENT);
                                for insert in &track.inserts {
                                    let name = if insert.bypass {
                                        format!("    {} (bypassed)", insert.name)
                                    } else {
                                        format!("    {}", insert.name)
                                    };
                                    let label =
                                        ui.label(RichText::new(name).color(theme::TEXT_DIM));
                                    if insert.bypass {
                                        label.on_hover_text(
                                            "Bypassed: it isn't called, so it costs nothing \
                                             here (its delay compensation still runs).",
                                        );
                                    }
                                    row(ui, &insert.load, theme::METER_MID);
                                }
                            }
                        });
                });
            crate::marks::put(ui, "cpu.list", list.inner_rect);
            ui.separator();
            ui.label(
                RichText::new(
                    "Most costly first. Tracks run side by side on the audio threads, so their \
                     shares can add up past 100 % without a dropout: the DSP readout is the \
                     whole buffer's. Tracks and plug-ins keep running while stopped.",
                )
                .size(theme::HINT_SIZE)
                .color(theme::TEXT_DIM),
            );
        });
    if let Some(shown) = shown {
        crate::marks::put_rect("cpu", shown.response.rect);
        let (layer, rect) = (shown.response.layer_id, shown.response.rect);
        app.help.note(ctx, crate::help::Place::Cpu, layer, rect);
    }
    app.show_cpu = open;
    if let Some(track) = pick {
        app.selection = Some(track.map_or(Selection::Master, Selection::Track));
    }
}

/// A reading's time per buffer, share of the deadline, and a bar of the share.
fn row(ui: &mut egui::Ui, load: &Load, color: Color32) {
    ui.label(RichText::new(format!("{:>7.0} µs", load.per_buffer_us)).monospace());
    ui.label(RichText::new(format!("{:>5.1} %", load.share * 100.0)).monospace());
    let (rect, _) = ui.allocate_exact_size(egui::vec2(90.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2, Color32::from_gray(0x30));
    let mut fill = rect;
    fill.set_width(rect.width() * (load.share as f32).clamp(0.0, 1.0));
    ui.painter().rect_filled(fill, 2, color);
    ui.end_row();
}
