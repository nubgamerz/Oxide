//! The start screen: a new project (a name, a template to start from or none, and the folder
//! it goes in) on the left, and an existing one on the right: Open project… and the recent
//! projects (dev plan 17.7), each a row with its name, where it is, when it was last saved and
//! its template, a missing one greyed with "Remove from the list", and a right-click menu.
//! The app always works inside a project's folder, so it shows whenever no project is open
//! (and can't be dismissed then), and from File > New project.
//!
//! The recent projects' files are looked at on workers (`recent::Probe`, asked for in
//! `logic` while this shows), so a slow or disconnected drive never holds up a frame.

use crate::app::DawApp;
use crate::recent::{self, Entry, Found};
use crate::theme;
use egui::{Color32, RichText};
use std::path::{Path, PathBuf};

pub struct StartScreen {
    /// Opened over a project (File > New project): it can be cancelled.
    pub open: bool,
    name: String,
    parent: String,
    /// The template the new project starts from (`daw_core::template`); none: empty.
    pub template: Option<String>,
    icon: Option<egui::TextureHandle>,
}

impl Default for StartScreen {
    fn default() -> Self {
        Self {
            open: false,
            name: "New project".into(),
            parent: String::new(),
            template: None,
            icon: None,
        }
    }
}

/// The new project's column, and the recent projects' beside it.
const NEW_WIDTH: f32 = 340.0;
const OPEN_WIDTH: f32 = 460.0;
const GAP: f32 = 28.0;
/// The new project's labels.
const LABEL_WIDTH: f32 = 70.0;
const ROW_HEIGHT: f32 = 46.0;

/// Whether the start screen shows: no project open, or File > New project.
pub fn visible(app: &DawApp) -> bool {
    app.session.path.is_none() || app.start.open
}

/// Where new projects go when none has been made yet: a folder on the program's own drive,
/// never the system drive's user folders.
fn suggested_parent() -> PathBuf {
    let app = crate::settings::app_dir();
    let root = app.ancestors().last().map(Path::to_path_buf).unwrap_or(app);
    root.join("Oxide Projects")
}

/// What a recent project's row asks for.
enum RecentAsk {
    Open(PathBuf),
    /// The path, and whether it was found missing.
    Explore(PathBuf, bool),
    Remove(PathBuf),
}

pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    let required = app.session.path.is_none();
    if !visible(app) {
        return;
    }
    if app.start.parent.is_empty() {
        let parent = app
            .settings
            .projects_parent
            .clone()
            .unwrap_or_else(suggested_parent);
        app.start.parent = parent.display().to_string();
        // The manual's shots name no machine's drive (`--neutral`).
        if app.neutral {
            app.start.parent = crate::neutral::PROJECTS_FOLDER.into();
        }
    }
    let icon = app
        .start
        .icon
        .get_or_insert_with(|| {
            let icon = crate::icon::window_icon();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            ctx.load_texture("oxide-icon", image, egui::TextureOptions::LINEAR)
        })
        .clone();
    let mut entries = recent::entries(&app.settings, &app.recent_probe);
    // The manual's shots name no machine's folders (`--neutral`).
    if app.neutral {
        for entry in &mut entries {
            entry.folder = crate::neutral::PROJECTS_FOLDER.into();
        }
    }
    let current = app.session.path.clone();
    let mut create = false;
    let mut browse = false;
    let mut open_other = false;
    let mut asked = None;
    let mut cancel = false;
    let shown = crate::theme::modal(ctx, "start-screen").show(ctx, |ui| {
        ui.set_width(NEW_WIDTH + GAP + OPEN_WIDTH);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&icon).fit_to_exact_size(egui::vec2(48.0, 48.0)));
            ui.vertical(|ui| {
                ui.label(RichText::new("Oxide").size(24.0).strong());
                ui.label(theme::hint(
                    "A project is a folder: its file, recordings, imported copies, renders, \
                     exports and backups all live inside it.",
                ));
            });
        });
        ui.add_space(6.0);
        ui.separator();
        ui.add_space(4.0);
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let new = ui.vertical(|ui| {
                ui.set_width(NEW_WIDTH);
                ui.spacing_mut().item_spacing.x = 6.0;
                (create, browse) = new_project(app_start_parts(app), ui);
            });
            crate::marks::put(ui, "start.new", new.response.rect);
            // The line between the columns is drawn once both are laid out (a separator here
            // would grow to the whole screen's height).
            ui.add_space(GAP);
            let divider = ui.min_rect().right() - GAP / 2.0;
            let top = ui.min_rect().top();
            ui.vertical(|ui| {
                ui.set_width(OPEN_WIDTH);
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Open").size(17.0).strong());
                    theme::button_row(ui, |ui| {
                        let open = ui
                            .button("Open project…")
                            .on_hover_text("Pick a project file (.oxide) anywhere");
                        crate::marks::put(ui, "start.open", open.rect);
                        open_other = open.clicked();
                    });
                });
                ui.add_space(6.0);
                let recent = ui.vertical(|ui| {
                    ui.label(RichText::new("Recent projects").strong());
                    ui.add_space(2.0);
                    recent_list(ui, &entries, current.as_deref())
                });
                crate::marks::put(ui, "start.recent", recent.response.rect);
                asked = recent.inner;
            });
            let bottom = ui.min_rect().bottom();
            ui.painter().vline(
                divider,
                top..=bottom,
                ui.visuals().widgets.noninteractive.bg_stroke,
            );
        });
        if !required {
            ui.add_space(8.0);
            ui.separator();
            theme::button_row(ui, |ui| {
                cancel = ui.button("Cancel").clicked();
            });
        }
    });
    // F1 here: getting started (27.3).
    let (layer, rect) = (shown.response.layer_id, shown.response.rect);
    app.help.note(ctx, crate::help::Place::Start, layer, rect);
    if browse {
        let dialog = rfd::FileDialog::new();
        let dialog = match PathBuf::from(app.start.parent.trim()) {
            p if p.is_dir() => dialog.set_directory(p),
            _ => dialog,
        };
        crate::file_dialog::pick_folder(app, dialog, |app, dir| {
            app.start.parent = dir.display().to_string();
        });
    }
    if create && app.neutral {
        // Its folder is only a name to show (`neutral::PROJECTS_FOLDER`).
        app.toast("A neutral run (--neutral) makes no projects");
    } else if create {
        let parent = PathBuf::from(app.start.parent.trim());
        let name = app.start.name.trim().to_string();
        let template = app.start.template.clone();
        crate::unsaved::then(app, "starting a new project", move |app| {
            match app
                .session
                .new_project_in(&name, &parent, template.as_deref())
            {
                Ok(path) => {
                    app.settings.projects_parent = Some(parent);
                    app.settings.add_recent(path.clone());
                    if let Some(template) = template {
                        app.settings
                            .project_templates
                            .insert(path.clone(), template);
                    }
                    app.save_settings();
                    app.project_opened();
                    app.start.open = false;
                    app.toast(format!("Created {}", path.display()));
                }
                Err(e) => app.toast(format!("{e:#}")),
            }
        });
    }
    if open_other {
        crate::dialogs::open_dialog(app);
    }
    match asked {
        Some(RecentAsk::Open(path)) => app.open_project_asking(&path),
        Some(RecentAsk::Explore(path, missing)) => {
            if let Err(e) = recent::show_in_explorer(&path, missing) {
                app.toast(format!("{e:#}"));
            }
        }
        Some(RecentAsk::Remove(path)) => app.remove_recent(&path),
        None => {}
    }
    if cancel {
        app.start.open = false;
    }
}

/// The parts of the app the new project's column edits.
struct NewParts<'a> {
    start: &'a mut StartScreen,
    templates: Vec<daw_core::template::Template>,
    /// A neutral run's folder is a name to show, not a full path (`--neutral`).
    neutral: bool,
}

fn app_start_parts(app: &mut DawApp) -> NewParts<'_> {
    let templates = app.session.templates();
    NewParts {
        start: &mut app.start,
        templates,
        neutral: app.neutral,
    }
}

