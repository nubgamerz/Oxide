//! View > Metadata in the Waveform view: the open file's tags (title, artist, album…) and
//! cover art, edited in a draft and applied as one step of the file's history
//! (`Session::set_document_metadata`), so typing never fills the history; Save and Export
//! write them.

use crate::app::DawApp;
use crate::theme;
use daw_core::metadata::{Cover, Metadata};
use daw_core::session::DocumentId;
use std::sync::Arc;

/// The panel while it is open: the file, the tags it showed when last in step with the file,
/// the draft being edited, and the cover's thumbnail.
pub struct MetadataPanel {
    document: DocumentId,
    base: Arc<Metadata>,
    draft: Metadata,
    thumbnail: Option<(*const Vec<u8>, egui::TextureHandle)>,
}

impl MetadataPanel {
    pub fn open(app: &DawApp, document: DocumentId) -> Option<Self> {
        let doc = app.session.document(document)?;
        Some(Self {
            document,
            base: doc.metadata.clone(),
            draft: (*doc.metadata).clone(),
            thumbnail: None,
        })
    }
}

/// A cover image as a texture, decoded once per image.
fn thumbnail(ctx: &egui::Context, cover: &Cover) -> Option<egui::TextureHandle> {
    let image = image::load_from_memory(&cover.data)
        .ok()?
        .thumbnail(256, 256);
    let rgba = image.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    let pixels = egui::ColorImage::from_rgba_unmultiplied(size, rgba.as_raw());
    Some(ctx.load_texture("cover-art", pixels, egui::TextureOptions::LINEAR))
}

pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    let Some(panel) = app.metadata_panel.as_mut() else {
        return;
    };
    let Some(doc) = app.session.document(panel.document) else {
        app.metadata_panel = None;
        return;
    };
    // The file's tags changed (undo, an agent): follow them unless the draft is being edited.
    if !Arc::ptr_eq(&doc.metadata, &panel.base) {
        if panel.draft == *panel.base {
            panel.draft = (*doc.metadata).clone();
        }
        panel.base = doc.metadata.clone();
    }
    let name = doc
        .path
        .file_name()
        .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
    let changed = panel.draft != *panel.base;
    let mut open = true;
    let (mut apply, mut revert, mut choose) = (false, false, false);
    let shown = theme::dialog_window(format!("Metadata: {name}"), ctx)
        .open(&mut open)
        .default_width(460.0)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                egui::Grid::new("metadata-fields")
                    .num_columns(2)
                    .spacing([8.0, 4.0])
                    .show(ui, |ui| {
                        let d = &mut panel.draft;
                        for (label, field) in [
                            ("Title", &mut d.title),
                            ("Artist", &mut d.artist),
                            ("Album", &mut d.album),
                            ("Album artist", &mut d.album_artist),
                            ("Year", &mut d.year),
                            ("Track", &mut d.track),
                            ("Genre", &mut d.genre),
                            ("Composer", &mut d.composer),
                        ] {
                            ui.label(label);
                            ui.add(egui::TextEdit::singleline(field).desired_width(230.0));
                            ui.end_row();
                        }
                        ui.label("Comment");
                        ui.add(
                            egui::TextEdit::multiline(&mut d.comment)
                                .desired_rows(2)
                                .desired_width(230.0),
                        );
                        ui.end_row();
                    });
                ui.vertical(|ui| {
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(128.0, 128.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(rect, 4, egui::Color32::from_gray(0x28));
                    match &panel.draft.cover {
                        Some(cover) => {
                            let key = Arc::as_ptr(&cover.data);
                            if panel.thumbnail.as_ref().map(|t| t.0) != Some(key) {
                                panel.thumbnail = thumbnail(ui.ctx(), cover).map(|t| (key, t));
                            }
                            match &panel.thumbnail {
                                Some((_, texture)) => {
                                    egui::Image::new(texture)
                                        .fit_to_exact_size(rect.size())
                                        .paint_at(ui, rect);
                                }
                                None => {
                                    ui.painter().text(
                                        rect.center(),
                                        egui::Align2::CENTER_CENTER,
                                        "can't show it",
                                        egui::FontId::proportional(11.0),
                                        theme::TEXT_DIM,
                                    );
                                }
                            }
                            ui.label(theme::hint(format!(
                                "{}, {} KB",
                                cover.mime,
                                cover.data.len().div_ceil(1024)
                            )));
                        }
                        None => {
                            ui.painter().text(
                                rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "No cover",
                                egui::FontId::proportional(12.0),
                                theme::TEXT_DIM,
                            );
                        }
                    }
                    ui.horizontal(|ui| {
                        choose = ui.small_button("Choose image…").clicked();
                        if panel.draft.cover.is_some() && ui.small_button("Remove").clicked() {
                            panel.draft.cover = None;
                        }
                    });
                });
            });
            ui.add_space(4.0);
            ui.add(
                egui::Label::new(theme::hint(
                    "Written by Save and Export: ID3 in an MP3, Vorbis comments in FLAC and Ogg, \
                     INFO and ID3 chunks in a WAV, ID3 in an AIFF. Apply is a step of the file's \
                     history.",
                ))
                .wrap(),
            );
            ui.add_space(4.0);
            theme::button_row(ui, |ui| {
                apply = ui
                    .add_enabled(changed, theme::primary_button("Apply"))
                    .clicked();
                revert = ui
                    .add_enabled(changed, egui::Button::new("Revert"))
                    .clicked();
            });
        });
    if let Some(shown) = &shown {
        crate::marks::put_rect("metadata", shown.response.rect);
    }
    if choose {
        let dialog = app
            .dialog_in(None)
            .add_filter("Image (JPEG, PNG)", &["jpg", "jpeg", "png"]);
        let document = app.metadata_panel.as_ref().map(|p| p.document);
        crate::file_dialog::pick_file(app, dialog, move |app, path| {
            match Cover::from_path(&path) {
                // Into the draft it was chosen for, if that is still open.
                Ok(cover) => {
                    if let Some(panel) = app
                        .metadata_panel
                        .as_mut()
                        .filter(|p| Some(p.document) == document)
                    {
                        panel.draft.cover = Some(cover);
                    }
                }
                Err(e) => app.toast(format!("{e:#}")),
            }
        });
    }
    let Some(panel) = app.metadata_panel.as_mut() else {
        return;
    };
    if revert {
        panel.draft = (*panel.base).clone();
    }
    if apply {
        let (document, draft) = (panel.document, panel.draft.clone());
        if let Err(e) = app.session.set_document_metadata(document, draft) {
            app.toast(format!("{e:#}"));
        }
    }
    if !open {
        app.metadata_panel = None;
    }
}
