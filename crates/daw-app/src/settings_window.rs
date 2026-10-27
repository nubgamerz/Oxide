//! Options > Settings… (dev plan 17.7): the app's settings in sections, a list of them down
//! the side. Audio (the device, which restarts with Apply), MIDI, Projects (backups, the file
//! type) and AI control (MCP, `mcp_settings`). Everything but the audio device applies at
//! once. There is no Appearance section: the app has no appearance settings yet.
//!
//! Each section is a column of rows (`theme::setting_row`): the setting's name in a label
//! column of one width, its control, and a line on what it does. The window opens under the
//! menu bar, centred (`theme::dialog_window`), and keeps the height of the tallest section
//! seen, so the list down the side doesn't jump when the section changes.

use crate::app::DawApp;
use crate::theme;
use daw_core::device::{self, AudioSettings, DeviceCaps, DeviceList};
use egui::{Color32, RichText};

/// The settings' sections, in the side list's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Audio,
    Midi,
    Projects,
    Mcp,
}

impl Section {
    pub const ALL: [Section; 4] = [Self::Audio, Self::Midi, Self::Projects, Self::Mcp];

    /// The section's word in the manual's mark names (`settings.audio`, `settings.nav.midi`).
    pub fn key(self) -> &'static str {
        match self {
            Section::Audio => "audio",
            Section::Midi => "midi",
            Section::Projects => "projects",
            Section::Mcp => "mcp",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Audio => "Audio",
            Self::Midi => "MIDI",
            Self::Projects => "Projects",
            Self::Mcp => "AI control (MCP)",
        }
    }

    /// The line under the section's heading: what it holds and when changes apply.
    fn summary(self) -> &'static str {
        match self {
            Self::Audio => {
                "The audio device and how the engine runs on it. Apply restarts the device with \
                 these settings."
            }
            Self::Midi => "Keyboards and controllers. Changes apply at once.",
            Self::Projects => "Backups and how Windows opens project files. Changes apply at once.",
            Self::Mcp => {
                "Lets AI agents such as Claude Code operate the DAW through a local MCP server. \
                 Changes apply at once."
            }
        }
    }
}

/// The labels' column, in every section.
const LABEL_WIDTH: f32 = 120.0;
/// The section's column, beside the list.
const CONTENT_WIDTH: f32 = 580.0;
const SIDE_WIDTH: f32 = 150.0;
/// Every section's least height (about the tallest, Audio's), so the window keeps its size
/// as the section changes.
const MIN_HEIGHT: f32 = 480.0;
/// The controls' usual width (combo boxes).
const CONTROL_WIDTH: f32 = 330.0;

/// The settings window, while open: the section shown and an editable copy of the audio
/// settings, with what the chosen driver and device support.
pub struct SettingsDialog {
    pub section: Section,
    settings: AudioSettings,
    mcp: crate::mcp_settings::Panel,
    hosts: Vec<String>,
    devices: DeviceList,
    caps: DeviceCaps,
    error: Option<String>,
    /// The tallest section shown so far: the window keeps that height.
    tallest: f32,
    /// The manual's shots (`--neutral`): neutral drivers and devices, never listed afresh.
    neutral: bool,
}

impl SettingsDialog {
    /// `mcp_port`: the port MCP is asked to serve on, as the port field starts.
    pub fn new(current: &AudioSettings, mcp_port: u16, section: Section) -> Self {
        let mut dialog = Self {
            section,
            settings: running(current),
            mcp: crate::mcp_settings::Panel::new(mcp_port),
            hosts: device::host_names(),
            devices: DeviceList::default(),
            caps: DeviceCaps::default(),
            error: None,
            tallest: 0.0,
            neutral: false,
        };
        dialog.refresh();
        dialog
    }

    /// The dialog as the manual's shots show it (`--neutral`): neutral drivers, devices and
    /// rates (`neutral.rs`), at `rate`; changing them lists nothing from the machine.
    pub fn neutral(rate: u32, mcp_port: u16, section: Section) -> Self {
        Self {
            section,
            settings: crate::neutral::settings(),
            mcp: crate::mcp_settings::Panel::new(mcp_port),
            hosts: crate::neutral::hosts(),
            devices: crate::neutral::devices(),
            caps: crate::neutral::caps(rate),
            error: None,
            tallest: 0.0,
            neutral: true,
        }
    }