/// The new project's column: its name, template and folder, and Create. Returns (Create
/// clicked, Browse clicked).
fn new_project(parts: NewParts, ui: &mut egui::Ui) -> (bool, bool) {
    let NewParts {
        start,
        templates,
        neutral,
    } = parts;
    let mut browse = false;
    ui.label(RichText::new("New project").size(17.0).strong());
    ui.add_space(8.0);
    // A template deleted or renamed since it was picked: back to none.
    if start
        .template
        .as_ref()
        .is_some_and(|t| !templates.iter().any(|x| x.name == *t))
    {
        start.template = None;
    }
    let field = NEW_WIDTH - LABEL_WIDTH - 6.0;
    theme::setting_row(ui, LABEL_WIDTH, "Name", "", |ui| {
        ui.add(egui::TextEdit::singleline(&mut start.name).desired_width(field));
    });
    let template_hint = if templates.is_empty() {
        "No templates yet: File > Save as template… makes one from a project."
    } else {
        "Starts with the template's tracks, routing and plug-ins."
    };
    let (_, template_row) =
        theme::setting_row_at(ui, LABEL_WIDTH, "Template", template_hint, |ui| {
            ui.add_enabled_ui(!templates.is_empty(), |ui| {
                egui::ComboBox::from_id_salt("start-template")
                    .width(field)
                    .selected_text(start.template.as_deref().unwrap_or("None (empty)"))
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut start.template, None, "None (empty)");
                        for t in &templates {
                            ui.selectable_value(&mut start.template, Some(t.name.clone()), &t.name);
                        }
                    });
            });
        });
    crate::marks::put(ui, "start.template", template_row);
    let parent = PathBuf::from(start.parent.trim());
    let folder = parent.join(daw_core::folder::file_name(&start.name));
    let ready = !start.name.trim().is_empty() && (parent.is_absolute() || neutral);
    let made = if ready {
        format!("Makes {}", folder.display())
    } else {
        "Give it a name and a folder (a full path, such as D:\\Music)".into()
    };
    let (_, folder_row) = theme::setting_row_at(ui, LABEL_WIDTH, "In folder", &made, |ui| {
        ui.horizontal(|ui| {
            let browse_width = 72.0;
            ui.add(
                egui::TextEdit::singleline(&mut start.parent)
                    .desired_width(field - browse_width - 2.0),
            );
            browse = ui.button("Browse…").clicked();
        });
    });
    crate::marks::put(ui, "start.folder", folder_row);
    ui.add_space(4.0);
    let create = theme::button_row(ui, |ui| {
        let create = ui
            .add_enabled(ready, theme::primary_button("Create project"))
            .on_disabled_hover_text("Give it a name and a folder first");
        crate::marks::put(ui, "start.create", create.rect);
        create.clicked()
    });
    (create, browse)
}

/// The recent projects, a row each; what was clicked or picked from a row's menu.
fn recent_list(ui: &mut egui::Ui, entries: &[Entry], current: Option<&Path>) -> Option<RecentAsk> {
    if entries.is_empty() {
        ui.label(theme::hint(
            "Projects you make or open show here, the latest first.",
        ));
        return None;
    }
    let mut asked = None;
    egui::ScrollArea::vertical()
        .max_height(ROW_HEIGHT * 6.0 + 12.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for entry in entries {
                if let Some(a) = recent_row(ui, entry, current == Some(entry.path.as_path())) {
                    asked = Some(a);
                }
            }
        });
    asked
}

