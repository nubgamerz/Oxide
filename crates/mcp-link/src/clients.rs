//! Claude Code and Claude Desktop: where their configs are, whether Oxide is in them, and
//! putting it there (Settings > AI control (MCP), dev plan 17.3).
//!
//! Both clients run the bridge (`oxide-mcp.exe --port N [--token T]`, stdio), which lists
//! Oxide's tools whether or not Oxide runs (23.3), so Oxide stays in Claude while it is closed
//! and its tools work as soon as it starts, with nothing to reconnect.
//!
//! - **Claude Code** keeps user-scope servers in `.claude.json` (in `CLAUDE_CONFIG_DIR` when
//!   set, else the home folder), under `mcpServers`. It is read directly to see whether Oxide
//!   is there: `claude mcp get` would also do it, but it connects to every server it reports
//!   (half a second or more), prints the token in full and has no stable output. Changes go
//!   through the CLI (`claude mcp add --transport stdio --scope user oxide -- <bridge> --port
//!   N`, `remove`), which owns that file and rewrites it often. Before 23.3 Oxide was added
//!   over HTTP (`--transport http <url>`), which fails at Claude Code's start while Oxide is
//!   closed: such an entry reads as [`Status::OverHttp`], and Reconfigure moves it.
//! - **Claude Desktop** only starts local programs (stdio). Its `claude_desktop_config.json` is in
//!   `%APPDATA%\Claude` for the .exe installer, and in the package's
//!   `LocalCache\Roaming\Claude` for the Store (MSIX) build: a packaged app reads that private
//!   copy first, so a file written to `%APPDATA%` would never be seen. The entry is edited in
//!   the text ([`crate::json_edit`]), every other byte kept, a copy of the file written beside
//!   it first (`.oxide-backup`) and the new file put in place by a rename.
//!
//! Both run one copy of the bridge (`mcp-bridge\oxide-mcp.exe` beside the app), not the one
//! the build makes: the clients keep their servers running while they are open (Claude Code
//! one per session), and a running `.exe` can't be overwritten, so pointing them at the
//! build's own would make every build fail while one is open. The copy is refreshed by
//! Configure and Reconfigure (a running one is moved aside first, [`install_bridge`]), so the
//! clients sharing it costs nothing.
//!
//! Everything takes a [`Locations`]: the app builds it from the environment, checks and tests
//! from a folder of their own, so they never touch the user's files.

use crate::json_edit;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The server's name in both clients.
pub const NAME: &str = "oxide";

/// The name the settings' copyable command used before 17.3. An entry under it that points at
/// a loopback `/mcp` address is taken for Oxide's, and Reconfigure or Remove replace it.
pub const OLD_NAME: &str = "daw";

/// The member both clients keep their servers in.
const SERVERS: &str = "mcpServers";

/// The bridge's file name.
pub const BRIDGE: &str = "oxide-mcp.exe";

/// Claude Desktop's config file name.
const DESKTOP_CONFIG: &str = "claude_desktop_config.json";

/// How long a `claude mcp` command may take.
const CLI_LIMIT: Duration = Duration::from_secs(60);

/// Where the clients and the bridge are.
#[derive(Clone, Debug)]
pub struct Locations {
    /// The home folder (`%USERPROFILE%`): Claude Code's `.claude.json` unless `code_dir` is set,
    /// and where the CLI runs.
    pub home: PathBuf,
    /// `%APPDATA%`.
    pub roaming: PathBuf,
    /// `%LOCALAPPDATA%`.
    pub local: PathBuf,
    /// `CLAUDE_CONFIG_DIR`: Claude Code's folder when set (the CLI is given it too).
    pub code_dir: Option<PathBuf>,
    /// The `claude` command, if found.
    pub cli: Option<PathBuf>,
    /// The bridge the build made (beside `daw.exe`).
    pub bridge_source: PathBuf,
    /// The copy both clients run.
    pub bridge: PathBuf,
}

impl Locations {
    /// The user's own, from the environment; the bridge beside `exe_dir`. `None` without a home
    /// folder or app data folders (not Windows as we know it).
    pub fn from_env(exe_dir: &Path) -> Option<Self> {
        let var = |name: &str| {
            std::env::var_os(name)
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
        };
        let home = var("USERPROFILE").or_else(|| var("HOME"))?;
        Some(Self {
            roaming: var("APPDATA")?,
            local: var("LOCALAPPDATA")?,
            code_dir: var("CLAUDE_CONFIG_DIR"),
            cli: find_cli(std::env::var_os("PATH"), &home),
            home,
            bridge_source: exe_dir.join(BRIDGE),
            bridge: exe_dir.join("mcp-bridge").join(BRIDGE),
        })
    }

    /// A profile of its own under `root`, for checks and tests: home, app data and Claude
    /// Code's folder (`claude-code`, given to the CLI as `CLAUDE_CONFIG_DIR`) all inside it,
    /// and the clients' bridge copied to `root\bridge`. `cli` is the `claude` to run, if any.
    pub fn profile(root: &Path, cli: Option<PathBuf>, bridge_source: PathBuf) -> Self {
        Self {
            home: root.join("home"),
            roaming: root.join("AppData").join("Roaming"),
            local: root.join("AppData").join("Local"),
            code_dir: Some(root.join("claude-code")),
            cli,
            bridge_source,
            bridge: root.join("bridge").join(BRIDGE),
        }
    }

    /// Claude Code's user config.
    pub fn code_config(&self) -> PathBuf {
        self.code_dir
            .as_deref()
            .unwrap_or(&self.home)
            .join(".claude.json")
    }

    /// Claude Desktop's configs that apply: the .exe installer's (when it is installed, or its
    /// config exists) and each Store package's. Empty: Desktop isn't installed.
    pub fn desktop_configs(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        let classic = self.roaming.join("Claude").join(DESKTOP_CONFIG);
        // `%APPDATA%\Claude` alone proves nothing: other tools put things there (skills).
        if classic.is_file() || self.local.join("AnthropicClaude").is_dir() {
            found.push(classic);
        }
        if let Ok(entries) = std::fs::read_dir(self.local.join("Packages")) {
            let mut packages: Vec<PathBuf> = entries
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.is_dir())
                .collect();
            packages.sort();
            for package in packages {
                let name = package
                    .file_name()
                    .map(|n| n.to_string_lossy().to_ascii_lowercase())
                    .unwrap_or_default();
                let folder = package.join("LocalCache").join("Roaming").join("Claude");
                // `Claude_<publisher id>` is Anthropic's package; another name with Claude in
                // it counts only if it has Desktop's folder.
                if name.starts_with("claude_") || (name.contains("claude") && folder.is_dir()) {
                    found.push(folder.join(DESKTOP_CONFIG));
                }
            }
        }
        // The same file twice (a process inside the package sees `%APPDATA%` redirected).
        let mut seen = Vec::new();
        found.retain(|path| {
            let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.clone());
            let new = !seen.contains(&key);
            seen.push(key);
            new
        });
        found
    }
}