    fn refresh(&mut self) {
        if self.neutral {
            return;
        }
        self.error = None;
        match device::list_devices(&self.settings.host) {
            Ok(list) => self.devices = list,
            Err(e) => {
                self.devices = DeviceList::default();
                self.error = Some(format!("{e:#}"));
            }
        }
        match device::device_caps(&self.settings.host, self.settings.output_device.as_deref()) {
            Ok(caps) => self.caps = caps,
            Err(e) => {
                self.caps = DeviceCaps::default();
                self.error.get_or_insert(format!("{e:#}"));
            }
        }
    }
}

/// The settings as running: no host saved means the default one runs, so it is named.
fn running(settings: &AudioSettings) -> AudioSettings {
    let mut settings = settings.clone();
    if settings.host.is_empty() {
        settings.host = device::default_host_name();
    }
    settings
}

/// What a frame of the window asks the app to do, once the window is drawn.
#[derive(Default)]
struct Asked {
    apply: bool,
    mcp: Option<crate::mcp_settings::Change>,
    registered: Option<anyhow::Result<()>>,
    backups: Option<(u32, usize)>,
    reconnect_midi: bool,
}

pub fn show(app: &mut DawApp, ctx: &egui::Context) {
    if app.settings_dialog.is_none() {
        return;
    }
    let mcp_view = crate::mcp_settings::View {
        settings: app.settings.mcp.clone(),
        port: app.mcp_port(),
        shown_port: if app.neutral {
            crate::neutral::MCP_PORT
        } else {
            app.mcp_port()
        },
        status: app.mcp.status().clone(),
        command: app.mcp_connect_command(),
        token: app.mcp_wanted_token(),
        link: app.claude.view(),
    };
    // The manual's shots (`--neutral`) show neutral settings, applied.
    let now = if app.neutral {
        crate::neutral::settings()
    } else {
        running(&app.session.settings)
    };
    let shown = app.shown_device();
    let ports = app.shown_midi_ports();
    // The manual's shots (`--neutral`) name no machine's folders.
    let settings_file = if app.neutral {
        crate::neutral::SETTINGS_FILE.to_string()
    } else {
        crate::settings::app_dir()
            .join("settings.json")
            .display()
            .to_string()
    };
    let Some(dialog) = &mut app.settings_dialog else {
        return;
    };
    let unapplied = dialog.settings != now;
    let mut open = true;
    let mut asked = Asked::default();
    let room = theme::below_menu(ctx).height();
    let window = theme::dialog_window("Settings", ctx)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.horizontal_top(|ui| {
                side_list(ui, &mut dialog.section, unapplied);
                // The line between the list and the section, drawn once both are laid out (a
                // separator would grow to the screen's height).
                ui.add_space(12.0);
                let divider = ui.min_rect().right() - 6.0;
                let list_top = ui.min_rect().top();
                let shown = ui.vertical(|ui| {
                    ui.set_width(CONTENT_WIDTH);
                    // The window's title and frame take about 60 points of the room.
                    let most = (room - 90.0).max(200.0);
                    ui.set_min_height(dialog.tallest.max(MIN_HEIGHT).min(most));
                    let top = ui.min_rect().top();
                    egui::ScrollArea::vertical()
                        .max_height(most)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.set_width(CONTENT_WIDTH - 12.0);
                            theme::dialog_heading(
                                ui,
                                dialog.section.name(),
                                dialog.section.summary(),
                            );
                            match dialog.section {
                                Section::Audio => audio(
                                    ui,
                                    dialog,
                                    &app.session,
                                    shown.as_ref(),
                                    unapplied,
                                    &mut asked,
                                ),
                                Section::Midi => midi(ui, &app.session, &ports, &mut asked),
                                Section::Projects => {
                                    projects(ui, &app.settings, &settings_file, &mut asked)
                                }
                                Section::Mcp => {
                                    asked.mcp =
                                        crate::mcp_settings::show(ui, &mut dialog.mcp, &mcp_view);
                                }
                            }
                        });
                    dialog.tallest = dialog.tallest.max(ui.min_rect().bottom() - top);
                });
                let section = dialog.section;
                crate::marks::put_with(
                    ui,
                    || format!("settings.{}", section.key()),
                    shown.response.rect,
                );
                let bottom = ui.min_rect().bottom();
                ui.painter().vline(
                    divider,
                    list_top..=bottom,
                    ui.visuals().widgets.noninteractive.bg_stroke,
                );
            });
        });
    if let Some(window) = window {
        crate::marks::put_rect("settings", window.response.rect);
        // F1 here: the settings page at the section shown (27.3).
        if let Some(section) = app.settings_dialog.as_ref().map(|d| d.section) {
            let (layer, rect) = (window.response.layer_id, window.response.rect);
            app.help
                .note(ctx, crate::help::Place::Settings(section), layer, rect);
        }
    }
    if let Some((minutes, keep)) = asked.backups {
        app.settings.autosave_minutes = minutes;
        app.settings.keep_backups = keep;
        app.settings.apply_backups(&mut app.session);
        app.save_settings();
    }
    if asked.reconnect_midi {
        app.session.restart_midi();
    }
    match asked.registered {
        Some(Ok(())) => app.toast(".oxide files now open with Oxide"),
        Some(Err(e)) => app.toast(format!("{e:#}")),
        None => {}
    }
    match asked.mcp {
        Some(crate::mcp_settings::Change::Settings(mcp)) => app.apply_mcp_settings(mcp),
        Some(crate::mcp_settings::Change::Retry) => app.retry_mcp(),
        Some(crate::mcp_settings::Change::Link(client, act)) => {
            let wanted = app.claude_wanted();
            app.claude.apply(client, act, wanted);
        }
        Some(crate::mcp_settings::Change::CheckLink) => {
            let wanted = app.claude_wanted();
            app.claude.refresh(wanted);
        }
        None => {}
    }
    // A neutral run's devices aren't real: Apply there is only a picture.
    if asked.apply && !app.neutral {
        if let Some(dialog) = &app.settings_dialog {
            let settings = dialog.settings.clone();
            let apply = move |app: &mut DawApp| {
                app.session.apply_audio_settings(settings.clone());
                app.settings.audio = settings;
                app.save_settings();
                if let Some(error) = app.session.device_error.clone() {
                    app.toast(format!("Audio device: {error}"));
                }
            };
            // Restarting the device takes a moment of its own: the overlay goes up first. A
            // new sample rate's plug-ins then load behind it, a plug-in a frame.
            if app.session.settings.needs_restart(&dialog.settings) {
                crate::busy::run(app, "Restarting the audio device", apply);
            } else {
                apply(app);
            }
        }
    }
    if !open {
        app.settings_dialog = None;
    }
}