/// One recent project: its name and folder on the left, when it was saved and its template
/// (or "Missing" and Remove) on the right. A click opens it; right-click for the rest.
fn recent_row(ui: &mut egui::Ui, entry: &Entry, is_open: bool) -> Option<RecentAsk> {
    let width = ui.available_width();
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW_HEIGHT), egui::Sense::click());
    let missing = entry.missing();
    let mut asked = None;
    // The Remove button of a missing project sits over the row and takes its clicks.
    let remove_rect = egui::Rect::from_min_size(
        egui::pos2(rect.right() - 150.0, rect.center().y + 1.0),
        egui::vec2(140.0, 18.0),
    );
    // The row's background goes under the button, filled in once the hover is known.
    let painter = ui.painter_at(rect);
    let background = painter.add(egui::Shape::Noop);
    let remove = missing.then(|| {
        ui.put(
            remove_rect,
            egui::Button::new(RichText::new("Remove from the list").size(theme::HINT_SIZE)),
        )
        .on_hover_text("Take it off this list; nothing is deleted")
    });
    let over_remove = remove.as_ref().is_some_and(|r| r.hovered());
    let hovered = response.hovered() && !over_remove;
    let fill = if hovered && !missing {
        theme::LANE_SELECTED
    } else {
        theme::LANE
    };
    painter.set(background, egui::Shape::rect_filled(rect, 4.0, fill));
    if hovered && !missing {
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
            1.0,
            theme::ACCENT,
        );
    }
    let left = rect.left() + 12.0;
    let right = rect.right() - 12.0;
    let right_width = 150.0;
    let text_width = (right - right_width) - left - 8.0;
    let (name_color, folder_color) = if missing {
        (theme::TEXT_DIM, Color32::from_gray(0x6a))
    } else {
        (Color32::from_gray(0xf0), theme::TEXT_DIM)
    };
    let name_font = egui::FontId::proportional(14.5);
    let small = egui::FontId::proportional(theme::HINT_SIZE);
    let fit = |text: &str, font: &egui::FontId| {
        recent::shorten_to_width(text, text_width, |s| {
            painter
                .layout_no_wrap(s.to_string(), font.clone(), Color32::WHITE)
                .size()
                .x
        })
    };
    let top = rect.top() + 7.0;
    let bottom = rect.bottom() - 7.0;
    painter.text(
        egui::pos2(left, top),
        egui::Align2::LEFT_TOP,
        fit(&entry.name, &name_font),
        name_font.clone(),
        name_color,
    );
    painter.text(
        egui::pos2(left, bottom),
        egui::Align2::LEFT_BOTTOM,
        fit(&entry.folder, &small),
        small.clone(),
        folder_color,
    );
    let (when, when_color) = match entry.found {
        None => ("Checking…".to_string(), theme::TEXT_DIM),
        Some(Found::Missing) => ("Missing".to_string(), theme::MUTE),
        Some(Found::Present(Some(t))) => (recent::saved(t), Color32::from_gray(0xc8)),
        Some(Found::Present(None)) => (String::new(), theme::TEXT_DIM),
    };
    painter.text(
        egui::pos2(right, top + 1.0),
        egui::Align2::RIGHT_TOP,
        when,
        egui::FontId::proportional(13.0),
        when_color,
    );
    if !missing {
        let note = if is_open {
            Some(("Open now".to_string(), theme::ACCENT))
        } else {
            entry
                .template
                .as_ref()
                .map(|t| (format!("from {t}"), theme::TEXT_DIM))
        };
        if let Some((note, color)) = note {
            painter.text(
                egui::pos2(right, bottom),
                egui::Align2::RIGHT_BOTTOM,
                note,
                small,
                color,
            );
        }
    }
    let tip = if missing {
        format!(
            "{}\nNot found: moved, deleted, or on a drive that isn't connected.",
            entry.path.display()
        )
    } else {
        entry.path.display().to_string()
    };
    let response = if missing {
        response
    } else {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    };
    let response = response.on_hover_text(tip);
    if response.clicked() && !missing && !over_remove {
        asked = Some(RecentAsk::Open(entry.path.clone()));
    }
    if remove.is_some_and(|r| r.clicked()) {
        asked = Some(RecentAsk::Remove(entry.path.clone()));
    }
    response.context_menu(|ui| {
        if ui
            .add_enabled(!missing, egui::Button::new("Open"))
            .clicked()
        {
            asked = Some(RecentAsk::Open(entry.path.clone()));
            ui.close();
        }
        if ui.button("Show in Explorer").clicked() {
            asked = Some(RecentAsk::Explore(entry.path.clone(), missing));
            ui.close();
        }
        ui.separator();
        if ui
            .button("Remove from the list")
            .on_hover_text("Nothing is deleted")
            .clicked()
        {
            asked = Some(RecentAsk::Remove(entry.path.clone()));
            ui.close();
        }
    });
    asked
}