/// The `claude` command: on the `PATH` (`claude.exe`, or npm's `claude.cmd`), else where the
/// native installer puts it (`~\.local\bin`).
pub fn find_cli(path: Option<std::ffi::OsString>, home: &Path) -> Option<PathBuf> {
    let names = ["claude.exe", "claude.cmd"];
    let on_path = path
        .iter()
        .flat_map(std::env::split_paths)
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)));
    let installed = names
        .iter()
        .map(|name| home.join(".local").join("bin").join(name));
    on_path.chain(installed).find(|p| p.is_file())
}

/// The port and token the settings ask for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Wanted {
    pub port: u16,
    pub token: Option<String>,
}

impl Wanted {
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/mcp", self.port)
    }
}

/// Whether a client has Oxide, as the settings show it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    NotInstalled,
    NotConfigured,
    /// Configured for the port and token asked for.
    Configured {
        port: u16,
    },
    /// Configured for another port (`None`: an address that isn't Oxide's form).
    OtherPort {
        port: Option<u16>,
    },
    /// The right port with an old token (or none where one is required).
    OtherToken {
        port: u16,
    },
    /// Under the old name ([`OLD_NAME`]).
    OtherName {
        name: String,
        port: Option<u16>,
    },
    /// Desktop runs something other than this app's bridge for it.
    OtherProgram {
        port: Option<u16>,
    },
    /// The client runs this app's bridge copy, but it isn't there (the build folder was
    /// cleaned or the app moved): it can't start it until Reconfigure copies it again.
    BridgeMissing {
        port: Option<u16>,
    },
    /// Claude Code reaches Oxide over HTTP (before 23.3), which fails while Oxide is closed:
    /// Reconfigure moves it to the bridge.
    OverHttp {
        port: Option<u16>,
    },
    /// Configured as asked, but the bridge copy the client runs isn't the one beside the app
    /// (Oxide was updated since): it lists the old version's tools while Oxide is closed, or
    /// behaves as an old bridge did. Reconfigure copies the new one.
    BridgeStale {
        port: u16,
    },
    /// In some of Desktop's configs only.
    Partly {
        configured: usize,
        of: usize,
    },
    /// A config that couldn't be read or parsed.
    Unreadable(String),
}

impl Status {
    pub fn text(&self) -> String {
        let at = |port: &Option<u16>| port.map(|p| format!(" (port {p})")).unwrap_or_default();
        match self {
            Self::NotInstalled => "Not installed".into(),
            Self::NotConfigured => "Not configured".into(),
            Self::Configured { port } => format!("Configured (port {port})"),
            Self::OtherPort { port: Some(port) } => format!("Configured for another port ({port})"),
            Self::OtherPort { port: None } => "Configured for another address".into(),
            Self::OtherToken { port } => format!("Configured (port {port}) with an old token"),
            Self::OtherName { name, port } => format!("Configured as \"{name}\"{}", at(port)),
            Self::OtherProgram { port } => {
                format!("Configured with another program{}", at(port))
            }
            Self::BridgeMissing { port } => {
                format!("Configured{}, but its bridge is missing", at(port))
            }
            Self::BridgeStale { port } => {
                format!("Configured (port {port}), but its bridge is out of date")
            }
            Self::OverHttp { port } => {
                format!(
                    "Configured over HTTP{}: Reconfigure to use the bridge",
                    at(port)
                )
            }
            Self::Partly { configured, of } => {
                format!("Configured in {configured} of {of} configs")
            }
            Self::Unreadable(why) => format!("Couldn't read the config: {why}"),
        }
    }

    /// Configure is offered only where Oxide isn't configured at all.
    pub fn can_configure(&self) -> bool {
        matches!(self, Self::NotConfigured)
    }

    /// Reconfigure and Remove: wherever an entry of Oxide's is.
    pub fn can_reconfigure(&self) -> bool {
        !matches!(
            self,
            Self::NotInstalled | Self::NotConfigured | Self::Unreadable(_)
        )
    }

    /// Configured as asked: nothing to do.
    pub fn is_current(&self) -> bool {
        matches!(self, Self::Configured { .. })
    }

    /// Several configs' statuses as one.
    fn combine(statuses: Vec<Status>) -> Status {
        let Some(first) = statuses.first().cloned() else {
            return Status::NotInstalled;
        };
        if let Some(bad) = statuses.iter().find(|s| matches!(s, Status::Unreadable(_))) {
            return bad.clone();
        }
        if statuses.iter().all(|s| *s == first) {
            return first;
        }
        let configured = statuses.iter().filter(|s| s.can_reconfigure()).count();
        if configured < statuses.len() {
            return Status::Partly {
                configured,
                of: statuses.len(),
            };
        }
        statuses
            .into_iter()
            .find(|s| !s.is_current())
            .unwrap_or(first)
    }
}

/// The port in a loopback MCP address (`http://127.0.0.1:N/mcp`, or `localhost`).
fn loopback_port(url: &str) -> Option<u16> {
    let rest = url.strip_prefix("http://")?;
    let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (name, port) = host.rsplit_once(':')?;
    let loopback = matches!(name, "127.0.0.1" | "localhost" | "[::1]");
    (loopback && path.trim_end_matches('/') == "mcp")
        .then(|| port.parse().ok())
        .flatten()
}

/// Claude Code's entry against what is asked for: the bridge as Desktop's is, or the HTTP
/// address it was given before 23.3.
fn code_entry(entry: &Value, wanted: &Wanted, bridge: &Path) -> Status {
    match entry.get("url").and_then(Value::as_str) {
        Some(url) => match loopback_port(url) {
            Some(port) => Status::OverHttp { port: Some(port) },
            None => Status::OtherPort { port: None },
        },
        None => bridge_entry(entry, wanted, bridge),
    }
}

