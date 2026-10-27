//! Settings > AI control (MCP): Enabled, the port and the token apply at once (saved, and the
//! server restarted on its worker, `DawApp::apply_mcp_settings`), the status with Retry when it
//! couldn't start, and the `claude mcp add` line to copy, which follows the port (greyed, with
//! a note, while the server is off). Rows in the settings window's columns (17.7).
//!
//! The section draws from a [`View`] taken before the dialog borrows the app, and answers with
//! a [`Change`] the dialog applies after it.
//!
//! "Connect Claude" (dev plan 17.3): a row each for Claude Code and Claude Desktop with whether
//! Oxide is in its config for this port and token, and Configure (only when it isn't),
//! Reconfigure and Remove. [`Link`] reads and changes the configs on a worker
//! (`mcp_link::clients`), when the section opens, when the port or token changes, and after
//! each click; a frame never waits for a file or for `claude`. These are the app's only writes
//! to the user's Claude settings, made only on a click: no action does it, so an agent can't
//! rewrite its own client's config and the token never leaves the app.

use crate::mcp::{Status, PORTS};
use crate::settings::McpSettings;
use crate::theme;
use egui::RichText;
use mcp_link::clients::{self, Act, Client, Detected, Locations, Wanted};
use std::path::Path;
use std::sync::mpsc::{Receiver, Sender};

/// What the section shows, read from the app before the dialog is drawn.
pub struct View {
    pub settings: McpSettings,
    /// The port asked for (`--mcp-port`, else the settings').
    pub port: u16,
    /// The port drawn: `port`, or MCP's default in the manual's shots (`--neutral`, TD194),
    /// which serve on a port of their own.
    pub shown_port: u16,
    pub status: Status,
    /// `claude mcp add …` for that port, with the token.
    pub command: String,
    /// The token in `command`, kept off the screen.
    pub token: Option<String>,
    /// Claude Code's and Claude Desktop's rows.
    pub link: LinkView,
}

impl View {
    /// `text` as drawn: the port in it is the shown one (`--neutral`).
    fn shown(&self, text: String) -> String {
        if self.shown_port == self.port {
            text
        } else {
            crate::neutral::mcp_text(&text, self.port)
        }
    }
}

/// What the user changed.
pub enum Change {
    /// Enabled, the port or the token: save and apply.
    Settings(McpSettings),
    /// Start again on the same port (it was taken).
    Retry,
    /// Configure, Reconfigure or Remove Oxide in a client.
    Link(Client, Act),
    /// Look at the clients' configs again (changed outside the app).
    CheckLink,
}

/// What a [`Link`] worker reports.
enum Done {
    Detected(u64, Detected),
    Applied(u64, Client, Result<String, String>, Detected),
}

/// The clients' state for the settings: detected and changed on workers, the newest answer
/// kept.
pub struct Link {
    /// `None`: no home or app data folders to look in (the section says so).
    locations: Option<Locations>,
    wake: egui::Context,
    sender: Sender<Done>,
    receiver: Receiver<Done>,
    /// The newest detection asked for; older answers are dropped.
    serial: u64,
    detected: Option<Detected>,
    checking: bool,
    working: Option<Client>,
    message: Option<Result<String, String>>,
}

/// A copy of [`Link`]'s state for one frame's drawing.
#[derive(Clone)]
pub struct LinkView {
    pub available: bool,
    pub detected: Option<Detected>,
    pub checking: bool,
    pub working: Option<Client>,
    pub message: Option<Result<String, String>>,
    /// Where Claude Code's config is read from (the hover).
    pub code_config: Option<String>,
}

