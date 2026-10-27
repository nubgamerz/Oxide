//! The export format controls, shared by the multitrack export dialog and the Waveform view's
//! (File > Export file…): format, its settings, sample rate and dither. The choice is kept in
//! the settings (`AppSettings::export_encoding`), so the next export starts from it.

use crate::app::DawApp;
use crate::theme;
use daw_core::encode::{Encoding, Format, MP3_BITRATES};
use daw_core::session::DocumentId;
use egui::RichText;

/// The formats offered, each with the settings it starts from when picked.
const FORMATS: [(&str, Format); 5] = [
    ("WAV", Format::Wav { bits: 24 }),
    ("AIFF", Format::Aiff { bits: 24 }),
    ("FLAC", Format::Flac { bits: 24, level: 5 }),
    (
        "MP3",
        Format::Mp3 {
            bitrate: Some(320),
            vbr: None,
        },
    ),
    ("Ogg Vorbis", Format::Ogg { quality: 6 }),
];

const RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

fn name(format: &Format) -> &'static str {
    match format {
        Format::Wav { .. } => "WAV",
        Format::Aiff { .. } => "AIFF",
        Format::Flac { .. } => "FLAC",
        Format::Mp3 { .. } => "MP3",
        Format::Ogg { .. } => "Ogg Vorbis",
    }
}

/// The controls for `encoding`, for audio at `rate` Hz. Returns why it can't be written, if
/// it can't (shown under the controls; the caller disables its button).
pub fn controls(ui: &mut egui::Ui, encoding: &mut Encoding, rate: u32) -> Option<String> {
    let grid = egui::Grid::new("export-format")
        .num_columns(2)
        .spacing([10.0, 6.0])
        .show(ui, |ui| {
            ui.label("Format");
            egui::ComboBox::from_id_salt("export-format-kind")
                .selected_text(name(&encoding.format))
                .show_ui(ui, |ui| {
                    for (label, start) in FORMATS {
                        let current = name(&encoding.format) == label;
                        if ui.selectable_label(current, label).clicked() && !current {
                            encoding.format = start;
                        }
                    }
                });
            ui.end_row();
            match &mut encoding.format {
                Format::Wav { bits } => {
                    ui.label("Samples");
                    ui.horizontal(|ui| {
                        ui.selectable_value(bits, 16, "16-bit");
                        ui.selectable_value(bits, 24, "24-bit");
                        ui.selectable_value(bits, 32, "32-bit float");
                    });
                    ui.end_row();
                }
                Format::Aiff { bits } => {
                    ui.label("Samples");
                    ui.horizontal(|ui| {
                        ui.selectable_value(bits, 16, "16-bit");
                        ui.selectable_value(bits, 24, "24-bit");
                    });
                    ui.end_row();
                }
                Format::Flac { bits, level } => {
                    ui.label("Samples");
                    ui.horizontal(|ui| {
                        ui.selectable_value(bits, 16, "16-bit");
                        ui.selectable_value(bits, 24, "24-bit");
                    });
                    ui.end_row();
                    ui.label("Compression");
                    ui.add(egui::Slider::new(level, 0..=8))
                        .on_hover_text("0 is fastest, 8 smallest; all are lossless");
                    ui.end_row();
                }
                Format::Mp3 { bitrate, vbr } => {
                    ui.label("Bitrate");
                    ui.horizontal(|ui| {
                        let mut variable = vbr.is_some();
                        ui.selectable_value(&mut variable, false, "Constant");
                        ui.selectable_value(&mut variable, true, "Variable");
                        if variable && vbr.is_none() {
                            (*bitrate, *vbr) = (None, Some(0));
                        } else if !variable && bitrate.is_none() {
                            (*bitrate, *vbr) = (Some(320), None);
                        }
                    });
                    ui.end_row();
                    match vbr {
                        Some(q) => {
                            ui.label("Quality");
                            ui.add(egui::Slider::new(q, 0..=9).prefix("V"))
                                .on_hover_text("V0 is the best (about 245 kbps), V9 the smallest");
                        }
                        None => {
                            ui.label("kbps");
                            let kbps = bitrate.get_or_insert(320);
                            egui::ComboBox::from_id_salt("export-mp3-kbps")
                                .selected_text(format!("{kbps} kbps"))
                                .show_ui(ui, |ui| {
                                    for b in MP3_BITRATES {
                                        ui.selectable_value(kbps, b, format!("{b} kbps"));
                                    }
                                });
                        }
                    }
                    ui.end_row();
                }
                Format::Ogg { quality } => {
                    ui.label("Quality");
                    ui.add(egui::Slider::new(quality, 0..=10))
                        .on_hover_text("0 is the smallest file, 10 the best sound");
                    ui.end_row();
                }
            }
            ui.label("Sample rate");
            let shown = match encoding.sample_rate {
                None => format!("As it is ({rate} Hz)"),
                Some(r) => format!("{r} Hz"),
            };
            egui::ComboBox::from_id_salt("export-rate")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut encoding.sample_rate,
                        None,
                        format!("As it is ({rate} Hz)"),
                    );
                    for r in RATES.into_iter().filter(|r| *r != rate) {
                        ui.selectable_value(&mut encoding.sample_rate, Some(r), format!("{r} Hz"));
                    }
                });
            ui.end_row();
            if encoding.format.int_bits().is_some() {
                ui.label("");
                ui.add(crate::theme::checkbox(&mut encoding.dither, "Dither"))
                    .on_hover_text("TPDF dither of one bit, so quiet passages fade into noise rather than distortion");
                ui.end_row();
            }
        });
    // The manual's mark (27.5): format, its settings, the rate and dither.
    crate::marks::put(ui, "export.format", grid.response.rect);
    let problem = encoding.check(rate).err().map(|e| e.to_string());
    if let Some(problem) = &problem {
        ui.label(RichText::new(problem).small().color(theme::MUTE));
    }
    problem
}