/// The sections down the side: the one shown lit, with a mark on Audio while it has changes
/// not applied yet.
fn side_list(ui: &mut egui::Ui, section: &mut Section, unapplied: bool) {
    ui.vertical(|ui| {
        ui.set_width(SIDE_WIDTH);
        ui.spacing_mut().item_spacing.y = 2.0;
        for each in Section::ALL {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(SIDE_WIDTH, 30.0), egui::Sense::click());
            let selected = *section == each;
            let painter = ui.painter_at(rect);
            if selected {
                painter.rect_filled(rect, 3.0, theme::LANE_SELECTED);
                painter.rect_filled(
                    egui::Rect::from_min_size(rect.min, egui::vec2(3.0, rect.height())),
                    1.0,
                    theme::ACCENT,
                );
            } else if response.hovered() {
                painter.rect_filled(rect, 3.0, Color32::from_gray(0x2e));
            }
            let text = if selected {
                Color32::WHITE
            } else {
                Color32::from_gray(0xc8)
            };
            painter.text(
                rect.left_center() + egui::vec2(12.0, 0.0),
                egui::Align2::LEFT_CENTER,
                each.name(),
                egui::FontId::proportional(14.0),
                text,
            );
            if each == Section::Audio && unapplied {
                painter.circle_filled(
                    rect.right_center() - egui::vec2(12.0, 0.0),
                    3.5,
                    theme::MUTE,
                );
            }
            crate::marks::put_with(ui, || format!("settings.nav.{}", each.key()), rect);
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            let response = if each == Section::Audio && unapplied {
                response.on_hover_text("Changes not applied yet")
            } else {
                response
            };
            if response.clicked() {
                *section = each;
            }
        }
    });
}

/// A setting's row in this window's label column.
/// The blocks device buffers can be gathered into (23.1).
const PROCESSING_BLOCKS: [u32; 5] = [32, 64, 128, 256, 512];