impl Link {
    pub fn new(locations: Option<Locations>, wake: egui::Context) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            locations,
            wake,
            sender,
            receiver,
            serial: 0,
            detected: None,
            checking: false,
            working: None,
            message: None,
        }
    }

    /// Read both clients' configs again, for `wanted`, on a worker.
    pub fn refresh(&mut self, wanted: Wanted) {
        let Some(locations) = self.locations.clone() else {
            return;
        };
        self.serial += 1;
        self.checking = true;
        let (serial, sender, wake) = (self.serial, self.sender.clone(), self.wake.clone());
        let _ = std::thread::Builder::new()
            .name("claude-link".into())
            .spawn(move || {
                let _ = sender.send(Done::Detected(serial, clients::detect(&locations, &wanted)));
                wake.request_repaint();
            });
    }

    /// Configure, Reconfigure or Remove on a worker; the configs read again after. One at a time.
    pub fn apply(&mut self, client: Client, act: Act, wanted: Wanted) {
        let Some(locations) = self.locations.clone() else {
            return;
        };
        if self.working.is_some() {
            return;
        }
        self.serial += 1;
        self.working = Some(client);
        self.message = None;
        let (serial, sender, wake) = (self.serial, self.sender.clone(), self.wake.clone());
        let _ = std::thread::Builder::new()
            .name("claude-link".into())
            .spawn(move || {
                let said = clients::apply(&locations, client, act, &wanted);
                let detected = clients::detect(&locations, &wanted);
                let _ = sender.send(Done::Applied(serial, client, said, detected));
                wake.request_repaint();
            });
    }

    /// Take the workers' answers (each frame, in `logic`).
    pub fn poll(&mut self) {
        while let Ok(done) = self.receiver.try_recv() {
            match done {
                Done::Detected(serial, detected) => {
                    if serial == self.serial {
                        self.detected = Some(detected);
                        self.checking = false;
                    }
                }
                Done::Applied(serial, client, said, detected) => {
                    if let Err(e) = &said {
                        log::warn!("{}: {e}", client.name());
                    }
                    self.message = Some(said);
                    self.working = None;
                    if serial == self.serial {
                        self.detected = Some(detected);
                        self.checking = false;
                    }
                }
            }
        }
    }

    /// Wait for the workers (checks: the shot shows the answer, not "Checking…").
    pub fn settle(&mut self, limit: std::time::Duration) {
        let start = std::time::Instant::now();
        while (self.checking || self.working.is_some()) && start.elapsed() < limit {
            self.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    /// Where it looks (`None`: nowhere to look).
    pub fn locations(&self) -> Option<Locations> {
        self.locations.clone()
    }

    pub fn view(&self) -> LinkView {
        LinkView {
            available: self.locations.is_some(),
            detected: self.detected.clone(),
            checking: self.checking,
            working: self.working,
            message: self.message.clone(),
            code_config: self
                .locations
                .as_ref()
                .map(|l| l.code_config().display().to_string()),
        }
    }
}

/// The clients' places for this run: the user's own, or with `--claude-profile DIR` (checks
/// and shots) a profile inside DIR; the bridge beside the running program either way.
pub fn locations(profile: Option<&Path>) -> Option<Locations> {
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    match profile {
        Some(root) => {
            let root = std::path::absolute(root).ok()?;
            let home = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from);
            let cli = clients::find_cli(std::env::var_os("PATH"), home.as_deref().unwrap_or(&root));
            Some(Locations::profile(
                &root,
                cli,
                exe_dir.join(clients::BRIDGE),
            ))
        }
        None => Locations::from_env(exe_dir),
    }
}

/// The marker that lets [`seed_profile`] clear a folder: one it made itself.
const PROFILE_MARK: &str = ".oxide-claude-profile";

/// `--show mcp-connect` / `mcp-connected`: a profile with Claude Code and the Store build of
/// Claude Desktop "installed", each with a server of the user's own; `connected` also has
/// Oxide in Code for `port` and in Desktop for port 7878, as after a port change. A folder is
/// only cleared if it is empty or one this made.
pub fn seed_profile(
    locations: &Locations,
    root: &Path,
    connected: bool,
    port: u16,
) -> anyhow::Result<()> {
    if root.exists() {
        let made_here = root.join(PROFILE_MARK).is_file();
        let empty = std::fs::read_dir(root)?.next().is_none();
        anyhow::ensure!(
            made_here || empty,
            "{} isn't a Claude profile this app made: not clearing it",
            root.display()
        );
        std::fs::remove_dir_all(root)?;
    }
    std::fs::create_dir_all(root)?;
    std::fs::write(root.join(PROFILE_MARK), "")?;
    let code = locations.code_config();
    std::fs::create_dir_all(code.parent().unwrap_or(root))?;
    // Claude Code's entry as `claude mcp add --transport stdio` writes it: the bridge.
    let oxide = if connected {
        let server = serde_json::json!({
            "type": "stdio",
            "command": locations.bridge.to_string_lossy(),
            "args": ["--port", port.to_string()],
            "env": {}
        });
        format!(",\n    \"oxide\": {server}")
    } else {
        String::new()
    };
    std::fs::write(
        &code,
        format!(
            "{{\n  \"numStartups\": 3,\n  \"mcpServers\": {{\n    \"github\": {{\n      \"type\": \"http\",\n      \"url\": \"https://api.githubcopilot.com/mcp/\"\n    }}{oxide}\n  }}\n}}\n"
        ),
    )?;
    let desktop = locations
        .local
        .join(r"Packages\Claude_pzs8sxrjxfjjc\LocalCache\Roaming\Claude");
    std::fs::create_dir_all(&desktop)?;
    let mut text = "{\n  \"mcpServers\": {\n    \"filesystem\": {\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\"]\n    }\n  }\n}\n".to_string();
    if connected {
        // The bridge copy the entry runs, so it reads as configured (for another port).
        clients::install_bridge(locations).map_err(anyhow::Error::msg)?;
        let old = Wanted {
            port: crate::mcp::DEFAULT_PORT,
            token: None,
        };
        text = mcp_link::json_edit::set(
            &text,
            "mcpServers",
            clients::NAME,
            &clients::desktop_server(locations, &old),
        )
        .map_err(anyhow::Error::msg)?;
    }
    std::fs::write(desktop.join("claude_desktop_config.json"), text)?;
    Ok(())
}