/// A file dialog for `encoding`'s format, starting in the project's Exported folder (shown
/// with `file_dialog::save_file`).
pub fn save_dialog(app: &DawApp, encoding: &Encoding, stem: &str) -> rfd::FileDialog {
    let ext = encoding.format.extension();
    app.dialog_in(Some(daw_core::folder::EXPORTED))
        .add_filter(name(&encoding.format), &[ext])
        .set_file_name(format!("{}.{ext}", daw_core::folder::file_name(stem)))
}

/// The Waveform view's File > Export file… dialog, for document `doc`.
pub fn document_dialog(app: &mut DawApp, ctx: &egui::Context) {
    let Some(doc) = app.export_document else {
        return;
    };
    let Some(document) = app.session.document(doc) else {
        app.export_document = None;
        return;
    };
    let rate = document.audio.sample_rate;
    let stem = document
        .path
        .file_stem()
        .map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let mut go = false;
    let mut close = false;
    let modal = crate::theme::modal(ctx, "export-document").show(ctx, |ui| {
        ui.set_max_width(460.0);
        ui.heading(format!("Export {stem}"));
        ui.add(
            egui::Label::new(theme::hint(
                "Writes the file as it is now, unsaved edits included, to a new file; the file \
                 you're editing stays as it is.",
            ))
            .wrap(),
        );
        ui.add_space(6.0);
        let problem = controls(ui, &mut app.settings.export_encoding, rate);
        ui.add_space(10.0);
        theme::button_row(ui, |ui| {
            go = ui
                .add_enabled(problem.is_none(), theme::primary_button("Export…"))
                .clicked();
            close = ui.button("Cancel").clicked();
        });
    });
    crate::marks::put_rect("export", modal.response.rect);
    if go || close || modal.should_close() {
        app.export_document = None;
        app.save_settings();
    }
    if go {
        export_document(app, doc, &stem);
    }
}

fn export_document(app: &mut DawApp, doc: DocumentId, stem: &str) {
    let encoding = app.settings.export_encoding;
    let dialog = save_dialog(app, &encoding, stem);
    crate::file_dialog::save_file(app, dialog, move |app, path| {
        let written = daw_core::encode::with_extension(&path, &encoding.format)
            .and_then(|path| app.session.export_document(doc, &path, encoding));
        if let Err(e) = written {
            app.toast(format!("{e:#}"));
        }
    });
}