/// The processing block's line: what gathering buys and what it costs with the device's buffer
/// now (`device`, 0 before a device runs), in samples and milliseconds.
fn processing_hint(block: Option<u32>, device: usize, rate: f64) -> String {
    const WHY: &str = "Plug-ins that cost mostly per call (some amp sims) need far less CPU \
                       in bigger blocks. Helps drivers that call back in bursts (FlexASIO); \
                       exports use the same block.";
    let Some(block) = block.map(|b| b as usize) else {
        return format!("{WHY} Off: no added latency.");
    };
    if device == 0 {
        return format!("{WHY} Adds the block less one buffer to the latency.");
    }
    if block <= device {
        return format!("{WHY} No larger than the {device}-sample buffer: nothing to gather.");
    }
    // The engine rounds up to whole buffers (`EngineCore::gathering`).
    let gathered = block.div_ceil(device) * device;
    let added = gathered - device;
    format!(
        "{WHY} With the {device}-sample buffer: blocks of {gathered}, {added} samples ({:.1} ms) \
         more between input and output.",
        added as f64 * 1000.0 / rate
    )
}

/// A setting's row, marked `mark` for the manual (`settings.audio.buffer_size`).
fn row<R>(
    ui: &mut egui::Ui,
    mark: &'static str,
    label: &str,
    hint: &str,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let (made, rect) = theme::setting_row_at(ui, LABEL_WIDTH, label, hint, add);
    crate::marks::put(ui, mark, rect);
    made
}