/// An option's value in a bridge entry's `args`.
fn arg<'a>(args: &'a [Value], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a.as_str() == Some(name))?;
    args.get(at + 1)?.as_str()
}

/// Whether two paths name the same file, as Windows compares them.
fn same_path(a: &str, b: &Path) -> bool {
    let norm = |s: &str| s.replace('/', "\\").to_lowercase();
    norm(a) == norm(&b.to_string_lossy())
}

/// The bridge's file name ends `command`.
fn runs_bridge(command: &str) -> bool {
    command
        .replace('/', "\\")
        .rsplit('\\')
        .next()
        .is_some_and(|name| {
            name.eq_ignore_ascii_case(BRIDGE) || name.eq_ignore_ascii_case("oxide-mcp")
        })
}

/// An entry that runs a program (Desktop's, and Claude Code's since 23.3) against what is
/// asked for: this app's bridge copy with the port and token.
fn bridge_entry(entry: &Value, wanted: &Wanted, bridge: &Path) -> Status {
    let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
    let args = entry
        .get("args")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let port = arg(args, "--port").and_then(|p| p.parse().ok());
    if !same_path(command, bridge) {
        // mcp-remote and the like name the address in their arguments.
        let port = port.or_else(|| {
            args.iter()
                .filter_map(Value::as_str)
                .find_map(loopback_port)
        });
        return Status::OtherProgram { port };
    }
    if !bridge.is_file() {
        return Status::BridgeMissing { port };
    }
    let Some(port) = port else {
        return Status::OtherPort { port: None };
    };
    if port != wanted.port {
        return Status::OtherPort { port: Some(port) };
    }
    match &wanted.token {
        Some(token) if arg(args, "--token") != Some(token.as_str()) => Status::OtherToken { port },
        _ => Status::Configured { port },
    }
}

/// Whether an entry under the old name is Oxide's: a loopback `/mcp` address, or the bridge.
fn old_entry_is_ours(entry: &Value) -> bool {
    let url = entry.get("url").and_then(Value::as_str);
    let command = entry.get("command").and_then(Value::as_str).unwrap_or("");
    let args = entry.get("args").and_then(Value::as_array);
    url.and_then(loopback_port).is_some()
        || runs_bridge(command)
        || args.is_some_and(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .any(|s| loopback_port(s).is_some())
        })
}

/// The status of a config's servers (`servers` is its `mcpServers`, if any).
fn status_of(servers: Option<&Value>, classify: impl Fn(&Value) -> Status) -> Status {
    let Some(servers) = servers else {
        return Status::NotConfigured;
    };
    if let Some(entry) = servers.get(NAME) {
        return classify(entry);
    }
    match servers.get(OLD_NAME) {
        Some(old) if old_entry_is_ours(old) => {
            let port = match classify(old) {
                Status::Configured { port }
                | Status::OtherToken { port }
                | Status::BridgeStale { port } => Some(port),
                Status::OtherPort { port }
                | Status::OtherProgram { port }
                | Status::BridgeMissing { port }
                | Status::OverHttp { port } => port,
                _ => None,
            };
            Status::OtherName {
                name: OLD_NAME.into(),
                port,
            }
        }
        _ => Status::NotConfigured,
    }
}