/// The section's own state while the dialog is open: the port as typed.
pub struct Panel {
    port: String,
    error: Option<String>,
}

impl Panel {
    pub fn new(port: u16) -> Self {
        Self {
            port: port.to_string(),
            error: None,
        }
    }
}

/// The port typed, if it is one the settings take.
pub fn parse_port(text: &str) -> Result<u16, String> {
    text.trim()
        .parse::<u16>()
        .ok()
        .filter(|p| PORTS.contains(p))
        .ok_or_else(|| {
            format!(
                "\"{}\" isn't a port Oxide can use: a whole number from {} to {} (those below \
                 {} are the system's)",
                text.trim(),
                PORTS.start(),
                PORTS.end(),
                PORTS.start()
            )
        })
}

/// The labels' column, as the rest of the settings window.
const LABEL_WIDTH: f32 = 120.0;
/// Connect Claude's status column.
const STATUS_WIDTH: f32 = 190.0;

/// The section's rows (the settings window draws its heading): the server on or off, its
/// port and token, its status with Retry, the command that connects Claude Code, and
/// "Connect Claude".
pub fn show(ui: &mut egui::Ui, panel: &mut Panel, view: &View) -> Option<Change> {
    let mut change = None;
    // Every change keeps the port in use unless the port itself changed.
    let mut changed = view.settings.clone();
    changed.port = view.port;
    // Each row marked for the manual (27.5): `settings.mcp.<mark>`.
    let row = |ui: &mut egui::Ui,
               mark: &'static str,
               label: &str,
               hint: &str,
               add: &mut dyn FnMut(&mut egui::Ui)| {
        let ((), rect) = theme::setting_row_at(ui, LABEL_WIDTH, label, hint, add);
        crate::marks::put(ui, mark, rect);
    };
    row(
        ui,
        "settings.mcp.server",
        "Server",
        "Only programs on this computer can connect; with a token, only those that send it.",
        &mut |ui| {
            let mut enabled = view.settings.enabled;
            if ui
                .add(crate::theme::checkbox(&mut enabled, "Enabled"))
                .changed()
            {
                change = Some(Change::Settings(McpSettings {
                    enabled,
                    ..changed.clone()
                }));
            }
        },
    );
    let port_hint = format!(
        "{} to {}; applies on Enter or when you click elsewhere.",
        PORTS.start(),
        PORTS.end()
    );
    row(ui, "settings.mcp.port", "Port", &port_hint, &mut |ui| {
        let field = ui.add(
            egui::TextEdit::singleline(&mut panel.port)
                .desired_width(64.0)
                .char_limit(5),
        );
        if field.changed() {
            panel.error = None;
        }
        if field.lost_focus() {
            match parse_port(&panel.port) {
                Ok(port) => {
                    panel.port = port.to_string();
                    if port != view.port {
                        change = Some(Change::Settings(McpSettings {
                            port,
                            ..changed.clone()
                        }));
                    }
                }
                Err(e) => panel.error = Some(e),
            }
        }
        if let Some(error) = &panel.error {
            ui.add(egui::Label::new(RichText::new(error).color(theme::RECORD)).wrap());
        }
    });
    row(
        ui,
        "settings.mcp.token",
        "Token",
        "Clients then send it as \"Authorization: Bearer <token>\"; the command below \
         includes it. A new one locks out clients with the old one at once.",
        &mut |ui| {
            ui.horizontal(|ui| {
                let mut require = view.settings.token.is_some();
                if ui
                    .add(crate::theme::checkbox(&mut require, "Require a token"))
                    .changed()
                {
                    let token = if require {
                        crate::mcp::new_token().map_err(|e| log::error!("{e}")).ok()
                    } else {
                        None
                    };
                    change = Some(Change::Settings(McpSettings {
                        token,
                        ..changed.clone()
                    }));
                }
                if let Some(token) = &view.settings.token {
                    // Shown by its end only: Copy has all of it.
                    let tail = &token[token.len().saturating_sub(6)..];
                    ui.label(
                        RichText::new(format!("…{tail}"))
                            .monospace()
                            .color(theme::TEXT_DIM),
                    );
                    if ui.button("New token").clicked() {
                        if let Ok(token) = crate::mcp::new_token() {
                            change = Some(Change::Settings(McpSettings {
                                token: Some(token),
                                ..changed.clone()
                            }));
                        }
                    }
                }
            });
        },
    );
    row(ui, "settings.mcp.status", "Status", "", &mut |ui| {
        ui.horizontal(|ui| {
            let text = RichText::new(view.shown(view.status.text()));
            if view.status.failed() {
                ui.label(text.color(theme::RECORD));
                if view.status.retryable()
                    && ui
                        .button("Retry")
                        .on_hover_text("Start the server again on this port (once it is free)")
                        .clicked()
                {
                    change = Some(Change::Retry);
                }
            } else {
                ui.label(text);
            }
        });
    });
    // The command only means something while the server runs (or is starting).
    let on = matches!(
        view.status,
        Status::Running { .. } | Status::Starting { .. }
    );
    let command_hint = if on {
        "Adds Oxide to Claude Code over HTTP, which connects only while Oxide runs; Connect \
         Claude below adds it through the bridge instead, so it stays listed while Oxide is \
         closed. The token stays hidden here; Copy has all of it."
    } else if !view.settings.enabled {
        "The server is off: turn it on above to connect a client."
    } else {
        "The server isn't running (see Status): a client can connect once it is."
    };
    row(
        ui,
        "settings.mcp.command",
        "Command",
        command_hint,
        &mut |ui| {
            ui.add_enabled_ui(on, |ui| {
                // The token stays off the screen; Copy has all of it.
                let shown = match &view.token {
                    Some(token) => view.command.replace(token.as_str(), "<token>"),
                    None => view.command.clone(),
                };
                let shown = view.shown(shown);
                ui.horizontal_top(|ui| {
                    egui::Frame::new()
                        .fill(theme::BG)
                        .corner_radius(3)
                        .inner_margin(egui::Margin::symmetric(6, 4))
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width() - 56.0);
                            ui.add(
                                egui::Label::new(RichText::new(shown).monospace().size(11.5))
                                    .wrap(),
                            );
                        });
                    if ui.button("Copy").clicked() {
                        ui.ctx().copy_text(view.command.clone());
                    }
                });
            });
        },
    );
    if let Some(link) = show_link(ui, &view.link, view.shown_port, &|t| view.shown(t)) {
        change = Some(link);
    }
    change
}