fn audio(
    ui: &mut egui::Ui,
    dialog: &mut SettingsDialog,
    session: &daw_core::session::Session,
    shown: Option<&daw_core::device::DeviceInfo>,
    unapplied: bool,
    asked: &mut Asked,
) {
    let mut changed = false;
    row(
        ui,
        "settings.audio.driver",
        "Driver type",
        "WASAPI shares the device with other programs; ASIO talks to the interface directly, \
         for the smallest buffers.",
        |ui| {
            egui::ComboBox::from_id_salt("host")
                .width(CONTROL_WIDTH)
                .selected_text(&dialog.settings.host)
                .show_ui(ui, |ui| {
                    for host in dialog.hosts.clone() {
                        if ui
                            .selectable_value(&mut dialog.settings.host, host.clone(), &host)
                            .changed()
                        {
                            dialog.settings.output_device = None;
                            dialog.settings.input_device = None;
                            changed = true;
                        }
                    }
                });
        },
    );
    let asio = device::is_asio(&dialog.settings.host);
    let (output_label, output_hint) = if asio {
        (
            "ASIO driver",
            "Plays and records through this driver. Drivers only appear when they can start: \
             FlexASIO, for one, is skipped while the device its settings name isn't connected.",
        )
    } else {
        ("Output device", "Where the master plays.")
    };
    row(
        ui,
        "settings.audio.output",
        output_label,
        output_hint,
        |ui| {
            let current = dialog
                .settings
                .output_device
                .clone()
                .unwrap_or_else(|| "System default".into());
            egui::ComboBox::from_id_salt("output")
                .width(CONTROL_WIDTH)
                .selected_text(current)
                .show_ui(ui, |ui| {
                    if !asio
                        && ui
                            .selectable_value(
                                &mut dialog.settings.output_device,
                                None,
                                "System default",
                            )
                            .changed()
                    {
                        changed = true;
                    }
                    for name in dialog.devices.outputs.clone() {
                        if ui
                            .selectable_value(
                                &mut dialog.settings.output_device,
                                Some(name.clone()),
                                &name,
                            )
                            .changed()
                        {
                            changed = true;
                        }
                    }
                });
        },
    );
    if !asio {
        row(
            ui,
            "settings.audio.input",
            "Input device",
            "What tracks record from.",
            |ui| {
                let current = dialog
                    .settings
                    .input_device
                    .clone()
                    .unwrap_or_else(|| "System default".into());
                egui::ComboBox::from_id_salt("input")
                    .width(CONTROL_WIDTH)
                    .selected_text(current)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(
                            &mut dialog.settings.input_device,
                            None,
                            "System default",
                        );
                        for name in dialog.devices.inputs.clone() {
                            ui.selectable_value(
                                &mut dialog.settings.input_device,
                                Some(name.clone()),
                                &name,
                            );
                        }
                    });
            },
        );
    }
    row(
        ui,
        "settings.audio.sample_rate",
        "Sample rate",
        "Projects play at this rate; a project made at another is converted when it opens.",
        |ui| {
            let rate_text = dialog.settings.sample_rate.map_or(
                format!("Device default ({} Hz)", dialog.caps.default_rate),
                |r| format!("{r} Hz"),
            );
            egui::ComboBox::from_id_salt("rate")
                .width(CONTROL_WIDTH)
                .selected_text(rate_text)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dialog.settings.sample_rate, None, "Device default");
                    for rate in dialog.caps.sample_rates.clone() {
                        ui.selectable_value(
                            &mut dialog.settings.sample_rate,
                            Some(rate),
                            format!("{rate} Hz"),
                        );
                    }
                });
        },
    );
    row(
        ui,
        "settings.audio.buffer_size",
        "Buffer size",
        "Smaller is less delay when monitoring through plug-ins, but more work for the CPU.",
        |ui| {
            // One list: the driver's own size, or a fixed one the device allows.
            let (min, max) = dialog.caps.buffer_range.unwrap_or((16, 8192));
            let label = |size: Option<u32>| match size {
                None => "The driver's default".to_string(),
                Some(n) => format!("{n} samples"),
            };
            egui::ComboBox::from_id_salt("buffer")
                .width(CONTROL_WIDTH)
                .selected_text(label(dialog.settings.buffer_size))
                .show_ui(ui, |ui| {
                    let size = &mut dialog.settings.buffer_size;
                    ui.selectable_value(size, None, label(None));
                    for candidate in [16u32, 32, 64, 128, 192, 256, 384, 512, 1024, 2048] {
                        if (min..=max).contains(&candidate) {
                            ui.selectable_value(size, Some(candidate), label(Some(candidate)));
                        }
                    }
                });
        },
    );
    let rate = f64::from(session.sample_rate().max(1));
    let device_block = session.stats().device_block;
    row(
        ui,
        "settings.audio.processing_block",
        "Processing block",
        &processing_hint(dialog.settings.processing_block, device_block, rate),
        |ui| {
            let label = |block: Option<u32>| match block {
                None => "Each buffer as it comes".to_string(),
                Some(n) => format!("Gather into {n} samples"),
            };
            egui::ComboBox::from_id_salt("processing-block")
                .width(CONTROL_WIDTH)
                .selected_text(label(dialog.settings.processing_block))
                .show_ui(ui, |ui| {
                    let block = &mut dialog.settings.processing_block;
                    ui.selectable_value(block, None, label(None));
                    for candidate in PROCESSING_BLOCKS {
                        ui.selectable_value(block, Some(candidate), label(Some(candidate)));
                    }
                });
        },
    );
    row(
        ui,
        "settings.audio.threads",
        "Audio threads",
        "Independent tracks run in parallel on these. Auto uses one per physical core but the \
         first, which is left to the driver.",
        |ui| {
            let auto = daw_core::cpu::default_workers();
            let logical = std::thread::available_parallelism().map_or(1, |n| n.get());
            let label = |t: Option<usize>| match t {
                None => format!("Auto ({auto} workers + audio thread)"),
                Some(0) => "Audio thread only".to_string(),
                Some(n) => format!("{n} workers + audio thread"),
            };
            egui::ComboBox::from_id_salt("threads")
                .width(CONTROL_WIDTH)
                .selected_text(label(dialog.settings.threads))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut dialog.settings.threads, None, label(None));
                    for n in 0..logical {
                        ui.selectable_value(&mut dialog.settings.threads, Some(n), label(Some(n)));
                    }
                });
        },
    );
    let measured = match session.round_trip() {
        Some(n) => format!(
            "Moves recordings earlier (negative: later), on top of the round trip measured \
             from the driver ({n} samples, {:.1} ms). Only for takes a loopback test still \
             finds off.",
            n as f64 * 1000.0 / rate
        ),
        None => "Moves recordings earlier (negative: later), on top of the round trip measured \
                 from the driver (not measured: no input running). Only for takes a loopback \
                 test still finds off."
            .to_string(),
    };
    row(
        ui,
        "settings.audio.record_offset",
        "Record offset",
        &measured,
        |ui| {
            ui.add(
                egui::DragValue::new(&mut dialog.settings.record_offset)
                    .range(-48_000..=48_000)
                    .suffix(" samples"),
            );
        },
    );
    if changed {
        dialog.refresh();
    }
    if let Some(error) = &dialog.error {
        ui.add(egui::Label::new(RichText::new(error).color(theme::RECORD)).wrap());
    }
    ui.separator();
    // What runs now, and Apply on the right, enabled while there's something to apply.
    ui.horizontal(|ui| {
        theme::button_row(ui, |ui| {
            let apply = ui
                .add_enabled(unapplied, theme::primary_button("Apply"))
                .on_hover_text(
                    "Restart the audio device with these settings (the record offset and the \
                     processing block \
                     apply without a restart)",
                )
                .on_disabled_hover_text("Nothing changed");
            crate::marks::put(ui, "settings.audio.apply", apply.rect);
            asked.apply = apply.clicked();
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                if unapplied {
                    ui.label(RichText::new("Not applied yet").color(theme::MUTE));
                } else if let Some(device) = shown {
                    ui.label(theme::hint(format!(
                        "Running: {}, {} Hz, {} in / {} out",
                        device.output_name, device.sample_rate, device.inputs, device.outputs
                    )));
                } else {
                    ui.label(theme::hint("No device running"));
                }
            });
        });
    });
}