/// A config file's text, or `None` if it doesn't exist.
fn read(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read(path) {
        Ok(bytes) => String::from_utf8(bytes)
            .map(Some)
            .map_err(|_| format!("{} isn't UTF-8 text", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `status` with "configured" turned to [`Status::BridgeStale`] when the bridge copy the
/// clients run differs from the one beside the app (none beside it: nothing to compare).
fn with_bridge_age(status: Status, locations: &Locations) -> Status {
    let Status::Configured { port } = status else {
        return status;
    };
    match std::fs::read(&locations.bridge_source) {
        Ok(source) if std::fs::read(&locations.bridge).ok().as_deref() != Some(&source[..]) => {
            Status::BridgeStale { port }
        }
        _ => status,
    }
}

/// Claude Code's status.
pub fn code_status(locations: &Locations, wanted: &Wanted) -> Status {
    let path = locations.code_config();
    match read(&path) {
        Err(e) => Status::Unreadable(e),
        Ok(None) if locations.cli.is_none() => Status::NotInstalled,
        Ok(None) => Status::NotConfigured,
        Ok(Some(text)) => match json_edit::parse(&text) {
            Err(e) => Status::Unreadable(format!("{}: {e}", path.display())),
            Ok(value) => status_of(value.get(SERVERS), |e| {
                with_bridge_age(code_entry(e, wanted, &locations.bridge), locations)
            }),
        },
    }
}

/// Claude Desktop's status over its configs (each one's too).
pub fn desktop_status(locations: &Locations, wanted: &Wanted) -> (Status, Vec<(PathBuf, Status)>) {
    let each: Vec<(PathBuf, Status)> = locations
        .desktop_configs()
        .into_iter()
        .map(|path| {
            let status = match read(&path) {
                Err(e) => Status::Unreadable(e),
                Ok(None) => Status::NotConfigured,
                Ok(Some(text)) => match json_edit::parse(&text) {
                    Err(e) => Status::Unreadable(format!("{}: {e}", path.display())),
                    Ok(value) => status_of(value.get(SERVERS), |e| {
                        with_bridge_age(bridge_entry(e, wanted, &locations.bridge), locations)
                    }),
                },
            };
            (path, status)
        })
        .collect();
    (
        Status::combine(each.iter().map(|(_, s)| s.clone()).collect()),
        each,
    )
}

/// Both clients at once, as the settings show them.
#[derive(Clone, Debug)]
pub struct Detected {
    pub code: Status,
    pub desktop: Status,
    /// Desktop's configs and each one's status.
    pub desktop_configs: Vec<(PathBuf, Status)>,
    /// Whether Claude Code's CLI was found (it makes Code's changes).
    pub cli: bool,
    /// Why neither client can be configured from here, if they can't (no bridge beside the
    /// app); removing needs no bridge.
    pub no_bridge: Option<String>,
}

pub fn detect(locations: &Locations, wanted: &Wanted) -> Detected {
    let (desktop, desktop_configs) = desktop_status(locations, wanted);
    Detected {
        code: code_status(locations, wanted),
        desktop,
        desktop_configs,
        cli: locations.cli.is_some(),
        no_bridge: (!locations.bridge_source.is_file()).then(|| {
            format!(
                "{BRIDGE} isn't beside the app ({}): build it (cargo build -p daw-app) or copy it there",
                locations.bridge_source.display()
            )
        }),
    }
}

/// Which client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    Code,
    Desktop,
}

impl Client {
    pub fn name(self) -> &'static str {
        match self {
            Self::Code => "Claude Code",
            Self::Desktop => "Claude Desktop",
        }
    }
}

/// What to do to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    /// Add Oxide (offered only when it isn't there).
    Configure,
    /// Remove Oxide's entries (the old name's too), then add it for the current port and token.
    Reconfigure,
    /// Remove Oxide's entries.
    Remove,
}

/// Do `act` to `client`: a line to show on success, the reason on failure.
pub fn apply(
    locations: &Locations,
    client: Client,
    act: Act,
    wanted: &Wanted,
) -> Result<String, String> {
    match client {
        Client::Code => code_apply(locations, act, wanted),
        Client::Desktop => desktop_apply(locations, act, wanted),
    }
}

/// `text` with the token, if any, replaced (CLI output is shown and may echo it).
fn redact(text: &str, wanted: &Wanted) -> String {
    match &wanted.token {
        Some(token) if !token.is_empty() => text.replace(token.as_str(), "<token>"),
        _ => text.to_string(),
    }
}

/// Run `claude <args>`: its output, or why it failed (redacted).
fn cli(
    locations: &Locations,
    args: &[String],
    wanted: &Wanted,
) -> Result<String, (Option<i32>, String)> {
    let program = locations.cli.as_ref().ok_or((
        None,
        "Claude Code's `claude` command wasn't found on the PATH".to_string(),
    ))?;
    let mut command = std::process::Command::new(program);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    // In the home folder, where a terminal's `claude` would start (user scope doesn't depend
    // on it, but nothing then reads a project's `.mcp.json` from the app's folder).
    if locations.home.is_dir() {
        command.current_dir(&locations.home);
    }
    if let Some(dir) = &locations.code_dir {
        command.env("CLAUDE_CONFIG_DIR", dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: the app has no console, and a console program run from it would
        // flash one up.
        command.creation_flags(0x0800_0000);
    }
    let mut child = command
        .spawn()
        .map_err(|e| (None, format!("couldn't run {}: {e}", program.display())))?;
    // Read both pipes on threads, so a full pipe never stalls the command.
    let pipe = |r: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut text = String::new();
            if let Some(mut r) = r {
                let _ = r.read_to_string(&mut text);
            }
            text
        })
    };
    let out = pipe(child.stdout.take().map(|s| Box::new(s) as _));
    let err = pipe(child.stderr.take().map(|s| Box::new(s) as _));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() > CLI_LIMIT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err((
                    None,
                    format!(
                        "`claude {}` took over {} s",
                        args.first().map_or("", String::as_str),
                        CLI_LIMIT.as_secs()
                    ),
                ));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(25)),
            Err(e) => return Err((None, e.to_string())),
        }
    };
    let text = format!(
        "{}{}",
        out.join().unwrap_or_default(),
        err.join().unwrap_or_default()
    );
    let text = redact(text.trim(), wanted);
    if status.success() {
        Ok(text)
    } else {
        Err((status.code(), text))
    }
}

/// `claude mcp remove <name> --scope user`; a name that isn't there is fine.
fn code_remove(locations: &Locations, name: &str, wanted: &Wanted) -> Result<(), String> {
    let args = ["mcp", "remove", name, "--scope", "user"].map(String::from);
    match cli(locations, &args, wanted) {
        Ok(_) => Ok(()),
        Err((Some(_), text)) if text.contains("No MCP server") => Ok(()),
        Err((_, text)) => Err(format!("removing \"{name}\" from Claude Code: {text}")),
    }
}

fn code_apply(locations: &Locations, act: Act, wanted: &Wanted) -> Result<String, String> {
    let before = code_status(locations, wanted);
    if let Status::Unreadable(why) = &before {
        return Err(format!("Claude Code's config wasn't changed: {why}"));
    }
    if act == Act::Configure && !before.can_configure() {
        return Err(format!(
            "Claude Code is already {}",
            before.text().to_lowercase()
        ));
    }
    // The bridge first: without it nothing is removed either.
    if act != Act::Remove {
        install_bridge(locations)?;
    }
    if act != Act::Configure {
        code_remove(locations, NAME, wanted)?;
        if matches!(before, Status::OtherName { .. }) {
            code_remove(locations, OLD_NAME, wanted)?;
        }
        if act == Act::Remove {
            return Ok("Claude Code: Oxide removed".into());
        }
    }
    let mut args: Vec<String> = [
        "mcp",
        "add",
        "--transport",
        "stdio",
        "--scope",
        "user",
        NAME,
    ]
    .map(String::from)
    .to_vec();
    args.extend(code_command(locations, wanted));
    cli(locations, &args, wanted)
        .map_err(|(_, text)| format!("adding Oxide to Claude Code: {text}"))?;
    match code_status(locations, wanted) {
        Status::Configured { port } => Ok(format!(
            "Claude Code: connected through oxide-mcp.exe on port {port} (user scope); new \
             sessions list Oxide's tools, which work while Oxide runs"
        )),
        other => Err(format!(
            "Claude Code took the command, but its config now reads \"{}\"",
            other.text()
        )),
    }
}

/// What follows `claude mcp add ... oxide`: `--`, the bridge and its arguments (the same as
/// Desktop's entry).
pub fn code_command(locations: &Locations, wanted: &Wanted) -> Vec<String> {
    let mut args = vec![
        "--".to_string(),
        locations.bridge.to_string_lossy().into_owned(),
    ];
    args.extend(bridge_args(wanted));
    args
}

/// The bridge's arguments for the port and token asked for.
fn bridge_args(wanted: &Wanted) -> Vec<String> {
    let mut args = vec!["--port".to_string(), wanted.port.to_string()];
    if let Some(token) = &wanted.token {
        args.push("--token".into());
        args.push(token.clone());
    }
    args
}