/// A status's colour: green when current, amber when it needs Reconfigure, red unreadable.
fn status_color(status: &clients::Status) -> egui::Color32 {
    match status {
        clients::Status::Configured { .. } => theme::SOLO,
        clients::Status::Unreadable(_) => theme::RECORD,
        s if s.can_reconfigure() => theme::MUTE,
        _ => theme::TEXT_DIM,
    }
}

/// "Connect Claude": a row per client.
fn show_link(
    ui: &mut egui::Ui,
    link: &LinkView,
    port: u16,
    shown: &dyn Fn(String) -> String,
) -> Option<Change> {
    let mut change = None;
    ui.separator();
    ui.horizontal(|ui| {
        ui.label(RichText::new("Connect Claude").size(15.0).strong());
        theme::button_row(ui, |ui| {
            if link.available
                && ui
                    .add_enabled(
                        !link.checking && link.working.is_none(),
                        egui::Button::new("Check again"),
                    )
                    .on_hover_text("Read Claude's configs again (after changing them elsewhere)")
                    .clicked()
            {
                change = Some(Change::CheckLink);
            }
        });
    });
    ui.add(
        egui::Label::new(theme::hint(format!(
            "Adds Oxide (port {port}, with the token if one is required) to Claude Code's user \
             settings and to Claude Desktop, both through oxide-mcp.exe: Oxide stays listed \
             while it is closed, and its tools work once it runs. Reconfigure after changing \
             the port or the token."
        )))
        .wrap(),
    );
    ui.add_space(4.0);
    if !link.available {
        ui.label(
            RichText::new("Claude's configs can't be found: no home or AppData folder")
                .color(theme::RECORD),
        );
        return None;
    }
    let grid = egui::Grid::new("claude-link")
        .num_columns(3)
        .min_col_width(LABEL_WIDTH)
        .spacing([6.0, 6.0])
        .show(ui, |ui| {
            for client in [Client::Code, Client::Desktop] {
                ui.label(client.name());
                let status = link.detected.as_ref().map(|d| match client {
                    Client::Code => &d.code,
                    Client::Desktop => &d.desktop,
                });
                let busy = link.working == Some(client);
                // A column of its own width, a long status wrapping in it, so the buttons
                // keep their place.
                ui.horizontal_wrapped(|ui| {
                    ui.set_width(STATUS_WIDTH);
                    match (busy, status) {
                        (true, _) => {
                            ui.spinner();
                            ui.label(RichText::new("Changing…").color(theme::TEXT_DIM));
                        }
                        (false, None) => {
                            ui.label(RichText::new("Checking…").color(theme::TEXT_DIM));
                        }
                        (false, Some(status)) => {
                            let label = ui.add(
                                egui::Label::new(RichText::new(shown(status.text())).color(status_color(status)))
                                    .wrap(),
                            );
                            let hover = match (client, &link.detected) {
                                (Client::Desktop, Some(d)) if !d.desktop_configs.is_empty() => d
                                    .desktop_configs
                                    .iter()
                                    .map(|(path, s)| format!("{}\n    {}", path.display(), s.text()))
                                    .collect::<Vec<_>>()
                                    .join("\n"),
                                (Client::Desktop, _) => "Neither the installer's nor the Store build's Claude Desktop was found".into(),
                                (Client::Code, _) => link.code_config.clone().unwrap_or_default(),
                            };
                            label.on_hover_text(hover);
                        }
                    }
                });
                ui.horizontal(|ui| {
                    let Some(detected) = &link.detected else {
                        return;
                    };
                    let status = status.unwrap_or(&clients::Status::NotInstalled);
                    // What's missing for the client's changes: Claude Code's CLI, the bridge
                    // (both clients run it).
                    let missing = match client {
                        Client::Code => (!detected.cli)
                            .then(|| {
                                "Claude Code's `claude` command wasn't found on the PATH".to_string()
                            })
                            .or_else(|| detected.no_bridge.clone()),
                        Client::Desktop => detected.no_bridge.clone(),
                    };
                    let idle = link.working.is_none() && !link.checking;
                    let why_not = |ok: bool, otherwise: &str| -> String {
                        missing.clone().unwrap_or_else(|| if ok { String::new() } else { otherwise.into() })
                    };
                    let configure = status.can_configure() && missing.is_none() && idle;
                    if ui
                        .add_enabled(configure, egui::Button::new("Configure"))
                        .on_hover_text(format!("Add Oxide to {} for port {port}", client.name()))
                        .on_disabled_hover_text(why_not(
                            status.can_configure(),
                            "Already configured (or not installed): Reconfigure replaces it",
                        ))
                        .clicked()
                    {
                        change = Some(Change::Link(client, Act::Configure));
                    }
                    let reconfigure = status.can_reconfigure() && missing.is_none() && idle;
                    if ui
                        .add_enabled(reconfigure, egui::Button::new("Reconfigure"))
                        .on_hover_text(format!(
                            "Remove Oxide's entries from {} and add it again for port {port}",
                            client.name()
                        ))
                        .on_disabled_hover_text(why_not(status.can_reconfigure(), "Not configured: Configure adds it"))
                        .clicked()
                    {
                        change = Some(Change::Link(client, Act::Reconfigure));
                    }
                    // Removing needs no bridge.
                    let remove = status.can_reconfigure()
                        && idle
                        && (client == Client::Desktop || detected.cli);
                    if ui
                        .add_enabled(remove, egui::Button::new("Remove"))
                        .on_hover_text(format!("Remove Oxide's entries from {}", client.name()))
                        .clicked()
                    {
                        change = Some(Change::Link(client, Act::Remove));
                    }
                });
                ui.end_row();
            }
        });
    crate::marks::put(ui, "settings.mcp.connect", grid.response.rect);
    match &link.message {
        Some(Ok(said)) => {
            ui.add(egui::Label::new(theme::hint(said.as_str())).wrap());
        }
        Some(Err(why)) => {
            let text = RichText::new(why)
                .size(theme::HINT_SIZE)
                .color(theme::RECORD);
            ui.add(egui::Label::new(text).wrap());
        }
        None => {}
    }
    change
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_ports_from_1024_are_taken() {
        assert_eq!(parse_port(" 7899 "), Ok(7899));
        assert_eq!(parse_port("1024"), Ok(1024));
        assert_eq!(parse_port("65535"), Ok(65535));
        for bad in ["80", "1023", "0", "65536", "", "78a9", "-1"] {
            let e = parse_port(bad).unwrap_err();
            assert!(e.contains("1024 to 65535"), "{bad}: {e}");
        }
    }
}