fn midi(
    ui: &mut egui::Ui,
    session: &daw_core::session::Session,
    ports: &[String],
    asked: &mut Asked,
) {
    row(
        ui,
        "settings.midi.inputs",
        "Inputs",
        "Every input connected when Oxide starts is opened. Armed instrument tracks play and \
         record from them (each track's input picks the port and channel).",
        |ui| {
            if ports.is_empty() {
                ui.label(RichText::new("None connected").color(theme::TEXT_DIM));
            }
            for port in ports {
                ui.label(format!("✔ {port}"));
            }
            for error in &session.midi.errors {
                ui.add(egui::Label::new(RichText::new(error).color(theme::MUTE)).wrap());
            }
        },
    );
    row(
        ui,
        "settings.midi.reconnect",
        "",
        "Opens every MIDI input again: after plugging a keyboard in, or when one stopped \
         answering.",
        |ui| {
            asked.reconnect_midi = ui.button("Reconnect MIDI devices").clicked();
        },
    );
}

/// The Projects section; `settings_file` is the settings file's path as shown.
fn projects(
    ui: &mut egui::Ui,
    settings: &crate::settings::AppSettings,
    settings_file: &str,
    asked: &mut Asked,
) {
    let mut minutes = settings.autosave_minutes;
    let mut keep = settings.keep_backups;
    let mut changed = false;
    row(
        ui,
        "settings.projects.autosave",
        "Autosave",
        "A backup copy while the project has unsaved changes and playback is stopped; 0 turns \
         it off. Only Save writes the project file.",
        |ui| {
            changed |= ui
                .add(
                    egui::DragValue::new(&mut minutes)
                        .range(0..=120)
                        .prefix("every ")
                        .suffix(" min"),
                )
                .changed();
        },
    );
    row(
        ui,
        "settings.projects.backups",
        "Backups kept",
        "Per project, the newest; File > Open backup… lists them.",
        |ui| {
            changed |= ui
                .add(egui::DragValue::new(&mut keep).range(1..=500))
                .changed();
        },
    );
    row(
        ui,
        "settings.projects.file_type",
        "File type",
        "Makes Windows open .oxide files with Oxide and show its icon, for your account only.",
        |ui| {
            if ui.button("Open .oxide files with Oxide").clicked() {
                asked.registered = Some(crate::settings::register_file_type());
            }
        },
    );
    row(
        ui,
        "settings.projects.settings_file",
        "Settings file",
        "Kept beside the program, so copying its folder takes the settings along.",
        |ui| {
            ui.add(egui::Label::new(RichText::new(settings_file).monospace()).truncate())
                .on_hover_text(settings_file);
        },
    );
    if changed {
        asked.backups = Some((minutes, keep));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The labels of the rows `draw` puts in a frame of a headless context.
    fn rows_of(draw: impl FnMut(&mut egui::Ui)) -> Vec<String> {
        let mut draw = draw;
        theme::ROWS_DRAWN.with(|rows| rows.borrow_mut().clear());
        let ctx = egui::Context::default();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| draw(ui));
        output.textures_delta.clear();
        theme::ROWS_DRAWN.with(|rows| rows.borrow().clone())
    }

    /// The manual's settings page (27.5) names every row the settings window draws, in bold
    /// (`**Label**`) under its section's heading: the rows are drawn here as the window draws
    /// them (each section, Audio with WASAPI and with ASIO, a row without a label by its
    /// button's), and their labels are looked for in `docs/manual/settings/settings.md`. A row
    /// added, renamed or moved fails until the page follows.
    #[test]
    fn the_settings_page_names_every_row() {
        let page = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../docs/manual/settings/settings.md"),
        )
        .expect("the settings page");
        // Each `## Section`'s text, by its heading.
        let mut sections: std::collections::BTreeMap<String, String> = Default::default();
        let mut current = String::new();
        for line in page.lines() {
            if let Some(heading) = line.strip_prefix("## ") {
                current = heading.trim().to_string();
            } else {
                sections.entry(current.clone()).or_default().push_str(line);
                sections.entry(current.clone()).or_default().push('\n');
            }
        }
        let session = daw_core::session::Session::new(crate::neutral::session_settings());
        let device = crate::neutral::device_info(48_000);
        let mut drawn: Vec<(Section, Vec<String>)> = Vec::new();
        for host in ["WASAPI", "ASIO"] {
            let mut dialog = SettingsDialog::neutral(48_000, 7878, Section::Audio);
            dialog.settings.host = host.to_string();
            let mut asked = Asked::default();
            drawn.push((
                Section::Audio,
                rows_of(|ui| audio(ui, &mut dialog, &session, Some(&device), true, &mut asked)),
            ));
        }
        let ports = vec!["MIDI Keyboard".to_string()];
        drawn.push((
            Section::Midi,
            rows_of(|ui| midi(ui, &session, &ports, &mut Asked::default())),
        ));
        let settings = crate::settings::AppSettings::default();
        drawn.push((
            Section::Projects,
            rows_of(|ui| projects(ui, &settings, "settings.json", &mut Asked::default())),
        ));
        let view = crate::mcp_settings::View {
            settings: crate::settings::McpSettings::default(),
            port: 7878,
            shown_port: 7878,
            status: crate::mcp::Status::Running {
                address: "127.0.0.1:7878".into(),
                port: 7878,
                token: false,
            },
            command: "claude mcp add --transport http --scope user oxide http://127.0.0.1:7878/mcp"
                .into(),
            token: None,
            link: crate::mcp_settings::LinkView {
                available: true,
                detected: None,
                checking: false,
                working: None,
                message: None,
                code_config: None,
            },
        };
        let mut panel = crate::mcp_settings::Panel::new(7878);
        drawn.push((
            Section::Mcp,
            rows_of(|ui| {
                crate::mcp_settings::show(ui, &mut panel, &view);
            }),
        ));
        let mut missing = Vec::new();
        let mut count = 0;
        for (section, labels) in &drawn {
            let text = sections.get(section.name()).map_or("", String::as_str);
            for label in labels {
                // The MIDI section's Reconnect row has no label: its button names it.
                let label = if label.is_empty() && *section == Section::Midi {
                    "Reconnect MIDI devices"
                } else {
                    label.as_str()
                };
                count += 1;
                if !text.contains(&format!("**{label}**")) {
                    missing.push(format!("{} > {label}", section.name()));
                }
            }
        }
        assert!(count >= 20, "only {count} rows drawn: {drawn:?}");
        assert!(
            missing.is_empty(),
            "docs/manual/settings/settings.md doesn't name these rows (in bold, under their \
             section's ## heading): {missing:?}"
        );
    }

    #[test]
    fn the_processing_blocks_line_gives_the_latency_it_adds() {
        let hint = |block, device| processing_hint(block, device, 48_000.0);
        assert!(hint(None, 16).ends_with("Off: no added latency."));
        assert!(
            hint(Some(64), 16)
                .ends_with("blocks of 64, 48 samples (1.0 ms) more between input and output."),
            "{}",
            hint(Some(64), 16)
        );
        // Rounded up to whole buffers, as the engine does.
        assert!(hint(Some(64), 48).contains("blocks of 96, 48 samples"));
        assert!(hint(Some(64), 128).contains("nothing to gather"));
        assert!(hint(Some(64), 0).contains("less one buffer"));
    }
}