/// Put the bridge where the clients run it, unless the copy there is already the same. A copy
/// Desktop is running can't be replaced but can be renamed: it is moved aside first, and
/// older ones moved aside are cleared once nothing runs them.
pub fn install_bridge(locations: &Locations) -> Result<(), String> {
    let (source, target) = (&locations.bridge_source, &locations.bridge);
    let bytes = std::fs::read(source)
        .map_err(|e| format!("{BRIDGE} isn't beside the app ({}): {e}", source.display()))?;
    if std::fs::read(target).ok().as_deref() == Some(&bytes[..]) {
        return Ok(());
    }
    let folder = target
        .parent()
        .ok_or_else(|| format!("{} has no folder", target.display()))?;
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let fresh = folder.join(format!("{BRIDGE}.new"));
    std::fs::write(&fresh, &bytes).map_err(|e| format!("{}: {e}", fresh.display()))?;
    if std::fs::rename(&fresh, target).is_err() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis());
        let aside = folder.join(format!("oxide-mcp.old-{stamp}.exe"));
        std::fs::rename(target, &aside)
            .and_then(|()| std::fs::rename(&fresh, target))
            .map_err(|e| {
                let _ = std::fs::remove_file(&fresh);
                format!("{}: {e}", target.display())
            })?;
    }
    if let Ok(entries) = std::fs::read_dir(folder) {
        for entry in entries.flatten() {
            if entry
                .file_name()
                .to_string_lossy()
                .starts_with("oxide-mcp.old-")
            {
                // Fails while a Desktop still runs it; the next install tries again.
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    Ok(())
}

/// Desktop's entry for the bridge.
pub fn desktop_server(locations: &Locations, wanted: &Wanted) -> Value {
    json!({"command": locations.bridge.to_string_lossy(), "args": bridge_args(wanted)})
}

/// Write `text` to `path`: a copy of `original` beside it first (`.oxide-backup`), then the new
/// file under a temporary name in the same folder, renamed over the old one.
pub fn write_config(path: &Path, original: Option<&str>, text: &str) -> Result<(), String> {
    let at = |e: std::io::Error, p: &Path| format!("{}: {e}", p.display());
    let folder = path
        .parent()
        .ok_or_else(|| format!("{} has no folder", path.display()))?;
    std::fs::create_dir_all(folder).map_err(|e| at(e, folder))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    if let Some(original) = original {
        let backup = folder.join(format!("{name}.oxide-backup"));
        std::fs::write(&backup, original).map_err(|e| at(e, &backup))?;
    }
    let temp = folder.join(format!("{name}.oxide-tmp"));
    std::fs::write(&temp, text).map_err(|e| at(e, &temp))?;
    std::fs::rename(&temp, path).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        at(e, path)
    })
}

/// What `act` makes of one Desktop config's text (`None`: no change).
pub fn desktop_edit(text: &str, act: Act, server: &Value) -> Result<Option<String>, String> {
    let servers = json_edit::parse(text)?;
    let servers = servers.get(SERVERS);
    let mut edited = text.to_string();
    let mut changed = false;
    if act != Act::Configure {
        if let Some(t) = json_edit::remove(&edited, SERVERS, NAME)? {
            edited = t;
            changed = true;
        }
        if servers
            .and_then(|s| s.get(OLD_NAME))
            .is_some_and(old_entry_is_ours)
        {
            if let Some(t) = json_edit::remove(&edited, SERVERS, OLD_NAME)? {
                edited = t;
                changed = true;
            }
        }
    }
    if act != Act::Remove {
        edited = json_edit::set(&edited, SERVERS, NAME, server)?;
        changed = true;
    }
    Ok(changed.then_some(edited))
}

fn desktop_apply(locations: &Locations, act: Act, wanted: &Wanted) -> Result<String, String> {
    let configs = locations.desktop_configs();
    if configs.is_empty() {
        return Err("Claude Desktop isn't installed".into());
    }
    // Every file read and every edit made before anything is written: a malformed file stops
    // them all, so the configs never disagree.
    let server = desktop_server(locations, wanted);
    let mut plans = Vec::new();
    for path in &configs {
        let original = read(path).map_err(|e| format!("nothing was changed: {e}"))?;
        let text = original.clone().unwrap_or_default();
        let status = status_of(
            json_edit::parse(&text)
                .map_err(|e| {
                    format!(
                        "nothing was changed: {} {e}; fix or remove it first",
                        path.display()
                    )
                })?
                .get(SERVERS),
            |e| bridge_entry(e, wanted, &locations.bridge),
        );
        if act == Act::Configure && !status.can_configure() {
            continue;
        }
        if let Some(edited) = desktop_edit(&text, act, &server)
            .map_err(|e| format!("nothing was changed: {} {e}", path.display()))?
        {
            plans.push((path.clone(), original, edited));
        }
    }
    if act != Act::Remove {
        install_bridge(locations)?;
    }
    let mut written = Vec::new();
    for (path, original, edited) in &plans {
        write_config(path, original.as_deref(), edited)?;
        written.push(path.display().to_string());
    }
    if written.is_empty() {
        return Ok("Claude Desktop: nothing to change".into());
    }
    let done = if act == Act::Remove {
        "Oxide removed from"
    } else {
        "connected in"
    };
    Ok(format!(
        "Claude Desktop: {done} {}. Restart Claude Desktop to use it",
        written.join(" and ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanted(port: u16, token: Option<&str>) -> Wanted {
        Wanted {
            port,
            token: token.map(String::from),
        }
    }

    /// A folder of its own under the target directory (never the system drive's temp).
    fn scratch(name: &str) -> PathBuf {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-scratch/mcp-link")
            .join(name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn statuses_of_claude_code_entries() {
        let dir = std::fs::canonicalize(scratch("code-entries")).unwrap();
        let bridge = &dir.join(BRIDGE);
        std::fs::write(bridge, b"bridge").unwrap();
        let w = wanted(7878, None);
        let code = |e: &Value, w: &Wanted| code_entry(e, w, bridge);
        // What `claude mcp add --transport stdio --scope user oxide -- <bridge> --port N`
        // writes: the bridge, as Desktop's entry.
        let stdio = |args: &[&str]| json!({"type": "stdio", "command": bridge.to_string_lossy(), "args": args, "env": {}});
        assert_eq!(
            code(&stdio(&["--port", "7878"]), &w),
            Status::Configured { port: 7878 }
        );
        assert_eq!(
            code(&stdio(&["--port", "7899"]), &w),
            Status::OtherPort { port: Some(7899) }
        );
        let t = wanted(7878, Some("abc"));
        assert_eq!(
            code(&stdio(&["--port", "7878", "--token", "abc"]), &t),
            Status::Configured { port: 7878 }
        );
        assert_eq!(
            code(&stdio(&["--port", "7878"]), &t),
            Status::OtherToken { port: 7878 }
        );
        assert_eq!(
            code(
                &json!({"type": "stdio", "command": r"D:\elsewhere\oxide-mcp.exe", "args": ["--port", "7878"]}),
                &w
            ),
            Status::OtherProgram { port: Some(7878) }
        );
        // The HTTP address Connect Claude wrote before 23.3: Reconfigure moves it to the bridge.
        let http = |url: &str| json!({"type": "http", "url": url});
        let over = Status::OverHttp { port: Some(7878) };
        assert_eq!(code(&http("http://127.0.0.1:7878/mcp"), &w), over);
        assert_eq!(code(&http("http://localhost:7878/mcp/"), &t), over);
        assert_eq!(
            code(&http("http://127.0.0.1:7899/mcp"), &w),
            Status::OverHttp { port: Some(7899) }
        );
        assert_eq!(
            over.text(),
            "Configured over HTTP (port 7878): Reconfigure to use the bridge"
        );
        assert!(over.can_reconfigure() && !over.can_configure() && !over.is_current());
        assert_eq!(
            code(&http("https://example.com/mcp"), &w),
            Status::OtherPort { port: None }
        );

        let servers = |v: Value| status_of(Some(&v), |e| code(e, &w));
        assert_eq!(status_of(None, |e| code(e, &w)), Status::NotConfigured);
        assert_eq!(servers(json!({"other": {}})), Status::NotConfigured);
        assert_eq!(
            servers(json!({"daw": http("http://127.0.0.1:7878/mcp")})),
            Status::OtherName {
                name: "daw".into(),
                port: Some(7878)
            }
        );
        assert_eq!(
            servers(json!({"daw": http("https://elsewhere/mcp")})),
            Status::NotConfigured
        );
        assert_eq!(
            servers(
                json!({"daw": http("http://127.0.0.1:1/mcp"), "oxide": stdio(&["--port", "7878"])})
            ),
            Status::Configured { port: 7878 }
        );
    }

    #[test]
    fn claude_code_is_given_the_bridge_as_desktop_is() {
        let root = scratch("code-command");
        let l = Locations::profile(&root, None, root.join(BRIDGE));
        let t = wanted(7899, Some("feed"));
        let bridge = l.bridge.to_string_lossy().into_owned();
        assert_eq!(
            code_command(&l, &t),
            ["--", &bridge, "--port", "7899", "--token", "feed"]
        );
        assert_eq!(
            desktop_server(&l, &t),
            json!({"command": bridge, "args": ["--port", "7899", "--token", "feed"]})
        );
        assert_eq!(
            code_command(&l, &wanted(7878, None)),
            ["--", &bridge, "--port", "7878"]
        );
    }

    #[test]
    fn statuses_of_claude_desktop_entries() {
        let dir = std::fs::canonicalize(scratch("entries")).unwrap();
        let bridge = &dir.join(BRIDGE);
        std::fs::write(bridge, b"bridge").unwrap();
        let w = wanted(7878, Some("abc"));
        let entry = |command: &str, args: &[&str]| json!({"command": command, "args": args});
        // The same file as Windows compares names.
        let ours = &bridge.to_string_lossy().replace('\\', "/").to_uppercase();
        let ours = ours.as_str();
        assert_eq!(
            bridge_entry(
                &entry(ours, &["--port", "7878", "--token", "abc"]),
                &w,
                bridge
            ),
            Status::Configured { port: 7878 }
        );
        assert_eq!(
            bridge_entry(&entry(ours, &["--port", "7878"]), &w, bridge),
            Status::OtherToken { port: 7878 }
        );
        assert_eq!(
            bridge_entry(
                &entry(ours, &["--port", "7000", "--token", "abc"]),
                &w,
                bridge
            ),
            Status::OtherPort { port: Some(7000) }
        );
        assert_eq!(
            bridge_entry(&entry(ours, &[]), &w, bridge),
            Status::OtherPort { port: None }
        );
        assert_eq!(
            bridge_entry(
                &entry(r"D:\old\oxide-mcp.exe", &["--port", "7878"]),
                &w,
                bridge
            ),
            Status::OtherProgram { port: Some(7878) }
        );
        // The copy Desktop runs is gone (the build folder cleaned): Reconfigure copies it again.
        std::fs::remove_file(bridge).unwrap();
        let gone = Status::BridgeMissing { port: Some(7878) };
        assert_eq!(
            bridge_entry(
                &entry(ours, &["--port", "7878", "--token", "abc"]),
                &w,
                bridge
            ),
            gone
        );
        assert_eq!(
            gone.text(),
            "Configured (port 7878), but its bridge is missing"
        );
        assert!(gone.can_reconfigure() && !gone.can_configure());
        std::fs::write(bridge, b"bridge").unwrap();
        assert_eq!(
            bridge_entry(
                &entry("npx", &["mcp-remote", "http://127.0.0.1:7878/mcp"]),
                &w,
                bridge
            ),
            Status::OtherProgram { port: Some(7878) }
        );
        let servers = json!({"daw": entry("npx", &["mcp-remote", "http://127.0.0.1:7000/mcp"])});
        assert_eq!(
            status_of(Some(&servers), |e| bridge_entry(e, &w, bridge)),
            Status::OtherName {
                name: "daw".into(),
                port: Some(7000)
            }
        );
    }

    #[test]
    fn statuses_combine_and_read_as_the_settings_show_them() {
        let c = |p| Status::Configured { port: p };
        assert_eq!(Status::combine(vec![]), Status::NotInstalled);
        assert_eq!(Status::combine(vec![c(1), c(1)]), c(1));
        assert_eq!(
            Status::combine(vec![Status::NotConfigured, Status::NotConfigured]),
            Status::NotConfigured
        );
        assert_eq!(
            Status::combine(vec![c(1), Status::NotConfigured]),
            Status::Partly {
                configured: 1,
                of: 2
            }
        );
        assert_eq!(
            Status::combine(vec![c(1), Status::OtherPort { port: Some(2) }]),
            Status::OtherPort { port: Some(2) }
        );
        assert_eq!(
            Status::combine(vec![c(1), Status::Unreadable("x".into())]),
            Status::Unreadable("x".into())
        );
        let texts = [
            (Status::NotInstalled, "Not installed"),
            (Status::NotConfigured, "Not configured"),
            (c(7878), "Configured (port 7878)"),
            (
                Status::OtherPort { port: Some(7899) },
                "Configured for another port (7899)",
            ),
            (
                Status::OtherToken { port: 7878 },
                "Configured (port 7878) with an old token",
            ),
            (
                Status::OtherName {
                    name: "daw".into(),
                    port: Some(7878),
                },
                "Configured as \"daw\" (port 7878)",
            ),
            (
                Status::Partly {
                    configured: 1,
                    of: 2,
                },
                "Configured in 1 of 2 configs",
            ),
            (
                Status::Unreadable("bad".into()),
                "Couldn't read the config: bad",
            ),
        ];
        for (status, text) in texts {
            assert_eq!(status.text(), text);
            let configured = !matches!(
                status,
                Status::NotInstalled | Status::NotConfigured | Status::Unreadable(_)
            );
            assert_eq!(status.can_reconfigure(), configured, "{text}");
            assert_eq!(
                status.can_configure(),
                status == Status::NotConfigured,
                "{text}"
            );
        }
    }

    #[test]
    fn desktop_configs_are_found_in_both_layouts() {
        let root = scratch("layouts");
        let l = Locations::profile(&root, None, root.join("src").join(BRIDGE));
        assert!(l.desktop_configs().is_empty(), "nothing installed");
        // `%APPDATA%\Claude` with only other things in it isn't Desktop.
        std::fs::create_dir_all(l.roaming.join("Claude").join("skills")).unwrap();
        assert!(l.desktop_configs().is_empty());
        // The Store build: its package, whether or not Desktop has written its folder yet.
        let store = l.local.join("Packages").join("Claude_pzs8sxrjxfjjc");
        std::fs::create_dir_all(&store).unwrap();
        let store_config = store
            .join(r"LocalCache\Roaming\Claude")
            .join(DESKTOP_CONFIG);
        assert_eq!(l.desktop_configs(), vec![store_config.clone()]);
        // Another package with Claude in its name counts only with Desktop's folder.
        std::fs::create_dir_all(l.local.join("Packages").join("Someone.ClaudeTool_1")).unwrap();
        assert_eq!(l.desktop_configs(), vec![store_config.clone()]);
        // The .exe installer: installed, or its config there.
        std::fs::create_dir_all(l.local.join("AnthropicClaude")).unwrap();
        let classic = l.roaming.join("Claude").join(DESKTOP_CONFIG);
        assert_eq!(
            l.desktop_configs(),
            vec![classic.clone(), store_config.clone()]
        );
        std::fs::remove_dir_all(l.local.join("AnthropicClaude")).unwrap();
        std::fs::write(&classic, "{}").unwrap();
        assert_eq!(l.desktop_configs(), vec![classic, store_config]);
        // Claude Code's config follows CLAUDE_CONFIG_DIR.
        assert_eq!(
            l.code_config(),
            root.join("claude-code").join(".claude.json")
        );
        let mut home = l.clone();
        home.code_dir = None;
        assert_eq!(home.code_config(), l.home.join(".claude.json"));
    }

    #[test]
    fn the_cli_is_found_on_the_path_or_where_it_installs() {
        let root = scratch("cli");
        let (bin, home) = (root.join("bin"), root.join("home"));
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::create_dir_all(home.join(".local").join("bin")).unwrap();
        let path = std::env::join_paths([&bin]).unwrap();
        assert_eq!(find_cli(Some(path.clone()), &home), None);
        let installed = home.join(r".local\bin\claude.exe");
        std::fs::write(&installed, "").unwrap();
        assert_eq!(find_cli(Some(path.clone()), &home), Some(installed));
        std::fs::write(bin.join("claude.cmd"), "").unwrap();
        assert_eq!(find_cli(Some(path), &home), Some(bin.join("claude.cmd")));
    }

    const SAMPLE: &str = "{\n  \"globalShortcut\": \"Ctrl+Space\",\n  \"mcpServers\": {\n    \"filesystem\": {\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\"]\n    },\n    \"daw\": {\n      \"command\": \"npx\",\n      \"args\": [\"mcp-remote\", \"http://127.0.0.1:7878/mcp\"]\n    }\n  },\n  \"preferences\": {\"quickEntry\": false}\n}\n";

    #[test]
    fn desktop_configure_reconfigure_and_remove_keep_the_rest_of_the_file() {
        let root = scratch("desktop");
        let source = root.join("build").join(BRIDGE);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, b"bridge v1").unwrap();
        let l = Locations::profile(&root, None, source.clone());
        std::fs::create_dir_all(l.local.join("AnthropicClaude")).unwrap();
        let store = l
            .local
            .join(r"Packages\Claude_test\LocalCache\Roaming\Claude");
        std::fs::create_dir_all(&store).unwrap();
        let (classic, packaged) = (
            l.roaming.join("Claude").join(DESKTOP_CONFIG),
            store.join(DESKTOP_CONFIG),
        );
        std::fs::create_dir_all(classic.parent().unwrap()).unwrap();
        std::fs::write(&packaged, SAMPLE).unwrap();
        let w = wanted(7878, None);
        let d = detect(&l, &w);
        // The old name, in one of two configs (the classic one doesn't exist yet).
        assert_eq!(
            d.desktop,
            Status::Partly {
                configured: 1,
                of: 2
            }
        );
        assert_eq!(
            d.desktop_configs[1].1,
            Status::OtherName {
                name: "daw".into(),
                port: Some(7878)
            }
        );

        // Configure writes only where Oxide isn't: the missing file is made.
        let said = apply(&l, Client::Desktop, Act::Configure, &w).unwrap();
        assert!(
            said.contains(&classic.display().to_string())
                && said.ends_with("Restart Claude Desktop to use it"),
            "{said}"
        );
        assert_eq!(std::fs::read_to_string(&packaged).unwrap(), SAMPLE);
        assert!(!classic
            .with_file_name(format!("{DESKTOP_CONFIG}.oxide-backup"))
            .exists());
        assert_eq!(std::fs::read(&l.bridge).unwrap(), b"bridge v1");
        assert_eq!(
            desktop_status(&l, &w).1[0].1,
            Status::Configured { port: 7878 }
        );

        // Reconfigure on another port with a token: the old name goes, ours comes, the rest stays.
        std::fs::write(&source, b"bridge v2").unwrap();
        let t = wanted(7899, Some("feed"));
        assert_eq!(
            detect(&l, &t).desktop,
            Status::OtherPort { port: Some(7878) }
        );
        apply(&l, Client::Desktop, Act::Reconfigure, &t).unwrap();
        assert_eq!(detect(&l, &t).desktop, Status::Configured { port: 7899 });
        assert_eq!(std::fs::read(&l.bridge).unwrap(), b"bridge v2");
        let text = std::fs::read_to_string(&packaged).unwrap();
        let backup = store.join(format!("{DESKTOP_CONFIG}.oxide-backup"));
        assert_eq!(
            std::fs::read_to_string(&backup).unwrap(),
            SAMPLE,
            "the backup is the file before"
        );
        assert!(!store.join(format!("{DESKTOP_CONFIG}.oxide-tmp")).exists());
        let value: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(value["mcpServers"]["oxide"], desktop_server(&l, &t));
        assert!(value["mcpServers"].get("daw").is_none());
        let keys: Vec<&String> = value.as_object().unwrap().keys().collect();
        assert_eq!(keys.len(), 3);
        assert!(text.starts_with("{\n  \"globalShortcut\": \"Ctrl+Space\",\n  \"mcpServers\": {\n    \"filesystem\": {\n      \"command\": \"npx\",\n      \"args\": [\"-y\", \"@modelcontextprotocol/server-filesystem\"]\n    },\n    \"oxide\": {"), "{text}");
        assert!(
            text.ends_with("  },\n  \"preferences\": {\"quickEntry\": false}\n}\n"),
            "{text}"
        );
        // Configure is refused now (nothing written): it is configured.
        assert_eq!(
            apply(&l, Client::Desktop, Act::Configure, &t).unwrap(),
            "Claude Desktop: nothing to change"
        );

        // Remove: the file is the sample without the old entry.
        apply(&l, Client::Desktop, Act::Remove, &t).unwrap();
        assert_eq!(detect(&l, &t).desktop, Status::NotConfigured);
        let expected = json_edit::remove(SAMPLE, SERVERS, OLD_NAME)
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read_to_string(&packaged).unwrap(), expected);
    }

    #[test]
    fn a_malformed_desktop_config_is_left_alone_with_a_message() {
        let root = scratch("malformed");
        let source = root.join(BRIDGE);
        std::fs::write(&source, b"bridge").unwrap();
        let l = Locations::profile(&root, None, source);
        let store = l.local.join(r"Packages\Claude_x\LocalCache\Roaming\Claude");
        std::fs::create_dir_all(&store).unwrap();
        let path = store.join(DESKTOP_CONFIG);
        let broken = "{\"mcpServers\": {\"a\": {\"command\": \"b\"},}\n";
        std::fs::write(&path, broken).unwrap();
        let w = wanted(7878, None);
        let status = detect(&l, &w).desktop;
        assert!(
            matches!(&status, Status::Unreadable(why) if why.contains("isn't valid JSON")),
            "{status:?}"
        );
        assert!(!status.can_configure() && !status.can_reconfigure());
        for act in [Act::Configure, Act::Reconfigure, Act::Remove] {
            let e = apply(&l, Client::Desktop, act, &w).unwrap_err();
            assert!(
                e.starts_with("nothing was changed") && e.contains("isn't valid JSON"),
                "{e}"
            );
        }
        assert_eq!(std::fs::read_to_string(&path).unwrap(), broken);
        assert_eq!(
            std::fs::read_dir(&store).unwrap().count(),
            1,
            "no backup or temp file"
        );
        assert!(!l.bridge.exists());
    }

    #[test]
    fn claude_code_status_comes_from_its_file() {
        let root = scratch("code");
        let mut l = Locations::profile(&root, None, root.join(BRIDGE));
        let w = wanted(7878, None);
        assert_eq!(code_status(&l, &w), Status::NotInstalled);
        l.cli = Some(root.join("claude.exe"));
        assert_eq!(code_status(&l, &w), Status::NotConfigured);
        std::fs::create_dir_all(l.code_dir.as_ref().unwrap()).unwrap();
        std::fs::write(l.code_config(), r#"{"numStartups": 2, "mcpServers": {"oxide": {"type": "http", "url": "http://127.0.0.1:7878/mcp"}}}"#).unwrap();
        assert_eq!(code_status(&l, &w), Status::OverHttp { port: Some(7878) });
        std::fs::create_dir_all(l.bridge.parent().unwrap()).unwrap();
        std::fs::write(&l.bridge, b"bridge").unwrap();
        let entry = json!({"numStartups": 2, "mcpServers": {"oxide": {"type": "stdio", "command": l.bridge, "args": ["--port", "7878"], "env": {}}}});
        std::fs::write(l.code_config(), entry.to_string()).unwrap();
        assert_eq!(code_status(&l, &w), Status::Configured { port: 7878 });
        // Oxide updated: the bridge beside it is newer than the copy the client runs.
        std::fs::write(&l.bridge_source, b"bridge v2").unwrap();
        let stale = code_status(&l, &w);
        assert_eq!(stale, Status::BridgeStale { port: 7878 });
        assert_eq!(
            stale.text(),
            "Configured (port 7878), but its bridge is out of date"
        );
        assert!(stale.can_reconfigure() && !stale.can_configure() && !stale.is_current());
        std::fs::write(&l.bridge_source, b"bridge").unwrap();
        assert_eq!(code_status(&l, &w), Status::Configured { port: 7878 });
        std::fs::remove_file(&l.bridge_source).unwrap();
        std::fs::write(l.code_config(), "{oops").unwrap();
        assert!(matches!(code_status(&l, &w), Status::Unreadable(_)));
        // Without the bridge beside the app, Configure and Reconfigure change nothing (the
        // CLI isn't even run: this one doesn't exist).
        std::fs::write(l.code_config(), "{}").unwrap();
        for act in [Act::Configure, Act::Reconfigure] {
            let e = apply(&l, Client::Code, act, &w).unwrap_err();
            assert!(e.contains("isn't beside the app"), "{e}");
        }
        // Without the CLI nothing can change it.
        std::fs::write(&l.bridge_source, b"bridge").unwrap();
        l.cli = None;
        let e = apply(&l, Client::Code, Act::Configure, &w).unwrap_err();
        assert!(e.contains("wasn't found on the PATH"), "{e}");
    }
}
